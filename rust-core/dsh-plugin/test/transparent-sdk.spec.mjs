import {dshSourcePath,fixturePath} from './test-paths.mjs';
import{test}from'node:test';import assert from'node:assert/strict';import{readFile,mkdtemp,rm}from'node:fs/promises';import{tmpdir}from'node:os';import{join}from'node:path';import{pathToFileURL}from'node:url';
import{apply}from'../transparent/plugin.mjs';import{prepareSdkRuntime}from'../transparent/prepare-sdk-runtime.mjs';import{getEditorInvocation}from'../transparent/editor-invocation.mjs';import{nativeExchange}from'../transparent/native-boundary.mjs';import{digest}from'../transparent/transport.mjs';
const dsh=dshSourcePath();
const load=p=>import(pathToFileURL(join(dsh,p)).href);
const sessionAPI=await load('packages/core/session/lib/index.js');
const runtimePath=await prepareSdkRuntime(dsh,fixturePath('dsh-sdk-fixture-20261117')),{default:PatchedRuntime}=await import(pathToFileURL(runtimePath).href);
const{Context}=await load('vendor/cordis/lib/index.js');
const api=await load('packages/llm/llm/lib/index.js');
const{default:SystemPrompt,renderPrompt}=await load('packages/core/system-prompt/lib/index.js');
const{default:ToolRuntime}=await load('packages/core/tools/lib/index.js');const{createScope}=await load('packages/core/scope/lib/index.js');

