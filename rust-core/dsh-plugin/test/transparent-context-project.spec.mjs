import {dshSourcePath,fixturePath} from './test-paths.mjs';
import{test}from'node:test';import assert from'node:assert/strict';import{join}from'node:path';import{pathToFileURL}from'node:url';
import{repackageNativeContext}from'../transparent/native-layout.mjs';
import{projectHarnessContext}from'../transparent/context-project.mjs';import{prepareDeepSeekFixture}from'../transparent/prepare-deepseek-fixture.mjs';import{createHarnessEditorDriver}from'../transparent/editor-driver.mjs';import{getEditorInvocation}from'../transparent/editor-invocation.mjs';import{digest}from'../transparent/transport.mjs';
const dsh=dshSourcePath(),load=p=>import(pathToFileURL(join(dsh,p)).href);
const api=await load('packages/llm/llm/lib/index.js'),{Context}=await load('vendor/cordis/lib/index.js');
const module=await prepareDeepSeekFixture(dsh,fixturePath('dsh-context-fixture-20261117')),{DeepSeekAdapter,resolveAdapterOptions}=await import(pathToFileURL(module).href);
function sse(events){return new Response(events.map(e=>`event: ${e.type}\ndata: ${JSON.stringify(e)}\n\n`).join(''),{headers:{'content-type':'text/event-stream'}});}
function reply(editor){return sse([{type:'message_start',message:{id:editor?'e':'w',model:'deepseek-v4-flash',usage:{input_tokens:1,output_tokens:0}}},...editor?[{type:'content_block_start',index:0,content_block:{type:'tool_use',id:'delivery',name:'sic_edit_blocks',input:{}}},{type:'content_block_delta',index:0,delta:{type:'input_json_delta',partial_json:JSON.stringify({ops:[{op:'COMPRESS',target:'new',content:'short'}],pvi:{}})}}]:[{type:'content_block_start',index:0,content_block:{type:'text',text:''}},{type:'content_block_delta',index:0,delta:{type:'text_delta',text:'ok'}}],{type:'content_block_stop',index:0},{type:'message_delta',delta:{stop_reason:editor?'tool_use':'end_turn'},usage:{output_tokens:1}},{type:'message_stop'}]);}
test('real DSH serializer sends the same reviewed system/tools/selected CB prefix to Worker and native Editor, with I and role after it',async()=>{
 const ctx=new Context(),system='reviewed common system';let projection,workerWire,editorWire,driver,adoptedWire,finalProjection;
 const old=api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'archived material'}]}),fresh=api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'new material'}]}),
 runtime=api.createUserMessage({source:{kind:'runtime-context',sections:[{name:'environment',text:'host environment'},{name:'sic:activation',text:'WORKER'}]},content:[{type:'text',text:'old snapshot'}]});
 const cut={cut_id:'cut-3',sequence:3,instant_view_text:'{"cut_id":"cut-3","views":{"current":{"decision":"keep"}}}',instant_view_digest:'view-digest',view:{cbs:[{cb_id:'higher-cb',body:'higher-order selected body',level:3}],cb_origin_bindings:[{cb_id:'higher-cb',origins:[{message_id:old.id,block_index:0}],missing_source_ids:[]}]}};
 const adapter=new DeepSeekAdapter({options:()=>resolveAdapterOptions({baseURL:'https://fixture.invalid/anthropic'}),resolveAuth:async()=>({headers:{'x-api-key':'synthetic'}}),resolveUserId:()=> 'fixture',resolveAttachments:()=>undefined,resolveImageAccess:()=>undefined,prepareExtensions:async()=>({fields:{},accept:async()=>{}}),
  projectContext:async(history,options)=>{if(getEditorInvocation()){getEditorInvocation().verifyNativeOptions(options);return{history};}projection=await projectHarnessContext({history,options,cut,requestId:'sdk-request',api,system});return{history:projection.projected_history,projection};},
  fetch:async(_url,init,provenance)=>{
   const native=JSON.parse(init.body),editor=getEditorInvocation();if(editor){editor.verifyNativeOptions(provenance.options);editorWire=native;return reply(true);}
   workerWire=native;const source={source_id:'src',source_digest:digest(native),version:0,binding:{editable_unit_ids:['new']},units:[{unit_id:'new',text:'new material',anchor:['messages',0,'content',1,'text']}]};
   const delivered=await driver({source,invocation:{session_id:'fixture'},originalOptions:provenance.options,shared_messages:projection.shared_messages,runtime_sections:projection.runtime_sections,instant_message:projection.instant_message});assert.equal(delivered.ops[0].target,'new');
   const next={...cut,cut_id:'cut-4',sequence:4,instant_view_text:'{"cut_id":"cut-4","views":{"current":{"decision":"new decision"}}}',instant_view_digest:'new-view',view:{cbs:[...cut.view.cbs,{cb_id:'new-cb',body:'short',level:1}],cb_origin_bindings:[...cut.view.cb_origin_bindings,{cb_id:'new-cb',origins:[{message_id:fresh.id,block_index:0}],missing_source_ids:[]}]}};
   finalProjection=await projectHarnessContext({history:projection.original_history,options:provenance.options,cut:next,requestId:'sdk-request',api,system});
   const raw=JSON.stringify(native).replace('"new material"','"short"');
   adoptedWire=JSON.parse(repackageNativeContext(raw,provenance.serializeHistory(finalProjection.projected_history),projection.projected_history,finalProjection.projected_history));return reply(false);
  }});
 try{await ctx.plugin(api.default);ctx.llm.registerAdapter(['original'],adapter);driver=await createHarnessEditorDriver({llm:ctx.llm,llmAPI:api,record:async()=>{},authorizeEditor:async()=>({role:'EDITOR',scope_id:'scope',source_id:'src',source_digest:digest(workerWire),expected_version:0,assigned_unit_ids:['new'],allowed_tool_names:['sic_edit_blocks'],edit_capabilities:{ops:['COMPRESS'],pvi:['I']}})});
  const prepared=await ctx.llm.prepareCall({provider:'original',model:'deepseek-v4-flash',temperature:0.3,maxTokens:123});
  const tools=[{name:'sic_edit_blocks',description:'Deliver native candidate',parameters:{type:'object'}}];
  for await(const chunk of prepared.stream({...prepared.config,sessionId:'fixture',system,tools,messages:[api.createSystemMessage(system),runtime,old,fresh]})){if(chunk.type==='error')throw chunk.error;if(chunk.type==='finish')assert.notEqual(chunk.reason.kind,'error',JSON.stringify(chunk));}
  assert.deepEqual(workerWire.system,editorWire.system);assert.deepEqual(workerWire.tools,editorWire.tools);assert.equal(workerWire.model,editorWire.model);assert.equal(workerWire.max_tokens,editorWire.max_tokens);
  assert.deepEqual(workerWire.messages[0].content[0],editorWire.messages[0].content[0]);assert.equal(editorWire.messages[0].content[0].text,cut.view.cbs[0].body);
  assert.equal(workerWire.messages.some(m=>m.content.some(b=>b.text==='archived material')),false);assert.equal(workerWire.messages[0].content[1].text,'new material');
  assert.equal(editorWire.messages[0].content.some(b=>b.text==='new material'),false);
  const controlIndex=editorWire.messages[0].content.findIndex(b=>b.text?.includes('host environment')&&b.text.includes('authenticated role for this invocation is EDITOR'));
  const instantIndex=editorWire.messages[0].content.findIndex(b=>b.text===projection.instant_message.content[0].text);
  assert.ok(controlIndex>instantIndex&&instantIndex>0,'current authenticated control follows the readonly I view and common CB prefix');
  assert.deepEqual(adoptedWire.messages[0].content.slice(0,2).map(b=>b.text),['higher-order selected body','short']);assert.ok(adoptedWire.messages[0].content.some(b=>b.text===finalProjection.instant_message.content[0].text));assert.equal(adoptedWire.messages[0].content.some(b=>b.text==='new material'),false);
  assert.equal(projection.instant_message.content[0].text,'sic:instant-views\n'+cut.instant_view_text);assert.ok(editorWire.messages[0].content.some(b=>b.text===projection.instant_message.content[0].text));
 }finally{await ctx.fiber.dispose();}
});
test('selected CB without immutable source origin cannot silently delete or guess historical text',async()=>{
 await assert.rejects(projectHarnessContext({history:[],cut:{view:{cbs:[{cb_id:'old',body:'same words'}],cb_origin_bindings:[{cb_id:'old',origins:[],missing_source_ids:['legacy-source']}]}},requestId:'r',api,system:'s'}),/migration/);
});

