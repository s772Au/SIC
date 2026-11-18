import {dshSourcePath,fixturePath} from './test-paths.mjs';
import{test}from'node:test';import assert from'node:assert/strict';import{readFile,mkdtemp,rm}from'node:fs/promises';import{tmpdir}from'node:os';import{join}from'node:path';import{pathToFileURL}from'node:url';
import{apply}from'../transparent/plugin.mjs';import{isolated}from'./isolated-core-fixture.mjs';import{prepareSdkRuntime}from'../transparent/prepare-sdk-runtime.mjs';import{getEditorInvocation}from'../transparent/editor-invocation.mjs';import{nativeExchange}from'../transparent/native-boundary.mjs';import{digest}from'../transparent/transport.mjs';
const dsh=dshSourcePath();
const load=p=>import(pathToFileURL(join(dsh,p)).href);
const sessionAPI=await load('packages/core/session/lib/index.js');
const runtimePath=await prepareSdkRuntime(dsh,fixturePath('dsh-sdk-fixture-20261117')),{default:PatchedRuntime}=await import(pathToFileURL(runtimePath).href);
const{Context}=await load('vendor/cordis/lib/index.js');
const api=await load('packages/llm/llm/lib/index.js');
const{default:SystemPrompt,renderPrompt}=await load('packages/core/system-prompt/lib/index.js');
const{default:ToolRuntime}=await load('packages/core/tools/lib/index.js');const{createScope}=await load('packages/core/scope/lib/index.js');

