import {dshSourcePath,fixturePath,evidencePath,testCoreBinary} from './test-paths.mjs';
import{test}from'node:test';import assert from'node:assert/strict';import{spawn,execFile}from'node:child_process';import{promisify}from'node:util';import{mkdir,writeFile,readFile,copyFile}from'node:fs/promises';import{join}from'node:path';import{pathToFileURL,fileURLToPath}from'node:url';import{randomUUID,createHash}from'node:crypto';import{createServer}from'node:net';
import{prepareJobsRuntime}from'../transparent/prepare-jobs-runtime.mjs';import{installCoreJobsRuntime}from'../transparent/jobs-runtime.mjs';import{createCoreClient}from'../transparent/core-client.mjs';
import{createServer as createHttpServer}from'node:http';
const dsh=dshSourcePath(),evidence=process.env.SIC_JOBS_EVIDENCE_ROOT??evidencePath('dsh-sic-transparent-comparison-20261117/jobs-runtime'),build=process.env.SIC_JOBS_BUILD_ROOT??fixturePath('jobs-runtime-20261117-fixture');
const load=p=>import(pathToFileURL(join(dsh,p)).href),prepared=await prepareJobsRuntime(dsh,join(build,'source'));
const patched=p=>import(pathToFileURL(join(prepared.root,p,'index.mjs')).href);
const{Context}=await load('vendor/cordis/lib/index.js'),{mountAgentLoopTestDependencies,provideWorkingDirectoryFixture}=await load('packages/test-support/agent-loop-testkit/lib/index.js'),{default:AgentLoop}=await load('packages/core/agent-loop/lib/index.js'),{LlmAdapter,createUserMessage}=await load('packages/llm/llm/lib/index.js'),{appendPluginRecord}=await load('packages/core/session/lib/index.js');
const{default:LocalJobs}=await patched('jobs/jobs-local'),jobTools=await patched('jobs/tool-jobs'),pwshTool=await patched('shell/tool-pwsh'),{default:Pwsh}=await load('packages/shell/pwsh-local/lib/index.js'),{default:Subprocess}=await load('packages/subprocess/subprocess-local/lib/index.js'),shellEnv=await load('packages/shell/shell-env/lib/index.js');
const{default:SubagentRuntime}=await patched('subagent/subagent'),subagentTool=await patched('subagent/tool-subagent');
const{default:PtcWorkflow}=await patched('workflow/workflow-ptc'),workflowTool=await patched('workflow/tool-workflow'),{default:FileSystem}=await load('packages/fs/fs-local/lib/index.js'),{default:Sandbox}=await load('packages/sandbox/sandbox-local/lib/index.js'),{default:SandboxPolicy}=await load('packages/sandbox/sandbox-policy/lib/index.js'),{default:NodePtc}=await load('packages/ptc-runtime/ptc-runtime-node/lib/index.js');
const{default:JobController}=await patched('api/job-controller'),{default:Typert}=await load('packages/typert/registry/lib/index.js'),{default:RemoteAccess}=await load('packages/api/remote-access/lib/index.js'),{default:Gateway}=await load('packages/api/gateway/lib/index.js'),{Peer}=await load('packages/client/connection/lib/index.js'),{TYPERT:jobRemoteContract}=await load('packages/api/job-controller/lib/typert.host.js');
const sessionPersistence=await load('packages/session/session-persistence-jsonl/lib/index.js');
async function until(fn,message){for(let i=0;i<400;i++){if(await fn())return;await new Promise(r=>setTimeout(r,20));}throw Error('timeout: '+message);}
async function cleanup(h,server){try{await h?.close();}finally{await server.close();}}
async function core(root,existingDB,chain_id=null){
 await mkdir(root,{recursive:true});await mkdir(build,{recursive:true});
 const source=process.env.SIC_CORE_TEST_BIN??testCoreBinary(),bytes=await readFile(source),hash=createHash('sha256').update(bytes).digest('hex'),binary=join(build,'sic-core-'+hash+'.exe');
 try{await copyFile(source,binary,1);}catch(error){if(error.code!=='EEXIST')throw error;}
 assert.equal(createHash('sha256').update(await readFile(binary)).digest('hex'),hash,'immutable fixture binary differs from selected Core');
 const sums=join(build,'SHA256SUMS.txt'),line=hash+'  '+binary;let previous='';try{previous=await readFile(sums,'utf8');}catch(error){if(error.code!=='ENOENT')throw error;}if(!previous.split('\n').includes(line))await writeFile(sums,previous+line+'\n');
 const db=existingDB??join(root,'core.db'),token='jobs-fixture-'+randomUUID(),envName='SIC_JOBS_FIXTURE_AUTH',env={PATH:process.env.PATH,SystemRoot:process.env.SystemRoot,TEMP:root,TMP:root,[envName]:token};
 await writeFile(join(root,'core-binary.json'),JSON.stringify({source,immutable_copy:binary,sha256:hash,db},null,2));
 if(!existingDB)await promisify(execFile)(binary,['init',db],{windowsHide:true,env});
 const reserve=createServer();await new Promise(r=>reserve.listen(0,'127.0.0.1',r));const port=reserve.address().port;await new Promise(r=>reserve.close(r));
 const child=spawn(binary,['serve',db,`127.0.0.1:${port}`,'native-jobs-fixture',envName],{windowsHide:true,env,stdio:['ignore','pipe','pipe']}),baseURL=`http://127.0.0.1:${port}`;let logs='';child.stdout.on('data',d=>logs+=d);child.stderr.on('data',d=>logs+=d);
 try{await until(async()=>{try{return(await fetch(baseURL+'/health',{headers:{authorization:'Bearer '+token}})).ok;}catch{return false;}},'Core fixture health');}catch(error){child.kill();throw error;}
 const client=createCoreClient({baseURL,credential:()=>token,context:()=>({actor:'native-jobs-fixture',chain_id,allow_write:true,allow_model:false,allow_revoke:false}),fetchImpl:async(...args)=>{const r=await fetch(...args);if(!r.ok)console.error('fixture Core rejection',await r.clone().text());return r;}});
 let closing;const close=()=>closing??=(async()=>{child.kill();if(child.exitCode===null&&child.signalCode===null)await new Promise(r=>child.once('exit',r));await writeFile(join(root,'core.log'),logs);})();
 return{client,db,hash,baseURL,credential:()=>token,createChain:async()=>chain_id=(await client.operate('chain.create',{})).chain_id,close};
}
async function harness(client,root,script,options={}){
 const ctx=new Context(),trace=[],receipts=[];let agent,runtime;
 await mountAgentLoopTestDependencies(ctx);
 if(options.localPeer){
  await ctx.plugin(FileSystem);
  const {default:WorkingDirectory}=await load('packages/session/working-directory/lib/index.js');
  await ctx.plugin(WorkingDirectory,{defaultDirectory:root});
 }else provideWorkingDirectoryFixture(ctx,root);
 if(options.persistenceRoot)await ctx.plugin(sessionPersistence,{root:options.persistenceRoot,compression:'none'});
 if(options.localPeer){const{default:SessionQuery}=await load('packages/session-query/session-query/lib/index.js');await ctx.plugin(SessionQuery);}
 await ctx.plugin(LocalJobs,{pumpPollMs:10,...options.retainBytes?{retainBytes:options.retainBytes}:{}});await ctx.plugin(jobTools,{completionDelivery:'quiet'});
 await ctx.plugin(Subprocess);await ctx.plugin(Pwsh);await ctx.plugin(shellEnv,{dshHome:root});await ctx.plugin(pwshTool);
 await ctx.plugin(SubagentRuntime);let starts=0,peerDone=Promise.withResolvers();
 ctx.subagents.registerProvider({name:'native-fixture-child',inheritsParentContext:false,capabilities:{agentOptions:false,agentPreset:false,outputSchema:false,depthLimit:false},async start(){starts++;return{id:'native-child-'+starts,result:peerDone.promise,dispose:async()=>{options.peerDisposalStarted?.();if(options.peerDisposalGate)await options.peerDisposalGate;peerDone.resolve({output:[],stopReason:'aborted'});}};}});
 if(options.localPeer)ctx.subagents.registerProvider({name:'native-continuable',inheritsParentContext:false,capabilities:{agentOptions:true,agentPreset:true,outputSchema:true,depthLimit:true},prepareContinuable:async()=>({})});
 await ctx.plugin(subagentTool,{provider:'native-fixture-child',maxDepth:'provider-managed',...options.subagentName?{toolName:options.subagentName}:{}});
 if(options.workflow){await ctx.plugin(FileSystem);await ctx.plugin(Sandbox);await ctx.plugin(SandboxPolicy,{mode:'danger-full-access',workspaceRoot:root});await ctx.plugin(NodePtc);await ctx.plugin(PtcWorkflow,{provider:'native-fixture-child'});await ctx.plugin(workflowTool,{...options.workflowName?{toolName:options.workflowName}:{}});}
 runtime=await installCoreJobsRuntime(ctx,{client,bindingFor:()=>({scope_id:'native-jobs-scope',chain_id:options.chain_id??null,role:'WORKER'}),appendRecord:appendPluginRecord,onReceipt:r=>receipts.push(r)});
 await ctx.plugin(AgentLoop,{agents:[]});
 class Model extends LlmAdapter{async *stream(options){const fn=script.shift(),reply=fn?await fn({ctx,agent,runtime,options,call}):null;if(reply?.tool){const{id,name,args}=reply.tool,json=JSON.stringify(args);yield{type:'block-start',index:0,blockType:'tool-call'};yield{type:'tool-call-delta',index:0,id,name,argumentsDelta:json};yield{type:'block-end',index:0,block:{type:'tool-call',id,name,arguments:json}};yield{type:'finish',reason:{kind:'tool-calls'}};return;}yield{type:'block-start',index:0,blockType:'text'};yield{type:'block-end',index:0,block:{type:'text',text:reply?.text??'native fixture response'}};yield{type:'finish',reason:{kind:'stop'}};}}
 const call=(name,args,callId,signal=new AbortController().signal)=>ctx.tools.execute({agent,name,arguments:args,callId,signal});
 ctx.llm.registerAdapter(['native-fixture'],new Model());
 ctx.on('session/event',(_session,event)=>trace.push({kind:'session-event',event}));ctx.on('subagent/end',event=>trace.push({kind:'native-subagent-end',event:structuredClone(event)}));ctx.jobs.events.subscribe({owners:'all'},event=>trace.push({kind:'native-job-event',event:structuredClone(event)}));ctx.on('agent/error',payload=>console.error('fixture Agent error',payload.error));
 agent=options.resume?(await ctx.agentLoop.resume(ctx,{resumeSessionId:'native-jobs-session',agentOptions:{provider:'native-fixture',model:'original-model'}})).agent:await ctx.agentLoop.create('native-jobs-session',{provider:'native-fixture',model:'original-model'},{cwd:root});await runtime.refresh(agent);
 return{ctx,agent,runtime,trace,receipts,call,human:text=>agent.followup(createUserMessage({content:[{type:'text',text}],source:{kind:'user'}})),peerDone,get peerStarts(){return starts;},close:async()=>{await ctx.fiber.dispose();await runtime.dispose();}};
}

