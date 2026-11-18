import {dshSourcePath,fixturePath,evidencePath,testCoreBinary} from './test-paths.mjs';
import{test}from'node:test';import assert from'node:assert/strict';import{readFile,copyFile,mkdir,writeFile}from'node:fs/promises';import{join}from'node:path';import{pathToFileURL}from'node:url';import{createHash,randomUUID}from'node:crypto';
import{isolated}from'./isolated-core-fixture.mjs';import{prepareCompactionRuntime}from'../transparent/prepare-compaction-runtime.mjs';import{installCoreCompactionRuntime}from'../transparent/compaction-runtime.mjs';import{createHostHierarchyRuntime}from'../transparent/host-hierarchy.mjs';import{createFileProjectionStore}from'../transparent/file-store.mjs';import{createProjectionPublisher}from'../transparent/projection.mjs';import{createSessionRecorder}from'../transparent/session-records.mjs';import{getEditorInvocation}from'../transparent/editor-invocation.mjs';
const dsh=dshSourcePath(),build=process.env.SIC_COMPACTION_BUILD_ROOT??fixturePath('compaction-hierarchy-http-20261118'),evidence=process.env.SIC_COMPACTION_EVIDENCE_ROOT??evidencePath('dsh-sic-transparent-comparison-20261117/compaction-runtime');
await mkdir(build,{recursive:true});await mkdir(evidence,{recursive:true});
const sourceBinary=process.env.SIC_CORE_TEST_BIN??testCoreBinary(),binaryBytes=await readFile(sourceBinary),coreSHA=createHash('sha256').update(binaryBytes).digest('hex'),frozenBinary=join(build,'sic-core-'+coreSHA+'.exe');
try{await copyFile(sourceBinary,frozenBinary,1);}catch(error){if(error.code!=='EEXIST')throw error;}assert.equal(createHash('sha256').update(await readFile(frozenBinary)).digest('hex'),coreSHA);process.env.SIC_CORE_TEST_BIN=frozenBinary;await writeFile(join(build,'SHA256SUMS.txt'),coreSHA+'  '+frozenBinary+'\n');
const prepared=await prepareCompactionRuntime(dsh,join(build,'native-source')),load=p=>import(pathToFileURL(join(dsh,p)).href),patched=p=>import(pathToFileURL(join(prepared.root,p,'index.mjs')).href);
const{Context}=await load('vendor/cordis/lib/index.js'),{mountAgentLoopTestDependencies}=await load('packages/test-support/agent-loop-testkit/lib/index.js'),api=await load('packages/llm/llm/lib/index.js'),sessionAPI=await load('packages/core/session/lib/index.js');
const{default:AgentLoop}=await patched('agent-loop'),{default:Basic}=await patched('compaction-basic'),{default:Pruner}=await patched('compaction-tool-result-pruner'),command=await patched('command-compact'),{default:TokenMeter}=await load('packages/llm/token-meter/lib/index.js'),{default:Commands}=await load('packages/interaction/commands/lib/index.js'),jsonl=await load('packages/session/session-persistence-jsonl/lib/index.js');
const system=await readFile(new URL('../transparent/bundle/fused.md',import.meta.url),'utf8'),definitions=[...JSON.parse(await readFile(new URL('../transparent/bundle/native-tools.json',import.meta.url),'utf8')),...JSON.parse(await readFile(new URL('../transparent/bundle/sic-extra-tools.json',import.meta.url),'utf8'))];
const freshSignal=()=>new AbortController().signal;

