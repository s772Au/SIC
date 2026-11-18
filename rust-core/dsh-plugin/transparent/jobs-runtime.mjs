import{AsyncLocalStorage}from'node:async_hooks';import{randomUUID}from'node:crypto';import{digest}from'./transport.mjs';
const terminal=s=>['completed','killed','failed'].includes(s.status);
function assertPeerRuntime(subagents){
 if(subagents.externalPeerResidencyVersion!==2||typeof subagents.attachExternalLifecycle!=='function')throw Error('patched native subagent residency lifecycle v2 required');
}
export function assertNativeJobsRuntime(ctx){
 if(typeof ctx.jobs?.attachExternalLifecycle!=='function'||ctx.jobs.externalControlVersion!==1||ctx.jobs.externalExecutorJoinVersion!==1||typeof ctx.jobs.joinExecutor!=='function')throw Error('patched native job registry, tool-jobs controller and physical executor join v1 required');
 const subagents=ctx.get('subagents'),workflow=ctx.get('workflowEngine');
 if(subagents)assertPeerRuntime(subagents);
 if(workflow&&typeof workflow.attachExternalLifecycle!=='function')throw Error('patched native workflow lifecycle required');
}
export async function installCoreJobsRuntime(ctx,{client,bindingFor,appendRecord,launchKindFor,onReceipt,onCollection}={}){
 assertNativeJobsRuntime(ctx);
 if(!client?.operate||!client?.issueTurn||typeof bindingFor!=='function')throw Error('trusted Core jobs host bindings required');
 appendRecord??=(await import('@deepseek-ai/dsh-session')).appendPluginRecord;
 const als=new AsyncLocalStorage(),controlContext=new AsyncLocalStorage(),records=new Map(),handles=new Map(),inflight=new Map(),collectors=new Map(),modelKills=new Set(),collections=new Map(),seenControls=new Set(),disposers=[],nativeJobs=ctx.jobs,nativeAgents=ctx.agents;
 const completion=()=>{const value=Promise.withResolvers();value.promise.catch(()=>{});return value;};
 let closing=false;
 const binding=agent=>bindingFor(agent.session.id,agent),owner=id=>id?(nativeAgents.get(id)??[...records.values()].find(c=>c.agent?.id===id)?.agent):null;
 const selected=id=>!!owner(id)&&!!binding(owner(id));
 const key=(scope,id)=>scope+'\0'+id;
 const collectionKey=(agent,callId)=>agent.session.id+'\0'+callId;
 function notify(fn,value,label){try{void Promise.resolve(fn?.(structuredClone(value))).catch(error=>ctx.logger.warn(label+': '+error.message));}catch(error){ctx.logger.warn(label+': '+error.message);}}
 function assertWorker(agent){if(!agent||nativeAgents.get(agent.id)!==agent||binding(agent)?.role==='EDITOR')throw Error('exact selected Worker agent required');}
 function nativeExec(exec){if(closing)throw Error('native lifecycle adapter is closing; new execution and controls are not admitted');const agent=exec.agent;assertWorker(agent);if(agent.status!=='running'||ctx.agents.currentInitiator()!==agent||!exec.callId||!exec.rootCallId)throw Error('native executing tool provenance required');const turn=ctx.sessionProjections.stateOf(agent.session,'turnBoundary')?.openTurnStartSeq;if(!Number.isSafeInteger(turn))throw Error('native open tool turn required');return{agent,b:{...binding(agent)},turn_id:`${agent.session.id}:${turn}`,call_id:exec.callId,root_call_id:exec.rootCallId,logical_call_id:JSON.stringify([agent.session.id,turn,exec.rootCallId,exec.callId])};}
 function remember(c,state){
  c.peerSessionId=state.executor?.session_id??c.peerSessionId;c.peerEpoch=state.executor?.residency_epoch??c.peerEpoch;
  const scope=c.b.scope_id,prior=records.get(key(scope,state.object_id));
  if(prior&&prior!==c){prior.state=state;return prior;}
  c.state=state;c.offset=Math.max(c.offset??0,...(state.chunk_metadata??[]).filter(Boolean).map(chunk=>chunk.at+Buffer.byteLength(chunk.text)));
  records.set(key(scope,state.object_id),c);
  if(state.external_id){
   const address=key(scope,'native:'+state.external_id),previous=records.get(address);
   // Legacy local peers used the child address as their executor identity.
   // Refreshing such a row must not replace the newer durable child alias.
   if(previous?.peerSessionId!==state.external_id||previous===c)records.set(address,c);
  }
  if(c.peerSessionId){const alias=key(scope,'native:'+c.peerSessionId),previous=records.get(alias);if(!previous||(previous.peerEpoch??0)<=(c.peerEpoch??0))records.set(alias,c);}
  return c;
 }
 async function refresh(c){const state=await client.operate('harness.get',{scope_id:c.b.scope_id,object_id:c.state.object_id});remember(c,state);return state;}
 async function grant(c,name,role='executor',launch){return client.issueTurn({scope_id:c.b.scope_id,chain_id:c.b.chain_id??null,turn_id:c.turn_id,role,human_intent:'none',human_message_id:null,allowed_calls:[name],object_id:c.state?.object_id??null,goal_round:null,max_goal_rounds:null,wait_timeout_ms:null,...launch?{launch}:{}});}
 function receipt(c,result){if(terminal(c.state))c.finished?.resolve();if(c.agent&&ctx.get('agents')?.get(c.agent.id)===c.agent){try{appendRecord(c.agent.session,'plugin:sic/jobs/receipt',{object_id:c.state.object_id,revision:c.state.revision,external_id:c.state.external_id??null,status:c.state.status,receipt_id:result.receipt_id??null});}catch(error){ctx.logger.warn('native receipt read projection: '+error.message);}}notify(onReceipt,{object_id:c.state.object_id,revision:c.state.revision,result},'native receipt observer');}
 async function observe(c,kind,extra={},role='executor'){
  await refresh(c);const g=await grant(c,kind,role),payload={scope_id:c.b.scope_id,object_id:c.state.object_id,expected_revision:c.state.revision,grant_id:g.grant_id,event_id:`dsh:${kind}:${g.grant_id}`,observation:{kind,...extra}};
  let r;for(;;){try{r=await client.operate('harness.observe',payload);break;}catch(error){if(!String(error.message).endsWith('rejected: conflict'))throw error;const latest=await refresh(c);if(latest.revision===payload.expected_revision)throw error;payload.expected_revision=latest.revision;}}
  await refresh(c);receipt(c,r);return r;
 }
 function enqueue(c,fn){const run=c.serial.then(()=>{if(c.failure)throw c.failure;return fn();});c.serial=run.catch(error=>{if(!c.failure){c.failure=error;c.finished?.reject(error);ctx.logger.warn('SIC native lifecycle delivery: '+error.message);}});return run;}
 async function drain(c){await c.serial;if(c.failure)throw c.failure;}
 async function prepare(p,kind,name,args,call_id=p.logical_call_id??p.call_id){
  const launch={call_id,native_name:name,arguments_sha256:digest(args)};let c={...p,logical_call_id:call_id,kind,args,name,serial:Promise.resolve(),finished:completion(),state:null,offset:0,used:false};
  const g=await grant(c,kind+'.intent','worker',launch),r=await client.operate('harness.observe',{scope_id:p.b.scope_id,grant_id:g.grant_id,event_id:'dsh-intent:'+g.grant_id,observation:{kind:kind+'.intent',label:args.command??args.description??args.meta?.name??name,job_kind:kind==='job'?name:kind,started_at_ms:Date.now(),output_mode:kind==='peer'?'final':'stream'}});
  c=remember(c,r.value.activity);
  if(c.state.native_tool_result)return{c,replay:c.state.native_tool_result};
  c.claim_id=randomUUID();const claimed=await observe(c,'job.claim',{claim_id:c.claim_id},'worker');
  if(claimed.execution_permit!==true){await observe(c,'job.recover');throw Error('native execution was already admitted; outcome requires reconciliation, never redispatch');}
  return{c};
 }
 function project(c){const s=c.state,n=s.native_view??{},status=s.status==='unknown'?'running':s.status;return structuredClone({...n,id:c.peerSessionId??s.external_id,kind:s.job_kind,label:s.label,owner:c.agent.session.id,status,startedAt:s.startedAt,...s.finishedAt===undefined?{}:{finishedAt:s.finishedAt},...(s.status==='unknown'?{detail:'executor state unknown; reconciliation required; no redispatch permit'}:s.detail?{detail:s.detail}:{}),output:n.output??{total:(s.chunks??[]).reduce((n,t)=>n+Buffer.byteLength(t),0),earliest:0}});}
 function list(id){const agent=owner(id);if(!agent||!binding(agent))return[];const scope=binding(agent).scope_id;return[...new Set(records.values())].filter(c=>c.b.scope_id===scope&&c.state.external_id&&c.state.visible!==false&&records.get(key(scope,'native:'+(c.peerSessionId??c.state.external_id)))===c).map(project);}
 function byNative(id,caller){const agent=owner(caller);if(!agent||!binding(agent))throw Error('native activity owner is not live');const c=records.get(key(binding(agent).scope_id,'native:'+id));if(!c)throw Error('native activity not found in Core owner scope');return c;}
 function reserve(c,id,executor){if(c.used)throw Error('one logical launch cannot dispatch two native executors');c.used=true;c.native_id=id;handles.set(key(c.b.scope_id,id),executor);records.set(key(c.b.scope_id,'native:'+id),c);}
 async function attach(c,id,kind){await observe(c,'job.attach',{claim_id:c.claim_id,external_id:id,executor:{kind,id,...c.peerSessionId?{session_id:c.peerSessionId,residency_epoch:c.peerEpoch}:{}},started_at_ms:Date.now()});}
 async function workerCall(c,name,args,identity,waitTimeoutMs,launch){const g=await client.issueTurn({scope_id:c.b.scope_id,chain_id:c.b.chain_id??null,turn_id:identity,role:'worker',human_intent:'none',human_message_id:null,allowed_calls:[name],object_id:c.state.object_id,goal_round:null,max_goal_rounds:null,wait_timeout_ms:waitTimeoutMs??null,...launch?{launch}:{}});const r=await client.operate('harness.call',{scope_id:c.b.scope_id,name,arguments:{...args,...args.job_id===undefined?{}:{job_id:c.state.external_id}},grant_id:g.grant_id,event_id:'dsh-control:'+g.grant_id});await refresh(c);receipt(c,r);if(c.peerSessionId&&r.value?.job)r.value.job.id=c.peerSessionId;return r;}
 async function executeCancel(c,request,reason){if(terminal(c.state))return;const h=handles.get(key(c.b.scope_id,c.state.external_id));if(!h)throw Error('native cancellation handle is unavailable; Core cancellation remains requested');if(h.kind==='job')nativeJobs.cancelExecutor(c.state.external_id,c.agent.id,reason);else await h.cancel(reason);}
 async function nativeWait(c,timeoutMs,signal){
  const h=handles.get(key(c.b.scope_id,c.state.external_id));if(terminal(c.state)||!h)return;
  if(h.kind==='job')await nativeJobs.wait(c.state.external_id,timeoutMs,c.agent.id,signal);
  else await new Promise((resolve,reject)=>{let settled=false;const finish=error=>{if(settled)return;settled=true;clearTimeout(timer);signal.removeEventListener('abort',abort);if(error)reject(error);else resolve();},abort=()=>finish(terminal(c.state)?null:Error('wait aborted')),timer=setTimeout(()=>finish(),timeoutMs);signal.addEventListener('abort',abort,{once:true});c.finished.promise.then(()=>finish(),finish);if(signal.aborted)abort();});
  await drain(c);
 }
 async function control(name,args,exec,waitTimeoutMs){
  const p=nativeExec(exec);if(name==='job_list'){await refreshScope(p.agent);return list(p.agent.id);}
  const c=byNative(args.job_id,p.agent.id);await drain(c);
  const collecting=name==='job_output';
  if(collecting)collectors.set(c,(collectors.get(c)??0)+1);
  if(name==='job_kill')modelKills.add(c.state.external_id);
  try{
   let effectiveWait;
   if(name==='job_output'&&args.wait===true){
    if(!Number.isFinite(waitTimeoutMs)||waitTimeoutMs<=0||waitTimeoutMs>2_147_483_647)throw Error('invalid native wait timeout');
    // Native deadline uses Node setTimeout: fractional milliseconds truncate,
    // with sub-millisecond positive waits clamped to 1ms. No new wait policy.
    effectiveWait=Math.max(1,Math.trunc(waitTimeoutMs));
    if(!seenControls.has(p.logical_call_id))await nativeWait(c,waitTimeoutMs,exec.signal);
   }
   const r=await workerCall(c,name,{...args,...effectiveWait===undefined?{}:{timeout_ms:effectiveWait}},p.turn_id,effectiveWait===undefined?undefined:0,{call_id:p.logical_call_id,native_name:name,arguments_sha256:digest(args)});seenControls.add(p.logical_call_id);if(r.executor_request)await executeCancel(c,r.executor_request,args.reason);
   if(name==='job_output'){
    if(handles.get(key(c.b.scope_id,c.state.external_id))?.kind==='job'&&Number.isSafeInteger(c.state.native_model_cursor)){
     const view=nativeJobs.acknowledgeCollection(c.state.external_id,c.state.native_model_cursor,c.state.result_collected===true,c.agent.id);if(view)await enqueue(c,()=>observe(c,'job.native_view',{view}));
    }
    const proof={lifecycle_object_id:r.object_id,collection_receipt_id:r.receipt_id,collection_digest:r.collection.text_digest,chain_id:c.b.chain_id??null},definition=ctx.tools.get(name,exec.agent);
    collections.set(collectionKey(exec.agent,exec.callId),{proof,text:r.value.text,expected:definition.output.render(args,r.value),agent:exec.agent,turn_id:p.turn_id});
   }
   return r.value;
  }finally{
   if(collecting){
    const remaining=collectors.get(c)-1;
    if(remaining===0)collectors.delete(c);
    else collectors.set(c,remaining);
   }
  }
 }
 const jobPort={handles:selected,
  beforeStart(spec){if(!selected(spec.owner))return;const c=als.getStore();if(!c||c.agent.id!==spec.owner||c.used||spec.toolCall?.callId!==c.call_id)throw Error('native job producer has no matching durable one-shot tool claim');},
  reserve(id,spec){if(!selected(spec.owner))return;reserve(als.getStore(),id,{kind:'job'});},
  deliver(event,id,snapshot,next){const c=byNative(event.type==='output'?event.id:event.job.id,id),frozen=structuredClone(snapshot);enqueue(c,async()=>{
   if(event.type==='registered'){await attach(c,frozen.job.id,'dsh-job');for(const fn of c.workflowPreRegistration??[])fn();c.workflowPreRegistration=null;}
   if(event.type!=='removed'){
    for(const chunk of frozen.read.chunks){const end=chunk.at+Buffer.byteLength(chunk.text);if(end<=c.offset)continue;const at=Math.max(chunk.at,c.offset),text=Buffer.from(chunk.text).subarray(at-chunk.at).toString('utf8');await observe(c,'job.output',{text,native_chunk:{...chunk,at,text,...chunk.at>c.offset?{gapBefore:true}:{}}});c.offset=end;}
    c.offset=Math.max(c.offset,frozen.read.next);await observe(c,'job.native_view',{view:frozen.job});
   }
   if(event.type==='stopping')await observe(c,'job.stopping');
   if(event.type==='settled'){
    if(!terminal(c.state))await observe(c,'job.settle',{status:frozen.job.status,finished_at_ms:frozen.job.finishedAt??Date.now(),...frozen.result===undefined?{}:{result:frozen.result},...frozen.job.detail===undefined?{}:{detail:frozen.job.detail}});
    for(const deliver of c.workflowFinishEvents??[])deliver();c.workflowFinishEvents=null;
    event.awaited=event.awaited||collectors.has(c)||modelKills.delete(c.state.external_id);
   }
   if(event.type==='removed'){await observe(c,'job.remove',{reason:'native registry removed execution resource'});handles.delete(key(c.b.scope_id,c.state.external_id));}
   next();
  });},
  list,
  output(id,from,caller){if(!Number.isSafeInteger(from)||from<0)throw Error('invalid native observer offset');const c=byNative(id,caller),s=c.state,view=project(c),chunks=[];let at=0;for(let i=0;i<(s.chunks??[]).length;i++){const text=s.chunks[i],meta=s.chunk_metadata?.[i];let chunk=meta??{at,text};const end=chunk.at+Buffer.byteLength(text);if(end>Math.max(from,view.output.earliest)){if(chunk.at<view.output.earliest)chunk={...chunk,at:view.output.earliest,text:Buffer.from(chunk.text).subarray(view.output.earliest-chunk.at).toString('utf8')};chunks.push(structuredClone(chunk));}at=end;}return{chunks,next:view.output.total??at,lossy:from<view.output.earliest};},
  consumed(id,caller){const c=byNative(id,caller);enqueue(c,()=>workerCall(c,'job_output',{job_id:id},'native-foreground-read:'+c.call_id));},
  async kill(id,caller,reason,invocation){const c=byNative(id,caller),running=als.getStore(),request=invocation?.request?.args?.request;if(!running&&!(invocation?.service==='jobController'&&invocation.request?.namespace==='job'&&invocation.request.method==='kill'&&request?.sessionId===caller&&request?.jobId===id&&!invocation.peer?.signal.aborted&&!invocation.signal.aborted))throw Error('authorized native UI or executing host cancellation source required');await drain(c);const r=await workerCall(c,'job_kill',{job_id:id,reason},'native-host-cancel:'+randomUUID());if(r.executor_request)await executeCancel(c,r.executor_request,reason);return r.value.outcome==='already-finished'?'already-finished':'requested';},
  control,collectionMeta(){const exec=controlContext.getStore(),entry=exec?.agent?collections.get(collectionKey(exec.agent,exec.callId)):null,turn=exec?.agent?ctx.sessionProjections.stateOf(exec.agent.session,'turnBoundary')?.openTurnStartSeq:null;return entry&&entry.turn_id===`${exec.agent.session.id}:${turn}`?{sic_collection:entry.proof}:{};},drain:async()=>Promise.all([...new Set(records.values())].map(drain)),
 };
 disposers.push(ctx.jobs.attachExternalLifecycle(jobPort));
 const mountedPeers=new WeakSet(),mountedWorkflows=new WeakSet();
 function mountSubagents(subagents){if(closing||mountedPeers.has(subagents))return;assertPeerRuntime(subagents);mountedPeers.add(subagents);disposers.push(subagents.attachExternalLifecycle({
  async prepare(agent,nativeId,provider,kind){
   if(!binding(agent))return null;if(closing)throw Error('native lifecycle adapter is closing');
   if(kind!=='local'&&kind!=='external')throw Error('native peer activation kind required');
   await refreshScope(agent);const b=binding(agent),indexed=records.get(key(b.scope_id,'native:'+nativeId));
   const legacy=indexed?.state.executor?.kind==='dsh-subagent'&&indexed.state.external_id===nativeId?indexed:null;
   const prior=kind==='local'?[...new Set(records.values())].filter(row=>row.b.scope_id===b.scope_id&&row.peerSessionId===nativeId).reduce((latest,row)=>!latest||(row.peerEpoch??0)>(latest.peerEpoch??0)?row:latest,legacy):null;
   if(prior&&!terminal(prior.state))throw Error('previous peer residency is not acknowledged terminal; reconcile it before resume');
   const nextEpoch=(prior?.peerEpoch??0)+1;
   let c=als.getStore();if(!c||c.kind!=='peer'||c.used){
    // Core's launch ledger survives a crash before attachment, but job_list
    // cannot discover that unattached claim. Reuse the same launch identity
    // until an attached epoch is durably recorded, even after adapter restart.
    const residency=kind==='local'?digest([b.scope_id,nativeId,nextEpoch]):randomUUID();
    const call_id='native-residency:'+residency;
    const outer=c,p={agent,b,turn_id:outer?.turn_id??call_id,call_id,root_call_id:outer?.root_call_id};
    ({c}=await prepare(p,'peer','native-subagent:'+provider,{description:'native child '+nativeId},call_id));
   }
   if(kind==='local'){c.peerSessionId=nativeId;c.peerEpoch=nextEpoch;
    c.peerExecutorId='dsh-peer:'+digest([b.scope_id,nativeId,c.state.object_id]);}return c;
  },
  async attach(agent,id,dispose,c){if(!c)return;if(c.peerSessionId&&c.peerSessionId!==id)throw Error('peer claim address mismatch');const executionId=c.peerExecutorId??id;reserve(c,executionId,{kind:'peer',cancel:dispose});await enqueue(c,()=>attach(c,executionId,'dsh-subagent'));},
  async settle(agent,id,result,claim){
   if(!binding(agent))return;
   const c=claim;
   if(!c?.state?.object_id||c.agent!==agent||records.get(key(c.b.scope_id,c.state.object_id))!==c)throw Error('original native peer activation claim required');
   if((c.peerSessionId??c.native_id)!==id)throw Error('peer settlement address mismatch');
   await enqueue(c,async()=>{if(!terminal(c.state))await observe(c,'job.settle',{status:result.stopReason==='completed'?'completed':result.stopReason==='aborted'?'killed':'failed',finished_at_ms:Date.now(),result:result.output.filter(b=>b.type==='text').map(b=>b.text).join(''),detail:result.stopReason});});
  },
 }));}
 function mountWorkflow(workflow){if(closing||mountedWorkflows.has(workflow))return;if(!workflow.attachExternalLifecycle)throw Error('patched native workflow lifecycle required');mountedWorkflows.add(workflow);disposers.push(workflow.attachExternalLifecycle({
  start(request,execute){if(!binding(request.parent))return execute();const c=als.getStore();if(!c||c.kind!=='workflow')throw Error('native workflow has no durable tool claim');const background=c.used;c.workflowBackground=background;c.workflowBuffered=[];const run=execute();c.workflowRun=run;if(!background){reserve(c,run.id,{kind:'workflow',cancel:reason=>run.cancel(reason),join:()=>run.result.then(()=>{})});enqueue(c,()=>attach(c,run.id,'dsh-workflow'));for(const fn of c.workflowBuffered)fn();}else c.workflowPreRegistration=c.workflowBuffered;c.workflowBuffered=null;if(background)return run;return{id:run.id,meta:run.meta,cancel:reason=>run.cancel(reason),dispose:()=>run.dispose(),result:run.result.then(async result=>{await enqueue(c,async()=>{if(!terminal(c.state))await observe(c,'job.settle',{status:result.stopReason==='completed'?'completed':result.stopReason==='cancelled'?'killed':'failed',finished_at_ms:Date.now(),result:JSON.stringify(result.value),detail:result.stopReason});});return result;})};},
  event(name,args,next){const c=als.getStore();if(!c||c.kind!=='workflow')return next();if(name==='workflow/end'&&c.workflowBackground){(c.workflowFinishEvents??=[]).push(next);return;}
   const deliver=()=>enqueue(c,async()=>{if(name==='workflow/end'&&c.workflowRun&&!terminal(c.state)){const result=await c.workflowRun.result;await observe(c,'job.settle',{status:result.stopReason==='completed'?'completed':result.stopReason==='cancelled'?'killed':'failed',finished_at_ms:Date.now(),result:JSON.stringify(result.value),detail:result.stopReason});}if(name==='workflow/log'&&!c.state.external_id)return;next();});if(c.workflowBuffered)c.workflowBuffered.push(deliver);else deliver();},
 }));}
 if(ctx.get('subagents'))mountSubagents(ctx.get('subagents'));if(ctx.get('workflowEngine'))mountWorkflow(ctx.get('workflowEngine'));
 const peerInjection=ctx.inject(['subagents'],child=>{mountSubagents(child.subagents);}),workflowInjection=ctx.inject(['workflowEngine'],child=>{mountWorkflow(child.workflowEngine);});disposers.push(()=>peerInjection.dispose(),()=>workflowInjection.dispose());
 async function refreshScope(agent){if(!binding(agent))return;const b=binding(agent),rows=await client.operate('harness.call',{scope_id:b.scope_id,name:'job_list',arguments:{}});for(const row of rows.bindings??[]){const old=records.get(key(b.scope_id,row.object_id)),c=old??{agent,b,turn_id:'native-recovery:'+agent.session.id,serial:Promise.resolve(),state:{object_id:row.object_id},offset:0};await refresh(c);if(!terminal(c.state)&&c.state.dispatch_state!=='unknown'&&!handles.has(key(b.scope_id,c.state.external_id)))await observe(c,'job.recover');}}
 disposers.push(ctx.on('tools/execute',async(exec,next)=>{
  const kind=launchKindFor?.(exec)??nativeJobs.externalLaunchKind(ctx.tools.get(exec.name,exec.agent));if(!kind||!exec.agent||!binding(exec.agent))return exec.name==='job_output'?controlContext.run(exec,next):next();const p=nativeExec(exec),cacheKey=key(p.b.scope_id,p.logical_call_id),fingerprint=digest(exec.arguments),existing=inflight.get(cacheKey);if(existing){if(existing.fingerprint!==fingerprint)throw Error('logical native call changed during execution');return existing.run;}
  const run=(async()=>{const{c,replay}=await prepare(p,kind,exec.name,exec.arguments);if(replay)return structuredClone(replay);if(closing){await observe(c,'job.recover');throw Error('native lifecycle adapter closed before executor dispatch');}const result=await als.run(c,next);await drain(c);await enqueue(c,()=>observe(c,'job.tool_result',{result}));if(!c.used){if(!result.isError&&result.value?.kind==='foreground')await enqueue(c,()=>observe(c,'job.settle',{status:result.value.aborted||result.value.timedOut?'killed':result.value.exitCode===0?'completed':'failed',finished_at_ms:Date.now(),detail:'original native foreground executor returned after completion'}));else await enqueue(c,()=>observe(c,'job.recover'));}return result;})();inflight.set(cacheKey,{fingerprint,run});try{return await run;}finally{if(inflight.get(cacheKey)?.run===run)inflight.delete(cacheKey);}
 }));
 disposers.push(ctx.on('session/event',(session,event)=>{
  if(event.type!=='tool/result')return;const message=event.data.message,callId=message.toolCallId??message.source?.callId,collectionId=session.id+'\0'+callId,entry=collections.get(collectionId);if(!entry||entry.agent.session!==session)return;collections.delete(collectionId);
  const turn=ctx.sessionProjections.stateOf(session,'turnBoundary')?.openTurnStartSeq;if(entry.turn_id!==`${session.id}:${turn}`)return;
  if(message.isError||!entry.text||JSON.stringify(message.content)!==JSON.stringify(entry.expected)||entry.expected[0]?.type!=='text'||!entry.expected[0].text.startsWith(entry.text))return;
  const record={...entry.proof,message_id:message.id,call_id:callId,material:{block_index:0,utf8_start:0,utf8_end:Buffer.byteLength(entry.text)}};
  queueMicrotask(()=>{try{appendRecord(session,'plugin:sic/jobs/collection',record);notify(onCollection,record,'native collection observer');}catch(error){ctx.logger.warn('native collection binding failed: '+error.message);}});
 }));
 disposers.push(ctx.on('agent/created',async({agent})=>{await refreshScope(agent);}));for(const agent of ctx.agents.list())await refreshScope(agent);
 async function adoptPublication({agent,publication,request_id,adopted_cut_id}={}){
  assertWorker(agent);const b={...binding(agent)};publication=structuredClone(publication);
  if(closing||typeof request_id!=='string'||!request_id.trim()||!b.chain_id||publication?.chain_id!==b.chain_id||typeof publication.publication_id!=='string'||!publication.publication_id)throw Error('exact selected publication adoption binding required');
  if(adopted_cut_id!==undefined&&(typeof adopted_cut_id!=='string'||!adopted_cut_id))throw Error('actual adopted cut identity must be nonempty text');
  const proofs=publication.publication?.collection_adoptions??(publication.publication?.collection_adoption?[publication.publication.collection_adoption]:[]);
  if(!Array.isArray(proofs))throw Error('invalid publication collection adoption receipt');
  const adoptions=[],skipped=[],seen=new Set();let selectedCut=adopted_cut_id;
  for(const proof of structuredClone(proofs)){
   if(typeof proof.eligible_current_p!=='boolean')throw Error('invalid publication collection eligibility');
   if(typeof proof.lifecycle_object_id!=='string'||!proof.lifecycle_object_id||typeof proof.collection_receipt_id!=='string'||!proof.collection_receipt_id||!/^[a-f0-9]{64}$/.test(proof.collection_digest))throw Error('invalid eligible publication collection receipt');
   const identity=JSON.stringify([proof.lifecycle_object_id,proof.collection_receipt_id]);if(seen.has(identity))continue;seen.add(identity);
   let c=records.get(key(b.scope_id,proof.lifecycle_object_id));
   if(!c)c=remember({agent,b,turn_id:'native-publication:'+agent.session.id+':'+request_id,serial:Promise.resolve(),offset:0},{object_id:proof.lifecycle_object_id});
   if(c.b.chain_id!==b.chain_id)throw Error('publication activity belongs to another chain');
   const run=c.serial.then(async()=>{
    if(c.failure)throw c.failure;
    for(;;){
     const state=await client.operate('harness.get',{scope_id:b.scope_id,object_id:c.state.object_id,adoption_probe:{publication_id:publication.publication_id,collection_receipt_id:proof.collection_receipt_id,...selectedCut===undefined?{}:{adopted_cut_id:selectedCut}}});
     const probe=state.adoption_probe;delete state.adoption_probe;remember(c,state);
     if(probe?.publication_id!==publication.publication_id||probe.collection_receipt_id!==proof.collection_receipt_id||probe.lifecycle_object_id!==c.state.object_id||typeof probe.eligible_current_p!=='boolean'||typeof probe.adopted_cut_id!=='string'||!probe.adopted_cut_id||selectedCut!==undefined&&probe.adopted_cut_id!==selectedCut)throw Error('invalid actual-cut collection adoption probe');
     selectedCut??=probe.adopted_cut_id;
     const evidence={...proof,eligible_current_p:probe.eligible_current_p,publication_id:publication.publication_id,adopted_cut_id:probe.adopted_cut_id,p_lineage:structuredClone(probe.p_lineage),revision:c.state.revision};
     if(!probe.eligible_current_p)return{...evidence,skipped:'not_represented_in_adopted_p'};
     if(c.state.adoptions?.some(a=>a.collection_receipt_id===proof.collection_receipt_id&&a.publication_id===publication.publication_id))return{...evidence,replayed:true};
     const g=await grant({...c,turn_id:'native-publication:'+agent.session.id+':'+request_id},'result.adopt'),revision=c.state.revision;
     let result;try{result=await client.operate('harness.observe',{scope_id:b.scope_id,object_id:c.state.object_id,expected_revision:revision,grant_id:g.grant_id,event_id:'dsh-adoption:'+g.grant_id,
      observation:{kind:'result.adopt',collection_receipt_id:proof.collection_receipt_id,publication_id:publication.publication_id,adopted_cut_id:probe.adopted_cut_id}});}catch(error){
      if(!String(error.message).endsWith('rejected: conflict'))throw error;
      await refresh(c);if(c.state.revision===revision)throw error;continue;
     }
     await refresh(c);receipt(c,result);return{...evidence,revision:c.state.revision,receipt_id:result.receipt_id,replayed:false};
    }
   });
   // Adoption shares the native object's event order. A rejected host receipt
   // fails its dispatch without poisoning subsequent executor observations.
   c.serial=run.catch(()=>{});const result=await run;(result.skipped?skipped:adoptions).push(result);
  }
  return{recorded:true,request_id,publication_id:publication.publication_id,adopted_cut_id:selectedCut??null,adoptions,skipped};
 }
 let disposal;function dispose(){return disposal??=(async()=>{
  closing=true;const failures=[];
  const capture=async action=>{try{return await action();}catch(error){failures.push(error);}};
  const drainAll=()=>Promise.all([...new Set(records.values())].map(c=>capture(()=>drain(c))));
  const stopped=new Map();
  const stopNative=()=>Promise.all([...new Set(records.values())].map(c=>{
   const nativeId=c.state.external_id??c.native_id,h=handles.get(key(c.b.scope_id,nativeId));if(!h)return;
   if(stopped.has(h))return stopped.get(h);
   // Resource-owner teardown cannot wait behind uncertain control delivery.
   // Start local cancellation now; keep its physical join independent of Core.
   const physical=capture(async()=>{
    if(!terminal(c.state))await capture(()=>h.kind==='job'?nativeJobs.cancelExecutor(nativeId,c.agent.id,'native lifecycle adapter unloaded'):h.cancel('native lifecycle adapter unloaded'));
    if(h.kind==='job')await capture(()=>nativeJobs.joinExecutor(nativeId,c.agent.id));
    else if(h.join)await capture(h.join);
   });
   // Preserve observation ordering and report uncertainty without retrying the
   // failed queue or inventing a terminal Core state from local cancellation.
   const authoritative=capture(()=>enqueue(c,async()=>{if(!terminal(c.state))await workerCall(c,'job_kill',{job_id:nativeId,reason:'native lifecycle adapter unloaded'},'native-jobs-unload:'+randomUUID());}));
   const stopping=Promise.all([physical,authoritative]);stopped.set(h,stopping);return stopping;
  }));
  try{
   await Promise.all([stopNative(),drainAll()]);
   await Promise.allSettled([...inflight.values()].map(entry=>entry.run));
   await Promise.all([stopNative(),drainAll()]);await drainAll();
  }finally{for(const release of disposers.reverse())await capture(release);}
  if(failures.length)throw new AggregateError([...new Set(failures)],'native lifecycle unload could not acknowledge every authoritative observation');
 })();}

 ctx.effect(()=>dispose);
 return{facade:control,refresh:refreshScope,list,adoptPublication,dispose,drain:async()=>Promise.all([...new Set(records.values())].map(drain)),readState:(id,caller)=>structuredClone(byNative(id,caller).state)};
}