test('actual Core HTTP plus common original LlmRuntime SDK route edits private API adapters and drains Rust batch-2 higher plans through level 3 before each of five original adapter calls',{timeout:40000},async()=>{
 const server=await isolated(),ctx=new Context(),root=server.root;let scope,lastOptions,nativeRoute;
 try{
  await ctx.plugin(SystemPrompt,{includeHarnessIdentity:false});await ctx.plugin(ToolRuntime);await ctx.plugin(PatchedRuntime);
  const native=await readFile(new URL('../transparent/bundle/native.md',import.meta.url),'utf8');ctx.systemPrompt.section({name:'reviewed-native-fixture',order:0,text:native,interpolate:false});
  const definitions=JSON.parse(await readFile(new URL('../transparent/bundle/native-tools.json',import.meta.url),'utf8'));
  for(const t of definitions)ctx.tools.register({...t,output:{schema:{},render:(_a,v)=>[{type:'text',text:JSON.stringify(v)}]},execute:async()=>{throw Error('old executor must not run');}});
  const session=sessionAPI.Session.create('session'),agent={id:'session',status:'idle',session};
  scope=createScope(ctx,agent);agent.ctx=scope.ctx;
  let step=1,editorCalls=0,cut=0,current,sourceCount=0,goal=null,nativeGoalPort,goalChanged=0;const sent=[],operations=[],sources=new Map(),cbs=[],cbOrigins=[];
  class Model extends api.LlmAdapter{async *stream(request){
   assert.equal(request.temperature,0.3);assert.equal(request.maxTokens,128);
   if(getEditorInvocation()){
    editorCalls++;const wb=JSON.parse(request.messages.at(-1).content[0].text);assert.ok(wb.units.length>0);
    yield{type:'tool-call-delta',index:0,id:'editor-'+editorCalls,name:'sic_edit_blocks',argumentsDelta:JSON.stringify({ops:[],pvi:{}})};yield{type:'finish',reason:{kind:'tool-calls'}};return;
   }
   const selected=await server.core.operate('cut.get',{}),active=request.messages.filter(m=>m.source?.kind==='sic-cb');assert.deepEqual(active.map(m=>m.source.cb_id),selected.view.cbs.map(cb=>cb.cb_id));assert.deepEqual(active.map(m=>m.content[0].text),selected.view.cbs.map(cb=>cb.body));
   const protocolWire=request.provider==='private-a'?{a_dialogue:request.messages,a_controls:{model:request.model,temperature:request.temperature}}:{b_feed:{sequence:request.messages},b_choice:{target:request.model,max_output:request.maxTokens}};
   sent.push({selected_cbs:selected.view.cbs,serialized_payload:JSON.stringify(protocolWire),private_route:request.provider,private_model:request.model,opaque:request.vendorOpaque,frames:request.messages.map(m=>({speaker:m.role,id:m.id,data:m.content,source:m.source}))});
   yield{type:'text-delta',text:'original adapter success'};yield{type:'finish',reason:{kind:'stop'}};
  }}
  ctx.llm.registerAdapter(['private-a','private-b'],new Model());
  const handlers=new Map(),binding={chain_id:server.chain_id,scope_id:'scope',role:'WORKER',edit_capabilities:{ops:['KEEP','REPLACE','COMPRESS'],pvi:['I']}};
  const facade={llm:ctx.llm,tools:ctx.tools,agents:{list:()=>[agent],get:id=>id==='session'?agent:null,roots:()=>[agent],currentInitiator:()=>agent},jobs:{externalControlVersion:1,externalExecutorJoinVersion:1,joinExecutor:async()=>{},attachExternalLifecycle:()=>()=>{},externalLaunchKind:()=>null},get:()=>undefined,sessions:{get:id=>id==='session'?agent.session:null,flush:async()=>true},goals:{externalDriverVersion:1,defaultMaxGoalRounds:37,attachExternalState:(_agent,port)=>{nativeGoalPort=port;return()=>{}}},effect:()=>{},emit:()=>{},logger:{warn:()=>{}},
   sessionProjections:{stateOf:()=>({openTurnStartSeq:0,lastStepStartSeq:step})},on:(name,fn)=>{handlers.set(name,fn);return ctx.on(name,fn)},
   inject:(_deps,fn)=>_deps.some(d=>['subagents','workflowEngine'].includes(d))?{dispose:()=>{}}:fn({effect:body=>body(),connection:{fetch:{register:definition=>{nativeRoute=definition.fetch;return()=>{}}}}})};
  const installed=await apply(facade,{enabled:true,hierarchyPolicy:{max_level:3,level_batch_sizes:{2:2,3:2},max_host_editor_calls:16},mode:'edit',actor:'native-http-actor',coreURL:server.baseURL,credential:server.credential,projectionRoot:root,bindings:{session:binding},renderPrompt,
   llmAPI:api,sessionAPI,agentEvents:()=>({emit:()=>{goalChanged++;}})});
  assert.ok(handlers.has('llm/context-dispatch'));assert.equal(ctx.llm.contextDispatchVersion,1);
  const system=await readFile(new URL('../transparent/bundle/fused.md',import.meta.url),'utf8'),authorization={role:'WORKER',scope_id:'scope',edit_capabilities:{ops:['KEEP','REPLACE','COMPRESS'],pvi:['I']}};
  const sys={id:'system',role:'system',content:[{type:'text',text:system}],source:{kind:'system-prompt'}},runtime={id:'runtime',role:'user',content:[{type:'text',text:'runtime'}],source:{kind:'runtime-context',sections:[{name:'sic:activation',text:JSON.stringify(authorization)}]}};
  const first={id:'human-first',role:'user',content:[{type:'text',text:'first'}],source:{kind:'user'}},second={id:'human-second',role:'user',content:[{type:'text',text:'second'}],source:{kind:'user'}};
  async function send(history,provider='private-a'){
   const prepared=await ctx.llm.prepareCall({provider,model:'model-'+provider,temperature:0.3,maxTokens:128}),options={...prepared.config,sessionId:'session',messages:history,tools:ctx.tools.schemas(agent),vendorOpaque:{unknown:9007199254740991,signature:'do not change'}};
   lastOptions=options;for await(const chunk of prepared.stream(options))if(chunk.type==='finish')assert.equal(chunk.reason.kind,'stop',JSON.stringify(chunk));return sent.at(-1);
  }
  const materials=[];for(let round=1;round<=5;round++){step=round;materials.push(api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'material-'+round}]}));await send([sys,runtime,...materials]);}
  const final=sent.at(-1).selected_cbs;assert.ok(final.some(cb=>cb.level===3));assert.ok(final.some(cb=>cb.level===1));assert.equal(final.map(cb=>cb.body).join('\n'),'material-1\nmaterial-2\nmaterial-3\nmaterial-4\nmaterial-5');assert.ok(editorCalls>5);
  const page=await installed.store.list('session');assert.equal(page.rows.length,5);let highest;
  for(const row of page.rows){const record=await installed.store.get('session',row.id);assert.ok(record.layout_receipt.hierarchy.recorded);if(record.layout_receipt.hierarchy.jobs.some(job=>job.level===3))highest=record;}
  assert.ok(highest);assert.ok((await installed.store.traces('session',highest.source_id)).rows.some(event=>event.stage==='hierarchy-plan'));
  const manual=await installed.hierarchy.scheduleCompaction({agent,binding,request_id:'manual-after-five',trigger:'manual',signal:new AbortController().signal,resolveCall:async()=>({options:lastOptions})});assert.equal(manual.recorded,true);assert.equal(sent.length,5,'manual hierarchy execution must not run a Worker');
  const manualRecords=(await installed.store.list('session')).rows;let manualRecord;for(const row of manualRecords){const candidate=await installed.store.get('session',row.id);if(candidate.source_kind==='host-hierarchy')manualRecord=candidate;}assert.ok(manualRecord);
  for(const action of ['get','history']){const response=await nativeRoute(new Request('http://fixture/native-projections',{method:'POST',body:JSON.stringify({sessionId:'session',action,projectionId:manualRecord.id})}));assert.equal(response.status,200);const value=(await response.json()).value;if(action==='history')assert.ok(value.core.reports);else assert.equal(value.source_kind,'host-hierarchy');}
  session.append('user/message',api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'continue to complete the task'}]}),{surfaceOp:'append'});agent.status='running';
  const goalResult=await ctx.tools.execute({agent,name:'create_goal',arguments:{objective:'native goal through main plugin'},callId:'host-create-once',signal:new AbortController().signal});
  assert.equal(goalResult.isError,false);assert.equal(typeof nativeGoalPort.get().id,'string');assert.equal(nativeGoalPort.get().objective,'native goal through main plugin');assert.ok(goalChanged>0);agent.status='idle';
  assert.doesNotThrow(()=>sessionAPI.Session.create('session',JSON.parse(JSON.stringify(agent.session.snapshotEvents()))));
 }finally{if(scope)await scope.dispose();await ctx.fiber.dispose();await server.close();}
});
