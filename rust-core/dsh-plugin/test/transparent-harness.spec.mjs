import {test} from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {createCoreClient} from '../transparent/core-client.mjs';
import {createTurnGrantResolver} from '../transparent/turn-authority.mjs';
import {createGoalFacade} from '../transparent/harness-facade.mjs';

test('host turn endpoint and familiar goal call bind actual event provenance, never tool arguments',async()=>{
  const received=[];
  const server=createServer(async(req,res)=>{
    let raw='';for await(const b of req)raw+=b;const body=JSON.parse(raw);received.push({path:req.url,body});
    assert.equal(req.headers.authorization,'Bearer fixture-host');res.setHeader('content-type','application/json');
    if(req.url==='/api/v2/harness/turn')res.end(JSON.stringify({grant_id:'host-issued'}));
    else res.end(JSON.stringify({result:{value:{goal:{id:'g',revision:1,objective:body.payload.arguments.objective,phase:'active',roundsStarted:0,maxGoalRounds:10},activation:'armed'}}}));
  });
  await new Promise(r=>server.listen(0,'127.0.0.1',r));
  try{
    const client=createCoreClient({baseURL:`http://127.0.0.1:${server.address().port}`,credential:()=> 'fixture-host',context:()=>({actor:'host',chain_id:null,allow_write:true,allow_model:false,allow_revoke:false})});
    const events=[{seq:0,type:'user/message',data:{id:'old',source:{kind:'user'}}},{seq:5,type:'turn/start',data:{turn:2}},{seq:6,type:'user/message',data:{id:'human-current',source:{kind:'user'}}}];
    const agent={id:'agent',status:'running',session:{id:'session',snapshotEvents:()=>events}};
    const ctx={agents:{get:()=>agent,currentInitiator:()=>agent,roots:()=>[agent]},sessionProjections:{stateOf:()=>({openTurnStartSeq:5})}};
    const resolveGrant=createTurnGrantResolver({ctx,client,roleFor:()=> 'WORKER',scopeFor:()=> 'session/agent',allowedCallsFor:()=>['create_goal','get_goal','update_goal']});
    const facade=createGoalFacade({operate:client.operate,resolveGrant});
    const value=await facade('create_goal',{objective:'Direct human task'},{agent,callId:"fixture-call",signal:new AbortController().signal});
    assert.equal(value.goal.id,'g');assert.equal(received[0].path,'/api/v2/harness/turn');
    assert.equal(received[0].body.human_message_id,'human-current');assert.equal(received[0].body.human_intent,'direct_human');
    assert.equal(received[1].body.operation,'harness.call');assert.equal(received[1].body.payload.grant_id,'host-issued');
    assert.deepEqual(received[1].body.payload.arguments,{objective:'Direct human task'});
    // A non-root producer and history outside the open turn do not get human authority.
    ctx.agents.roots=()=>[];events.splice(2,1,{seq:6,type:'user/message',data:{id:'tool-text',source:{kind:'subagent'}}});
    await resolveGrant({agent,callId:"fixture-call",signal:new AbortController().signal},'get_goal');
    assert.equal(received.at(-1).body.human_intent,'none');assert.equal(received.at(-1).body.human_message_id,null);
    ctx.agents.currentInitiator=()=>({id:'spoof'});
    await assert.rejects(resolveGrant({agent},'create_goal'),/exact live calling agent/);
    const editor=createTurnGrantResolver({ctx:{...ctx,agents:{...ctx.agents,currentInitiator:()=>agent}},client,roleFor:()=> 'EDITOR',scopeFor:()=> 's',allowedCallsFor:()=>['create_goal']});
    await assert.rejects(editor({agent,callId:'editor-call'},'create_goal'),/not authorized/);
  }finally{server.closeAllConnections();await new Promise(r=>server.close(r));}
});