test('actual native local ActivationManager cold-resumes the same durable child twice with distinct Core execution identities',{timeout:35000},async()=>{
 const root=join(evidence,'same-local-peer-'+randomUUID()),server=await core(root),childId='native-durable-peer',scope='native-jobs-scope';let h;
 try{
  let responses=0;const answer=async({options})=>({text:options.sessionId===childId?'durable peer output '+(++responses):'parent observed peer'});
  h=await harness(server.client,root,Array(12).fill(answer),{localPeer:true,persistenceRoot:join(root,'native-jsonl')});
  const content=text=>[{type:'text',text}],signal=new AbortController().signal;
  const activation=await h.ctx.subagents.startActivation({provider:'native-continuable',childId,label:'Durable peer',delivery:'caller',signal,request:{parent:h.agent,prompt:content('initial task')}});
  assert.equal(activation.childId,childId);await activation.result;
  const states=[];
  for(let epoch=1;epoch<=3;epoch++){
   if(epoch>1)await h.ctx.subagents.sendMessage(h.agent,childId,content('same durable child again'),{signal});
   await until(()=>{const state=h.runtime.readState(childId,h.agent.id);return state.executor.residency_epoch===epoch&&state.status==='completed'&&!h.ctx.agents.get(childId);},'real child residency settled '+epoch);
   const state=h.runtime.readState(childId,h.agent.id);
   assert.equal(state.executor.session_id,childId);assert.equal(state.result,'durable peer output '+epoch);
   assert.equal(h.runtime.list(h.agent.id).filter(row=>row.id===childId).length,1);
   states.push(state);
  }
  assert.equal(new Set(states.map(state=>state.object_id)).size,3);assert.equal(new Set(states.map(state=>state.external_id)).size,3);
  for(const state of states){const persisted=await server.client.operate('harness.get',{scope_id:scope,object_id:state.object_id});assert.equal(persisted.result,state.result);}
  await writeFile(join(root,'evidence.json'),JSON.stringify({states,trace:h.trace,receipts:h.receipts},null,2));
 }finally{await cleanup(h,server);}
});

test('native local activation cannot materialize a child after a pre-attachment claim loses its acknowledgement and both processes reopen',{timeout:35000},async()=>{
 const root=join(evidence,'local-peer-before-attach-'+randomUUID()),scope='native-jobs-scope',childId='unattached-local-peer';
 const options={localPeer:true,persistenceRoot:join(root,'native-jsonl')};
 let server=await core(root),h,objectId,workerCalls=0;
 const start=h=>h.ctx.subagents.startActivation({provider:'native-continuable',childId,label:'Unattached peer',delivery:'caller',signal:new AbortController().signal,request:{parent:h.agent,prompt:[{type:'text',text:'no execution should start'}]}});
 const models=[async()=>{workerCalls++;}];
 try{
  const client={...server.client,async operate(op,payload,...rest){
   const result=await server.client.operate(op,payload,...rest);
   if(op==='harness.observe'&&payload.observation?.kind==='job.claim'){objectId=payload.object_id;throw Error('claim committed before acknowledgement was lost');}
   return result;
  }};
  h=await harness(client,root,models,options);
  await assert.rejects(()=>start(h),/claim committed before acknowledgement was lost/);
  assert.ok(!h.ctx.agents.get(childId));assert.equal(workerCalls,0);
  const before=await server.client.operate('harness.get',{scope_id:scope,object_id:objectId});
  assert.equal(before.dispatch_state,'claimed');assert.equal(before.external_id,null);
  assert.equal(await h.ctx.sessions.flush(h.agent.session),true);
  await h.close();h=null;
  const db=server.db;await server.close();server=await core(join(root,'core-reopen'),db);
  h=await harness(server.client,root,models,{...options,resume:true});
  await assert.rejects(()=>start(h),/already admitted.*reconciliation/);
  assert.ok(!h.ctx.agents.get(childId));assert.equal(workerCalls,0);
  const after=await server.client.operate('harness.get',{scope_id:scope,object_id:objectId});
  assert.equal(after.dispatch_state,'unknown');assert.equal(after.external_id,null);
  await writeFile(join(root,'evidence.json'),JSON.stringify({before,after,workerCalls,trace:h.trace},null,2));
 }finally{await cleanup(h,server);}
});

