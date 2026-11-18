import {dshSourcePath,fixturePath,evidencePath} from './test-paths.mjs';
import{test}from'node:test';import assert from'node:assert/strict';import{mkdir,writeFile}from'node:fs/promises';import{join}from'node:path';import{pathToFileURL}from'node:url';import{randomUUID}from'node:crypto';
import{prepareCompactionRuntime}from'../transparent/prepare-compaction-runtime.mjs';import{installCoreCompactionRuntime}from'../transparent/compaction-runtime.mjs';
const dsh=dshSourcePath(),root=process.env.SIC_COMPACTION_EVIDENCE_ROOT??evidencePath('dsh-sic-transparent-comparison-20261117/compaction-runtime'),prepared=await prepareCompactionRuntime(dsh,process.env.SIC_COMPACTION_SOURCE_FIXTURE??fixturePath('compaction-runtime-20261118-source-fixture'));
const load=p=>import(pathToFileURL(join(dsh,p)).href),patched=p=>import(pathToFileURL(join(prepared.root,p,'index.mjs')).href);
const{Context}=await load('vendor/cordis/lib/index.js'),{mountAgentLoopTestDependencies}=await load('packages/test-support/agent-loop-testkit/lib/index.js'),{default:AgentLoop}=await patched('agent-loop'),{default:Basic}=await patched('compaction-basic'),{default:Pruner}=await patched('compaction-tool-result-pruner'),command=await patched('command-compact');
const{default:TokenMeter}=await load('packages/llm/token-meter/lib/index.js'),{default:Commands}=await load('packages/interaction/commands/lib/index.js'),api=await load('packages/llm/llm/lib/index.js'),offload=await load('packages/compaction/compaction-image-offload/lib/index.js');
const{LlmAdapter,LlmError,createUserMessage,CONTEXT_WINDOW_EXCEEDED_CODE,IMAGE_OFFLOAD_REQUIRED_CODE}=api;
const sessionPersistence=await load('packages/session/session-persistence-jsonl/lib/index.js');
const signal=()=>new AbortController().signal;
async function evidence(name,data){const folder=join(root,name+'-'+randomUUID());await mkdir(folder,{recursive:true});await writeFile(join(folder,'evidence.json'),JSON.stringify(data,null,2));}
async function setup({schedule,tool=false,image=false,auto=true}={}){
 const ctx=new Context(),calls=[],requests=[],errors=[],trace=[],summaries=[];await mountAgentLoopTestDependencies(ctx);
 await ctx.plugin(sessionPersistence,{root:join(root,'fixtures','native-jsonl-'+randomUUID()),compression:'none'});
 await ctx.plugin(TokenMeter);await ctx.plugin(Pruner,{thresholdChars:100,headChars:10,tailChars:10});
 await ctx.plugin(Basic,{auto,headroomTokens:0,thresholdRatio:0.99,retainTokens:0,maxTokens:200,maxOverflowRetries:1});if(image)await ctx.plugin(offload);await ctx.plugin(AgentLoop,{agents:[]});
 let mode='text',toolIssued=false,failures=0;
 class Model extends LlmAdapter{
  async resolveModel(provider,id){return{provider,id,name:id,context:{contextWindow:100000},defaultMaxTokens:provider==='route-b'?47:61};}
  async *stream(options){
   requests.push({provider:options.provider,model:options.model,purpose:options.purpose??null,messages:options.messages});
   if(options.purpose==='compaction'){summaries.push(options);yield{type:'block-start',index:0,blockType:'text'};yield{type:'block-end',index:0,block:{type:'text',text:'native compact summary'}};yield{type:'finish',reason:{kind:'stop'}};return;}
   if(mode==='overflow'||mode==='overflow-once'&&failures++===0)throw new LlmError('real adapter context overflow',CONTEXT_WINDOW_EXCEEDED_CODE);
   if(mode==='image-once'&&failures++===0)throw new LlmError('real adapter image budget',IMAGE_OFFLOAD_REQUIRED_CODE,{offloadImages:1});
   if(tool&&!toolIssued){toolIssued=true;const args='{}';yield{type:'block-start',index:0,blockType:'tool-call'};yield{type:'tool-call-delta',index:0,id:'native-large-result',name:'large_result',argumentsDelta:args};yield{type:'block-end',index:0,block:{type:'tool-call',id:'native-large-result',name:'large_result',arguments:args}};yield{type:'finish',reason:{kind:'tool-calls'}};return;}
   yield{type:'block-start',index:0,blockType:'text'};yield{type:'block-end',index:0,block:{type:'text',text:'native Worker answer'}};yield{type:'finish',reason:{kind:'stop'}};
  }
 }
 ctx.llm.registerAdapter(['route-a','route-b'],new Model());if(tool)ctx.tools.register({name:'large_result',description:'actual tool with a large native result',parameters:{type:'object',properties:{}},output:{schema:{},render:()=>[{type:'text',text:'真实工具结果 '.repeat(200)}]},execute:async()=>({text:'真实工具结果 '.repeat(200)})});
 const nativeAgent=await ctx.agentLoop.create('sic-selected',{provider:'route-a',model:'model-a'});
 ctx.on('agent/error',event=>errors.push(String(event.error)));ctx.on('session/event',(session,event)=>trace.push({session:session.id,event}));
 let selectedBinding={chain_id:'fixture-chain',scope_id:'fixture-scope',role:'WORKER'};
 const runtime=await installCoreCompactionRuntime(ctx,{bindingFor:id=>id==='sic-selected'?selectedBinding:null,appendRecord:(await load('packages/core/session/lib/index.js')).appendPluginRecord,schedule:async request=>{
  calls.push({trigger:request.trigger,request_id:request.request_id,sourceCommandId:request.sourceCommandId??null,source_event_seq:request.source_event_seq});
  return schedule?await schedule(request):{recorded:true,receipt_id:'fixture-host-receipt:'+request.request_id,progressed:request.trigger==='context-overflow',publication_ids:[]};
 }});
 return{ctx,agent:nativeAgent,runtime,calls,requests,errors,trace,summaries,resetTool:()=>{toolIssued=false;},set binding(value){selectedBinding=value;},set mode(value){mode=value;failures=0;},async send(agent=nativeAgent,content=[{type:'text',text:'native archived history '.repeat(200)}]){agent.followup(createUserMessage({source:{kind:'user'},content}));await agent.whenIdle();},async close(){await runtime.dispose();await ctx.fiber.dispose();}};
}

