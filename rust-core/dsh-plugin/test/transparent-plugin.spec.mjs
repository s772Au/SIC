import {dshSourcePath} from './test-paths.mjs';
import{test}from'node:test';import assert from'node:assert/strict';import{readFile,mkdtemp,rm}from'node:fs/promises';import{tmpdir}from'node:os';import{join}from'node:path';import{pathToFileURL}from'node:url';
import{apply}from'../transparent/plugin.mjs';import{nativeExchange}from'../transparent/native-boundary.mjs';import{digest}from'../transparent/transport.mjs';
const dsh=dshSourcePath();
const load=p=>import(pathToFileURL(join(dsh,p)).href);
const sessionAPI=await load('packages/core/session/lib/index.js');
const{Context}=await load('vendor/cordis/lib/index.js');
const api=await load('packages/llm/llm/lib/index.js');
const{default:SystemPrompt,renderPrompt}=await load('packages/core/system-prompt/lib/index.js');
const{default:ToolRuntime}=await load('packages/core/tools/lib/index.js');const{createScope}=await load('packages/core/scope/lib/index.js');

test('opt-in plugin mounts reviewed bundle, real native registry, CB publication, old-ID protection and retry reuse',async()=>{
 const ctx=new Context(),root=await mkdtemp(join(tmpdir(),'sic-plugin-'));let scope;
 try{
  await ctx.plugin(SystemPrompt,{includeHarnessIdentity:false});await ctx.plugin(ToolRuntime);await ctx.plugin(api.default);
  const native=await readFile(new URL('../transparent/bundle/native.md',import.meta.url),'utf8');ctx.systemPrompt.section({name:'reviewed-native-fixture',order:0,text:native,interpolate:false});
  const definitions=JSON.parse(await readFile(new URL('../transparent/bundle/native-tools.json',import.meta.url),'utf8'));
  for(const t of definitions)ctx.tools.register({...t,output:{schema:{},render:(_a,v)=>[{type:'text',text:JSON.stringify(v)}]},execute:async()=>{throw Error('old executor must not run');}});
  const session=sessionAPI.Session.create('session'),agent={id:'session',status:'idle',session};
  scope=createScope(ctx,agent);agent.ctx=scope.ctx;
  let step=1,editorCalls=0,cut=0,current,sourceCount=0,goal=null,nativeGoalPort,goalChanged=0;const operations=[],sources=new Map(),cbs=[],cbOrigins=[];
  class Model extends api.LlmAdapter{async *stream(request){editorCalls++;assert.equal(request.provider,'original');assert.equal(request.model,'original-model');const wb=JSON.parse(request.messages.at(-1).content[0].text);assert.equal(wb.units.length,1);assert.equal(request.messages.slice(0,-2).some(m=>m.source?.kind==='runtime-context'),false);if(editorCalls===2)assert.equal(request.messages.slice(0,-2).at(-1).content[0].text,'short-first');yield{type:'tool-call-delta',index:0,id:'editor-'+editorCalls,name:'sic_edit_blocks',argumentsDelta:JSON.stringify({ops:[{op:'COMPRESS',target:wb.units[0].unit_id,content:editorCalls===1?'short-first':'short-second'}],pvi:{}})};yield{type:'finish',reason:{kind:'tool-calls'}};}}
  ctx.llm.registerAdapter(['original'],new Model());
  const coreFetch=async(url,init)=>{
   const body=JSON.parse(init.body);if(new URL(url).pathname==='/api/v2/harness/turn'){assert.equal(body.human_intent,'direct_human');return Response.json({grant_id:'real-host-bridge-fixture'});}const p=body.payload;operations.push(body.operation);
   let result;
   if(body.operation==='harness.call'){if(p.name==='create_goal')goal={id:'core-goal',revision:1,objective:p.arguments.objective,phase:'active',maxGoalRounds:37,roundsStarted:0};result={value:{goal,activation:goal?'armed':null}};}
   else if(body.operation==='harness.get')result={revision:1,roundsStarted:0,roundsFinished:0};
   else if(body.operation==='cut.get')result={cut_id:cut?'cut-'+cut:null,sequence:cut,view:{cbs:[...cbs],cb_origin_bindings:[...cbOrigins]}};
   else if(body.operation==='chain.list')result={routes:[{chain_id:'chain',cut_seq:cut}]};
   else if(body.operation==='chain.get')result={chain_id:'chain'};
   else if(body.operation==='native.begin'){
    if(sources.has(p.idempotency_key))return Response.json({result:sources.get(p.idempotency_key)});
    const request=p.request,units=request.messages[0].content.map((b,i)=>({unit_id:'u'+(i+1),text:b.text,anchor:['messages',0,'content',i,'text']}));
    if(cut>0){assert.ok(p.protected_cb_bindings.length);assert.equal(request.messages[0].content[0].text,'short-first');}
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
   sessionProjections:{stateOf:()=>({openTurnStartSeq:0,lastStepStartSeq:step})},on:(name,fn)=>{handlers.set(name,fn);return()=>{}},
   inject:(_deps,fn)=>_deps.some(d=>['subagents','workflowEngine'].includes(d))?{dispose:()=>{}}:fn({effect:body=>body(),connection:{fetch:{register:()=>()=>{}}}})};
  const installed=await apply(facade,{enabled:true,drainHigher:async()=>({recorded:true,receipt_id:'fixture-higher',progressed:false}),transport:'native-wire',mode:'edit',actor:'host',coreURL:'http://127.0.0.1:18890',credential:()=> 'synthetic',projectionRoot:root,bindings:{session:binding},renderPrompt,coreFetch,
   admitFinal:async()=>{},llmAPI:api,sessionAPI,agentEvents:()=>({emit:()=>{goalChanged++;}})});
  assert.ok(handlers.has('llm/native-fetch'));
  const system=await readFile(new URL('../transparent/bundle/fused.md',import.meta.url),'utf8'),authorization={role:'WORKER',scope_id:'scope',edit_capabilities:{ops:['KEEP','REPLACE','COMPRESS'],pvi:['I']}};
  const sys={id:'system',role:'system',content:[{type:'text',text:system}],source:{kind:'system-prompt'}},runtime={id:'runtime',role:'user',content:[{type:'text',text:'runtime'}],source:{kind:'runtime-context',sections:[{name:'sic:activation',text:JSON.stringify(authorization)}]}};
  const first={id:'human-first',role:'user',content:[{type:'text',text:'first'}],source:{kind:'user'}},second={id:'human-second',role:'user',content:[{type:'text',text:'second'}],source:{kind:'user'}};
  async function send(history){
   const options={provider:'original',model:'original-model',sessionId:'session',messages:history,tools:ctx.tools.schemas(agent)};
   const contextExchange={options,history};const projected=await installed.projectContext(contextExchange,async()=>contextExchange.history);
   const serializeHistory=history=>({model:'original-model',system,messages:[{role:'user',content:history.filter(m=>m.role==='user').flatMap(m=>m.content.filter(b=>b.type!=='text'||b.text))}],stream:true}),native=serializeHistory(projected);
   const boundary=nativeExchange(new Request('https://fixture.invalid/native',{method:'POST',body:JSON.stringify(native)}),{options,history:projected,context_projection:contextExchange.projection,serializeHistory});
   let actual;const response=await installed.nativeFetch(boundary.exchange,()=>boundary.dispatch(async request=>{actual=await request.json();return new Response('native-response');}));return{response,actual};
  }
  assert.equal((await send([sys,runtime,first])).actual.messages[0].content[0].text,'short-first');step=2;
  const sent=await send([sys,runtime,first,second]);assert.deepEqual(sent.actual.messages[0].content.slice(0,2).map(b=>b.text),['short-first','short-second']);
  const before=operations.length;await send([sys,runtime,first,second]);assert.ok(operations.length>before);assert.equal(editorCalls,2);
  const page=await installed.store.list('session');assert.equal(page.rows.length,2);
  const record=await installed.store.get('session',page.rows[0].id);assert.ok(record.publication.cb_id);assert.equal(record.outcomes[0].stage,'response-returned');
  const readsBefore=operations.filter(op=>op==='native.get').length;
  for(const role of ['WORKER','EDITOR']){
   binding.role=role;binding.editor_scope={source_id:'assigned-only',assigned_unit_ids:['u1'],version:0};
   binding.editor_readable_sources=[{kind:'core-source',source_id:'core-archive-id',unit_ids:['u1']}];
   for(const source_id of ['private-other','core-archive-id']){const denied=await ctx.tools.execute({agent,name:'sic_source_text_page',arguments:{source_id,unit_id:'u1'},callId:'denied-'+role+'-'+source_id,signal:new AbortController().signal});assert.equal(denied.isError,true);}
  }
  assert.equal(operations.filter(op=>op==='native.get').length,readsBefore);binding.role='WORKER';delete binding.editor_scope;delete binding.editor_readable_sources;
  session.append('user/message',api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'continue to complete the task'}]}),{surfaceOp:'append'});agent.status='running';
  const goalResult=await ctx.tools.execute({agent,name:'create_goal',arguments:{objective:'native goal through main plugin'},callId:'host-create-once',signal:new AbortController().signal});
  assert.equal(goalResult.isError,false);assert.equal(nativeGoalPort.get().id,'core-goal');assert.equal(nativeGoalPort.get().objective,'native goal through main plugin');assert.ok(goalChanged>0);agent.status='idle';
  assert.doesNotThrow(()=>sessionAPI.Session.create('session',JSON.parse(JSON.stringify(agent.session.snapshotEvents()))));
 }finally{if(scope)await scope.dispose();await ctx.fiber.dispose();await rm(root,{recursive:true,force:true});}
});