test('actual native PowerShell tool/JobRegistry preserves prelaunch, stream output, collection, one-shot replay and true cancellation',{timeout:40000},async()=>{
 const root=join(evidence,'process-'+randomUUID()),server=await core(root);let h,complete=false,result;
 try{
  const marker=join(root,'dispatch-count.txt'),quoted=marker.replaceAll("'","''"),args={description:'native process fixture',command:`Add-Content -LiteralPath '${quoted}' -Value 'dispatch'; [Console]::Out.Write('native stdout'); Start-Sleep -Milliseconds 300; [Console]::Error.Write('native stderr'); Start-Sleep -Seconds 20`,run_in_background:true};
  h=await harness(server.client,root,[async({call,runtime,agent,ctx})=>{
   const started=await call('pwsh',args,'actual-pwsh-call');assert.equal(started.isError,false,JSON.stringify(started));const id=started.value.jobId;
   const initial=runtime.readState(id,agent.id);assert.equal(initial.dispatch_state,'attached');assert.equal(initial.native_tool_result.value.jobId,id);
   const replay=await call('pwsh',args,'actual-pwsh-call');assert.deepEqual(replay,started);
   await until(async()=>{await runtime.drain();return runtime.readState(id,agent.id).chunks?.join('').includes('native stderr');},'native output durable before collection');
   const output=await call('job_output',{job_id:id},'actual-output');assert.equal(output.isError,false);assert.equal(output.value.text,'native stdout\n[stderr]\nnative stderr');
   const outputReplay=await call('job_output',{job_id:id},'actual-output');assert.deepEqual(outputReplay,output);
   const again=await call('job_output',{job_id:id},'actual-output-again');assert.equal(again.value.text,'');
   const beforeAbortedWait=runtime.readState(id,agent.id),waitAbort=new AbortController(),waiting=call('job_output',{job_id:id,wait:true,timeout_ms:5000},'actual-aborted-wait',waitAbort.signal);setTimeout(()=>waitAbort.abort(),20);const abortedWait=await waiting;assert.equal(abortedWait.isError,true);const afterAbortedWait=runtime.readState(id,agent.id);assert.equal(afterAbortedWait.status,'running');assert.deepEqual(afterAbortedWait.last_collection,beforeAbortedWait.last_collection);
   const before=runtime.readState(id,agent.id);const cancel=await call('job_kill',{job_id:id,reason:'fixture cancellation'},'actual-kill');assert.equal(cancel.value.outcome,'cancellation-requested');
   assert.equal(cancel.value.job.status,'stopping');
   await until(async()=>{await runtime.drain();return runtime.readState(id,agent.id).status==='killed';},'actual native executor termination');
   const final=runtime.readState(id,agent.id);assert.equal(final.status,'killed');assert.ok(final.finishedAt);assert.ok(before.last_collection);
   const ui=ctx.jobs.get(id,agent.id),uiOutput=ctx.jobs.readAt(id,0,agent.id);assert.equal(ui.status,'killed');assert.ok(uiOutput.chunks.some(c=>c.channel==='stderr'));
   assert.equal((await readFile(marker,'utf8')).trim(),'dispatch');result={started,replay,output,again,cancel,before,final,ui,uiOutput};complete=true;
  }]);h.human('Run and inspect the isolated native background process, then stop it.');await until(()=>complete,'native process full integration');
  await writeFile(join(root,'evidence.json'),JSON.stringify({core_sha256:server.hash,result,receipts:h.receipts,trace:h.trace},null,2));
 }finally{if(h)await writeFile(join(root,'trace.json'),JSON.stringify(h.trace,null,2));await cleanup(h,server);}
});

test('actual native subagent tool/ActivationManager holds one Core peer identity and reports real release before native notice',{timeout:30000},async()=>{
 const root=join(evidence,'peer-'+randomUUID()),server=await core(root);let h,complete=false,result;
 try{
  h=await harness(server.client,root,[async({call,agent,runtime})=>{
   const args={description:'native child fixture',prompt:'return the synthetic child result'},started=await call('native_delegate',args,'actual-subagent-call');assert.equal(started.isError,false,JSON.stringify(started));assert.equal(started.value.kind,'activation');const id=started.value.subagentId;
   const replay=await call('native_delegate',args,'actual-subagent-call');assert.deepEqual(replay,started);assert.equal(h.peerStarts,1);
   const running=runtime.readState(id,agent.id);assert.equal(running.object_kind,'peer');assert.equal(running.dispatch_state,'attached');
   h.peerDone.resolve({output:[{type:'text',text:'native child final result'}],stopReason:'completed'});
   await until(async()=>{await runtime.drain();return runtime.readState(id,agent.id).status==='completed';},'native child manager settlement');
   const output=await call('job_output',{job_id:id,wait:true,timeout_ms:1000},'actual-peer-output');assert.equal(output.value.text,'native child final result');
   result={started,replay,running,output,final:runtime.readState(id,agent.id)};complete=true;
  }],{subagentName:'native_delegate'});h.human('Delegate the synthetic child fixture and collect its real result.');await until(()=>complete,'native subagent integration');await writeFile(join(root,'evidence.json'),JSON.stringify({result,receipts:h.receipts,trace:h.trace},null,2));
 }finally{if(h)await writeFile(join(root,'trace.json'),JSON.stringify(h.trace,null,2));await cleanup(h,server);}
});

test('one aborted native output waiter does not revoke another waiter or inject a duplicate completion notice',{timeout:30000},async()=>{
 const root=join(evidence,'concurrent-waiters-'+randomUUID()),server=await core(root);let h,done=false;
 try{
  h=await harness(server.client,root,[async({call,runtime,agent,ctx})=>{
   const releaseFile=join(root,'release.txt'),quoted=releaseFile.replaceAll("'","''");
   const command=`$deadline = [DateTime]::UtcNow.AddSeconds(20); while (-not (Test-Path -LiteralPath '${quoted}')) { if ([DateTime]::UtcNow -ge $deadline) { throw 'fixture release timed out' }; Start-Sleep -Milliseconds 20 }; [Console]::Out.Write('one terminal output')`;
   const started=await call('pwsh',{description:'actual concurrent native wait fixture',command,run_in_background:true},'concurrent-wait-launch');assert.equal(started.isError,false);const id=started.value.jobId;
   const abort=new AbortController(),finishSecond=Promise.withResolvers();
   const nativeWait=ctx.jobs.wait.bind(ctx.jobs);let waitIndex=0;
   ctx.jobs.wait=(...args)=>{
    const index=waitIndex++;
    // The second collection stays in flight while native JobRegistry has no
    // waiter. Completion ownership must come from the adapter in this window.
    return index===0?nativeWait(...args):finishSecond.promise;
   };
   let aborted,collected,first,second;
   try{
    first=call('job_output',{job_id:id,wait:true,timeout_ms:15000},'concurrent-wait-abort',abort.signal);
    await until(()=>waitIndex===1,'first native waiter registered');
    second=call('job_output',{job_id:id,wait:true,timeout_ms:15000},'concurrent-wait-consume');
    await until(()=>waitIndex===2,'second collection entered wait');
    abort.abort(Error('one caller abandoned its wait'));
    aborted=await first;assert.equal(aborted.isError,true);
    await writeFile(releaseFile,'release owned process');
    await until(()=>h.trace.some(row=>row.kind==='native-job-event'&&row.event.type==='settled'&&row.event.job.id===id),'native settlement while second collection is active');
    finishSecond.resolve();collected=await second;
   }finally{ctx.jobs.wait=nativeWait;abort.abort();finishSecond.resolve();await Promise.allSettled([first,second]);}
   assert.equal(collected.isError,false);assert.equal(collected.value.text,'one terminal output');await runtime.drain();
   const settlement=h.trace.find(row=>row.kind==='native-job-event'&&row.event.type==='settled'&&row.event.job.id===id);assert.ok(settlement);assert.equal(settlement.event.awaited,true,'the remaining real waiter still owns native completion delivery');
   const duplicate=h.trace.filter(row=>row.kind==='session-event'&&row.event.type==='agent/inbox/spliced'&&row.event.data.inserted.some(message=>message.source.kind==='tool-jobs'));assert.equal(duplicate.length,0,'an awaited completion must not inject an extra native model notification');
   await writeFile(join(root,'evidence.json'),JSON.stringify({started,aborted,collected,settlement,duplicate,state:runtime.readState(id,agent.id),trace:h.trace},null,2));done=true;
  }]);h.human('Wait twice for one isolated process, abort only the first wait, and collect once.');await until(()=>done,'concurrent native wait delivery');
 }finally{if(h)await writeFile(join(root,'trace.json'),JSON.stringify(h.trace,null,2));await cleanup(h,server);}
});

