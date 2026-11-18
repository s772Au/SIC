import {digest} from './transport.mjs';
// Mirrors DSH's real goal-tool authority boundary using host session provenance.
// Never accepts a model-supplied role, direct-human flag or grant metadata.
export function createTurnGrantResolver({ctx,client,roleFor,scopeFor,allowedCallsFor,goalFor=()=>null,chainFor=()=>null,maxRoundsFor=()=>null,waitTimeoutFor=()=>null}){
  if(![roleFor,scopeFor,allowedCallsFor,client?.issueTurn].every(f=>typeof f==='function'))throw TypeError('host turn bindings required');
  const grants=new Map();
  return async(exec,name,args)=>{
    const agent=exec.agent;
    if(!agent||ctx.agents.get(agent.id)!==agent||agent.status!=='running'||ctx.agents.currentInitiator()!==agent)throw Error('exact live calling agent required');
    const boundary=ctx.sessionProjections.stateOf(agent.session,'turnBoundary');
    if(!Number.isSafeInteger(boundary?.openTurnStartSeq))throw Error('open native model turn required');
    const role=roleFor(agent),scope_id=scopeFor(agent),allowed_calls=allowedCallsFor(agent);
    if(role!=='WORKER'||!scope_id||!Array.isArray(allowed_calls)||!allowed_calls.includes(name))throw Error('native lifecycle not authorized for this role');
    const events=agent.session.snapshotEvents().filter(e=>e.seq>boundary.openTurnStartSeq);
    const human=ctx.agents.roots().includes(agent)?events.findLast(e=>e.type==='user/message'&&e.data?.source?.kind==='user'):null;
    if(human&&typeof human.data.id!=='string')throw Error('human provenance message identity missing');
    const goal=await goalFor(agent,exec.signal);
    const round=events.findLast(e=>e.type==='user/message'&&e.data?.source?.kind==='goal'
      &&e.data.source.goalId===goal?.id&&e.data.source.revision===goal?.revision&&e.data.source.round===goal?.roundsStarted);
    if(typeof exec.callId!=='string'||!exec.callId)throw Error('native tool call identity required');
    const metadata={scope_id,chain_id:chainFor(agent),turn_id:`${agent.session.id}:${boundary.openTurnStartSeq}`,role:'worker',
      human_intent:human?'direct_human':'none',human_message_id:human?.data.id??null,allowed_calls,
      object_id:(name==='update_goal'||round)&&goal?goal.id:null,goal_round:round?goal.roundsStarted:null,max_goal_rounds:goal?.maxGoalRounds??maxRoundsFor(agent),wait_timeout_ms:waitTimeoutFor(agent,name,args)};
    const event_id='dsh-tool:'+digest([agent.session.id,boundary.openTurnStartSeq,exec.rootCallId??exec.callId,name,exec.callId]),key=JSON.stringify({metadata,event_id});
    let grant=grants.get(key);
    if(!grant){grant=await client.issueTurn(metadata,exec.signal);grants.set(key,grant);if(grants.size>256)grants.delete(grants.keys().next().value);}
    return{...grant,scope_id,role,allowed_calls,event_id};
  };
}
