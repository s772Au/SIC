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

test('actual SDK Worker continues after higher V and consumes that V once with fresh same-level sources',{timeout:40000},async()=>{
 const server=await isolated(),ctx=new Context(),root=server.root;let scope,lastOptions,nativeRoute;
 try{
  await ctx.plugin(SystemPrompt,{includeHarnessIdentity:false});await ctx.plugin(ToolRuntime);await ctx.plugin(PatchedRuntime);
  const native=await readFile(new URL('../transparent/bundle/native.md',import.meta.url),'utf8');ctx.systemPrompt.section({name:'reviewed-native-fixture',order:0,text:native,interpolate:false});
  const definitions=JSON.parse(await readFile(new URL('../transparent/bundle/native-tools.json',import.meta.url),'utf8'));
  for(const t of definitions)ctx.tools.register({...t,output:{schema:{},render:(_a,v)=>[{type:'text',text:JSON.stringify(v)}]},execute:async()=>{throw Error('old executor must not run');}});
  const session=sessionAPI.Session.create('session'),agent={id:'session',status:'idle',session};
  scope=createScope(ctx,agent);agent.ctx=scope.ctx;
  let step=1,editorCalls=0,higherCalls=0,cut=0,current,sourceCount=0,goal=null,nativeGoalPort,goalChanged=0;const sent=[],operations=[],sources=new Map(),cbs=[],cbOrigins=[];
  class Model extends api.LlmAdapter{async *stream(request){
   assert.equal(request.temperature,0.3);assert.equal(request.maxTokens,128);
   if(getEditorInvocation()){
    editorCalls++;const wb=JSON.parse(request.messages.at(-1).content[0].text),auth=JSON.parse(request.messages.findLast(m=>m.source?.kind==='runtime-context').source.sections.find(s=>s.name==='sic:activation').text);assert.ok(wb.units.length>0);let ops=[];if(auth.level===2){higherCalls++;assert.ok(auth.edit_capabilities.pvi.includes('V'));if(higherCalls===1)ops=wb.units.map(u=>({op:'COMPRESS',target:u.unit_id,content:'deferred '+u.text,lifecycle:'V'}));else assert.deepEqual(wb.units.map(u=>u.text),['deferred material-1','deferred material-2','material-3','material-4']);}
    yield{type:'tool-call-delta',index:0,id:'editor-'+editorCalls,name:'sic_edit_blocks',argumentsDelta:JSON.stringify({ops,pvi:{}})};yield{type:'finish',reason:{kind:'tool-calls'}};return;
   }
   const selected=await server.core.operate('cut.get',{}),active=request.messages.filter(m=>m.source?.kind==='sic-cb');assert.deepEqual(active.map(m=>m.source.cb_id),selected.view.cbs.map(cb=>cb.cb_id));assert.deepEqual(active.map(m=>m.content[0].text),selected.view.cbs.map(cb=>cb.body));
   const protocolWire=request.provider==='private-a'?{a_dialogue:request.messages,a_controls:{model:request.model,temperature:request.temperature}}:{b_feed:{sequence:request.messages},b_choice:{target:request.model,max_output:request.maxTokens}};
   sent.push({selected_cbs:selected.view.cbs,serialized_payload:JSON.stringify(protocolWire),private_route:request.provider,private_model:request.model,opaque:request.vendorOpaque,frames:request.messages.map(m=>({speaker:m.role,id:m.id,data:m.content,source:m.source}))});
   yield{type:'text-delta',text:'original adapter success'};yield{type:'finish',reason:{kind:'stop'}};
  }}
  ctx.llm.registerAdapter(['private-a','private-b'],new Model());
  const handlers=new Map(),binding={chain_id:server.chain_id,scope_id:'scope',role:'WORKER',edit_capabilities:{ops:['KEEP','REPLACE','COMPRESS'],pvi:['P','V','I']}};
  const facade={llm:ctx.llm,tools:ctx.tools,agents:{list:()=>[agent],get:id=>id==='session'?agent:null,roots:()=>[agent],currentInitiator:()=>agent},jobs:{externalControlVersion:1,externalExecutorJoinVersion:1,joinExecutor:async()=>{},attachExternalLifecycle:()=>()=>{},externalLaunchKind:()=>null},get:()=>undefined,sessions:{get:id=>id==='session'?agent.session:null,flush:async()=>true},goals:{externalDriverVersion:1,defaultMaxGoalRounds:37,attachExternalState:(_agent,port)=>{nativeGoalPort=port;return()=>{}}},effect:()=>{},emit:()=>{},logger:{warn:()=>{}},
   sessionProjections:{stateOf:()=>({openTurnStartSeq:0,lastStepStartSeq:step})},on:(name,fn)=>{handlers.set(name,fn);return ctx.on(name,fn)},
   inject:(_deps,fn)=>_deps.some(d=>['subagents','workflowEngine'].includes(d))?{dispose:()=>{}}:fn({effect:body=>body(),connection:{fetch:{register:definition=>{nativeRoute=definition.fetch;return()=>{}}}}})};
  const installed=await apply(facade,{enabled:true,hierarchyPolicy:{max_level:2,level_batch_sizes:{2:2},max_host_editor_calls:16},mode:'edit',actor:'native-http-actor',coreURL:server.baseURL,credential:server.credential,projectionRoot:root,bindings:{session:binding},renderPrompt,
   llmAPI:api,sessionAPI,agentEvents:()=>({emit:()=>{goalChanged++;}})});
  assert.ok(handlers.has('llm/context-dispatch'));assert.equal(ctx.llm.contextDispatchVersion,1);
  const system=await readFile(new URL('../transparent/bundle/fused.md',import.meta.url),'utf8'),authorization={role:'WORKER',scope_id:'scope',edit_capabilities:{ops:['KEEP','REPLACE','COMPRESS'],pvi:['P','V','I']}};
  const sys={id:'system',role:'system',content:[{type:'text',text:system}],source:{kind:'system-prompt'}},runtime={id:'runtime',role:'user',content:[{type:'text',text:'runtime'}],source:{kind:'runtime-context',sections:[{name:'sic:activation',text:JSON.stringify(authorization)}]}};
  const first={id:'human-first',role:'user',content:[{type:'text',text:'first'}],source:{kind:'user'}},second={id:'human-second',role:'user',content:[{type:'text',text:'second'}],source:{kind:'user'}};
  async function send(history,provider='private-a'){
   const prepared=await ctx.llm.prepareCall({provider,model:'model-'+provider,temperature:0.3,maxTokens:128}),options={...prepared.config,sessionId:'session',messages:history,tools:ctx.tools.schemas(agent),vendorOpaque:{unknown:9007199254740991,signature:'do not change'}};
   lastOptions=options;for await(const chunk of prepared.stream(options))if(chunk.type==='finish')assert.equal(chunk.reason.kind,'stop',JSON.stringify(chunk));return sent.at(-1);
  }
  const materials=[];for(let round=1;round<=4;round++){
   step=round;materials.push(api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'material-'+round}]}));await send([sys,runtime,...materials]);
   assert.equal(sent.length,round,'a higher volatile wait must not deadlock the next Worker');
   if(round===2||round===3){const chain=await server.core.operate('chain.get',{});assert.ok(chain.contexts.main.levels['2'].pending.length>0);}
  }
  assert.equal(higherCalls,2);assert.equal(editorCalls,6);const final=sent.at(-1).selected_cbs,body=final.map(cb=>cb.body).filter(Boolean).join('\n');
  assert.ok(final.some(cb=>cb.level===2));assert.equal(body,'deferred material-1\ndeferred material-2\nmaterial-3\nmaterial-4');
  assert.deepEqual((await server.core.operate('chain.get',{})).contexts.main.levels['2'].pending,[]);
  const page=await installed.store.list('session');assert.equal(page.rows.length,4);const before=editorCalls;await send([sys,runtime,...materials]);assert.equal(editorCalls,before);assert.equal(sent.length,5);
  const records=[];for(const row of page.rows)records.push(await installed.store.get('session',row.id));
  assert.ok(records.some(r=>r.layout_receipt.hierarchy.reports.some(report=>report.wait?.code==='higher_volatile_input')));
  assert.ok(records.some(r=>r.layout_receipt.hierarchy.jobs.length===1));
 }finally{if(scope)await scope.dispose();await ctx.fiber.dispose();await server.close();}
});