test('Core reopen replays the same native tool result and consuming cursor without dispatching again',{timeout:30000},async()=>{
 const root=join(evidence,'reopen-'+randomUUID()),first=await core(join(root,'first'));let server=first,h,saved,done=false;
 const marker=join(root,'once.txt'),args={description:'durable one shot fixture',command:`Add-Content -LiteralPath '${marker.replaceAll("'","''")}' -Value 'once'; [Console]::Out.Write('durable native stdout')`,run_in_background:true};
 try{
  h=await harness(server.client,root,[async({call,runtime,agent})=>{saved=await call('pwsh',args,'persistent-native-call');assert.equal(saved.isError,false);await until(async()=>{await runtime.drain();return runtime.readState(saved.value.jobId,agent.id).status==='completed';},'first native process completes');const read=await call('job_output',{job_id:saved.value.jobId},'persistent-first-collection');assert.equal(read.value.text,'durable native stdout');done=true;}]);h.human('Run the one shot native fixture.');await until(()=>done,'first durable native tool result');await h.close();h=null;await server.close();
  server=await core(join(root,'reopened'),first.db);done=false;
  h=await harness(server.client,root,[async({call,runtime,agent})=>{const replay=await call('pwsh',args,'persistent-native-call');assert.deepEqual(replay,saved);const read=await call('job_output',{job_id:saved.value.jobId},'persistent-second-collection');assert.equal(read.value.text,'');assert.equal((await readFile(marker,'utf8')).trim(),'once');await writeFile(join(root,'evidence.json'),JSON.stringify({saved,replay,read,state:runtime.readState(saved.value.jobId,agent.id)},null,2));done=true;}]);h.human('Inspect the same logical call after Core reopens.');await until(()=>done,'durable native same call replay');
 }finally{await cleanup(h,server);}
});

test('fixture actual host crash producer',{skip:process.env.SIC_NATIVE_CRASH_CHILD!=='1',timeout:30000},async()=>{
 const root=process.env.SIC_NATIVE_CRASH_ROOT,chain_id=process.env.SIC_NATIVE_CRASH_CHAIN,client=createCoreClient({baseURL:process.env.SIC_NATIVE_CRASH_CORE,credential:()=>process.env.SIC_NATIVE_CRASH_CREDENTIAL,context:()=>({actor:'native-jobs-fixture',chain_id,allow_write:true,allow_model:false,allow_revoke:false})});
 const marker=join(root,'dispatch-once.txt'),args={description:'native host crash fixture',command:`Add-Content -LiteralPath '${marker.replaceAll("'","''")}' -Value 'once'; [Console]::Out.Write('buffered before host crash'); Start-Sleep -Seconds 5; Stop-Process -Id $PID -Force`,run_in_background:true};let h;
 h=await harness(client,root,[async()=>({tool:{id:'crash-original-call',name:'pwsh',args}}),async({runtime,agent,ctx})=>{
  const id=runtime.list(agent.id).find(job=>job.kind==='pwsh').id;await until(async()=>{await runtime.drain();return runtime.readState(id,agent.id).chunks?.join('').includes('buffered before host crash');},'pre-crash native bytes acknowledged');assert.equal(await ctx.sessions.flush(agent.session),true);await writeFile(join(root,'host-ready.json'),JSON.stringify({pid:process.pid,id,state:runtime.readState(id,agent.id)}));await new Promise(()=>{});
 }],{chain_id,persistenceRoot:join(root,'native-sessions')});h.human('Run this isolated background task once, then wait for the host crash.');await new Promise(()=>{});
});

test('a hard native host process crash cold-opens its real JSONL session and preserves unknown execution without redispatch',{timeout:40000},async()=>{
 const root=join(evidence,'actual-host-crash-'+randomUUID()),server=await core(root);let child,h,logs='',done=false,result;
 try{
  const chain_id=await server.createChain(),env={PATH:process.env.PATH,SystemRoot:process.env.SystemRoot,TEMP:root,TMP:root,DSH_SOURCE_ROOT:dsh,SIC_NATIVE_CRASH_CHILD:'1',SIC_NATIVE_CRASH_ROOT:root,SIC_NATIVE_CRASH_CHAIN:chain_id,SIC_NATIVE_CRASH_CORE:server.baseURL,SIC_NATIVE_CRASH_CREDENTIAL:server.credential(),SIC_JOBS_BUILD_ROOT:join(build,'crash-child-source'),SIC_JOBS_EVIDENCE_ROOT:evidence};
  child=spawn(process.execPath,['--test','--test-isolation=none','--test-name-pattern=fixture actual host crash producer',fileURLToPath(import.meta.url)],{windowsHide:true,env,stdio:['ignore','pipe','pipe']});child.stdout.on('data',bytes=>logs+=bytes);child.stderr.on('data',bytes=>logs+=bytes);
  let ready;await until(async()=>{try{ready=JSON.parse(await readFile(join(root,'host-ready.json'),'utf8'));return true;}catch{return false;}},'real native child host ready');assert.equal(ready.pid,child.pid,'kill only the fixture process that owns the real native driver');child.kill();await new Promise(resolve=>child.once('exit',resolve));await writeFile(join(root,'crashed-host.log'),logs);
  h=await harness(server.client,root,[async({call,runtime,agent})=>{
   const before=runtime.readState(ready.id,agent.id);assert.equal(before.status,'unknown');const output=await call('job_output',{job_id:ready.id},'cold-native-output');assert.equal(output.isError,false);assert.equal(output.value.text,'buffered before host crash');const again=await call('job_output',{job_id:ready.id},'cold-native-output-next');assert.equal(again.value.text,'');const baseline=runtime.readState(ready.id,agent.id);await runtime.refresh(agent);const refreshed=runtime.readState(ready.id,agent.id);result={ready,before,output,again,baseline,refreshed,trace:h.trace};done=true;
  }],{chain_id,persistenceRoot:join(root,'native-sessions'),resume:true});h.human('Inspect the interrupted background task without starting it again.');await until(()=>done,'cold native unknown execution');await writeFile(join(root,'evidence.json'),JSON.stringify(result,null,2));assert.equal((await readFile(join(root,'dispatch-once.txt'),'utf8')).trim(),'once');assert.ok(!h.trace.some(row=>row.kind==='native-job-event'&&row.event.type==='registered'));assert.equal(result.refreshed.revision,result.baseline.revision,'reading an already unknown recovered activity must not write another recovery event');
 }finally{if(child?.exitCode===null&&child.signalCode===null){child.kill();await new Promise(resolve=>child.once('exit',resolve));}await cleanup(h,server);await new Promise(resolve=>setTimeout(resolve,5500));}
});

test('a failure after actual ToolRuntime preclaim never permits a blind native redispatch',{timeout:30000},async()=>{
 const root=join(evidence,'unknown-'+randomUUID()),server=await core(root);let h,done=false;
 try{
  const marker=join(root,'must-not-exist.txt'),args={description:'crash-window fixture',command:`Set-Content -LiteralPath '${marker.replaceAll("'","''")}' -Value 'unexpected'`,run_in_background:true};
  h=await harness(server.client,root,[async({call,ctx})=>{
   const dispose=ctx.on('tools/execute',async(exec,next)=>{if(exec.callId==='native-crash-window')throw Error('synthetic dispatch interruption after durable preclaim');return next();});
   const first=await call('pwsh',args,'native-crash-window');assert.equal(first.isError,true);dispose();const retry=await call('pwsh',args,'native-crash-window');assert.equal(retry.isError,true);assert.match(JSON.stringify(retry),/reconciliation|redispatch/);
   await assert.rejects(()=>readFile(marker),{code:'ENOENT'});await writeFile(join(root,'evidence.json'),JSON.stringify({first,retry,receipts:h.receipts,trace:h.trace},null,2));done=true;
  }]);h.human('Exercise only the isolated interrupted native-dispatch fixture.');await until(()=>done,'unknown claim blocks actual redispatch');
 }finally{await cleanup(h,server);}
});

