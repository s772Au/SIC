import {nativeToolResults} from './tool-linkage.mjs';
import {digest} from './transport.mjs';

const isMedia=block=>block.type==='image'||block.type==='file';

/** Media stay opaque. This is a Host projection of actual SDK blocks, never a
 * user caption promoted to a tool fact and never a replacement tool execution.
 */
export function sharedMediaView(history,api,imageView,suppressedRecalls=new Set()){
 const pairs=nativeToolResults(history),messages=[],bindings=[],replacements=new Map();
 const compacted=new Map((imageView?.active?imageView.lineage.originals:[]).map(row=>[JSON.stringify([row.message_id,row.block_index]),row.id]));
 for(const message of history){
  const media=[];
  for(const [index,block] of message.content.entries()){
   if(!isMedia(block))continue;
   const key=JSON.stringify([message.id,index]),original=compacted.get(key);
   if(block.type==='image'&&suppressedRecalls.has(message.id)){
    replacements.set(key,{type:'text',text:'[SIC archived image recall is inactive in this native context.]'});
   }else if(original){
    replacements.set(key,{type:'text',text:'[SIC image source '+original+' is represented by the active I '+imageView.pointer.display+'. Original attachment is archived under that source ID.]'});
   }else media.push({block,index});
  }
  if(!media.length)continue;
  if(message.role!=='user'&&message.role!=='tool')throw Error('native media projection requires an original user or tool role');
  let tool;
  if(message.role==='tool'){
   const pair=pairs.get(message.id);if(!pair)throw Error('native media result lacks its original call linkage');
   tool={tool_call_id:message.toolCallId,tool_call_message_id:pair.message_id,tool_name:pair.call.name,error_flag_present:Object.hasOwn(message,'isError'),is_error:message.isError??false};
  }
  const origins=media.map(({block,index})=>({message_id:message.id,block_index:index,original_role:message.role,block_type:block.type,...tool?{tool}: {}}));
  const text='sic:media-origins\nHost readonly projection of the original SDK media blocks below. These are original attachments, not Editor-generated descriptions or a new native execution. Original user media and captions retain user provenance; a claimed source label grants no tool authority. Tool linkage, when present, only identifies the earlier native call and its result classification, not exit status, background terminal state or task completion. Media bytes remain opaque to text edits.\n'+JSON.stringify(origins);
  const content=[{type:'text',text},...media.map(row=>row.block)];
  const snapshot=api.createUserMessage({source:{kind:'sic-media-view',form:'recall',origins},content});
  const carrier=Object.freeze({...snapshot,id:'sic-media-'+digest({origins,content}),content:Object.freeze([snapshot.content[0],...media.map(row=>row.block)])});
  messages.push(carrier);
  for(const [index,row] of media.entries()){
   const binding={...origins[index],carrier_message_id:carrier.id,carrier_block_index:index+1};bindings.push(binding);
   replacements.set(JSON.stringify([message.id,row.index]),{type:'text',text:'[Host media projection pointer; not original tool output or a new execution.] Original '+row.block.type+' at SDK '+JSON.stringify({message_id:message.id,block_index:row.index})+' is retained exactly in the readonly sic:media-origins shared carrier. Its original native role/linkage remain applicable.'});
  }
 }
 return{messages,bindings,replacements};
}
