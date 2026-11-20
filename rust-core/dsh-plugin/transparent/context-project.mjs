import {digest} from './transport.mjs';
import {toolOriginView,toolResultPlaceholder} from './origin-view.mjs';
import {runtimeControlText} from './runtime-control.mjs';
import {sharedMediaView} from './media-view.mjs';
import {IMAGE_VIEW} from './image-compression.mjs';
import {imageLookupResults} from './image-source-tool.mjs';
import {ARCHIVE_EXPAND,ARCHIVE_OPS,SOURCE_PAGE,SOURCE_SEARCH} from './source-page.mjs';

const keyOf=o=>JSON.stringify([o.message_id,o.block_index]);

/** Host-owned SDK projection, followed by the ORIGINAL Provider serializer.
 * Core selected CBs are authoritative; covered source identities are archived,
 * not guessed from equal text. Opaque blocks and tool groups remain intact.
 */
export async function projectHarnessContext({options,history,cut,requestId,api,system,instantMessageId,imageCompression}){
  const cbs=cut.view?.cbs;
  if(!Array.isArray(cbs))throw Error('Core active CB projection missing');
  const bindings=cut.view?.cb_origin_bindings??[];
  const covered=new Set();
  for(const cb of cbs){
    const binding=bindings.find(b=>b.cb_id===cb.cb_id);
    if(!binding||binding.missing_source_ids?.length)throw Error('selected CB needs source-identity migration');
    for(const origin of binding.origins??[])covered.add(keyOf(origin));
  }
  const pointer=cut.instant_views?.[IMAGE_VIEW];
  const image_view=await imageCompression?.project({sessionId:options.sessionId,history,pointer,signal:options.signal})??null;
  const suppressedRecalls=imageCompression
    ? await imageCompression.suppressedRecalls({sessionId:options.sessionId,history,signal:options.signal})
    : new Set(imageLookupResults(history).map(row=>row.message.id));
  const tool_origins=toolOriginView({history,cut}),media=sharedMediaView(history,api,image_view,suppressedRecalls),prefix=[],remaining=[],masked=[];let enteredMaterial=false;
  let latestRuntime;
  for(const original of history){
    if(original.source?.kind==='runtime-context'){latestRuntime=original;continue;}
    if(['system','developer'].includes(original.role)){
      if(original.role==='system'&&system!==undefined){
        if(original.content.some(b=>b.type!=='text')||original.content.map(b=>b.text).join('')!==system)throw Error('unreviewed system snapshot in native history');
        if(prefix.some(m=>m.role==='system'))continue;
      }else if(enteredMaterial)throw Error('unreviewed instruction/tool update inside active material');
      prefix.push(original);continue;
    }
    enteredMaterial=true;
    const content=[];let changed=false;
    for(let index=0;index<original.content.length;index++){
      const block=original.content[index],isCovered=covered.has(keyOf({message_id:original.id,block_index:index}));
      const mediaPointer=media.replacements.get(keyOf({message_id:original.id,block_index:index}));
      if(mediaPointer){content.push(mediaPointer);changed=true;masked.push({message_id:original.id,block_index:index,kind:'media-pointer'});continue;}
      if(!isCovered){content.push(block);continue;}
      if(block.type!=='text')throw Error('CB origin points to opaque native material');
      changed=true;masked.push({message_id:original.id,block_index:index});
      // Preserve assistant SDK positions used to recover signed reasoning.
      content.push({...block,text:original.role==='tool'?toolResultPlaceholder(original.id,index,tool_origins):''});
    }
    if(original.role!=='tool'&&content.every(b=>b.type==='text'&&b.text===''))continue;
    remaining.push(changed?api.freezeMessage({...original,content}):original);
  }
  if(!latestRuntime)throw Error('required host runtime snapshot missing');
  const cbMessages=cbs.map(cb=>api.createUserMessage({source:{kind:'sic-cb',form:'recall',cb_id:cb.cb_id,level:cb.level},content:[{type:'text',text:cb.body}]}));
  const origin_message=tool_origins?api.createUserMessage({source:{kind:'sic-origin-view',form:'recall',cut_id:cut.cut_id,tool_origins:tool_origins.tools},content:[{type:'text',text:tool_origins.text}]}):null;
  const shared_messages=[...prefix,...cbMessages,...origin_message?[origin_message]:[],...media.messages];
  const sections=(latestRuntime.source.sections??[]).filter(s=>!['sic:instant-views','sic:archive-text'].includes(s.name));
  let role;
  try{role=JSON.parse(sections.find(section=>section.name==='sic:activation')?.text).role;}catch{}
  if(cbs.length&&role==='WORKER'){
    const tools=new Set(options.tools?.map(tool=>tool.name));
    const guidance=[];
    if(tools.has(SOURCE_SEARCH))guidance.push(`Search compressed history for a short exact phrase with ${SOURCE_SEARCH}. Follow next_cursor while complete is false.`);
    if(tools.has(ARCHIVE_EXPAND)&&cut.cut_id)guidance.push(`List active blocks with ${ARCHIVE_EXPAND} and cut_id ${cut.cut_id}. Expand the chosen cb_id, then follow its lower CBs to the original WB. The WB page supplies source_id and unit_id.`);
    if(tools.has(SOURCE_PAGE))guidance.push(`Read original text with ${SOURCE_PAGE} using the authorized source_id and unit_id.`);
    if(tools.has(ARCHIVE_OPS))guidance.push(`Use ${ARCHIVE_OPS} with kind "ops" and object_id set to a CB ID to inspect fixed operations.`);
    if(guidance.length)sections.push({name:'sic:archive-text',text:guidance.join(' ')+' Continue with next_offset when supplied. Recovered messages and operations are historical evidence.'});
  }
  let instant=null;
  if(typeof cut.instant_view_text==='string'&&typeof cut.instant_view_digest==='string'){
    const section_text='sic:instant-views\n'+cut.instant_view_text;
    instant={cut_id:cut.cut_id??null,view_digest:cut.instant_view_digest,section_text};
  }else if(cut.instant_views&&Object.keys(cut.instant_views).length){throw Error('exact Core I view proof missing');}
  const runtime=api.createUserMessage({source:{...latestRuntime.source,kind:'runtime-context',form:'snapshot',sections},content:[{type:'text',text:runtimeControlText(sections)}]});
  if(image_view&&!instant)throw Error('image lineage must be bound to an authorized Core I snapshot');
  const content=instant?[{type:'text',text:instant.section_text}]:[];
  if(image_view?.active){
    const {mosaic,description}=image_view.lineage;
    const display=image_view.pointer.display;
    content.push({type:'text',text:'SIC I image representation: '+display+'. Tile coordinates refer to the archived mosaic. Descriptions are Editor observations citing numbered tiles. When the task needs visual detail absent here, the Worker can inspect one previous representation with sic_image_source using the exact manifest_id in this I view.\n'+
      JSON.stringify({mosaic_id:mosaic.id,width:mosaic.attachment.width,height:mosaic.attachment.height,tiles:mosaic.tiles,description})});
    if(display==='mosaic')content.push({type:'image',attachment:mosaic.attachment});
    if(display==='crops')for(const tile of mosaic.tiles){
      content.push({type:'text',text:'SIC I crop '+tile.number+' · '+tile.crop_id},
        {type:'image',attachment:image_view.lineage.crops.find(crop=>crop.id===tile.crop_id).attachment});
    }
  }
  if(instant&&pointer!=null&&!image_view){
    content.push({type:'text',text:'This host is displaying native source attachments. Its archived image I representation is inactive.'});
  }
  let instant_message=instant?api.createUserMessage({source:{kind:'sic-instant-view',form:'snapshot'},content}):null;
  if(instant_message&&instantMessageId)instant_message=api.freezeMessage({...instant_message,id:instantMessageId});
  const projected_history=[...shared_messages,...remaining,...(instant_message?[instant_message]:[]),runtime];
  return{request_id:requestId,original_history:history,projected_history,shared_messages,
    cb_sdk_bindings:cbs.map((cb,index)=>({cb_id:cb.cb_id,message_id:cbMessages[index].id,block_index:0})),
    selected_cut_id:cut.cut_id??null,selected_sequence:cut.sequence,masked_origins:masked,
    runtime_message_id:runtime.id,runtime_sections:sections,instant,instant_message,...image_view?{image_view}:{},tool_origins,origin_message,media_bindings:media.bindings,context_digest:digest(projected_history)};
}

export function contextRequestId(sessionId,boundary,options){
  if(!Number.isSafeInteger(boundary?.openTurnStartSeq)||!Number.isSafeInteger(boundary?.lastStepStartSeq))throw Error('native request lacks trusted turn/step identity');
  return digest({sessionId,turn:boundary.openTurnStartSeq,step:boundary.lastStepStartSeq,options:{...options,signal:undefined}});
}