test('actual native workflow tool keeps its original Node PTC executor and one background Core lifecycle',{timeout:40000},async()=>{
 const root=join(evidence,'workflow-'+randomUUID()),server=await core(root);let h,done=false,result;
 try{
  h=await harness(server.client,root,[async({call,runtime,agent})=>{
   const args={script:'return { answer: 42, execution: "original-native-ptc" };',meta:{name:'native-ptc-fixture',description:'isolated original workflow executor'},run_in_background:true},started=await call('native_workflow',args,'native-workflow-call');assert.equal(started.isError,false,JSON.stringify(started));assert.equal(started.value.kind,'background');
   const id=started.value.jobId;await until(async()=>{await runtime.drain();return runtime.readState(id,agent.id).status==='completed';},'actual workflow PTC completion');
   const collected=await call('job_output',{job_id:id,wait:true,timeout_ms:1000},'native-workflow-collect');assert.equal(collected.isError,false);assert.match(collected.value.text,/42/);assert.match(collected.value.text,/original-native-ptc/);const state=runtime.readState(id,agent.id);assert.equal(state.object_kind,'workflow');
   const replay=await call('native_workflow',args,'native-workflow-call');assert.deepEqual(replay,started);result={started,collected,state,replay};done=true;
  }],{workflow:true,workflowName:'native_workflow'});h.human('Run the explicitly requested isolated workflow through its native JavaScript executor.');await until(()=>done,'original native workflow full integration');await writeFile(join(root,'evidence.json'),JSON.stringify({result,receipts:h.receipts,trace:h.trace},null,2));
 }finally{if(h)await writeFile(join(root,'trace.json'),JSON.stringify(h.trace,null,2));await cleanup(h,server);}
});

test('real subagent teardown remains stopping until its native provider releases, and Core settlement precedes the native end notice',{timeout:30000},async()=>{
 const root=join(evidence,'subagent-cancel-'+randomUUID()),server=await core(root),entered=Promise.withResolvers(),release=Promise.withResolvers();let h,done=false,result;
 try{
  h=await harness(server.client,root,[async({call,runtime,agent})=>{
   const started=await call('native_delegate',{description:'native delayed child cleanup',prompt:'wait until cancelled'},'native-cancel-peer');assert.equal(started.isError,false);const id=started.value.subagentId,kill=call('job_kill',{job_id:id,reason:'fixture native child cancellation'},'native-kill-peer');await entered.promise;const stopping=runtime.readState(id,agent.id),beforeEnd=h.trace.filter(row=>row.kind==='native-subagent-end');assert.equal(stopping.status,'stopping');assert.equal(beforeEnd.length,0);release.resolve();const cancelled=await kill;await runtime.drain();const final=runtime.readState(id,agent.id);assert.equal(cancelled.isError,false);assert.equal(final.status,'killed');assert.equal(h.trace.filter(row=>row.kind==='native-subagent-end').length,1);result={started,stopping,beforeEnd,cancelled,final};done=true;
  }],{subagentName:'native_delegate',peerDisposalStarted:()=>entered.resolve(),peerDisposalGate:release.promise});h.human('Cancel only the isolated real native child activation.');await until(()=>done,'real child cancellation');await writeFile(join(root,'evidence.json'),JSON.stringify({result,trace:h.trace,receipts:h.receipts},null,2));
 }finally{release.resolve();await cleanup(h,server);}
});

test('actual Node PTC background workflow cancellation reaches native executor settlement on its original lifecycle',{timeout:30000},async()=>{
 const root=join(evidence,'workflow-cancel-'+randomUUID()),server=await core(root);let h,done=false,result;
 try{
  h=await harness(server.client,root,[async({call,runtime,agent,ctx})=>{
   const started=await call('native_workflow',{script:'await new Promise(() => {}); return 42;',meta:{name:'pending-native-workflow',description:'actual Node PTC cancellation'},run_in_background:true},'native-cancel-workflow');assert.equal(started.isError,false);const id=started.value.jobId,cancelled=await call('job_kill',{job_id:id,reason:'fixture native workflow cancellation'},'native-kill-workflow');assert.equal(cancelled.isError,false);await until(async()=>{await runtime.drain();return runtime.readState(id,agent.id).status==='killed';},'original Node PTC executor cancellation settles');await ctx.jobs.joinExecutor(id,agent.id);result={started,cancelled,final:runtime.readState(id,agent.id)};done=true;
  }],{workflow:true,workflowName:'native_workflow'});h.human('Cancel the isolated original Node PTC workflow executor.');await until(()=>done,'real native workflow cancellation');await writeFile(join(root,'evidence.json'),JSON.stringify({result,trace:h.trace,receipts:h.receipts},null,2));
 }finally{await cleanup(h,server);}
});

test('actual AgentLoop tool/result binds Core collection to native message id and exact renderer material',{timeout:30000},async()=>{
 const root=join(evidence,'collection-'+randomUUID()),server=await core(root);let h;
 try{
  h=await harness(server.client,root,[
   async()=>({tool:{id:'native-model-pwsh',name:'pwsh',args:{description:'real model tool result fixture',command:"[Console]::Out.Write('exact collected native bytes')",run_in_background:true}}}),
   async({runtime,agent})=>({tool:{id:'native-model-output',name:'job_output',args:{job_id:runtime.list(agent.id).find(j=>j.kind==='pwsh').id,wait:true,timeout_ms:2000}}}),
  ]);h.human('Collect the isolated native process result through actual model tool messages.');
  await until(()=>h.agent.session.snapshotEvents().some(e=>e.type==='plugin:sic/jobs/collection'),'trusted native renderer/message collection binding');
  const events=h.agent.session.snapshotEvents(),record=events.find(e=>e.type==='plugin:sic/jobs/collection').data,message=events.find(e=>e.type==='tool/result'&&e.data.message.id===record.message_id).data;
  assert.equal(message.message.toolCallId,'native-model-output');assert.equal(message.meta.sic_collection.collection_receipt_id,record.collection_receipt_id);assert.equal(record.material.utf8_start,0);
  const bytes=Buffer.from(message.message.content[record.material.block_index].text).subarray(record.material.utf8_start,record.material.utf8_end);assert.equal(bytes.toString(),'exact collected native bytes');assert.equal(createHash('sha256').update(bytes).digest('hex'),record.collection_digest);
  await writeFile(join(root,'evidence.json'),JSON.stringify({record,message,trace:h.trace},null,2));
 }finally{await cleanup(h,server);}
});

test('actual native JobController streams Core-backed rows/output and owner Gateway kill reaches the native process',{timeout:30000},async()=>{
 const root=join(evidence,'ui-controller-'+randomUUID()),server=await core(root);let h,id,ready=false,rows,follow;const abort=new AbortController();
 try{
  h=await harness(server.client,root,[async({call})=>{const r=await call('pwsh',{description:'native UI observation fixture',command:"[Console]::Out.Write('native UI stdout'); Start-Sleep -Seconds 20",run_in_background:true},'native-ui-process');assert.equal(r.isError,false);id=r.value.jobId;ready=true;}]);h.human('Run the isolated process for native UI observation.');await until(()=>ready,'native UI process starts');
  await h.ctx.plugin(Typert);await h.ctx.plugin(RemoteAccess);await h.ctx.plugin(Gateway);await h.ctx.plugin(JobController,{observeFlushMs:1});h.ctx.typert.register(jobRemoteContract);
  rows=h.ctx.jobController.list({sessionId:h.agent.id},abort.signal)[Symbol.asyncIterator]();const roster=await rows.next();assert.match(JSON.stringify(roster.value),new RegExp(id));
  await until(async()=>{await h.runtime.drain();return h.runtime.readState(id,h.agent.id).chunks?.join('').includes('native UI stdout');},'native UI durable output');
  follow=h.ctx.jobController.follow({sessionId:h.agent.id,jobId:id,from:0},abort.signal)[Symbol.asyncIterator]();const opened=await follow.next(),output=await follow.next();assert.equal(opened.value.type,'opened');assert.equal(output.value.type,'output');assert.equal(output.value.chunks.map(c=>c.text).join(''),'native UI stdout');
  const peer=new Peer(h.ctx,{principal:{kind:'owner'}}),cancel=await h.ctx.typertGateway.invoke({peer,namespace:'job',method:'kill',args:{request:{sessionId:h.agent.id,jobId:id}}});assert.equal(cancel.outcome,'requested');
  await until(async()=>{await h.runtime.drain();return h.runtime.readState(id,h.agent.id).status==='killed';},'UI cancellation actual native release');
  const status=await follow.next();assert.equal(status.value.type,'status');assert.equal(status.value.job.status,'killed');await writeFile(join(root,'evidence.json'),JSON.stringify({roster:roster.value,opened:opened.value,output:output.value,cancel,status:status.value,state:h.runtime.readState(id,h.agent.id),trace:h.trace},null,2));
 }finally{abort.abort();await follow?.return?.();await rows?.return?.();await cleanup(h,server);}
});