async function setup(trigger){
 const server=await isolated(),ctx=new Context(),folder=join(evidence,'combined-'+trigger+'-'+randomUUID());await mkdir(folder,{recursive:true});
 const requests=[],scheduleReceipts=[],errors=[];let workerCalls=0;
 await mountAgentLoopTestDependencies(ctx);await ctx.plugin(jsonl,{root:join(server.root,'native-sessions'),compression:'none'});await ctx.plugin(TokenMeter);await ctx.plugin(Pruner,{thresholdChars:100,headChars:10,tailChars:10});await ctx.plugin(Basic,{auto:true,headroomTokens:0,maxTokens:128});await ctx.plugin(AgentLoop,{agents:[]});await ctx.plugin(Commands);await ctx.plugin(command);
 for(const tool of definitions)ctx.tools.register({...tool,output:{schema:{},render:(_args,value)=>[{type:'text',text:JSON.stringify(value)}]},execute:async()=>{throw Error('Worker/tool executor must not run in a native compaction request');}});
 class Model extends api.LlmAdapter{
  async resolveModel(provider,id){return{provider,id,name:id,context:{contextWindow:100000},defaultMaxTokens:93};}
  async *stream(request){
   if(!getEditorInvocation()){workerCalls++;throw Error('compaction dispatched an unexpected Worker');}
   assert.equal(request.provider,'native-original-route');assert.equal(request.model,'native-original-model');assert.equal(request.temperature,0.29);assert.equal(request.maxTokens,128);assert.deepEqual(request.stop,['END']);
   const cut=await server.core.operate('cut.get',{}),shared=request.messages.filter(m=>m.source?.kind==='sic-cb');assert.deepEqual(shared.map(m=>m.source.cb_id),cut.view.cbs.map(cb=>cb.cb_id));assert.deepEqual(shared.map(m=>m.content[0].text),cut.view.cbs.map(cb=>cb.body));assert.deepEqual(request.tools,ctx.tools.schemas(agent));
   requests.push(JSON.parse(JSON.stringify(request)));const wb=JSON.parse(request.messages.at(-1).content[0].text);assert.ok(wb.units.length);
   yield{type:'tool-call-delta',index:0,id:'actual-higher-editor-'+requests.length,name:'sic_edit_blocks',argumentsDelta:JSON.stringify({ops:wb.units.map(unit=>({op:'COMPRESS',target:unit.unit_id,content:unit.text})),pvi:{}})};yield{type:'finish',reason:{kind:'tool-calls'}};
  }
 }
 ctx.llm.registerAdapter(['native-original-route'],new Model());ctx.on('agent/request',async(_event,next)=>({...await next(),temperature:0.29,maxTokens:128,stop:['END']}));ctx.on('agent/error',payload=>errors.push(String(payload.error)));
 const agent=await ctx.agentLoop.create('actual-native-compaction-'+trigger,{provider:'native-original-route',model:'native-original-model'}),binding={chain_id:server.chain_id,scope_id:'native-compaction-hierarchy-scope',role:'WORKER'};
 const store=createFileProjectionStore(join(server.root,'projections')),recordSession=await createSessionRecorder(ctx,sessionAPI),publisher=createProjectionPublisher({save:store.save,getSession:id=>ctx.sessions.get(id),appendEvent:(session,event)=>recordSession(session,event.type,event.data,{key:event.data.projection_id})});
 const hierarchy=createHostHierarchyRuntime({ctx,cfg:{actor:'native-http-actor',hierarchyPolicy:{max_level:3,level_batch_sizes:{2:2,3:2},max_host_editor_calls:16}},api,system,store,clientFor:()=>server.core,bindingFor:id=>id===agent.id?binding:null,recordSession,publisher});
 const port=await installCoreCompactionRuntime(ctx,{bindingFor:id=>id===agent.id?binding:null,appendRecord:sessionAPI.appendPluginRecord,schedule:async input=>{const result=await hierarchy.scheduleCompaction(input);scheduleReceipts.push(result);return result;}});
 // The actual AgentLoop owns the pressure turn. This ordinary native admission
 // guard stops the Worker after BasicCompaction has completed its middleware.
 if(trigger==='pressure')ctx.on('agent/pre-step',async()=>({kind:'reject'}));
 async function seed(index){
  const message=api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'actual-native-fact-'+index}]});agent.session.append('user/message',message,{surfaceOp:'append'});
  const cut=await server.core.operate('cut.get',{}),recalled=cut.view.cbs.map(cb=>api.createUserMessage({source:{kind:'sic-cb'},content:[{type:'text',text:cb.body}]})),messages=[...recalled,message],position=recalled.length;
  const source=await server.core.begin({format:'dsh-messages',request:{provider:'native-original-route',model:'native-original-model',messages},idempotency_key:'actual-native-seed-'+trigger+'-'+index,editable_anchors:[['messages',position,'content',0,'text']],protected_cb_bindings:cut.view.cbs.map((cb,i)=>({cb_id:cb.cb_id,native_anchors:[['messages',i,'content',0,'text']]})),host_message_origins:messages.map((item,i)=>({message_id:item.id,block_index:0,native_anchor:['messages',i,'content',0,'text']}))});
  await server.core.commit({source_id:source.source_id,expected_source_digest:source.source_digest,expected_version:0,edits:[],delivery:{ops:[],pvi:{}}});const publication=await server.core.publish({source_id:source.source_id,expected_source_digest:source.source_digest,expected_version:1,expected_cut:cut.sequence,idempotency_key:'actual-native-seed-pub-'+trigger+'-'+index});return{message_id:message.id,source_id:source.source_id,publication};
 }
 return{...server,folder,ctx,agent,port,hierarchy,store,requests,scheduleReceipts,errors,seed,get workerCalls(){return workerCalls;},async close(){try{await port.dispose();await hierarchy.drain();await ctx.fiber.dispose();}finally{await server.close();}}};
}

