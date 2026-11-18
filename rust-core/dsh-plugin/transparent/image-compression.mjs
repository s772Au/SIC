import {createHash} from 'node:crypto';
import {isDeepStrictEqual} from 'node:util';
import {digest} from './transport.mjs';
import {imageLookupResults,IMAGE_SOURCE_RESULT} from './image-source-tool.mjs';

export const IMAGE_VIEW = 'sic-image-compression-v1';
const MAX_CROPS = 12;
const MAX_CROP_PIXELS = 8_000_000;
const TILE = 512;
const GUTTER = 8;
const ID = /^[a-f\d]{64}$/u;
const DISPLAYS = ['crops','mosaic','description'];
const bytesHash = data => createHash('sha256').update(data).digest('hex');
const position = (messageId, index) => JSON.stringify([messageId,index]);
const exactKeys = (value, names) => value && typeof value === 'object' && !Array.isArray(value)
  && Object.keys(value).length === names.length && names.every(name => Object.hasOwn(value,name));

function originalsIn(history) {
  const originals = new Map();
  const recalls = new Set(imageLookupResults(history).map(row=>row.message.id));
  for (const message of history) {
    if (!['user','tool'].includes(message.role) || message.source?.kind === 'runtime-context'||recalls.has(message.id)) continue;
    for (const [index,block] of message.content.entries()) {
      if (block.type !== 'image') continue;
      const key = position(message.id,index);
      if (originals.has(key)) throw Error('ambiguous native image occurrence');
      const original = {role:message.role,message_id:message.id,block_index:index,attachment:block.attachment,
        ...message.role === 'tool' ? {tool_call_id:message.toolCallId} : {}};
      originals.set(key,{id:digest(original),original,offloaded:block.offloaded === true});
    }
  }
  return originals;
}

function parseRecipe(recipe, history) {
  if (!exactKeys(recipe,['crops','description']) && !exactKeys(recipe,['crops','description','display'])) throw Error('invalid image crop recipe');
  if (Object.hasOwn(recipe,'display') && !DISPLAYS.includes(recipe.display)) throw Error('invalid image view representation');
  if (!Array.isArray(recipe.crops)
      || recipe.crops.length < 1 || recipe.crops.length > MAX_CROPS) throw Error('invalid image crop group');
  const originals = originalsIn(history), parents = new Map();
  let pixels = 0;
  const crops = recipe.crops.map(row => {
    if (!exactKeys(row,['message_id','block_index','x','y','width','height'])
        || typeof row.message_id !== 'string' || !Number.isSafeInteger(row.block_index) || row.block_index < 0) {
      throw Error('invalid image crop coordinates or source');
    }
    const parent = originals.get(position(row.message_id,row.block_index));
    if (!parent || parent.offloaded) throw Error('image crop source is outside this native assignment or offloaded');
    const {x,y,width,height} = row;
    const ref = parent.original.attachment;
    if (![x,y,width,height].every(Number.isSafeInteger) || x < 0 || y < 0 || width < 1 || height < 1
        || x + width > ref.width || y + height > ref.height) throw Error('image crop is outside the original raster');
    pixels += width * height;
    if (pixels > MAX_CROP_PIXELS) throw Error('image crop group exceeds its total pixel limit');
    parents.set(parent.id,parent.original);
    return {parent_id:parent.id,region:{x,y,width,height}};
  });
  if (new Set(crops.map(digest)).size !== crops.length) throw Error('image crop group has identical repeated regions');
  const parts = recipe.description;
  if (!Array.isArray(parts) || parts.length < 1 || parts.length > MAX_CROPS) throw Error('image description requires cited mosaic tiles');
  const description = parts.map(part => {
    if (!exactKeys(part,['text','tiles']) || typeof part.text !== 'string' || !part.text.trim()
        || part.text.length > 800 || !Array.isArray(part.tiles) || !part.tiles.length
        || new Set(part.tiles).size !== part.tiles.length
        || part.tiles.some(tile => !Number.isSafeInteger(tile) || tile < 1 || tile > crops.length)) {
      throw Error('invalid image description or tile citation');
    }
    return {text:part.text,tiles:part.tiles};
  });
  return {originals:[...parents].map(([id,original])=>({id,...original})),crops,description};
}