test('adapter unload cancels and drains the actual native execution before removing its lifecycle ports',{timeout:30000},async()=>{
 const root=join(evidence,'unload-'+randomUUID()),server=await core(root);let h,id,ready=false;
 try{
  h=await harness(server.client,root,[async({call})=>{const started=await call('pwsh',{description:'native unload fixture',command:'Start-Sleep -Seconds 20',run_in_background:true},'native-unload-call');assert.equal(started.isError,false);id=started.value.jobId;ready=true;}]);h.human('Start only the isolated lifecycle-unload fixture.');await until(()=>ready,'native unload process running');
  const before=h.runtime.readState(id,h.agent.id);assert.equal(before.status,'running');await h.runtime.dispose();
  assert.equal(h.ctx.jobs.externalLifecycle,undefined);assert.equal(h.ctx.subagents.externalLifecycle,undefined);
  const after=await server.client.operate('harness.get',{scope_id:'native-jobs-scope',object_id:before.object_id});assert.equal(after.status,'killed');assert.ok(after.finishedAt);assert.equal(h.ctx.jobs.get(id,h.agent.id).status,'killed');
  await writeFile(join(root,'evidence.json'),JSON.stringify({before,after,receipts:h.receipts,trace:h.trace},null,2));
 }finally{await cleanup(h,server);}
});

test('native tool admission is closed before an adapter unload waits for an existing executor to cancel',{timeout:30000},async()=>{
 const root=join(evidence,'unload-admission-race-'+randomUUID()),server=await core(root),entered=Promise.withResolvers(),release=Promise.withResolvers();let h,done=false,result;
 const client={...server.client,operate:async(op,payload,...rest)=>{if(op==='harness.call'&&payload.name==='job_kill'&&payload.arguments.reason==='native lifecycle adapter unloaded'){entered.resolve();await release.promise;}return server.client.operate(op,payload,...rest);}};
 try{
  h=await harness(client,root,[async({call,runtime,agent})=>{
   const first=await call('pwsh',{description:'native unload owns this execution',command:'Start-Sleep -Seconds 20',run_in_background:true},'unload-original-call');assert.equal(first.isError,false);
   const closing=runtime.dispose();await entered.promise;let late;try{late=await call('pwsh',{description:'must not start after native lifecycle unload begins',command:'Start-Sleep -Seconds 20',run_in_background:true},'unload-late-call');}finally{release.resolve();}
   await closing;result={first,late,firstState:runtime.readState(first.value.jobId,agent.id),...late.value?.jobId?{lateState:runtime.readState(late.value.jobId,agent.id)}:{},portsRemoved:!h.ctx.jobs.externalLifecycle};done=true;
  }]);h.human('Start one isolated native process and reject new launches once its lifecycle adapter starts unloading.');await until(()=>done,'native unload admission race');await writeFile(join(root,'evidence.json'),JSON.stringify({result,trace:h.trace},null,2));
  assert.equal(result.late.isError,true,'late native tool launch must fail before one-shot Core admission and native dispatch');assert.equal(result.firstState.status,'killed');assert.equal(result.portsRemoved,true);
 }finally{release.resolve();await cleanup(h,server);}
});

test('unload joins an in-flight durable claim and refuses its original executor after closing',{timeout:30000},async()=>{
 const root=join(evidence,'unload-pending-claim-'+randomUUID()),server=await core(root),entered=Promise.withResolvers(),release=Promise.withResolvers();let h,done=false,result,object_id;
 const client={...server.client,operate:async(op,payload,...rest)=>{const value=await server.client.operate(op,payload,...rest);if(op==='harness.observe'&&payload.observation?.kind==='job.claim'){object_id=payload.object_id;entered.resolve();await release.promise;}return value;}};
 try{
  const marker=join(root,'must-not-dispatch.txt');h=await harness(client,root,[async({call,runtime})=>{
   const launch=call('pwsh',{description:'held real Core claim',command:`Set-Content -LiteralPath '${marker.replaceAll("'","''")}' -Value 'unexpected'; Start-Sleep -Seconds 20`,run_in_background:true},'pending-core-claim');await entered.promise;let closed=false;const closing=runtime.dispose().then(()=>{closed=true;});await new Promise(r=>setTimeout(r,20));const closedBeforeClaimReturned=closed;release.resolve();const response=await launch;await closing;const state=await server.client.operate('harness.get',{scope_id:'native-jobs-scope',object_id});result={response,state,closedBeforeClaimReturned};done=true;
  }]);h.human('Hold one native dispatch claim while its adapter unloads.');await until(()=>done,'unload joins real durable claim');await writeFile(join(root,'evidence.json'),JSON.stringify({result,trace:h.trace},null,2));assert.equal(result.closedBeforeClaimReturned,false);assert.equal(result.response.isError,true);assert.equal(result.state.external_id,null);assert.equal(result.state.dispatch_state,'unknown');await assert.rejects(()=>readFile(marker),{code:'ENOENT'});
 }finally{release.resolve();await cleanup(h,server);}
});

test('Core process loss during native observations cannot bypass physical executor cancellation on adapter unload',{timeout:30000},async()=>{
 const root=join(evidence,'core-loss-unload-'+randomUUID()),server=await core(root),disconnected=Promise.withResolvers();let h,done=false,result,cut=false;
 const client={...server.client,operate:async(op,payload,...rest)=>{if(!cut&&op==='harness.observe'&&payload.observation?.kind==='job.output'){cut=true;await server.close();disconnected.resolve();}return server.client.operate(op,payload,...rest);}};
 try{
  h=await harness(client,root,[async({call,runtime,agent,ctx})=>{
   const started=await call('pwsh',{description:'executor must terminate even after actual Core process loss',command:"[Console]::Out.Write('disconnect Core now'); Start-Sleep -Seconds 20",run_in_background:true},'core-loss-native-call');await disconnected.promise;const id=runtime.list(agent.id).find(job=>job.kind==='pwsh').id;let unloadError;try{await runtime.dispose();}catch(error){unloadError=String(error);}const native=ctx.jobs.get(id,agent.id);result={started,id,unloadError,native,portsRemoved:!ctx.jobs.externalLifecycle};done=true;
  }]);h.human('Lose only the isolated Core process and then unload the native lifecycle adapter.');await until(()=>done,'native cleanup after Core process loss');await writeFile(join(root,'evidence.json'),JSON.stringify({result,trace:h.trace},null,2));assert.ok(result.unloadError,'the missing authoritative acknowledgement must remain an error');assert.ok(['killed','failed','completed'].includes(result.native.status),'physical native execution must be terminal before its observation port is removed');
 }finally{try{await cleanup(h,server);}catch(error){await writeFile(join(root,'cleanup-error.json'),JSON.stringify({error:String(error)}));}}
});

