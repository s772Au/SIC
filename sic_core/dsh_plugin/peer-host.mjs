/** Execute explicitly registered SIC peer tools through the real DSH runtime. */
import {randomUUID} from 'node:crypto';

export function peerHost(ctx, cfg, {request, log, recordOf, delegation}) {
  const pumps=new Map(), active=new Map();
  let stopped=false;
  const pause=ms=>new Promise(resolve=>setTimeout(resolve,ms));
  const action=(pump,kind,task,fields={})=>request('/api/v2/bridge/host/action',{
    action:kind,host_id:pump.hostId,...(task?{task_id:task.task_id,lease:task.lease}:{}),...fields});
  const admit=async(pump,kind,task,controller,fields={})=>{
    while(true){
      controller.signal.throwIfAborted();
      try{return await action(pump,kind,task,fields);}
      catch(error){
        // Retry only a known pre-admission pause. Unknown transport/reservation
        // outcomes still terminate; no native tool is invoked by this loop.
        if(error?.code!=='host_tool_paused')throw error;
        await pause(100);
      }
    }
  };

  async function execute(pump, task) {
    const controller=new AbortController();
    pump.controllers.add(controller);
    let handle, began=false, watcher;
    try {
      if(task.session_key!==String(pump.parent.id)||!pump.tools.includes(task.name))throw new Error('Peer tool scope mismatch');
      const parentTools=pump.parent.ctx.get('tools');
      if(!parentTools?.get(task.name,pump.parent))throw new Error('Parent tool has been withdrawn');
      // Capture before awaiting: this is the parent's actual delegation scope.
      const policies=delegation.captureDelegatedPolicyOverrides(pump.parent);
      const depth=delegation.resolveChildDepth(pump.parent,cfg.peerMaxDepth);
      const cwd=await pump.parent.ctx.get('workingDirectory').ensure(pump.parent,controller.signal);
      handle=await ctx.agents.create({sessionId:'session-'+randomUUID(),parentAgent:pump.parent,
        meta:delegation.childSessionMeta(pump.parent,depth,false),
        agentOptions:delegation.resolveChildAgentOptions(pump.parent,undefined,depth),signal:controller.signal,
        setup:async (childCtx,child)=>{
          delegation.appendDelegatedPolicyOverrides(child.session,policies);
          await childCtx.get('workingDirectory').set(child,cwd,controller.signal);
          await delegation.applyChildComposition(childCtx,pump.parent,{toolFilter:{allow:pump.tools}});
        }});
      const peer=handle.agent;
      active.set(String(peer.id),{pump,task,controller});
      log(peer.session,'plugin:sic/peer-binding',{schema:1,task,root_session:String(pump.parent.id),peer_session:String(peer.id),scope:'tool execution carrier; SIC peer identity is chain_id'});
      log(pump.parent.session,'plugin:sic/peer-start',{task_id:task.task_id,chain_id:task.chain_id,peer_session:String(peer.id)});
      // Reserve durably before execution. An ambiguous reservation is never retried.
      await admit(pump,'begin',task,controller,{peer_session_id:String(peer.id),host_time_ms:Date.now()});
      began=true;
      let checking=false;
      watcher=setInterval(async()=>{
        if(checking)return;checking=true;
        try {const status=await action(pump,'status',task);if(['cancelled','indeterminate'].includes(status.status))controller.abort(new Error(status.status));}
        catch {controller.abort(new Error('Peer host status transport failed'));}
        finally {checking=false;}
      },250);
      const result=await peer.ctx.get('tools').execute({agent:peer,callId:task.call_id,name:task.name,arguments:task.arguments,signal:controller.signal});
      const wire=JSON.parse(JSON.stringify(result));
      // The native log is the durable outbox; completion retries transmit data only.
      log(peer.session,'plugin:sic/peer-result',{schema:1,task_id:task.task_id,lease:task.lease,host_id:pump.hostId,result:wire});
      log(pump.parent.session,'plugin:sic/peer-result-outbox',{schema:1,task_id:task.task_id,lease:task.lease,host_id:pump.hostId,result:wire});
      let accepted=false;
      for(let attempt=0;attempt<3&&!accepted;attempt++){
        try {await action(pump,'complete',task,{result:wire,is_error:wire.isError===true,host_time_ms:Date.now()});accepted=true;}
        catch {if(attempt<2)await pause(100);}
      }
      if(!accepted)throw new Error('Peer result delivery remains uncertain; tool will not be repeated');
      log(peer.session,'plugin:sic/peer-result-delivered',{schema:1,task_id:task.task_id});
      log(pump.parent.session,'plugin:sic/peer-result-delivered',{schema:1,task_id:task.task_id});
    }catch(error){
      log(pump.parent.session,'plugin:sic/peer-host-error',{task_id:task.task_id,phase:began?'execution-or-result':'reservation',error_type:error?.name??'Error',automatic_redispatch:false});
      try {await action(pump,'indeterminate',task);}catch{}
    }finally{
      if(watcher)clearInterval(watcher);
      controller.abort();
      pump.controllers.delete(controller);
      if(handle){active.delete(String(handle.agent.id));await handle.dispose();}
    }
  }

  // This hook runs after native pre-policy and execution guards. Reservation is
  // not a tool_start; only reaching this actual dispatch hook creates one.
  ctx.on('tools/execute',async(exec,next)=>{
    const item=active.get(String(exec.agent?.id));
    if(!item||exec.parent)return next();
    if(String(exec.callId)!==item.task.call_id||exec.name!==item.task.name)throw new Error('Unexpected peer tool dispatch');
    await admit(item.pump,'dispatch',item.task,item.controller,{host_time_ms:Date.now()});
    log(exec.agent.session,'plugin:sic/peer-dispatch',{task_id:item.task.task_id,call_id:item.task.call_id,host_time_ms:Date.now()});
    return next();
  });

  async function ensure(parent){
    if(stopped||!cfg.peerTools.length)return;
    const id=String(parent.id);
    if(pumps.has(id)){
      if(pumps.get(id).closed)throw new Error('Peer host registration is no longer authorized; explicit host recovery is required');
      return;
    }
    const tools=cfg.peerTools.filter(name=>parent.ctx.get('tools')?.get(name,parent));
    if(tools.length!==cfg.peerTools.length)throw new Error('Requested peer tools must already be visible in the parent');
    const pump={parent,tools,hostId:'dsh-host-'+randomUUID(),inflight:new Set(),controllers:new Set(),closed:false};
    const registration=await request('/api/v2/bridge/host/register',{session_key:id,host_id:pump.hostId,tools});
    pump.generation=registration.generation;
    log(parent.session,'plugin:sic/host-registration',{host_id:pump.hostId,generation:pump.generation,tools});
    // Persisted results are never executed again after resume. Their uncertain
    // delivery stays visible in the session log for explicit host recovery.
    const delivered=new Set(parent.session.snapshotEvents().map(recordOf).filter(r=>r?.type==='plugin:sic/peer-result-delivered').map(r=>r.data.task_id));
    for(const r of parent.session.snapshotEvents().map(recordOf))if(r?.type==='plugin:sic/peer-result-outbox'&&!delivered.has(r.data.task_id))log(parent.session,'plugin:sic/peer-recovery-required',{task_id:r.data.task_id,automatic_redispatch:false});
    pumps.set(id,pump);
    pump.work=(async()=>{
      while(!stopped&&!pump.closed){
        try {
          if(pump.inflight.size<cfg.peerConcurrency){const value=await action(pump,'pull');if(value.task){const work=execute(pump,value.task);pump.inflight.add(work);void work.finally(()=>pump.inflight.delete(work));}}
        }catch(error){
          if(error?.httpStatus===403){
            pump.closed=true;
            log(parent.session,'plugin:sic/host-registration-closed',{generation:pump.generation,automatic_redispatch:false});
          }else log(parent.session,'plugin:sic/host-poll-error',{error_type:error?.name??'Error'});
        }
        await pause(100);
      }
    })();
  }
  ctx.effect(()=>async()=>{
    stopped=true;
    for(const p of pumps.values()){
      p.closed=true;
      for(const controller of p.controllers)controller.abort(new Error('Peer host is shutting down'));
    }
    await Promise.allSettled([...pumps.values()].map(async p=>{
      try {await action(p,'close');}
      catch {log(p.parent.session,'plugin:sic/host-close-unconfirmed',{generation:p.generation,automatic_redispatch:false});}
    }));
    await Promise.allSettled([...pumps.values()].flatMap(p=>[p.work,...p.inflight]));
  });
  return {ensure};
}