test('canonical native structural layout preserves outer opaque lexemes and exact signed block raw fragments',()=>{
 const beforeHistory=[{id:'old',role:'user',source:{kind:'user'},content:[{type:'text',text:'old'}]},{id:'model',role:'assistant',content:[{type:'reasoning',text:'reason'}]}];
 const afterHistory=[{id:'cb',role:'user',content:[{type:'text',text:'canonical CB'}]},beforeHistory[1]];
 const raw='{ "system":"same", "messages":[{"role":"user","content":[{"type":"text","text":"old"}]},{"role":"assistant","content":[{"type":"thinking","thinking":"reason","signature":"\\u0061","vendor":{"big":900719925474099312345}}]}],"opaque":900719925474099398765,"n":2,"stream":true }';
 const serialized=JSON.parse(raw);serialized.messages[0].content[0].text='canonical CB';
 const result=repackageNativeContext(raw,serialized,beforeHistory,afterHistory);
 assert.ok(result.includes('"opaque":900719925474099398765'));assert.ok(result.includes('"signature":"\\u0061","vendor":{"big":900719925474099312345}'));assert.equal(JSON.parse(result).messages[0].content[0].text,'canonical CB');
 const corrupted=structuredClone(serialized);corrupted.messages[1].content[0].signature='changed';assert.throws(()=>repackageNativeContext(raw,corrupted,beforeHistory,afterHistory),/opaque native block changed/);
});
test('tool result source text maps nested anchors while canonical adoption preserves call/error metadata and untouched result text',async()=>{
 const{messagesOrigins}=await import('../transparent/origins.mjs');
 const beforeHistory=[{id:'q',role:'user',source:{kind:'user'},content:[{type:'text',text:'question'}]},{id:'a',role:'assistant',source:{kind:'model'},content:[{type:'tool-call',id:'call1',name:'read',arguments:'{}'}]},{id:'result',role:'tool',toolCallId:'call1',isError:false,source:{kind:'tool',callId:'call1'},content:[{type:'text',text:'collected body'},{type:'text',text:'status footer'}]}];
 const raw='{"system":"protected","messages":[{"role":"user","content":[{"type":"text","text":"question"}]},{"role":"assistant","content":[{"type":"tool_use","id":"call1","name":"read","input":{}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"call\\u0031","is_error":false,"opaque":900719925474099312345,"content":[{"type":"text","text":"collected body"},{"type":"text","text":"status footer"}]}]}],"stream":true}';
 const native=JSON.parse(raw),origin=messagesOrigins(beforeHistory,native).find(o=>o.message_id==='result'&&o.block_index===0);assert.deepEqual(origin.anchor,['messages',2,'content',0,'content',0,'text']);assert.equal(origin.call_id,'call1');
 const afterHistory=[{id:'cb',role:'user',source:{kind:'sic-cb'},content:[{type:'text',text:'canonical summary'}]},beforeHistory[1],{...beforeHistory[2],content:[{type:'text',text:''},beforeHistory[2].content[1]]}],serialized=structuredClone(native);serialized.messages[0].content[0].text='canonical summary';serialized.messages[2].content[0].content.shift();
 const adopted=repackageNativeContext(raw,serialized,beforeHistory,afterHistory);assert.ok(adopted.includes('"tool_use_id":"call\\u0031"'));assert.ok(adopted.includes('"opaque":900719925474099312345'));assert.deepEqual(JSON.parse(adopted).messages[2].content[0].content,[{type:'text',text:'status footer'}]);
 const forged=structuredClone(native);forged.messages[2].content[0].tool_use_id='other-call';assert.throws(()=>messagesOrigins(beforeHistory,forged),/identity mismatch/);
});