test('silent Core blackholes cannot defer physical cancellation behind queued native output RPC deadlines',{timeout:30000},async()=>{
 const root=join(evidence,'core-blackhole-unload-'+randomUUID()),server=await core(root),blocked=Promise.withResolvers(),twoOutputs=Promise.withResolvers();let h,armed=false,outputs=0,hung=0,result;
 const proxy=createHttpServer(async(request,response)=>{try{const parts=[];for await(const part of request)parts.push(part);if(armed){hung++;response.writeHead(200,{'content-type':'application/json'});response.write('{');blocked.resolve();return;}const upstream=await fetch(server.baseURL+request.url,{method:'POST',headers:request.headers,body:Buffer.concat(parts)});response.writeHead(upstream.status,{'content-type':'application/json'});response.end(await upstream.text());}catch(error){response.destroy(error);}});
 await new Promise(resolve=>proxy.listen(0,'127.0.0.1',resolve));
 const timeoutMs=1200;
 const proxyOptions={baseURL:`http://127.0.0.1:${proxy.address().port}`,credential:server.credential,context:()=>({actor:'native-jobs-fixture',chain_id:null,allow_write:true,allow_model:false,allow_revoke:false})};
 const healthyClient=createCoreClient(proxyOptions),faultClient=createCoreClient({...proxyOptions,requestTimeoutMs:timeoutMs});
 const client={
  operate:(...args)=>(armed?faultClient:healthyClient).operate(...args),
  issueTurn:(...args)=>(armed?faultClient:healthyClient).issueTurn(...args),
 };
 try{
  const firstRelease=join(root,'first.txt'),secondRelease=join(root,'second.txt');
  const waitForFile=path=>`while (-not (Test-Path -LiteralPath '${path.replaceAll("'","''")}')) { if ([DateTime]::UtcNow -ge $deadline) { throw 'fixture release timed out' }; Start-Sleep -Milliseconds 20 }; `;
  const command="$deadline = [DateTime]::UtcNow.AddSeconds(20); "+waitForFile(firstRelease)+"[Console]::Out.WriteLine('first blackhole output'); "+waitForFile(secondRelease)+"[Console]::Out.WriteLine('second queued output'); Start-Sleep -Seconds 20";
  let started,ready=false;h=await harness(client,root,[async({call})=>{started=await call('pwsh',{description:'actual executor behind an owned silent Core response',command,run_in_background:true},'silent-core-real-executor');assert.equal(started.isError,false,JSON.stringify(started));ready=true;}]);h.human('Start one isolated native process and blackhole only its Core observations.');await until(()=>ready,'actual native process tool result');
  const id=started.value.jobId,before=h.runtime.readState(id,h.agent.id),port=h.ctx.jobs.externalLifecycle,deliver=port.deliver;
  port.deliver=(event,...rest)=>{const value=deliver(event,...rest);if(event.type==='output'&&++outputs===2)twoOutputs.resolve();return value;};
  armed=true;await writeFile(firstRelease,'emit first output');await blocked.promise;
  await writeFile(secondRelease,'emit second output');await twoOutputs.promise;
  const cancel=h.ctx.jobs.cancelExecutor.bind(h.ctx.jobs),joinExecutor=h.ctx.jobs.joinExecutor.bind(h.ctx.jobs),start=performance.now();let cancelAt,joinedAt,portsPresentAtJoin;
  h.ctx.jobs.cancelExecutor=(...args)=>{cancelAt??=performance.now()-start;return cancel(...args);};h.ctx.jobs.joinExecutor=async(...args)=>{await joinExecutor(...args);joinedAt??=performance.now()-start;portsPresentAtJoin=!!h.ctx.jobs.externalLifecycle;};
  let unloadError;try{await h.runtime.dispose();}catch(error){unloadError=String(error);}const elapsed=performance.now()-start,native=h.ctx.jobs.get(id,h.agent.id),after=await server.client.operate('harness.get',{scope_id:'native-jobs-scope',object_id:before.object_id});result={timeoutMs,outputs,hung,cancelAt,joinedAt,portsPresentAtJoin,elapsed,native,before,after,unloadError};await writeFile(join(root,'evidence.json'),JSON.stringify({result,trace:h.trace},null,2));
  assert.ok(cancelAt<timeoutMs/2,'physical cancel must start independently of the blocked Core observation');assert.ok(joinedAt<timeoutMs,'physical executor join must precede even the first silent RPC deadline');assert.equal(portsPresentAtJoin,true);assert.ok(['killed','failed','completed'].includes(native.status));assert.ok(unloadError,'uncertain authoritative delivery remains an explicit unload error');assert.equal(hung,1,'failed serial delivery must not send queued output or cancellation into more blackholes');assert.equal(after.revision,before.revision,'local cancellation must not fabricate any Core observation');assert.equal(h.ctx.jobs.externalLifecycle,undefined);
 }finally{armed=false;proxy.closeAllConnections();await new Promise(resolve=>proxy.close(resolve));try{await cleanup(h,server);}catch(error){await writeFile(join(root,'cleanup-error.json'),JSON.stringify({error:String(error)}));}}
});

test('native ring loss keeps the native dropped-output notice and observer offsets through Core',{timeout:30000},async()=>{
 const root=join(evidence,'retention-'+randomUUID()),server=await core(root);let h,done=false;
 try{
  h=await harness(server.client,root,[async({call,runtime,agent})=>{const started=await call('pwsh',{description:'native retained tail fixture',command:"[Console]::Out.Write('0123456789abcdefghijklmnopqrstuvwxyz')",run_in_background:true},'native-retention-call');assert.equal(started.isError,false);const id=started.value.jobId;await until(async()=>{await runtime.drain();return runtime.readState(id,agent.id).status==='completed';},'retained native tail completes');const output=await call('job_output',{job_id:id},'native-retention-output');assert.match(output.value.text,/some output was dropped from memory/);const ui=h.ctx.jobs.readAt(id,0,agent.id);assert.equal(ui.lossy,true);assert.ok(ui.chunks[0].at>0);await writeFile(join(root,'evidence.json'),JSON.stringify({started,output,ui,state:runtime.readState(id,agent.id)},null,2));done=true;}],{retainBytes:8});h.human('Collect only the native retained-output fixture.');await until(()=>done,'native lost-output rendering');
 }finally{await cleanup(h,server);}
});

test('native publication consumption adopts actual collected jobs once, survives Core reopen, and excludes DELETE and V',{timeout:45000},async()=>{
 for(const route of ['KEEP','DELETE','V']){
  const root=join(evidence,'adoption-'+route.toLowerCase()+'-'+randomUUID());let server=await core(root),h,done=false,saved;
  try{
   const chain_id=await server.createChain();
   h=await harness(server.client,root,[async({call,runtime,agent})=>{
    const started=await call('pwsh',{description:'native adoption '+route,command:"[Console]::Out.Write('real collection to publication')",run_in_background:true},'native-adoption-launch');assert.equal(started.isError,false);
    const id=started.value.jobId;await until(async()=>{await runtime.drain();return runtime.readState(id,agent.id).status==='completed';},'adoption native process settles');
    const collected=await call('job_output',{job_id:id},'native-adoption-collection');assert.equal(collected.isError,false);const before=runtime.readState(id,agent.id),proof=collected.meta.sic_collection;
    assert.equal(before.adopted,false);assert.equal(typeof proof.collection_receipt_id,'string');
    const source=await server.client.begin({format:'openai-responses',request:{input:collected.value.text},provenances:[{lifecycle_object_id:before.object_id,collection_receipt_id:proof.collection_receipt_id,native_anchor:['input'],utf8_start:0,utf8_end:Buffer.byteLength(collected.value.text)}]});
    const unit=source.units[0],edits=route==='KEEP'?[]:[{unit_id:unit.unit_id,expected_text:collected.value.text,replacement:''}],ops=route==='KEEP'?[]:route==='DELETE'?[{op:'DELETE',target:unit.unit_id}]:[{op:'COMPRESS',target:unit.unit_id,content:'deferred native summary',lifecycle:'V'}];
    await server.client.commit({source_id:source.source_id,expected_source_digest:source.source_digest,expected_version:0,edits});
    const published=await server.client.publish({source_id:source.source_id,expected_source_digest:source.source_digest,expected_version:1,expected_cut:0,idempotency_key:'native-adoption-'+route,ops,pvi:{}}),publication=published.publication;
    assert.equal(publication.publication.collection_adoptions[0].eligible_current_p,route==='KEEP');assert.equal(runtime.readState(id,agent.id).adopted,false,'publication qualification alone is not host consumption');
    const requests=Array.from({length:3},()=>runtime.adoptPublication({agent,publication,request_id:'actual-consumer-'+route})),acks=await Promise.all(requests),after=runtime.readState(id,agent.id);
    assert.equal(after.adopted,route==='KEEP');assert.equal((after.adoptions??[]).length,route==='KEEP'?1:0);
    if(route==='KEEP'){
     assert.equal(acks.flatMap(a=>a.adoptions).filter(a=>!a.replayed).length,1);assert.equal(after.revision,before.revision+1);
     const forged=structuredClone(publication);forged.publication_id='missing-native-publication';await assert.rejects(()=>runtime.adoptPublication({agent,publication:forged,request_id:'untrusted-publication'}));
     const retried=await runtime.adoptPublication({agent,publication,request_id:'actual-consumer-retry'});assert.equal(retried.adoptions[0].replayed,true);assert.equal(runtime.readState(id,agent.id).revision,after.revision);
    }else assert.ok(acks.every(a=>a.adoptions.length===0));
    saved={id,chain_id,publication,before,after,acks};done=true;
   }],{chain_id});h.human('Run and collect the isolated native job, then consume its exact publication.');await until(()=>done,'native actual publication adoption');
   if(route==='KEEP'){
    const db=server.db;await h.close();h=null;await server.close();server=await core(join(root,'reopened'),db,chain_id);
    h=await harness(server.client,root,[],{chain_id});const beforeReplay=await server.client.operate('harness.get',{scope_id:'native-jobs-scope',object_id:saved.before.object_id}),replay=await h.runtime.adoptPublication({agent:h.agent,publication:saved.publication,request_id:'reopened-consumer'});assert.equal(replay.adoptions[0].replayed,true);const recovered=h.runtime.readState(saved.id,h.agent.id);assert.equal(recovered.adoptions.length,1);assert.equal(recovered.revision,beforeReplay.revision);saved.reopened={beforeReplay,replay,recovered};
   }
   await writeFile(join(root,'evidence.json'),JSON.stringify({core_sha256:server.hash,...saved},null,2));
  }finally{await cleanup(h,server);}
 }
});