test('common real LlmRuntime SDK seam edits before arbitrary Provider serialization, preserving original opaque SDK data and model controls',async()=>{
 const ctx=new Context(),root=await mkdtemp(join(tmpdir(),'sic-plugin-'));let scope;
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
    editorCalls++;const wb=JSON.parse(request.messages.at(-1).content[0].text);assert.equal(wb.units.length,1);
    yield{type:'tool-call-delta',index:0,id:'editor-'+editorCalls,name:'sic_edit_blocks',argumentsDelta:JSON.stringify({ops:[{op:'COMPRESS',target:wb.units[0].unit_id,content:editorCalls===1?'short-first':'short-second'}],pvi:{}})};yield{type:'finish',reason:{kind:'tool-calls'}};return;
   }
   const protocolWire=request.provider==='private-a'?{a_dialogue:request.messages,a_controls:{model:request.model,temperature:request.temperature}}:{b_feed:{sequence:request.messages},b_choice:{target:request.model,max_output:request.maxTokens}};
   sent.push({serialized_payload:JSON.stringify(protocolWire),private_route:request.provider,private_model:request.model,opaque:request.vendorOpaque,frames:request.messages.map(m=>({speaker:m.role,id:m.id,data:m.content,source:m.source}))});
   yield{type:'text-delta',text:'original adapter success'};yield{type:'finish',reason:{kind:'stop'}};
  }}
  ctx.llm.registerAdapter(['private-a','private-b'],new Model());
  const coreFetch=async(url,init)=>{
   const body=JSON.parse(init.body);if(new URL(url).pathname==='/api/v2/harness/turn'){assert.equal(body.human_intent,'direct_human');return Response.json({grant_id:'real-host-bridge-fixture'});}const p=body.payload;operations.push(body.operation);
   let result;
   if(body.operation==='harness.call'){if(p.name==='create_goal')goal={id:'core-goal',revision:1,objective:p.arguments.objective,phase:'active',maxGoalRounds:37,roundsStarted:0};result={value:{goal,activation:goal?'armed':null}};}
   else if(body.operation==='harness.get')result={revision:1,roundsStarted:0,roundsFinished:0};
   else if(body.operation==='cut.get')result={cut_id:cut?'cut-'+cut:null,sequence:cut,view:{cbs:[...cbs],cb_origin_bindings:[...cbOrigins]}};
   else if(body.operation==='chain.get')result={chain_id:'chain',contexts:{main:{levels:{'1':{pending:[]}}}}};
   else if(body.operation==='chain.list')result={routes:[{chain_id:'chain',cut_seq:cut}]};
   else if(body.operation==='native.begin'){
    if(sources.has(p.idempotency_key))return Response.json({result:sources.get(p.idempotency_key)});
    assert.equal(p.format,'dsh-messages');const request=p.request,units=[];
    request.messages.forEach((m,i)=>{if(['system','developer'].includes(m.role))return;m.content.forEach((b,j)=>{if(b.type==='text')units.push({unit_id:'u'+(units.length+1),text:b.text,anchor:['messages',i,'content',j,'text']});});});
    if(cut>0)assert.ok(p.protected_cb_bindings.length);
    current={source_id:'source-'+(++sourceCount),source_digest:digest(request),version:0,units,source_request:structuredClone(request),request:structuredClone(request),receipt:null,binding:{editable_unit_ids:units.filter(u=>p.editable_anchors.some(a=>JSON.stringify(a)===JSON.stringify(u.anchor))).map(u=>u.unit_id)}};sources.set(p.idempotency_key,current);current.key=p.idempotency_key;current.host_origins=p.host_message_origins;result=current;
   }else if(body.operation==='native.get'){result=current;
   }else if(body.operation==='native.commit'){
    for(const e of p.edits){const unit=current.units.find(u=>u.unit_id===e.unit_id);let target=current.request;for(const k of unit.anchor.slice(0,-1))target=target[k];target[unit.anchor.at(-1)]=e.replacement;}
    current={...current,version:1,receipt:{version:1},current_delivery:p.delivery,delivery_version:1};result=current;
   }else if(body.operation==='native.publish'){
    const editable=current.units.filter(u=>current.binding.editable_unit_ids.includes(u.unit_id));
    const origin=current.host_origins.find(o=>JSON.stringify(o.native_anchor)===JSON.stringify(editable[0].anchor));
    cbs.push({cb_id:'cb-'+(cut+1),body:editable[0].anchor.reduce((v,k)=>v[k],current.request),level:1});cbOrigins.push({cb_id:'cb-'+(cut+1),origins:[origin],missing_source_ids:[]});
    current={...current,publication:{publication_id:'pub-'+(++cut),cb_id:'cb-'+cut,publication:{wb_id:'wb-'+cut,cut_id:'cut-'+cut,sequence:cut}}};sources.set(current.key,current);result=current;
   }else throw Error('unexpected Core operation '+body.operation);
   return Response.json({result});
  };
  const handlers=new Map(),binding={chain_id:'chain',scope_id:'scope',role:'WORKER',edit_capabilities:{ops:['KEEP','REPLACE','COMPRESS'],pvi:['I']}};
  const facade={llm:ctx.llm,tools:ctx.tools,agents:{list:()=>[agent],get:id=>id==='session'?agent:null,roots:()=>[agent],currentInitiator:()=>agent},jobs:{externalControlVersion:1,externalExecutorJoinVersion:1,joinExecutor:async()=>{},attachExternalLifecycle:()=>()=>{},externalLaunchKind:()=>null},get:()=>undefined,sessions:{get:id=>id==='session'?agent.session:null,flush:async()=>true},goals:{externalDriverVersion:1,defaultMaxGoalRounds:37,attachExternalState:(_agent,port)=>{nativeGoalPort=port;return()=>{}}},effect:()=>{},emit:()=>{},logger:{warn:()=>{}},
   sessionProjections:{stateOf:()=>({openTurnStartSeq:0,lastStepStartSeq:step})},on:(name,fn)=>{handlers.set(name,fn);return ctx.on(name,fn)},
   inject:(_deps,fn)=>_deps.some(d=>['subagents','workflowEngine'].includes(d))?{dispose:()=>{}}:fn({effect:body=>body(),connection:{fetch:{register:()=>()=>{}}}})};
  const installed=await apply(facade,{enabled:true,drainHigher:async()=>({recorded:true,receipt_id:'fixture-higher',progressed:false}),mode:'edit',actor:'host',coreURL:'http://127.0.0.1:18890',credential:()=> 'synthetic',projectionRoot:root,bindings:{session:binding},renderPrompt,coreFetch,
   admitFinal:async()=>{},llmAPI:api,sessionAPI,agentEvents:()=>({emit:()=>{goalChanged++;}})});
  assert.ok(handlers.has('llm/context-dispatch'));assert.equal(ctx.llm.contextDispatchVersion,1);
  const system=await readFile(new URL('../transparent/bundle/fused.md',import.meta.url),'utf8'),authorization={role:'WORKER',scope_id:'scope',edit_capabilities:{ops:['KEEP','REPLACE','COMPRESS'],pvi:['I']}};
  const sys={id:'system',role:'system',content:[{type:'text',text:system}],source:{kind:'system-prompt'}},runtime={id:'runtime',role:'user',content:[{type:'text',text:'runtime'}],source:{kind:'runtime-context',sections:[{name:'sic:activation',text:JSON.stringify(authorization)}]}};
  const first={id:'human-first',role:'user',content:[{type:'text',text:'first'}],source:{kind:'user'}},second={id:'human-second',role:'user',content:[{type:'text',text:'second'}],source:{kind:'user'}};
  async function send(history,provider='private-a'){
   const prepared=await ctx.llm.prepareCall({provider,model:'model-'+provider,temperature:0.3,maxTokens:128}),options={...prepared.config,sessionId:'session',messages:history,tools:(await ctx.systemPrompt.assemble({scope:agent})).tools,vendorOpaque:{unknown:9007199254740991,signature:'do not change'}};
   for await(const chunk of prepared.stream(options))if(chunk.type==='finish')assert.equal(chunk.reason.kind,'stop',JSON.stringify(chunk));return sent.at(-1);
  }
  const firstSent=await send([sys,runtime,first]);assert.equal(firstSent.frames.find(m=>m.source?.kind==='sic-cb').data[0].text,'short-first');step=2;
  const secondSent=await send([sys,runtime,first,second],'private-b');assert.deepEqual(secondSent.frames.filter(m=>m.source?.kind==='sic-cb').map(m=>m.data[0].text),['short-first','short-second']);assert.ok(JSON.parse(firstSent.serialized_payload).a_dialogue);assert.ok(JSON.parse(secondSent.serialized_payload).b_feed);assert.equal(secondSent.private_route,'private-b');assert.deepEqual(secondSent.opaque,{unknown:9007199254740991,signature:'do not change'});
  installed.store.observe=async()=>{throw Error('synthetic telemetry disk failure');};await send([sys,runtime,first,second],'private-b');assert.equal(editorCalls,2);
  const page=await installed.store.list('session');assert.equal(page.rows.length,2);
  const record=await installed.store.get('session',page.rows[0].id);assert.ok(record.publication.cb_id);assert.equal(record.format,'dsh-messages');assert.equal(record.outcomes[0].stage,'adapter-stream-finished');
  session.append('user/message',api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'continue to complete the task'}]}),{surfaceOp:'append'});agent.status='running';
  const goalResult=await ctx.tools.execute({agent,name:'create_goal',arguments:{objective:'native goal through main plugin'},callId:'host-create-once',signal:new AbortController().signal});
  assert.equal(goalResult.isError,false);assert.equal(nativeGoalPort.get().id,'core-goal');assert.equal(nativeGoalPort.get().objective,'native goal through main plugin');assert.ok(goalChanged>0);agent.status='idle';
  assert.doesNotThrow(()=>sessionAPI.Session.create('session',JSON.parse(JSON.stringify(agent.session.snapshotEvents()))));
 }finally{if(scope)await scope.dispose();await ctx.fiber.dispose();await rm(root,{recursive:true,force:true});}
});
