import {dshSourcePath,fixturePath,evidencePath,testCoreBinary} from './test-paths.mjs';
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {spawn,execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdir,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {randomUUID} from 'node:crypto';
import {createServer} from 'node:net';
import {prepareGoalRuntime} from '../transparent/prepare-goal-runtime.mjs';
import {installCoreGoalRuntime} from '../transparent/goal-runtime.mjs';
import {createCoreClient} from '../transparent/core-client.mjs';
import {createTurnGrantResolver} from '../transparent/turn-authority.mjs';
import {installGoalCompatibility} from '../transparent/harness-facade.mjs';

const dsh=dshSourcePath();
const evidence=process.env.SIC_GOAL_EVIDENCE_ROOT??evidencePath('dsh-sic-transparent-comparison-20261117/goal-runtime');
const load=p=>import(pathToFileURL(join(dsh,p)).href);
const {Context}=await load('vendor/cordis/lib/index.js');
const {mountAgentLoopTestDependencies}=await load('packages/test-support/agent-loop-testkit/lib/index.js');
const {default:AgentLoop}=await load('packages/core/agent-loop/lib/index.js');
const {agentEvents}=await load('packages/core/agent/lib/index.js');
const {LlmAdapter,createUserMessage}=await load('packages/llm/llm/lib/index.js');
const {default:Commands}=await load('packages/interaction/commands/lib/index.js');
const {appendPluginRecord}=await load('packages/core/session/lib/index.js');
const sessionPersistence=await load('packages/session/session-persistence-jsonl/lib/index.js');
const goalTools=await load('packages/goal/tool-goal/lib/index.js');
const {default:TypertRegistry}=await load('packages/typert/registry/lib/index.js');
const {default:RemoteAccess}=await load('packages/api/remote-access/lib/index.js');
const {default:Gateway}=await load('packages/api/gateway/lib/index.js');
const {Peer}=await load('packages/client/connection/lib/index.js');
const {TYPERT:goalRemoteContract}=await load('packages/goal/goal/lib/typert.host.js');
const prepared=await prepareGoalRuntime(dsh,join(fixturePath(),'goal-runtime-'+randomUUID()));
const {default:GoalService}=await import(pathToFileURL(prepared.goal));
const driver=await import(pathToFileURL(prepared.driver));
const command=await import(pathToFileURL(prepared.command));
async function until(predicate,message){for(let i=0;i<300;i++){if(await predicate())return;await new Promise(r=>setTimeout(r,20));}throw Error('timeout: '+message);}
async function isolatedCore(root,existingDB){
 const binary=process.env.SIC_CORE_TEST_BIN??testCoreBinary();
 await mkdir(root,{recursive:true});const db=existingDB??join(root,'core.db'),token='fixture-'+randomUUID(),envName='SIC_GOAL_NATIVE_FIXTURE_TOKEN';
 const env={PATH:process.env.PATH,SystemRoot:process.env.SystemRoot,TEMP:root,TMP:root,[envName]:token};
 if(!existingDB)await promisify(execFile)(binary,['init',db],{windowsHide:true,env});
 const reserve=createServer();await new Promise(r=>reserve.listen(0,'127.0.0.1',r));const port=reserve.address().port;await new Promise(r=>reserve.close(r));
 const child=spawn(binary,['serve',db,`127.0.0.1:${port}`,'goal-native-fixture',envName],{windowsHide:true,env,stdio:['ignore','pipe','pipe']}),baseURL=`http://127.0.0.1:${port}`;
 let logs='';child.stderr.on('data',x=>logs+=x);child.stdout.on('data',x=>logs+=x);
 try{await until(async()=>{try{return(await fetch(baseURL+'/health',{headers:{authorization:'Bearer '+token}})).ok;}catch{return false;}},'isolated Core health');}catch(error){child.kill();throw error;}
 const core=createCoreClient({baseURL,credential:()=>token,context:()=>({actor:'goal-native-fixture',chain_id:null,allow_write:true,allow_model:false,allow_revoke:false}),fetchImpl:async(...args)=>{const r=await fetch(...args);if(!r.ok)console.error('fixture Core rejection',await r.clone().text());return r;}});
 return{core,db,close:async()=>{child.kill();if(child.exitCode===null)await new Promise(r=>child.once('exit',r));await writeFile(join(root,'core.log'),logs);}};
}
async function harness(core,script,scope='native-goal-scope',persistence,nativeCap=6){
 const ctx=new Context(),trace=[],requests=[];let runtime,agent;
 await mountAgentLoopTestDependencies(ctx);
 if(persistence)await ctx.plugin(sessionPersistence,{root:persistence.root,compression:'none'});
 await ctx.plugin(GoalService,{defaultMaxGoalRounds:nativeCap});
 await ctx.plugin(Commands);await ctx.plugin(command);
 const current=async()=> (await core.operate('harness.call',{scope_id:scope,name:'get_goal',arguments:{}})).value.goal;
 const resolveGrant=createTurnGrantResolver({ctx,client:core,roleFor:()=> 'WORKER',scopeFor:()=>scope,allowedCallsFor:()=>['get_goal','create_goal','update_goal'],goalFor:current,maxRoundsFor:()=>ctx.goals.defaultMaxGoalRounds});
 runtime=await installCoreGoalRuntime(ctx,{client:core,bindingFor:()=>({scope_id:scope,chain_id:null}),resolveGrant,appendRecord:appendPluginRecord,emitGoalChanged:(agent,change)=>agentEvents(ctx,agent).emit('goal/changed',{change})});
 await ctx.plugin(goalTools,{});
 await ctx.plugin(driver);await ctx.plugin(AgentLoop,{agents:[]});
 class Script extends LlmAdapter{async *stream(options){requests.push(options);const fn=script.shift();if(!fn)throw Error('fixture script exhausted');const reason=await fn({agent,ctx,runtime,options,requests});yield{type:'block-start',index:0,blockType:'text'};yield{type:'block-end',index:0,block:{type:'text',text:'native round response'}};yield{type:'finish',reason:{kind:reason??'stop'}};}}
 ctx.llm.registerAdapter(['fixture-native'],new Script());
 ctx.on('agent/inbox/inserted',({message})=>trace.push({kind:'queued',id:message.id,source:message.source}));
 ctx.on('agent/inbox/claimed',({message})=>trace.push({kind:'claimed',id:message.id,source:message.source}));
 ctx.on('session/event',(_session,event)=>{trace.push({kind:'session-event',event});});
 ctx.on('agent/error',payload=>console.error('fixture Agent error',payload.error));
 agent=persistence?.resume?(await ctx.agentLoop.resume(ctx,{resumeSessionId:'native-goal-session',agentOptions:{provider:'fixture-native',model:'original-native-model'}})).agent:await ctx.agentLoop.create('native-goal-session',{provider:'fixture-native',model:'original-native-model'});
 await new Promise((resolve,reject)=>agent.ctx.inject(['tools'],child=>{try{installGoalCompatibility({tools:child.tools,scope:agent,facade:runtime.facade});resolve();}catch(error){reject(error);}}));
 await runtime.attach(agent);await runtime.refresh(agent);
 return{ctx,runtime,agent,trace,requests,current,
  state:async()=>{const g=await current();return g?core.operate('harness.get',{scope_id:scope,object_id:g.id}):null;},
  human:text=>agent.followup(createUserMessage({content:[{type:'text',text}],source:{kind:'user'}})),
  close:async()=>{await runtime.dispose();await ctx.fiber.dispose();}};
}

test('actual DSH GoalService + unique native driver automatically queues Core rounds, completes, and feeds native UI',{timeout:30000},async()=>{
 const root=join(evidence,'run-'+randomUUID()),server=await isolatedCore(root);let h;
 try{
  h=await harness(server.core,[
   async({ctx,agent})=>{const result=await ctx.tools.execute({name:'create_goal',arguments:{objective:'verify native driver completion'},agent,callId:'actual-native-create',signal:new AbortController().signal});assert.equal(result.isError,false);},
   async()=>{},
   async({ctx,agent})=>{const goal=h.ctx.goals.get(agent);assert.equal(goal.roundsStarted,2);const result=await ctx.tools.execute({name:'update_goal',arguments:{goal_id:goal.id,revision:goal.revision,action:'complete'},agent,callId:'actual-native-complete',signal:new AbortController().signal});assert.equal(result.isError,false);},
  ]);
  h.human('Please finish the full objective across goal rounds.');
  await until(async()=>{const s=await h.state();return s?.phase==='complete'&&s.roundsFinished===2;},'actual native driver completion');
  assert.equal(h.requests.length,3);const view=h.ctx.goals.get(h.agent),state=await h.state();
  assert.equal(view.id,state.object_id);assert.equal(view.phase,'complete');assert.equal(view.roundsStarted,2);assert.equal(view.revision,2);
  const ui=h.ctx.sessionProjections.snapshot(h.agent.session,['goal']).values.goal;
  assert.equal(ui.goal.id,view.id);assert.equal(ui.goal.phase,'complete');assert.equal(ui.roundsStarted,2);
  assert.equal(h.agent.session.snapshotEvents().some(e=>e.type==='goal/change'),false);
  assert.deepEqual(h.trace.filter(e=>e.kind==='claimed'&&e.source.kind==='goal').map(e=>e.source.round),[1,2]);
  assert.equal(state.round_receipts.length,2);
  await writeFile(join(root,'evidence.json'),JSON.stringify({requests:h.requests.map(r=>({provider:r.provider,model:r.model})),view,state,ui,trace:h.trace},null,2));
 }finally{if(h)await writeFile(join(root,'trace.json'),JSON.stringify(h.trace,null,2));await h?.close();await server.close();}
});

test('native /goal command shares Core, human pause interrupts live round, resume rearms, clear keeps tombstone',{timeout:30000},async()=>{
 const root=join(evidence,'pause-'+randomUUID()),server=await isolatedCore(root);let h,entered=false;
 try{
  h=await harness(server.core,[
   async({options})=>{entered=true;await new Promise((resolve,reject)=>{options.signal.addEventListener('abort',()=>reject(Error('native fixture cancelled')),{once:true});});},
   async()=>{},
  ],'native-command-scope',undefined,2);
  const signal=new AbortController().signal,run=line=>h.ctx.commands.execute(h.agent,line,[],signal);
  // Call the native command registry, which supplies the committed command/run
  // identity. No fixture writes Core observation events directly.
  const created=await run('/goal finish command objective');assert.ok(created);
  await until(()=>entered,'native round enters model');
  const before=h.ctx.goals.get(h.agent);
  await run('/goal pause');
  await until(async()=>{const s=await h.state();return s?.phase==='paused'&&s.roundsFinished===1&&s.activation==='disarmed';},'human pause interrupts actual round');
  assert.equal(h.requests.length,1);
  const paused=await h.state();assert.equal(paused.interrupted_rounds.length,1);
  await run('/goal resume');
  await until(async()=>{const s=await h.state();return s?.roundsFinished===2&&s.activation==='disarmed';},'human resume drives next native round');
  const old=await h.state();await run('/goal clear');assert.equal(await h.current(),null);assert.equal(h.ctx.goals.get(h.agent),undefined);
  assert.equal(h.ctx.sessionProjections.snapshot(h.agent.session,['goal']).values.goal,null);
  const tombstone=await server.core.operate('harness.get',{scope_id:'native-command-scope',object_id:before.id});assert.equal(tombstone.cleared,true);assert.equal(tombstone.round_receipts.length,old.round_receipts.length);
  await writeFile(join(root,'evidence.json'),JSON.stringify({before,paused,old,tombstone,trace:h.trace},null,2));
 }finally{if(h)await writeFile(join(root,'trace.json'),JSON.stringify(h.trace,null,2));await h?.close();await server.close();}
});

test('actual JSONL session resume + Core SQLite reopen reads same disarmed goal and requires human resume',{timeout:30000},async()=>{
 const root=join(evidence,'resume-'+randomUUID()),first=await isolatedCore(join(root,'first'));let h,second;
 try{
  const persistence={root:join(root,'native-jsonl')};
  h=await harness(first.core,[async()=> 'max-tokens'],'native-resume-scope',persistence,3);
  const run=line=>h.ctx.commands.execute(h.agent,line,[],new AbortController().signal);
  await run('/goal persisted objective');
  await until(async()=>{const s=await h.state();return s?.roundsStarted===1&&s.roundsFinished===1&&s.activation==='disarmed';},'native output limit disarms unfinished round');
  const old=await h.state(),oldMessages=h.agent.session.snapshotEvents().filter(e=>e.type==='user/message').map(e=>e.data.id);
  await h.close();h=null;await first.close();
  second=await isolatedCore(join(root,'reopen'),first.db);
  h=await harness(second.core,[async({ctx,agent})=>{const goal=ctx.goals.get(agent);const result=await ctx.tools.execute({name:'update_goal',arguments:{goal_id:goal.id,revision:goal.revision,action:'complete'},agent,callId:'resume-complete',signal:new AbortController().signal});assert.equal(result.isError,false);}], 'native-resume-scope',{...persistence,resume:true},3);
  const resumed=await h.state();assert.equal(resumed.object_id,old.object_id);assert.equal(resumed.activation,'disarmed');assert.equal(h.requests.length,0);
  assert.ok(oldMessages.every(id=>h.agent.session.snapshotEvents().some(e=>e.type==='user/message'&&e.data.id===id)));
  await h.ctx.commands.execute(h.agent,'/goal resume',[],new AbortController().signal);
  await until(async()=>{const s=await h.state();return s?.phase==='complete'&&s.roundsFinished===2;},'human rearm after actual native persisted resume');
  assert.equal(h.requests.length,1);
  await writeFile(join(root,'evidence.json'),JSON.stringify({old,resumed,final:await h.state(),trace:h.trace},null,2));
 }finally{await h?.close();if(second)await second.close();else await first.close();}
});

test('three actual native automatic rounds audit the same blocked tool condition through Core receipts',{timeout:30000},async()=>{
 const root=join(evidence,'blocked-'+randomUUID()),server=await isolatedCore(root);let h;
 try{
  const blocked=async({ctx,agent})=>{const g=ctx.goals.get(agent);const result=await ctx.tools.execute({name:'update_goal',arguments:{goal_id:g.id,revision:g.revision,action:'blocked',blocked_reason:'external fixture condition is unchanged'},agent,callId:'blocked-round-'+g.roundsStarted,signal:new AbortController().signal});assert.equal(result.isError,g.roundsStarted<3);};
  h=await harness(server.core,[blocked,blocked,blocked],'native-blocked-scope');
  await h.ctx.commands.execute(h.agent,'/goal inspect the repeated external blocker',[],new AbortController().signal);
  await until(async()=>{const s=await h.state();return s?.phase==='blocked';},'same blocker across three native driver rounds');
  const state=await h.state();assert.equal(h.requests.length,3);assert.equal(state.blockedCount,3);assert.equal(state.roundsFinished,3);assert.equal(state.round_receipts.length,3);
  await writeFile(join(root,'evidence.json'),JSON.stringify({state,trace:h.trace},null,2));
 }finally{await h?.close();await server.close();}
});

test('actual native Typert owner UI RPC reads and mutates Core goal; non-owner cannot mint human authority',{timeout:30000},async()=>{
 const root=join(evidence,'ui-rpc-'+randomUUID()),server=await isolatedCore(root);let h,entered=false;
 try{
  h=await harness(server.core,[async({options})=>{entered=true;await new Promise((_resolve,reject)=>options.signal.addEventListener('abort',()=>reject(Error('UI pause aborted native round')),{once:true}));}], 'native-ui-scope');
  await h.ctx.plugin(TypertRegistry);await h.ctx.plugin(RemoteAccess);await h.ctx.plugin(Gateway);
  // Reuse every actual generated lookup/parser/result contract. A normal DSH
  // build regenerates these implementation names from the patched decorators.
  h.ctx.typert.register({...goalRemoteContract,invocations:goalRemoteContract.invocations.map(d=>({...d,implementation:'remoteExport'+d.method[0].toUpperCase()+d.method.slice(1)}))});
  const owner=new Peer(h.ctx,{principal:{kind:'owner'}}),guest=new Peer(h.ctx,{principal:{kind:'fixture-non-owner'}});
  const rpc=(peer,method,args={})=>h.ctx.typertGateway.invoke({peer,namespace:'goals',method,args:{agentId:h.agent.session.id,...args}});
  const made=await rpc(owner,'create',{request:{objective:'actual UI owner objective'}});
  await until(()=>entered,'UI-created native round enters');
  const running=await rpc(owner,'get');assert.equal(running.id,made.ref.id);assert.equal(running.roundsStarted,1);
  await assert.rejects(()=>rpc(guest,'pause',{ref:{id:running.id,revision:running.revision}}),/access denied|owner/);
  assert.equal((await h.state()).phase,'active');
  const paused=await rpc(owner,'pause',{ref:{id:running.id,revision:running.revision}});assert.equal(paused.phase,'paused');
  await until(async()=> (await h.state()).roundsFinished===1,'UI pause actual executor interruption');
  const edited=await rpc(owner,'edit',{ref:{id:paused.id,revision:paused.revision},request:{objective:'edited via native UI RPC'}});assert.equal(edited.objective,'edited via native UI RPC');
  const cleared=await rpc(owner,'clear',{ref:{id:edited.id,revision:edited.revision}});assert.equal(cleared.id,edited.id);assert.equal(await rpc(owner,'get'),undefined);
  assert.ok(h.agent.session.snapshotEvents().filter(e=>e.type==='plugin:sic/goal/intent').length>=4);
  await writeFile(join(root,'evidence.json'),JSON.stringify({made,running,paused,edited,cleared,trace:h.trace},null,2));
 }finally{await h?.close();await server.close();}
});

test('actual native goal tool call ids may repeat in a later human turn without colliding with an earlier mutation',{timeout:30000},async()=>{
 const root=join(evidence,'turn-call-id-'+randomUUID()),server=await isolatedCore(root);let h,first,second;
 try{
  h=await harness(server.core,[
   async({ctx,agent})=>{const created=await ctx.tools.execute({agent,name:'create_goal',arguments:{objective:'native repeated call id fixture'},callId:'initial-goal-create',signal:new AbortController().signal});assert.equal(created.isError,false);const goal=await h.current();const args={goal_id:goal.id,revision:goal.revision,action:'pause'};first=await ctx.tools.execute({agent,name:'update_goal',arguments:args,callId:'provider-call-0',signal:new AbortController().signal});const replay=await ctx.tools.execute({agent,name:'update_goal',arguments:args,callId:'provider-call-0',signal:new AbortController().signal});assert.deepEqual(replay,first,'the same original invocation still replays its first receipt');},
   async({ctx,agent})=>{const goal=await h.current();second=await ctx.tools.execute({agent,name:'update_goal',arguments:{goal_id:goal.id,revision:goal.revision,action:'edit',objective:'second human turn goal edit'},callId:'provider-call-0',signal:new AbortController().signal});},
  ]);
  h.human('Create and pause this isolated goal.');await h.agent.whenIdle();assert.equal(first.isError,false,JSON.stringify(first));h.human('Edit the paused isolated goal in this new human turn.');await h.agent.whenIdle();
  const goal=await h.current();await writeFile(join(root,'evidence.json'),JSON.stringify({first,second,goal,trace:h.trace},null,2));assert.equal(second.isError,false,'a provider-local call id reused in another actual native turn must receive a distinct durable event identity');assert.equal(goal.objective,'second human turn goal edit');
 }finally{await h?.close();await server.close();}
});
