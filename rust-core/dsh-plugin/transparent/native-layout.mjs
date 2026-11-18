import{isDeepStrictEqual}from'node:util';
import{messagesBlockOrigins}from'./origins.mjs';

// Strict JSON span indexing includes containers, preserving all untouched bytes.
function spansOf(raw){
 JSON.parse(raw);let at=0;const spans=new Map(),duplicate=new Set();
 const ws=()=>{while(at<raw.length&&/\s/u.test(raw[at]))at++;};
 function str(){const start=at++;while(at<raw.length){if(raw[at]==='\\'){at+=2;continue;}if(raw[at++]==='"')return JSON.parse(raw.slice(start,at));}throw Error('invalid native string');}
 function value(path){ws();const start=at;
  if(raw[at]==='"')str();
  else if(raw[at]==='{'){at++;ws();if(raw[at]==='}')at++;else while(at<raw.length){ws();const key=str();ws();at++;value([...path,key]);ws();if(raw[at++]==='}')break;}}
  else if(raw[at]==='['){at++;ws();if(raw[at]===']')at++;else{let n=0;while(at<raw.length){value([...path,n++]);ws();if(raw[at++]===']')break;}}}
  else while(at<raw.length&&!/[\s,}\]]/u.test(raw[at]))at++;
  const key=JSON.stringify(path);if(spans.has(key))duplicate.add(key);spans.set(key,{start,end:at});
 }
 value([]);return{spans,duplicate};
}
function patch(raw,changes){let out=raw,last=raw.length;for(const c of changes.sort((a,b)=>b.start-a.start)){if(c.end>last)throw Error('overlapping native layout');out=out.slice(0,c.start)+c.raw+out.slice(c.end);last=c.start;}return out;}
const at=(obj,path)=>path.reduce((v,k)=>v[k],obj);

// Only the original serializer creates the new structure. Keep original outer
// extensions/controls verbatim and graft unchanged SDK-identified opaque blocks
// as their exact original raw slices, rather than JSON-normalizing signatures.
export function repackageNativeContext(raw,serialized,beforeHistory,afterHistory,mediaBindings=[]){
 const moved=new Set();
 for(const binding of mediaBindings){
  const original=beforeHistory.find(message=>message.id===binding.message_id);
  const priorCarrier=beforeHistory.find(message=>message.id===binding.carrier_message_id);
  const carrier=afterHistory.find(message=>message.id===binding.carrier_message_id);
  const priorBlock=priorCarrier?.content[binding.carrier_block_index]??original?.content[binding.block_index];
  const priorOrigin=priorCarrier?.source?.origins?.[binding.carrier_block_index-1];
  const matchingOrigin=priorCarrier
    ?priorCarrier.source?.kind==='sic-media-view'&&priorOrigin?.message_id===binding.message_id&&priorOrigin?.block_index===binding.block_index&&priorOrigin?.original_role===binding.original_role
    :original?.role===binding.original_role;
  if(!matchingOrigin||carrier?.source?.kind!=='sic-media-view'||!['image','file'].includes(priorBlock?.type)||!isDeepStrictEqual(priorBlock,carrier.content[binding.carrier_block_index]))throw Error('invalid trusted media relocation binding');
  moved.add(JSON.stringify([binding.message_id,binding.block_index,binding.original_role==='tool'?'result-image':'image']));
 }

 const before=JSON.parse(raw),oldMap=spansOf(raw),oldBlocks=new Map(messagesBlockOrigins(beforeHistory,before,mediaBindings).map(o=>[o.identity,o]));
 let context=JSON.stringify({messages:serialized.messages,...serialized.system===undefined?{}:{system:serialized.system}});
 const newMap=spansOf(context),grafts=[],whole=new Set();
 for(const block of messagesBlockOrigins(afterHistory,serialized,mediaBindings)){
  if(block.parent_identity&&whole.has(block.parent_identity))continue;
  const old=oldBlocks.get(block.identity);if(!old)continue;
  const oldValue=at(before,old.anchor),newValue=at(serialized,block.anchor);
  if(!isDeepStrictEqual(oldValue,newValue)){
   if(block.type==='tool_result'){
    const metadata=value=>Object.fromEntries(Object.entries(value).filter(([key])=>key!=='content'));
    if(!isDeepStrictEqual(metadata(oldValue),metadata(newValue))||!isDeepStrictEqual(oldValue.content.filter((b,n)=>b.type!=='text'&&![...oldBlocks.values()].some(o=>o.parent_identity===old.identity&&o.anchor.at(-1)===n&&moved.has(o.identity))),newValue.content.filter(b=>b.type!=='text')))throw Error('opaque native tool result changed during canonical context adoption');
    for(const name of Object.keys(metadata(newValue))){
     const oldSpan=oldMap.spans.get(JSON.stringify([...old.anchor,name])),newSpan=newMap.spans.get(JSON.stringify([...block.anchor,name]));
     grafts.push({...newSpan,raw:raw.slice(oldSpan.start,oldSpan.end)});
    }
    continue;
   }
   if(block.type!=='text')throw Error('opaque native block changed during canonical context adoption');continue;
  }
  const a=oldMap.spans.get(JSON.stringify(old.anchor)),b=newMap.spans.get(JSON.stringify(block.anchor));
  if(oldMap.duplicate.has(JSON.stringify(old.anchor))||newMap.duplicate.has(JSON.stringify(block.anchor)))throw Error('ambiguous native layout identity');
  grafts.push({...b,raw:raw.slice(a.start,a.end)});
  whole.add(block.identity);
 }
 context=patch(context,grafts);const finalMap=spansOf(context),changes=[];
 for(const name of ['messages','system']){
  const key=JSON.stringify([name]),from=finalMap.spans.get(key),to=oldMap.spans.get(key);
  if(!from&&!to)continue;
  if(!from||!to||oldMap.duplicate.has(key))throw Error('native context fields differ from reviewed serializer');
  changes.push({...to,raw:context.slice(from.start,from.end)});
 }
 return patch(raw,changes);
}