/** Host-only transform and lineage store. The Editor supplies coordinates, never file paths or attachment references. */
export function createImageCompression({store,attachments,sharp}) {
  if (typeof attachments?.readImage !== 'function' || typeof attachments.saveImage !== 'function'
      || typeof sharp !== 'function') throw Error('image compression requires the native attachment store and Sharp');
  const sharpImage = data => sharp(data,{failOn:'error',limitInputPixels:64_000_000});
  async function read(ref,signal) {
    signal?.throwIfAborted();
    const {data} = await attachments.readImage(ref,signal);
    signal?.throwIfAborted();
    if (ref.attachmentId !== 'sha256:'+bytesHash(data)) throw Error('native attachment bytes differ from the lineage reference');
    return Buffer.from(data);
  }
  async function save(data,width,height,signal) {
    signal?.throwIfAborted();
    const ref = await attachments.saveImage({data,mediaType:'image/png'});
    if (ref.width !== width || ref.height !== height) throw Error('native attachment policy changed the image lineage dimensions');
    await read(ref,signal);
    return ref;
  }
  function offer(history) {
    return [...originalsIn(history).values()].filter(row=>!row.offloaded).map(({original})=>({
      message_id:original.message_id,block_index:original.block_index,width:original.attachment.width,height:original.attachment.height,
    }));
  }
  async function prepare({args,history,sessionId,signal}) {
    if (Array.isArray(args?.ops) && args.ops.some(op=>op?.view_key === IMAGE_VIEW)) throw Error('image view updates require pvi.I image recipes');
    const instant = args?.pvi?.I;
    if (!Object.hasOwn(instant ?? {},IMAGE_VIEW) || instant[IMAGE_VIEW] === null) return args;
    if (!sessionId) throw Error('image compression requires the trusted native session');
    const recipe = parseRecipe(instant[IMAGE_VIEW],history);
    const key = digest({contract:IMAGE_VIEW,policy:{tile:TILE,gutter:GUTTER},recipe});
    const record = await store.context(sessionId,key,async()=>{
      const available = new Map();
      for (const original of recipe.originals) available.set(original.id,await read(original.attachment,signal));
      const crops = [], tiles = [], composite = [];
      const columns = Math.min(3,recipe.crops.length);
      let left=GUTTER,top=GUTTER,rowHeight=0;
      for (const [index,crop] of recipe.crops.entries()) {
        signal?.throwIfAborted();
        const {x,y,width,height}=crop.region;
        const raster = await sharpImage(available.get(crop.parent_id)).extract({left:x,top:y,width,height}).png().toBuffer();
        const attachment = await save(raster,width,height,signal);
        const id = digest({crop,attachment});
        crops.push({id,...crop,attachment});
        // Fit inside a fixed tile, but never enlarge or distort the source.
        const retained = await read(attachment,signal);
        const rendered = await sharpImage(retained).resize(TILE,TILE,{fit:'inside',withoutEnlargement:true}).png().toBuffer({resolveWithObject:true});
        if(index>0&&index%columns===0){top+=rowHeight+GUTTER;left=GUTTER;rowHeight=0;}
        tiles.push({number:index+1,crop_id:id,placement:{x:left,y:top,width:rendered.info.width,height:rendered.info.height}});
        composite.push({input:rendered.data,left,top});
        left+=rendered.info.width+GUTTER;
        rowHeight=Math.max(rowHeight,rendered.info.height);
      }
      const width = Math.max(...tiles.map(({placement:p})=>p.x+p.width))+GUTTER;
      const height = Math.max(...tiles.map(({placement:p})=>p.y+p.height))+GUTTER;
      const raster = await sharp({create:{width,height,channels:3,background:'#ffffff'}}).composite(composite).png().toBuffer();
      const attachment = await save(raster,width,height,signal);
      const mosaic = {id:digest({tiles,attachment}),attachment,tiles};
      const description = {parent_id:mosaic.id,parts:recipe.description};
      const lineage={originals:recipe.originals,crops,mosaic,description:{id:digest(description),...description}};
      return {contract:IMAGE_VIEW,key,session_id:sessionId,lineage_digest:digest(lineage),lineage};
    });
    validateRecord(record,sessionId,key);
    signal?.throwIfAborted();
    // Edited I values contain an immutable host receipt, never Editor-supplied paths.
    return {...args,pvi:{...args.pvi,I:{...instant,[IMAGE_VIEW]:{manifest_id:key,lineage_digest:record.lineage_digest,display:instant[IMAGE_VIEW].display??'mosaic'}}}};
  }
  function validateRecord(record,sessionId,key) {
    if (record?.contract!==IMAGE_VIEW || record.session_id!==sessionId || record.key!==key
        || record.lineage_digest!==digest(record.lineage)) throw Error('invalid immutable image lineage record');
    return record;
  }
  async function load(sessionId,pointer) {
    if (!sessionId || !exactKeys(pointer,['manifest_id','lineage_digest','display']) || !DISPLAYS.includes(pointer.display)
        || ![pointer.manifest_id,pointer.lineage_digest].every(value=>typeof value==='string'&&ID.test(value))) throw Error('invalid image lineage binding');
    const record = validateRecord(await store.context(sessionId,pointer.manifest_id,()=>{
      throw Error('image lineage manifest missing from this session');
    }),sessionId,pointer.manifest_id);
    if (record.lineage_digest!==pointer.lineage_digest) throw Error('image lineage digest does not match active I');
    return record.lineage;
  }
  async function project({sessionId,history,pointer,signal}) {
    if (pointer == null) return null;
    const lineage = await load(sessionId,pointer);
    const originals = originalsIn(history);
    // A new native offload decision (or disappearance of a source) wins over
    // the old view. Never use a composite to send offloaded source pixels again.
    const active = available(lineage,originals);
    if (active && pointer.display==='mosaic') await read(lineage.mosaic.attachment,signal);
    if (active && pointer.display==='crops') for(const crop of lineage.crops) await read(crop.attachment,signal);
    return {pointer,lineage,active};
  }
  function available(lineage,originals) {
    return lineage.originals.every(({id,...original})=>{
      const match=originals.get(position(original.message_id,original.block_index));
      return match?.id===id && !match.offloaded && isDeepStrictEqual(match.original,original);
    });
  }
  async function suppressedRecalls({sessionId,history,signal}){
    const results=imageLookupResults(history);
    if(!results.length)return new Set();
    const originals=originalsIn(history), cache=new Map(),suppressed=new Set();
    for(const {message,call} of results){
      signal?.throwIfAborted();
      if(!message.content.some(block=>block.type==='image'))continue;
      const text=message.content[0]?.text;
      let eligible=false;
      try{
        if(message.isError===true||typeof text!=='string'||!text.startsWith(IMAGE_SOURCE_RESULT))throw Error('unverified image recall metadata');
        const receipt=JSON.parse(text.slice(IMAGE_SOURCE_RESULT.length)),args=JSON.parse(call.arguments);
        if(args.manifest_id!==receipt.manifest_id)throw Error('image recall manifest mismatch');
        const pointer={manifest_id:receipt.manifest_id,lineage_digest:receipt.lineage_digest,display:'mosaic'};
        const key=digest(pointer);
        if(!cache.has(key))cache.set(key,await load(sessionId,pointer));
        const lineage=cache.get(key);
        const item=[lineage.mosaic,...lineage.crops,...lineage.originals].find(row=>row.id===receipt.asset_id);
        const images=message.content.filter(block=>block.type==='image');
        eligible=available(lineage,originals)&&images.length===1&&isDeepStrictEqual(item?.attachment,images[0].attachment)&&images[0].offloaded!==true;
      }catch{signal?.throwIfAborted();}
      if(!eligible)suppressed.add(message.id);
    }
    return suppressed;
  }
  async function readAsset(sessionId,pointer,id,signal) {
    const lineage = await load(sessionId,pointer);
    const item = [lineage.mosaic,...lineage.crops,...lineage.originals].find(row=>row.id===id);
    if (!item) return null;
    const data = await read(item.attachment,signal);
    return {data,mediaType:item.attachment.mediaType};
  }
  async function assetReference(sessionId,pointer,id,signal){
    const lineage=await load(sessionId,pointer);
    const item=[lineage.mosaic,...lineage.crops,...lineage.originals].find(row=>row.id===id);
    if(!item)return null;
    await read(item.attachment,signal);
    return item.attachment;
  }
  return {offer,prepare,project,suppressedRecalls,readAsset,assetReference};
}

export function assertImageDeliveryDisabled(args) {
  if (Object.hasOwn(args?.pvi?.I ?? {},IMAGE_VIEW) || (Array.isArray(args?.ops)&&args.ops.some(op=>op?.view_key === IMAGE_VIEW))) {
    throw Error('native image compression is not active');
  }
  return args;
}