test('selected native compaction and pruning retain their authority after binding removal, replacement and runtime unload',async()=>{
 const h=await setup({tool:true});try{
  await h.send();const before=h.agent.session.snapshotEvents(),surface=[...h.agent.session.surface.nodes],count=h.calls.length;
  for(const binding of [null,{chain_id:'different-chain',scope_id:'fixture-scope',role:'WORKER'}]){
   h.binding=binding;await assert.rejects(()=>h.ctx.compaction.compactNow(h.agent,signal()),/binding changed|binding.*removed/);assert.throws(()=>h.ctx.toolResultPruner.pruneSession(h.agent.session),/binding changed|binding.*removed/);
  }
  h.binding={chain_id:'fixture-chain',scope_id:'fixture-scope',role:'WORKER'};await h.runtime.dispose();
  await assert.rejects(()=>h.ctx.compaction.compactNow(h.agent,signal()),/authority is unavailable/);await assert.rejects(()=>h.ctx.compaction.compactRegion(surface[0],surface.at(-1),h.agent,signal()),/authority is unavailable/);assert.deepEqual(h.ctx.toolResultPruner.pruneSession(h.agent.session),{pruned:[],charsRemoved:0});
  assert.deepEqual([...h.agent.session.surface.nodes],surface);assert.equal(h.summaries.length,0);assert.equal(h.calls.length,count);assert.ok(!h.agent.session.snapshotEvents().slice(before.length).some(event=>event.type==='compaction/prune'||event.type==='compaction/summary'));
  const other=await h.ctx.agentLoop.create('native-after-unload',{provider:'route-a',model:'model-a'});h.resetTool();await h.send(other);const nodes=[...other.session.surface.nodes];let native;h.ctx.on('agent/pre-step',async({agent,signal},next)=>{if(agent.id===other.id&&!native)native=await h.ctx.compaction.compactRegion(nodes[1],nodes.at(-1),other,signal);return next();},{prepend:true});await h.send(other);assert.deepEqual(h.errors,[]);assert.ok(native.shadowedSeqs.length>0);assert.ok(h.summaries.length>0);assert.ok(other.session.snapshotEvents().some(event=>event.type==='compaction/summary'));
  await evidence('sticky-selection',{calls:h.calls,selected:h.agent.session.snapshotEvents(),nonselected:other.session.snapshotEvents()});
 }finally{await h.close();}
});

test('disabling native auto preserves selected manual handoff, and a stale in-flight binding cannot acknowledge compaction',async()=>{
 const entered=Promise.withResolvers(),release=Promise.withResolvers(),h=await setup({auto:false,schedule:async request=>{entered.resolve();await release.promise;return{recorded:true,receipt_id:'stale-fixture:'+request.request_id,progressed:false};}});
 try{
  await h.send();assert.equal(h.calls.length,0);const surface=[...h.agent.session.surface.nodes],before=h.agent.session.snapshotEvents();
  const manual=h.ctx.compaction.compactNow(h.agent,signal());await entered.promise;h.binding=null;release.resolve();await assert.rejects(()=>manual,/binding changed|binding.*removed/);assert.equal(h.calls.length,1);assert.equal(h.calls[0].trigger,'manual');assert.equal(h.summaries.length,0);assert.deepEqual([...h.agent.session.surface.nodes],surface);assert.ok(!h.agent.session.snapshotEvents().slice(before.length).some(event=>event.type==='plugin:sic/compaction/receipt'));
  await evidence('disabled-auto-stale-binding',{calls:h.calls,events:h.agent.session.snapshotEvents()});
 }finally{release.resolve();await h.close();}
});

