// Real authenticated Core with a minimal trusted lifecycle host. No task executor.
import{test}from'node:test';import assert from'node:assert/strict';
import{isolated}from'./isolated-core-fixture.mjs';
import{installCoreJobsRuntime}from'../transparent/jobs-runtime.mjs';
import{apply as applyTransparentHost}from'../transparent/plugin.mjs';

test('a mounted legacy peer host fails before Jobs attachment, admission, or Core writes',async()=>{
 const writes=[],unexpected=()=>{writes.push('mutation');throw Error('unexpected mutation');};
 const ctx={llm:{contextDispatchVersion:1},
  jobs:{externalControlVersion:1,externalExecutorJoinVersion:1,joinExecutor:unexpected,attachExternalLifecycle:unexpected},
  get:name=>name==='subagents'?{externalPeerResidencyVersion:1,attachExternalLifecycle:unexpected}:undefined,
  agents:{list:unexpected},effect:unexpected,on:unexpected,inject:unexpected,
 };
 const expected=/patched native subagent residency lifecycle v2 required/;
 await assert.rejects(()=>installCoreJobsRuntime(ctx,{client:{operate:unexpected,issueTurn:unexpected},bindingFor:unexpected}),expected);
 await assert.rejects(()=>applyTransparentHost(ctx,{enabled:true,actor:'fixture',coreURL:'http://127.0.0.1:1',projectionRoot:'not-used',credential:unexpected,coreFetch:unexpected}),expected);
 assert.deepEqual(writes,[]);
});

test('peer epochs retain immutable old results, reject uncertain resume and restore the latest durable child alias',{timeout:30000},async()=>{
 const host=await isolated();let runtime,port;
 const parent={id:'readonly-parent',status:'running',session:{id:'readonly-parent'}},agents=new Map([[parent.id,parent]]);
 const warnings=[];
 const ctx={
  agents:{get:id=>agents.get(id),list:()=>[parent],currentInitiator:()=>parent},
  jobs:{externalControlVersion:1,externalExecutorJoinVersion:1,joinExecutor:async()=>{},attachExternalLifecycle:()=>()=>{}},
  subagents:{externalPeerResidencyVersion:2,attachExternalLifecycle:value=>{port=value;return()=>{};}},
  tools:{get:()=>({output:{render:(_args,value)=>[{type:'text',text:value.text}]}})},
  sessionProjections:{stateOf:()=>({openTurnStartSeq:0})},
  logger:{warn:message=>warnings.push(message)},on:()=>()=>{},effect:()=>{},inject:()=>({dispose(){}}),
  get(name){return this[name];},
 };
 const mount=()=>installCoreJobsRuntime(ctx,{client:host.core,bindingFor:()=>({scope_id:'peer-residency-fixture',chain_id:host.chain_id,role:'WORKER'}),appendRecord:()=>{}});
 const settle=(claim,text)=>port.settle(parent,'same-durable-child',{stopReason:'completed',output:[{type:'text',text}]},claim);
 const prepare=()=>port.prepare(parent,'same-durable-child','readonly','local');
 let call=0;
 const collect=()=>runtime.facade('job_output',{job_id:'same-durable-child'},{agent:parent,callId:'collect-'+(++call),rootCallId:'root-'+call,signal:new AbortController().signal});
 try{
  runtime=await mount();const first=await prepare();
  await port.attach(parent,'same-durable-child',async()=>{},first);await settle(first,'first immutable result');
  const original=runtime.readState('same-durable-child',parent.id);
  assert.equal((await collect()).text,'first immutable result');
  const second=await prepare();await port.attach(parent,'same-durable-child',async()=>{},second);
  const active=runtime.readState('same-durable-child',parent.id);
  assert.notEqual(active.object_id,original.object_id);assert.notEqual(active.external_id,original.external_id);
  assert.equal(active.executor.session_id,'same-durable-child');assert.equal(active.executor.residency_epoch,2);
  // A delayed old notification cannot settle the new activation.
  await settle(first,'stale result must not replace anything');
  await assert.rejects(settle(undefined,'old host omitted claim'),/original native peer activation claim required/);
  assert.equal(runtime.readState('same-durable-child',parent.id).status,'running');
  await assert.rejects(prepare(),/reconcile it before resume/);
  await settle(second,'second fresh result');assert.equal((await collect()).text,'second fresh result');
  const old=await host.core.operate('harness.get',{scope_id:'peer-residency-fixture',object_id:original.object_id});
  assert.equal(old.result,'first immutable result');assert.equal(old.external_id,original.external_id);
  assert.equal(runtime.list(parent.id).length,1);assert.equal(runtime.list(parent.id)[0].id,'same-durable-child');
  await runtime.dispose();runtime=await mount();await runtime.refresh(parent);
  assert.equal(runtime.readState('same-durable-child',parent.id).executor.residency_epoch,2);
  assert.equal(runtime.list(parent.id).length,1);assert.equal((await collect()).text,'');
  const third=await prepare();
  await assert.rejects(prepare(),/reconcile it before resume/,'claimed but not attached residency cannot be replaced');
  await port.attach(parent,'same-durable-child',async()=>{},third);
  await settle(third,'third after adapter reopen');assert.equal((await collect()).text,'third after adapter reopen');
  assert.equal(runtime.readState('same-durable-child',parent.id).executor.residency_epoch,3);
  assert.equal(runtime.list(parent.id).length,1);assert.deepEqual(warnings,[]);

  // Legacy already-attached peers used the durable child ID as their
  // executor ID. Model that persisted schema using real Core operations.
  const legacyId='legacy-attached-child';
  const legacy=await port.prepare(parent,legacyId,'readonly','external');
  await port.attach(parent,legacyId,async()=>{},legacy);
  assert.equal(runtime.readState(legacyId,parent.id).executor.session_id,undefined);
  await assert.rejects(()=>port.prepare(parent,legacyId,'readonly','local'),/reconcile it before resume/);
  await port.settle(parent,legacyId,{stopReason:'completed',output:[{type:'text',text:'legacy result'}]},legacy);
  const upgraded=await port.prepare(parent,legacyId,'readonly','local');
  await port.attach(parent,legacyId,async()=>{},upgraded);
  await port.settle(parent,legacyId,{stopReason:'completed',output:[{type:'text',text:'v2 result'}]},upgraded);
  await runtime.dispose();runtime=await mount();
  assert.equal(runtime.readState(legacyId,parent.id).result,'v2 result');
  assert.equal(runtime.list(parent.id).filter(row=>row.id===legacyId).length,1);
  assert.equal((await host.core.operate('harness.get',{scope_id:'peer-residency-fixture',object_id:legacy.state.object_id})).result,'legacy result');

  // The process-local claim record disappears with the adapter. Core must
  // still reject a second attempt for the same unacknowledged residency.
  const unbound = await prepare();
  assert.equal(unbound.state.dispatch_state, 'claimed');
  await runtime.dispose();
  runtime = await mount();
  await assert.rejects(
    prepare(),
    /already admitted.*reconciliation/,
  );
  const activity = await host.core.operate('harness.get', {
    scope_id: 'peer-residency-fixture', object_id: unbound.state.object_id,
  });
  assert.equal(activity.dispatch_state, 'unknown');
  assert.equal(activity.external_id, null);
 }finally{await runtime?.dispose();await host.close();}
});