for(const trigger of ['manual','pressure'])test('actual native '+trigger+' compaction consumes real Rust host plans and original Harness Editors through levels 2 and 3 without rewriting the native archive',{timeout:45000},async()=>{
 const f=await setup(trigger);try{
  const seeds=[];for(let index=0;index<(trigger==='manual'?4:5);index++)seeds.push(await f.seed(index));
  const before=f.agent.session.snapshotEvents(),surface=[...f.agent.session.surface.nodes],messages=f.agent.session.deriveMessages(),beforeCut=await f.core.operate('cut.get',{});const commands=[];
  if(trigger==='manual'){
   for(let invocation=0;invocation<2;invocation++){const result=await f.ctx.commands.execute(f.agent,'/compact',[],freshSignal());assert.ok(result);assert.doesNotMatch(JSON.stringify(result),/"kind":"error"/);commands.push(result);}
  }else{f.agent.followup(api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'Run the native pressure boundary, without admitting a Worker.'}]}));await f.agent.whenIdle();}
  assert.equal(f.workerCalls,0);assert.deepEqual(f.errors,[]);assert.ok(f.requests.length>0);assert.ok(f.scheduleReceipts.length>0);assert.ok(f.scheduleReceipts.every(receipt=>receipt.recorded&&receipt.reports.every(report=>report.editor_mode==='host'&&report.model_calls_observed===0)));
  const jobs=f.scheduleReceipts.flatMap(receipt=>receipt.jobs);assert.ok(jobs.some(job=>job.level===2));assert.ok(jobs.some(job=>job.level===3));
  const chain=await f.core.operate('chain.get',{}),cut=await f.core.operate('cut.get',{});assert.equal(chain.role_views.worker.level,3);assert.ok(cut.sequence>beforeCut.sequence);assert.ok(cut.view.cbs.some(cb=>cb.level===3));if(trigger==='pressure')assert.ok(cut.view.cbs.some(cb=>cb.level===1),'the unmatched low-level tail remains selected');
  for(let index=0;index<seeds.length;index++)assert.ok(cut.view.cbs.map(cb=>cb.body).join('\n').includes('actual-native-fact-'+index));
  assert.deepEqual([...f.agent.session.surface.nodes],surface);assert.deepEqual(f.agent.session.deriveMessages(),messages);assert.deepEqual(f.ctx.toolResultPruner.pruneSession(f.agent.session),{pruned:[],charsRemoved:0});
  const after=f.agent.session.snapshotEvents();assert.deepEqual(after.slice(0,before.length),before);assert.ok(!after.some(event=>event.type==='compaction/summary'||event.type==='compaction/prune'||event.type==='request/header'||event.type==='assistant/message'));assert.ok(after.some(event=>event.type==='plugin:sic/compaction/receipt'));assert.ok(after.some(event=>event.type==='plugin:sic/hierarchy'));assert.ok(after.some(event=>event.type==='plugin:sic/native-projection'));assert.doesNotThrow(()=>sessionAPI.Session.create(f.agent.session.id,JSON.parse(JSON.stringify(after))));
  const rows=(await f.store.list(f.agent.session.id,0,100)).rows;assert.ok(rows.some(row=>row.stage==='hierarchy-published-awaiting-dispatch'));for(const row of rows){const record=await f.store.get(f.agent.session.id,row.id);assert.equal(record.source_kind,'host-hierarchy');assert.deepEqual(record.adopted.messages.filter(message=>message.source?.kind==='sic-cb').map(message=>message.content[0].text),record.layout_receipt.projected_history.filter(message=>message.source?.kind==='sic-cb').map(message=>message.content[0].text));}
  assert.equal(await f.ctx.sessions.flush(f.agent.session),true);
  // A separate backend has no live-session tracker or in-memory event cache.
  // Read the fsynced JSONL through the original native storage decoder.
  const cold=new Context();let handle,reopened;
  try{await cold.plugin(jsonl,{root:join(f.root,'native-sessions'),compression:'none'});handle=await cold.sessionPersistence.open(f.agent.session.id,'read');reopened=await handle.read();assert.deepEqual(reopened.events,after);assert.doesNotThrow(()=>sessionAPI.Session.create(f.agent.session.id,reopened.events));}finally{await handle?.close();await cold.fiber.dispose();}
  await writeFile(join(f.folder,'evidence.json'),JSON.stringify({result:'PASS',trigger,core_sha256:coreSHA,frozen_core:frozenBinary,private_core_root:f.root,seeds,beforeCut,cut,chain,commands,requests:f.requests,scheduleReceipts:f.scheduleReceipts,worker_calls:f.workerCalls,native_surface_before:surface,native_surface_after:[...f.agent.session.surface.nodes],native_events:after,reopened_jsonl_event_count:reopened.events.length,projection_rows:rows},null,2));
 }finally{await f.close();}
});