test('actual BasicCompaction pressure and native /compact route to one external port while original sources and nonselected compaction remain intact',async()=>{
 const h=await setup({tool:true});try{
  await h.send();assert.equal(h.errors.length,0,JSON.stringify(h.errors));assert.ok(h.calls.some(c=>c.trigger==='pressure'));assert.equal(h.summaries.length,0);assert.equal(await h.ctx.sessions.flush(h.agent.session),true,'real native JSONL durability listener must acknowledge the selected session');
  const before=h.agent.session.snapshotEvents(),surface=[...h.agent.session.surface.nodes],pruned=h.ctx.toolResultPruner.pruneSession(h.agent.session);assert.deepEqual(pruned,{pruned:[],charsRemoved:0});assert.deepEqual([...h.agent.session.surface.nodes],surface);assert.equal(h.agent.session.snapshotEvents().length,before.length);assert.ok(before.some(e=>e.type==='tool/result'));
  await h.ctx.plugin(Commands);await h.ctx.plugin(command);const execution=await h.ctx.commands.execute(h.agent,'/compact',[],signal());assert.ok(execution);assert.match(JSON.stringify(execution),/No SIC context compaction was needed/);assert.ok(h.calls.some(c=>c.trigger==='manual'&&c.sourceCommandId));assert.deepEqual([...h.agent.session.surface.nodes],surface);assert.equal(h.summaries.length,0);
  assert.ok(!h.agent.session.snapshotEvents().some(e=>e.type==='compaction/summary'||e.type==='compaction/prune'));const{Session}=await load('packages/core/session/lib/index.js');assert.doesNotThrow(()=>Session.create(h.agent.session.id,JSON.parse(JSON.stringify(h.agent.session.snapshotEvents()))));
  await assert.rejects(()=>h.ctx.compaction.compactRegion(surface[0],surface.at(-1),h.agent,signal()),/no authorized SIC CB range mapping/);
  const other=await h.ctx.agentLoop.create('native-unselected',{provider:'route-a',model:'model-a'});h.resetTool();await h.send(other);const nativePrune=h.ctx.toolResultPruner.pruneSession(other.session);assert.equal(nativePrune.pruned.length,1);assert.ok(nativePrune.charsRemoved>0);assert.ok(other.session.snapshotEvents().some(e=>e.type==='compaction/prune'));const native=await h.ctx.compaction.compactNow(other,signal());assert.ok(native&&!('external'in native));assert.ok(h.summaries.length>0);assert.ok(other.session.snapshotEvents().some(e=>e.type==='compaction/summary'));assert.ok(h.calls.every(c=>c.request_id.startsWith('dsh-compaction:')));
  await evidence('native-entrypoints',{calls:h.calls,selected:h.agent.session.snapshotEvents(),nonselected:other.session.snapshotEvents(),surfaceBefore:surface,command:execution,native});
 }finally{await h.close();}
});

test('native overflow without durable SIC progress preserves the original error, and scheduler failure cannot fall back to the old compactor',async()=>{
 const noProgress=await setup({schedule:async request=>({recorded:true,receipt_id:'fixture-no-progress:'+request.request_id,progressed:false})});
 try{noProgress.mode='overflow';await noProgress.send();assert.equal(noProgress.requests.length,1);assert.equal(noProgress.calls.filter(c=>c.trigger==='context-overflow').length,1);assert.ok(noProgress.errors.length>0);assert.equal(noProgress.summaries.length,0);await evidence('no-progress-overflow',{calls:noProgress.calls,errors:noProgress.errors,events:noProgress.agent.session.snapshotEvents()});}finally{await noProgress.close();}
 const failure=await setup({schedule:async()=>{throw Error('fixture SIC scheduler failure');}});
 try{await failure.send();assert.equal(failure.requests.length,0);assert.equal(failure.summaries.length,0);assert.ok(failure.errors.some(e=>e.includes('fixture SIC scheduler failure')));assert.ok(!failure.agent.session.snapshotEvents().some(e=>e.type==='compaction/summary'||e.type==='compaction/prune'));await evidence('scheduler-fail-closed',{calls:failure.calls,errors:failure.errors,events:failure.agent.session.snapshotEvents()});}finally{await failure.close();}
});

