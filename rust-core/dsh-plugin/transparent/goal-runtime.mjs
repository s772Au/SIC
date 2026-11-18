import {randomUUID} from 'node:crypto';
import {createGoalFacade} from './harness-facade.mjs';

// Core owns durable state. This module owns only receipt-derived synchronous
// views consumed by GoalService/DSH UI and the port used by the original driver.
export async function installCoreGoalRuntime(ctx,{client,bindingFor,resolveGrant,emitGoalChanged,remoteHumanSource,appendRecord}={}){
 if(typeof ctx.goals.attachExternalState!=='function')throw Error('DSH GoalService external-state patch is required');
 if(!client?.operate||!client?.issueTurn||typeof bindingFor!=='function'||typeof resolveGrant!=='function'||typeof emitGoalChanged!=='function')throw Error('Core goal runtime requires native host bindings');
 appendRecord??=(await import('@deepseek-ai/dsh-session')).appendPluginRecord;
 const ports=new Map(),disposers=[],remoteIds=new WeakMap();
 const nativeFacade=createGoalFacade({operate:client.operate,resolveGrant});
 const selected=agent=>bindingFor(agent.session.id,agent);
 function assertLive(agent){if(ctx.agents.get(agent.id)!==agent)throw Error('exact live goal agent required');}
 async function attach(agent){
  if(!selected(agent)||ports.has(agent))return ports.get(agent);
  assertLive(agent);
  if(ctx.goals.externalDriverVersion!==1)throw Error('the unique patched native goal-round driver must be active');
  const binding=selected(agent),scope_id=binding.scope_id;
  if(!scope_id)throw Error('goal scope binding missing');
  let current,full,lastProjection='',serial=Promise.resolve(),closed=false,detach,closing;
  const enqueue=(fn,requireLive=true)=>{const next=serial.then(()=>{if(closed)throw Error('goal state port is closed');if(requireLive)assertLive(agent);return fn();});serial=next.catch(()=>{});return next;};
  function project(value,state,operation){
   const prior=current,goal=value.goal;
   current=goal?{...goal,activation:value.activation,createdAt:prior?.id===goal.id?prior.createdAt:Date.now(),updatedAt:Date.now()}:undefined;
   full=state??null;
   const fingerprint=JSON.stringify({value,revision:full?.revision});
   if(fingerprint===lastProjection)return;
   lastProjection=fingerprint;
   if(ctx.agents.get(agent.id)!==agent)return;
   const {activation,roundsStarted,createdAt,updatedAt,...snapshot}=current??{};
   appendRecord(agent.session,'plugin:sic/goal/receipt',{revision:full?.revision??0,current:current?{goal:snapshot,roundsStarted,createdAt,updatedAt}:null});
   ctx.emit('goal/activation-changed',{sessionId:agent.session.id,...current?{goal:{id:current.id,revision:current.revision,activation:current.activation}}:{}});
   // Observation receipts must not masquerade as human pause/resume mutations.
   if(operation)emitGoalChanged(agent,{operation,ref:{id:current?.id??prior?.id,revision:current?.revision??prior?.revision},...current?{goal:{...current}}:{}});
  }
  async function refresh_(operation){
   const result=await client.operate('harness.call',{scope_id,name:'get_goal',arguments:{}});
   const state=result.value.goal?await client.operate('harness.get',{scope_id,object_id:result.value.goal.id}):null;
   project(result.value,state,operation);
  }
  async function observe(kind,round,extra={}){
   await refresh_();if(!current)return;
   const id=current.id,revision=full.revision;
   const grant=await client.issueTurn({scope_id,chain_id:binding.chain_id??null,turn_id:`dsh-goal:${agent.session.id}:${id}:${kind}:${round??0}:${revision}`,role:'executor',human_intent:'none',human_message_id:null,
    allowed_calls:[kind],object_id:id,goal_round:round??null,max_goal_rounds:current.maxGoalRounds??null,wait_timeout_ms:null});
   await client.operate('harness.observe',{scope_id,object_id:id,expected_revision:revision,grant_id:grant.grant_id,event_id:`dsh-goal:${id}:${kind}:${round??0}:${revision}`,observation:{kind,...extra}});
   await refresh_();
  }
  async function interrupt(reason){
   await refresh_();if(!full)return;
   if(full.roundsStarted>full.roundsFinished)await observe('goal.round.interrupted',full.roundsStarted,{reason});
   else if(current.activation==='armed')await observe('goal.disarm',null);
  }
  const port={
   get:()=>current?structuredClone(current):undefined,
   refresh:()=>enqueue(()=>refresh_()),
   start:source=>enqueue(async()=>{
    await refresh_();
    if(!current||current.id!==source.goalId||current.revision!==source.revision||current.roundsStarted+1!==source.round)throw Error('goal reservation changed before native admission');
    await observe('goal.round.start',source.round);
   }),
   settle:attempt=>enqueue(async()=>{
    await refresh_();if(!full||full.roundsStarted===full.roundsFinished)return;
    if(attempt.cancelled||attempt.phase!=='admitted')await interrupt('native-goal-round-cancelled');
    else await observe('goal.round.finish',full.roundsStarted,{outcome:'progress'});
   }),
   disarm:reason=>enqueue(()=>interrupt(reason??'native-driver-disarm')),
   recordBlocked:args=>enqueue(async()=>{
    await refresh_();
    const boundary=ctx.sessionProjections.stateOf(agent.session,'turnBoundary');
    const admitted=agent.session.snapshotEvents().findLast(e=>e.seq>boundary?.openTurnStartSeq&&e.type==='user/message'&&e.data.source?.kind==='goal');
    if(admitted?.data.source.goalId!==current?.id||admitted.data.source.revision!==current?.revision||admitted.data.source.round!==full?.roundsStarted)return;
    if(args.goal_id!==current.id||args.revision!==current.revision)return;
    if(typeof args.blocked_reason!=='string'||!args.blocked_reason.trim())return;
    if(full.roundsStarted>full.roundsFinished)await observe('goal.round.finish',full.roundsStarted,{outcome:'blocked',condition:args.blocked_reason});
   }),
   mutate:(action,ref,request,source)=>enqueue(async()=>{
    await refresh_();
    // Commands carry a committed command/run user-source record. Remote/UI
    // adapters must pass their authenticated native request identity explicitly.
    let human;
    if(source?.commandId&&source.agent===agent){
     const event=agent.session.snapshotEvents().find(e=>e.type==='command/run'&&e.data.commandId===source.commandId&&e.data.name==='goal'&&e.data.source?.kind==='user');
     if(!event)throw Error('native human goal command provenance missing');
     human={id:`command:${source.commandId}`,turn_id:`command:${agent.session.id}:${event.seq}`};
    }else if(source?.remoteInvocation){
     const invocation=source.remoteInvocation;
     if(invocation.service!=='goals'||invocation.request?.namespace!=='goals'||invocation.request.method!==action||invocation.peer?.principal?.kind!=='owner'||invocation.peer.signal.aborted||invocation.signal.aborted)throw Error('native owner goal RPC required');
     let id=remoteIds.get(invocation);
     if(!id){id='native-goal-rpc:'+randomUUID();remoteIds.set(invocation,id);appendRecord(agent.session,'plugin:sic/goal/intent',{request_id:id,peer_id:invocation.peer.id,action,...ref?{goal_id:ref.id,revision:ref.revision}:{}});}
     human={id,turn_id:`rpc:${agent.session.id}:${id}`};
    }else human=await remoteHumanSource?.(agent,action,ref,request);
    if(!human?.id||!human?.turn_id)throw Error('authenticated native human goal mutation source required');
    const args=action==='create'?{objective:request.objective,...request.maxGoalRounds===undefined?{}:{max_goal_rounds:request.maxGoalRounds}}
     :{goal_id:ref.id,revision:ref.revision,action,...request?.objective===undefined?{}:{objective:request.objective},...request?.maxGoalRounds===undefined?{}:{max_goal_rounds:request.maxGoalRounds}};
    const name=action==='create'?'create_goal':action==='clear'?'clear_goal':'update_goal',grant=await client.issueTurn({scope_id,chain_id:binding.chain_id??null,turn_id:human.turn_id,human_message_id:human.id,human_intent:'direct_human',role:'worker',allowed_calls:[name],object_id:current?.id??null,goal_round:null,max_goal_rounds:ctx.goals.defaultMaxGoalRounds??null,wait_timeout_ms:null});
    const receipt=await client.operate('harness.call',{scope_id,name,arguments:args,grant_id:grant.grant_id,event_id:`dsh-goal-human:${human.id}:${action}`});
    await refresh_(action);return action==='clear'?receipt.value.cleared:port.get();
   }),
   close:()=>closing??=(async()=>{try{await enqueue(()=>interrupt('native-goal-port-unloaded'),false);}finally{closed=true;detach?.();ports.delete(agent);}})(),
  };
  ports.set(agent,port);detach=ctx.goals.attachExternalState(agent,port);
  try{await enqueue(()=>interrupt('native-session-start-or-resume'));}catch(error){detach();ports.delete(agent);throw error;}
  return port;
 }
 const facade=async(name,args,exec)=>{
  const port=await attach(exec.agent);if(!port)throw Error('unselected goal session');
  // Tool grant resolution reads authoritative Core state; notify the native
  // driver only after both mutation receipt and derived UI view are committed.
  if(name==='update_goal'&&args.action==='blocked'){
   await resolveGrant(exec,name,args);
   await port.recordBlocked(args);
  }
  const result=await nativeFacade(name,args,exec);
  await port.refresh();
  if(name!=='get_goal')emitGoalChanged(exec.agent,{operation:name==='create_goal'?'create':args.action,ref:{id:result.goal?.id,revision:result.goal?.revision},...result.goal?{goal:port.get()}:{}});
  return result;
 };
 disposers.push(ctx.on('agent/created',async({agent})=>{await attach(agent);}));
 disposers.push(ctx.on('agent/disposed',({agent})=>{const port=ports.get(agent);if(port)void port.close().catch(error=>ctx.logger.warn(String(error)));}));
 for(const agent of ctx.agents.list())await attach(agent);
 async function dispose(){for(const dispose of disposers.reverse())dispose();await Promise.all([...ports.values()].map(port=>port.close()));}
 ctx.effect(()=>dispose);
 return{facade,attach,refresh:async agent=>(await attach(agent))?.refresh(),readGoal:agent=>ports.get(agent)?.get(),defaultMaxGoalRounds:ctx.goals.defaultMaxGoalRounds,dispose};
}
