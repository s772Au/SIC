// The native protocol may reuse a call ID after its prior group closed.
export function nativeToolResults(history){
 const pending=new Map(),results=new Map();
 for(const message of history){
  if(message.role==='assistant')for(const call of message.content.filter(block=>block.type==='tool-call')){
   if(typeof call.id!=='string'||!call.id||typeof call.name!=='string'||!call.name||pending.has(call.id))throw Error('ambiguous simultaneously open native tool call identity');
   pending.set(call.id,{call,message_id:message.id});
  }
  if(message.role==='tool'){
   const pair=pending.get(message.toolCallId);if(!pair)throw Error('native tool result lacks an unambiguous original native call');
   if(results.has(message.id))throw Error('duplicate native tool result message identity');
   results.set(message.id,pair);pending.delete(message.toolCallId);
  }
 }
 return results;
}