test('actual native overflow retries only after external progress and inherits the existing overflow cap without a second native summary',async()=>{
 const h=await setup();try{
  h.mode='overflow-once';await h.send();assert.equal(h.errors.length,0,JSON.stringify(h.errors));assert.equal(h.requests.length,2);assert.equal(h.calls.filter(c=>c.trigger==='context-overflow').length,1);assert.equal(h.summaries.length,0);assert.equal(h.agent.session.surface.replaceGeneration,0);
  h.mode='overflow';const before=h.requests.length;await h.send();assert.equal(h.requests.length-before,2,'the original maxOverflowRetries=1 still bounds real request recovery');assert.equal(h.calls.filter(c=>c.trigger==='context-overflow').length,2);assert.equal(h.summaries.length,0);assert.ok(h.errors.length>0);
  await evidence('native-overflow',{calls:h.calls,errors:h.errors,events:h.agent.session.snapshotEvents()});
 }finally{await h.close();}
});

test('original ReactLoopAgent request resolver uses the real routing waterfall and adapter defaults without Worker dispatch or archive mutation',async()=>{
 let resolved;const h=await setup({schedule:async request=>{resolved=await request.resolveCall();return{recorded:true,receipt_id:'fixture-resolved:'+request.request_id,progressed:false};}});try{
  await h.send();const count=h.requests.length,events=h.agent.session.snapshotEvents().length;
  h.ctx.on('agent/request',async(_event,next)=>({...await next(),provider:'route-b',model:'model-b'}));
  const result=await h.agent.resolveContextEditCall(signal());assert.equal(result.config.provider,'route-b');assert.equal(result.config.model,'model-b');assert.equal(result.config.maxTokens,47);assert.equal(result.options.signal.aborted,false);assert.deepEqual(result.options.messages,h.agent.session.deriveMessages());assert.deepEqual(result.options.tools,h.ctx.tools.schemas(h.agent));assert.deepEqual(result.options.toolHistory,h.agent.session.toolHistory());assert.equal(h.requests.length,count);assert.equal(h.agent.session.snapshotEvents().length,events);
  const manual=await h.ctx.compaction.compactNow(h.agent,signal());assert.ok('external'in manual);assert.equal(resolved.config.provider,'route-b');assert.equal(h.requests.length,count);assert.equal(h.summaries.length,0);assert.ok(!h.agent.session.snapshotEvents().slice(events).some(e=>e.type==='request/header'||e.type==='request/context'));
  await evidence('original-native-config',{config:result.config,options:{...result.options,signal:undefined},manual,events:h.agent.session.snapshotEvents()});
 }finally{await h.close();}
});

test('manual native maintenance holds waking work and cancellation, while original image-offload repairs keep their real projection',async()=>{
 let gate,entered;const h=await setup({image:true,schedule:async request=>{if(request.trigger==='manual'){entered.resolve();await gate.promise;}return{recorded:true,receipt_id:'fixture-maintenance:'+request.request_id,progressed:false};}});try{
  h.mode='image-once';await h.send(h.agent,[{type:'text',text:'preserve this archived image'},{type:'image',attachment:{attachmentId:'sha256:'+'a'.repeat(64),name:'native-image',mediaType:'image/png',bytes:1,width:1,height:1}}]);assert.equal(h.errors.length,0,JSON.stringify(h.errors));assert.equal(h.requests.length,2);assert.ok(h.agent.session.snapshotEvents().some(e=>e.type==='image/offload'));assert.ok(h.requests.at(-1).messages.flatMap(m=>m.content).some(b=>b.type==='image'&&b.offloaded===true));assert.equal(h.calls.filter(c=>c.trigger==='context-overflow').length,0);
  gate=Promise.withResolvers();entered=Promise.withResolvers();const abort=new AbortController(),manual=h.ctx.compaction.compactNow(h.agent,abort.signal);await entered.promise;const turns=h.agent.session.snapshotEvents().filter(e=>e.type==='turn/start').length;h.agent.followup(createUserMessage({content:[{type:'text',text:'queued during native maintenance'}],source:{kind:'user'}}));await new Promise(r=>setTimeout(r,20));assert.equal(h.agent.session.snapshotEvents().filter(e=>e.type==='turn/start').length,turns);abort.abort(Error('native manual cancellation'));gate.resolve();await assert.rejects(()=>manual,/native manual cancellation/);await h.agent.whenIdle();assert.equal(h.agent.session.snapshotEvents().filter(e=>e.type==='turn/start').length,turns+1);assert.equal(h.summaries.length,0);
  await evidence('maintenance-image',{calls:h.calls,events:h.agent.session.snapshotEvents(),requests:h.requests});
 }finally{gate?.resolve();await h.close();}
});
