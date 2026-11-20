// Source identity belongs to the host/Core, not to the Editor's current role.
import {nativeToolResults} from './tool-linkage.mjs';
export function assignedEditorUnits(source,units,options,shared,{referenceShared=false}={}){
 const history=options.messages??[],records=source.binding?.host_message_origins??[],pairs=nativeToolResults(history);
 const views=shared.filter(message=>message.source?.kind==='sic-origin-view').flatMap(message=>message.source.tool_origins??[]);
 const sharedBlocks=referenceShared?shared.filter(message=>message.source?.kind==='sic-cb'):[];
 return units.map(unit=>{
  const origin=records.find(record=>record.unit_id===unit.unit_id);
  if(origin){
   if(JSON.stringify(origin.native_anchor)!==JSON.stringify(unit.anchor))throw Error('Editor source origin anchor differs from the Core unit');
   const matches=history.filter(message=>message.id===origin.message_id);if(matches.length!==1)return unit;
   const message=matches[0],block=message.content[origin.block_index];
   if(block?.type!=='text'||block.text!==unit.text)throw Error('Editor source origin does not match its immutable SDK text leaf');
   const host_origin={message_id:message.id,block_index:origin.block_index,original_role:message.role};
   if(message.role==='tool'){
    const pair=pairs.get(message.id);if(!pair||(message.isError!==undefined&&typeof message.isError!=='boolean'))throw Error('Editor tool source lacks its original execution linkage');
    Object.assign(host_origin,{kind:'native-tool-result',tool_call_id:message.toolCallId,tool_call_message_id:pair.message_id,tool_name:pair.call.name,is_error:message.isError===true,...(message.isError===undefined?{error_flag_present:false}:{}),call_returned_before_editor:true,does_not_imply:'command exit 0, background terminal state or whole-task completion'});
   }
   return{...unit,host_origin};
  }
  const cb=unit.anchor?.native?.cb_id;
  const position=cb===undefined?-1:sharedBlocks.findIndex(message=>message.source.cb_id===cb&&message.content.length===1&&message.content[0].text===unit.text);
  if(position>=0){
   const {text,...reference}=unit;
   return{...reference,text_ref:{kind:'shared-cb',cb_id:cb,ordinal:position+1},host_origin:{kind:'adopted-context-block',cb_id:cb,tool_origins:views.filter(view=>view.cb_id===cb)}};
  }
  return cb?{...unit,host_origin:{kind:'adopted-context-block',cb_id:cb,tool_origins:views.filter(view=>view.cb_id===cb)}}:unit;
 });
}