test('native collection adoption probes the actual higher cut, skips DELETE and V, and retains an authorized in-flight historical cut',{timeout:60000},async()=>{
 for(const route of ['COMPRESS','DELETE','V']){
  const root=join(evidence,'adoption-cut-'+route.toLowerCase()+'-'+randomUUID()),server=await core(root);let h,done=false;
  try{
   const chain_id=await server.createChain();if(route==='COMPRESS')await server.client.operate('board.create',{acl:{readers:[chain_id],writers:[chain_id],claimers:[chain_id],managers:[chain_id]}});h=await harness(server.client,root,[async({call,runtime,agent})=>{
    const started=await call('pwsh',{description:'native actual cut '+route,command:"[Console]::Out.Write('real output for higher P lineage')",run_in_background:true},'native-cut-launch');assert.equal(started.isError,false);const id=started.value.jobId;
    await until(async()=>{await runtime.drain();return runtime.readState(id,agent.id).status==='completed';},'actual higher-cut native process completion');
    const output=await call('job_output',{job_id:id},'native-cut-collection');assert.equal(output.isError,false);const state=runtime.readState(id,agent.id),proof=output.meta.sic_collection;
    const source=await server.client.begin({format:'openai-responses',request:{input:output.value.text},provenances:[{lifecycle_object_id:state.object_id,collection_receipt_id:proof.collection_receipt_id,native_anchor:['input'],utf8_start:0,utf8_end:Buffer.byteLength(output.value.text)}]});
    await server.client.commit({source_id:source.source_id,expected_source_digest:source.source_digest,expected_version:0,edits:[]});const low=(await server.client.publish({source_id:source.source_id,expected_source_digest:source.source_digest,expected_version:1,expected_cut:0,idempotency_key:'native-cut-low'})).publication,oldCut=low.publication.cut_id;
    assert.equal(low.publication.collection_adoptions[0].eligible_current_p,true,'lower publication eligibility does not establish final P survival');
    const highSource=await server.client.operate('hierarchy.form',{level:2,cb_ids:[low.cb_id]}),job=await server.client.operate('edit.prepare',{source_id:highSource.source_id}),ops=route==='DELETE'?[{op:'DELETE',target:'u1'}]:[{op:'COMPRESS',target:'u1',content:'higher real result summary',...route==='V'?{lifecycle:'V'}:{}}];
    await server.client.operate('edit.submit',{job_id:job.job_id,editor:'editor-1',ops,pvi:{}});const high=await server.client.operate('cut.publish',{job_id:job.job_id,expected_cut:1,idempotency_key:'native-cut-high'}),cut=await server.client.operate('cut.get',{});assert.equal(cut.view.cbs[0].cb_id,high.cb_id);
    await runtime.drain();const before=runtime.readState(id,agent.id),ack=await runtime.adoptPublication({agent,publication:low,request_id:'actual-higher-consumption',adopted_cut_id:cut.cut_id}),after=runtime.readState(id,agent.id);
    assert.equal(ack.adopted_cut_id,cut.cut_id);assert.equal(after.adopted,route==='COMPRESS');assert.equal(after.revision,before.revision+(route==='COMPRESS'?1:0));
    if(route==='COMPRESS'){
     assert.equal(ack.adoptions.length,1);assert.deepEqual(ack.adoptions[0].p_lineage.p_paths[0],[high.cb_id,low.cb_id]);assert.equal(after.adoptions[0].adopted_cut_id,cut.cut_id);
     const repeat=await runtime.adoptPublication({agent,publication:low,request_id:'higher-consumption-replay',adopted_cut_id:cut.cut_id});assert.equal(repeat.adoptions[0].replayed,true);assert.equal(runtime.readState(id,agent.id).revision,after.revision);
    }else{
     assert.equal(ack.adoptions.length,0);assert.equal(ack.skipped.length,1);assert.equal(ack.skipped[0].eligible_current_p,false);assert.equal(ack.skipped[0].p_lineage.publication_eligible_current_p,true);assert.equal(ack.skipped[0].p_lineage.eligible_current_p,false);
     const current=await runtime.adoptPublication({agent,publication:low,request_id:'default-current-cut'});assert.equal(current.skipped[0].adopted_cut_id,cut.cut_id);assert.equal(runtime.readState(id,agent.id).revision,before.revision);
    }
    await assert.rejects(()=>runtime.adoptPublication({agent,publication:low,request_id:'invalid-cut-must-fail',adopted_cut_id:'unrelated-cut-not-on-this-chain'}),/rejected: forbidden/);
    let historical;if(route!=='COMPRESS'){
     historical=await runtime.adoptPublication({agent,publication:low,request_id:'actual-original-inflight-consumption',adopted_cut_id:oldCut});assert.equal(historical.adoptions[0].adopted_cut_id,oldCut);assert.equal(historical.adoptions[0].p_lineage.historical_snapshot,true);assert.equal(runtime.readState(id,agent.id).adopted,true);assert.equal(runtime.readState(id,agent.id).adoptions.length,1);
     const repeat=await runtime.adoptPublication({agent,publication:low,request_id:'inflight-replay',adopted_cut_id:oldCut});assert.equal(repeat.adoptions[0].replayed,true);assert.equal(runtime.readState(id,agent.id).adoptions.length,1);
    }
    let aclDenied=false;if(route==='COMPRESS'){await server.client.operate('board.unbind',{});await assert.rejects(()=>runtime.adoptPublication({agent,publication:low,request_id:'revoked-board-must-fail',adopted_cut_id:cut.cut_id}),/rejected: forbidden/);aclDenied=true;assert.equal(runtime.readState(id,agent.id).revision,after.revision);}
    await writeFile(join(root,'evidence.json'),JSON.stringify({core_sha256:server.hash,route,started,output,low,high,cut,before,ack,after,historical,aclDenied,final:runtime.readState(id,agent.id),receipts:h.receipts},null,2));done=true;
   }],{chain_id});h.human('Collect the isolated native result and adopt only the actual selected P cut.');await until(()=>done,'real native higher P adoption probe');
  }finally{await cleanup(h,server);}
 }
});
