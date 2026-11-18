import {evidencePath,testCoreBinary} from './test-paths.mjs';
import{test}from'node:test';import assert from'node:assert/strict';import{spawn,execFile}from'node:child_process';import{promisify}from'node:util';import{mkdir}from'node:fs/promises';import{join}from'node:path';import{randomUUID}from'node:crypto';import{createServer}from'node:net';
import{createCoreClient}from'../transparent/core-client.mjs';import{createTurnGrantResolver}from'../transparent/turn-authority.mjs';import{createGoalFacade}from'../transparent/harness-facade.mjs';
async function isolatedCore(){
 const binary=process.env.SIC_CORE_TEST_BIN??testCoreBinary();
 const root=join(evidencePath('dsh-sic-transparent-comparison-20261117/adaptation/fixtures'),'goal-http-'+randomUUID());await mkdir(root,{recursive:true});
 const db=join(root,'core.db'),token='synthetic-'+randomUUID(),envName='SIC_ISOLATED_GOAL_FIXTURE_TOKEN',env={PATH:process.env.PATH,SystemRoot:process.env.SystemRoot,TEMP:root,TMP:root,[envName]:token};
 await promisify(execFile)(binary,['init',db],{windowsHide:true,env});const reserve=createServer();await new Promise(r=>reserve.listen(0,'127.0.0.1',r));const port=reserve.address().port;await new Promise(r=>reserve.close(r));
 const child=spawn(binary,['serve',db,`127.0.0.1:${port}`,'fixture-actor',envName],{windowsHide:true,env,stdio:['ignore','pipe','pipe']}),baseURL=`http://127.0.0.1:${port}`;
 for(let i=0;i<100;i++){try{if((await fetch(baseURL+'/health',{headers:{authorization:'Bearer '+token}})).ok)break;}catch{}await new Promise(r=>setTimeout(r,50));}
 const core=createCoreClient({baseURL,credential:()=>token,context:()=>({actor:'fixture-actor',chain_id:null,allow_write:true,allow_model:false,allow_revoke:false})});
 return{core,close:async()=>{child.kill();if(child.exitCode===null)await new Promise(r=>child.once('exit',r));}};
}

test('real Core HTTP + native-provenance facade handles human resume and exact automatic-round complete',{timeout:20000},async()=>{
 const{core,close}=await isolatedCore();try{
  const scope_id='fixture-native-scope';let start=0,events=[{seq:0,type:'turn/start',data:{turn:1}},{seq:1,type:'user/message',data:{id:'human-create',source:{kind:'user'}}}];
  const agent={id:'fixture-agent',status:'running',session:{id:'fixture-session',snapshotEvents:()=>events}},ctx={agents:{get:()=>agent,currentInitiator:()=>agent,roots:()=>[agent]},sessionProjections:{stateOf:()=>({openTurnStartSeq:start})}};
  const current=async()=> (await core.operate('harness.call',{scope_id,name:'get_goal',arguments:{}})).value.goal;
  const resolveGrant=createTurnGrantResolver({ctx,client:core,roleFor:()=> 'WORKER',scopeFor:()=>scope_id,allowedCallsFor:()=>['get_goal','create_goal','update_goal'],goalFor:current,maxRoundsFor:()=>5});
  const facade=createGoalFacade({operate:core.operate,resolveGrant}),signal=new AbortController().signal;
  let goal=(await facade('create_goal',{objective:'finish actual fixture'},{agent,callId:'create',signal})).goal;
  goal=(await facade('update_goal',{goal_id:goal.id,revision:goal.revision,action:'pause'},{agent,callId:'pause',signal})).goal;assert.equal(goal.phase,'paused');
  start=4;events=[{seq:4,type:'turn/start',data:{turn:2}},{seq:5,type:'user/message',data:{id:'human-resume',source:{kind:'user'}}}];
  goal=(await facade('update_goal',{goal_id:goal.id,revision:goal.revision,action:'resume'},{agent,callId:'resume',signal})).goal;assert.equal(goal.phase,'active');
  const state=await core.operate('harness.get',{scope_id,object_id:goal.id}),g=await core.issueTurn({scope_id,chain_id:null,turn_id:'host-round-1',role:'executor',human_intent:'none',human_message_id:null,allowed_calls:['goal.round.start'],object_id:goal.id,goal_round:1,max_goal_rounds:null,wait_timeout_ms:null});
  await core.operate('harness.observe',{scope_id,object_id:goal.id,expected_revision:state.revision,grant_id:g.grant_id,event_id:'round-start-1',observation:{kind:'goal.round.start'}});
  goal=await current();start=8;events=[{seq:8,type:'turn/start',data:{turn:3}},{seq:9,type:'user/message',data:{id:'automatic-source',source:{kind:'goal',goalId:goal.id,revision:goal.revision,round:goal.roundsStarted}}}];
  const completed=await facade('update_goal',{goal_id:goal.id,revision:goal.revision,action:'complete'},{agent,callId:'automatic-complete',signal});assert.equal(completed.goal.phase,'complete');assert.equal(completed.goal.roundsStarted,1);
 }finally{await close();}
});
