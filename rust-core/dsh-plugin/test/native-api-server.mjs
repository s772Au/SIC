import {createServer} from 'node:http';
export function strings(value){if(typeof value==='string')return[value];if(Array.isArray(value))return value.flatMap(strings);if(value&&typeof value==='object')return Object.values(value).flatMap(strings);return[];}
export async function nativeApiServer({image=false,mediaOnly=false,respond}={}){
 const requests=[];let count=0;
 const server=createServer(async(req,res)=>{try{
  let raw='';for await(const chunk of req)raw+=chunk;const body=JSON.parse(raw),all=strings(body);requests.push({path:req.url,body});
  if(req.url.endsWith('/files')){res.writeHead(404,{'content-type':'application/json'});res.end('{"error":{"message":"Files unavailable"}}');return;}
  const wbText=all.find(text=>text.startsWith('{"contract":"authenticated-wb-origins-v1"')),wb=wbText?JSON.parse(wbText):null;
  let tool,text;
  if(respond)({tool,text}=await respond({wb,body,texts:all,count:++count}));
  else if(mediaOnly)text='{"visual":"red"}';
  else if(wb){tool={id:'editor-'+(++count),name:'sic_edit_blocks',args:{ops:wb.units.map(unit=>({op:'KEEP',target:unit.unit_id})),pvi:{I:{local_protocol_witness:'scripted local server; not external model'}}}};}
  else if(!all.some(s=>s.includes('Host context projection annotation')||s==='LIVE_READ_MARKER_20261118\n')){
   const path=all.join('\n').match(/Read the file (.+?) with the native read tool\./)?.[1];if(!path)throw Error('scripted Worker cannot locate its own actual task');tool={id:'native-read-'+(++count),name:'read',args:{file_path:path}};
  }else text=JSON.stringify({marker:'LIVE_READ_MARKER_20261118',source:'read',...image?{visual:'red'}:{}});
  res.writeHead(200,{'content-type':'text/event-stream','cache-control':'no-cache'});
  const send=(event,value)=>res.write((event?'event: '+event+'\n':'')+'data: '+JSON.stringify(value)+'\n\n');
  if(req.url.endsWith('/messages'))messages(send,body.model,tool,text);
  else if(req.url.endsWith('/chat/completions'))chat(send,body.model,tool,text);
  else if(req.url.endsWith('/responses'))responses(send,body.model,tool,text);
  else throw Error('unsupported native server endpoint');
  res.end();
 }catch(error){res.writeHead(500,{'content-type':'application/json'});res.end(JSON.stringify({error:{message:String(error)}}));}});
 await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
 return{baseURL:'http://127.0.0.1:'+server.address().port+'/v1',requests,close:()=>new Promise(resolve=>server.close(resolve))};
}
function messages(send,model,tool,text){
 send('message_start',{type:'message_start',message:{id:'msg-local',type:'message',role:'assistant',model,content:[],stop_reason:null,usage:{input_tokens:100,output_tokens:0}}});
 send('content_block_start',{type:'content_block_start',index:0,content_block:tool?{type:'tool_use',id:tool.id,name:tool.name,input:{}}:{type:'text',text:''}});
 send('content_block_delta',{type:'content_block_delta',index:0,delta:tool?{type:'input_json_delta',partial_json:JSON.stringify(tool.args)}:{type:'text_delta',text}});
 send('content_block_stop',{type:'content_block_stop',index:0});send('message_delta',{type:'message_delta',delta:{stop_reason:tool?'tool_use':'end_turn'},usage:{output_tokens:30}});send('message_stop',{type:'message_stop'});
}
function chat(send,model,tool,text){
 const frame=(delta,finish_reason=null)=>({id:'chat-local',object:'chat.completion.chunk',created:0,model,choices:[{index:0,delta,finish_reason}]});
 send(null,frame(tool?{role:'assistant',tool_calls:[{index:0,id:tool.id,type:'function',function:{name:tool.name,arguments:JSON.stringify(tool.args)}}]}:{role:'assistant',content:text}));
 send(null,{...frame({},tool?'tool_calls':'stop'),usage:{prompt_tokens:100,completion_tokens:30,total_tokens:130}});
}
function responses(send,model,tool,text){
 const initial=tool?{type:'function_call',id:'fc_'+tool.id,call_id:tool.id,name:tool.name,arguments:''}:{type:'message',id:'msg_local',role:'assistant',status:'in_progress',content:[]};
 const completed=tool?{...initial,arguments:JSON.stringify(tool.args),status:'completed'}:{...initial,status:'completed',content:[{type:'output_text',text,annotations:[]}]};
 const response={id:'resp_local',object:'response',model,status:'in_progress',output:[]};
 send('response.created',{type:'response.created',response});send('response.output_item.added',{type:'response.output_item.added',output_index:0,item:initial});
 if(tool){send('response.function_call_arguments.delta',{type:'response.function_call_arguments.delta',output_index:0,item_id:initial.id,delta:JSON.stringify(tool.args)});send('response.function_call_arguments.done',{type:'response.function_call_arguments.done',output_index:0,item_id:initial.id,arguments:JSON.stringify(tool.args)});}
 else send('response.output_text.delta',{type:'response.output_text.delta',output_index:0,content_index:0,item_id:initial.id,delta:text});
 send('response.output_item.done',{type:'response.output_item.done',output_index:0,item:completed});
 send('response.completed',{type:'response.completed',response:{...response,status:'completed',output:[completed],usage:{input_tokens:100,output_tokens:30,total_tokens:130,input_tokens_details:{cached_tokens:0},output_tokens_details:{reasoning_tokens:0}}}});
}