test('actual native serializer retains tool linkage with an explicit readonly archival pointer instead of a misleading empty result',async()=>{
 const ctx=new Context();let wire,plan;
 const caller=api.freezeMessage({id:'native-caller',role:'assistant',source:{kind:'model'},content:[{type:'tool-call',id:'native-call',name:'read',arguments:'{}'}]});
 const result=api.createToolResultMessage({callId:'native-call',isError:false,content:[{type:'text',text:'ARCHIVED_PRIVATE_BODY'}]});
 const runtime=api.createUserMessage({source:{kind:'runtime-context',sections:[{name:'sic:activation',text:JSON.stringify({role:'WORKER'})}]},content:[{type:'text',text:'current'}]});
 const body='current retained summary',range={utf8_start:0,utf8_end:Buffer.byteLength(body)},cut={cut_id:'cut',sequence:1,view:{cbs:[{cb_id:'cb',body,level:1}],cb_origin_bindings:[{cb_id:'cb',missing_source_ids:[],origins:[{message_id:result.id,block_index:0,retained_in_p:true,p_paths:[['cb']],p_text_ranges:[range],p_spans:[{cb_id:'cb',range,relation:'materialized_unit'}]}]}]}};
 const adapter=new DeepSeekAdapter({options:()=>resolveAdapterOptions({baseURL:'https://fixture.invalid/anthropic'}),resolveAuth:async()=>({headers:{'x-api-key':'synthetic'}}),resolveUserId:()=> 'fixture',resolveAttachments:()=>undefined,resolveImageAccess:()=>undefined,
  prepareExtensions:async()=>({fields:{},accept:async()=>{}}),projectContext:async(history,options)=>{plan=await projectHarnessContext({history,options,cut,requestId:'r',api,system:'reviewed'});return{history:plan.projected_history};},fetch:async(_url,init)=>{wire=JSON.parse(init.body);return reply(false);}});
 try{await ctx.plugin(api.default);ctx.llm.registerAdapter(['native-pointer'],adapter);const call=await ctx.llm.prepareCall({provider:'native-pointer',model:'deepseek-v4-flash',maxTokens:32});
  for await(const chunk of call.stream({...call.config,messages:[api.createSystemMessage('reviewed'),runtime,caller,result]}))if(chunk.type==='finish')assert.equal(chunk.reason.kind,'stop');
  const tool=wire.messages.flatMap(message=>message.content).find(block=>block.type==='tool_result');assert.equal(tool.tool_use_id,'native-call');assert.equal(tool.is_error,false);
  assert.match(tool.content[0].text,/Host context projection annotation/);assert.match(tool.content[0].text,/not original tool output/);assert.match(tool.content[0].text,/not a new execution/);
  assert.equal(JSON.stringify(wire).includes('ARCHIVED_PRIVATE_BODY'),false);assert.ok(plan.masked_origins.some(origin=>origin.message_id===result.id));
  assert.equal(plan.origin_message.source.tool_origins[0].placeholder_text,tool.content[0].text);assert.equal(result.content[0].text,'ARCHIVED_PRIVATE_BODY');
 }finally{await ctx.fiber.dispose();}
});
