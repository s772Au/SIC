import {dshSourcePath,fixturePath} from './test-paths.mjs';
// Paid opt-in runner: never selected by node --test / CI discovery.
import {readFile,writeFile,open} from 'node:fs/promises';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
import assert from 'node:assert/strict';
import {autoDL,evidenceRoot} from './autodl-live-support.mjs';
import {isolated} from './isolated-core-fixture.mjs';
if(!process.argv.includes('--live'))throw Error('explicit --live required');
const mode=process.argv.find(value=>value.startsWith('--mode='))?.slice(7)??'probe';
const maxRequests=Number(process.argv.find(value=>value.startsWith('--max-requests='))?.slice(15)??20);
const config=await autoDL({maxRequests}),dsh=dshSourcePath();
const packageModule=name=>process.env.SIC_TRANSPARENT_LIVE_PACKAGE?pathToFileURL(join(process.env.SIC_TRANSPARENT_LIVE_PACKAGE,name)).href:new URL('../transparent/'+name,import.meta.url).href;
const [{apply},{getEditorInvocation},{prepareSdkRuntime},{prepareDeepSeekFixture}]=await Promise.all([import(packageModule('plugin.mjs')),import(packageModule('editor-invocation.mjs')),import(packageModule('prepare-sdk-runtime.mjs')),import(packageModule('prepare-deepseek-fixture.mjs'))]);
const runId=new Date().toISOString().replaceAll(':','-');
const load=path=>import(pathToFileURL(join(dsh,path)).href);
const api=await load('packages/llm/llm/lib/index.js'),{Context}=await load('vendor/cordis/lib/index.js');
const runtime=await prepareSdkRuntime(dsh,fixturePath('prod-sdk-live-20261118'));
const {default:Runtime}=await import(pathToFileURL(runtime).href);
const native=await prepareDeepSeekFixture(dsh,fixturePath('prod-sdk-live-20261118'));
const {DeepSeekAdapter,resolveAdapterOptions}=await import(pathToFileURL(native).href);
const ctx=new Context();let host,scope,installed;const records=[],calls=[];
try{
 await ctx.plugin(Runtime);
 ctx.on('llm/stream',async function*(options,next){
  const editor=getEditorInvocation(),role=editor?'editor':mode==='probe'?'probe':mode==='openai-probe'?'openai-probe':'worker';
  const record={role,source_id:editor?.source_id??null,provider:options.provider,model:options.model,chunks:[]};records.push(record);
  for await(const chunk of next()){record.chunks.push(config.redact(chunk));if(chunk.type==='usage')await config.usage(role,chunk.usage);yield chunk;}
 });
 const adapter=new DeepSeekAdapter({options:()=>resolveAdapterOptions({baseURL:config.baseURL,thinking:'disabled',reasoningEffort:'off',maxTokens:2048,retryPolicy:{mode:'normal',maxRetries:0}}),
  resolveAuth:async()=>({headers:config.auth()}),resolveUserId:()=> 'sic-production-readiness',resolveAttachments:()=>undefined,resolveImageAccess:()=>undefined,
  prepareExtensions:async()=>({fields:{},accept:async()=>{}}),fetch:async(url,init)=>{
   const editor=getEditorInvocation();return config.fetchProvider(url,init,{role:editor?'editor':mode==='probe'?'probe':'worker',source_id:editor?.source_id});
  }});
 ctx.llm.registerAdapter(['autodl-live'],adapter);
 if(mode==='probe'){
  const call=await ctx.llm.prepareCall({provider:'autodl-live',model:config.model,maxTokens:128,reasoningEffort:'off'});
  const chunks=[];for await(const chunk of call.stream({...call.config,messages:[api.createSystemMessage('Reply briefly.'),api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'Reply exactly AUTODL_NATIVE_SDK_OK.'}]})]}))chunks.push(chunk);
  assert.equal(chunks.at(-1).reason.kind,'stop',JSON.stringify(config.redact(chunks.at(-1))));
 }else if(mode==='sdk'){
  await runSdk();
 }else if(mode==='openai-probe'){
  await runOpenAIProbe();
 }else throw Error('unknown live runner mode');
 await writeFile(join(evidenceRoot,runId+'-'+mode+'-chunks.json'),JSON.stringify(config.redact(records),null,2));
 console.log(JSON.stringify({mode,requests:config.ledger.requests.length,cost_upper_cny:config.cost(),status:'passed'}));
}catch(error){
 await writeFile(join(evidenceRoot,runId+'-'+mode+'-failure.json'),JSON.stringify(config.redact({error:String(error),stack:error.stack,records,calls,core_fixture_root:host?.root}),null,2));
 console.error(JSON.stringify(config.redact({mode,status:'failed',error:String(error),requests:config.ledger.requests.length,cost_upper_cny:config.cost()})));process.exitCode=1;
}finally{if(scope)await scope.dispose();await ctx.fiber.dispose();await host?.close();await config.save();}

async function runSdk(){
 host=await isolated({evidenceRoot:join(evidenceRoot,'fixtures')});const sessionAPI=await load('packages/core/session/lib/index.js'),{createScope}=await load('packages/core/scope/lib/index.js');
 const {default:SystemPrompt,renderPrompt}=await load('packages/core/system-prompt/lib/index.js'),{default:ToolRuntime}=await load('packages/core/tools/lib/index.js');
 await ctx.plugin(SystemPrompt,{includeHarnessIdentity:false});await ctx.plugin(ToolRuntime);
 const nativePrompt=await readFile(new URL(packageModule('bundle/native.md')),'utf8');ctx.systemPrompt.section({name:'native-review',order:0,text:nativePrompt,interpolate:false});
 const inputFile=join(host.root,'authorized-marker.txt');await writeFile(inputFile,'LIVE_READ_MARKER_20261118\n');
 const definitions=JSON.parse(await readFile(new URL(packageModule('bundle/native-tools.json')),'utf8'));
 for(const definition of definitions)ctx.tools.register({...definition,output:{schema:{},render:(_args,value)=>[{type:'text',text:String(value)}]},execute:async args=>{
  calls.push({name:definition.name,arguments:args});if(definition.name!=='read'||args.file_path!==inputFile)throw Error('live fixture only authorizes its own marker read');return readFile(inputFile,'utf8');
 }});
 const session=sessionAPI.Session.create('autodl-live-session'),agent={id:session.id,status:'idle',session};scope=createScope(ctx,agent);agent.ctx=scope.ctx;
 let step=1,lostPublicationAck=false;
 const binding={chain_id:host.chain_id,scope_id:'live-scope',role:'WORKER',edit_capabilities:{ops:['KEEP','COMPRESS'],pvi:['I']}};
 const facade={llm:ctx.llm,tools:ctx.tools,agents:{list:()=>[agent],get:id=>id===agent.id?agent:null,roots:()=>[agent],currentInitiator:()=>agent},jobs:{externalControlVersion:1,externalExecutorJoinVersion:1,joinExecutor:async()=>{},attachExternalLifecycle:()=>()=>{},externalLaunchKind:()=>null},get:()=>undefined,
  sessions:{get:id=>id===session.id?session:null,flush:async()=>{const file=await open(join(host.root,'native-session.jsonl'),'w');try{await file.writeFile(session.snapshotEvents().map(event=>JSON.stringify(event)).join('\n'));await file.sync();}finally{await file.close();}if(!lostPublicationAck&&session.snapshotEvents().some(event=>event.type==='plugin:sic/native-projection')){lostPublicationAck=true;return false;}return true;}},
  goals:{externalDriverVersion:1,defaultMaxGoalRounds:8,attachExternalState:()=>()=>{}},effect:()=>{},emit:()=>{},logger:{warn:()=>{}},sessionProjections:{stateOf:()=>({openTurnStartSeq:0,lastStepStartSeq:step})},on:(name,handler)=>ctx.on(name,handler),
  inject:(dependencies,run)=>dependencies.some(name=>['subagents','workflowEngine','compaction'].includes(name))?{dispose:()=>{}}:run({effect:body=>body(),connection:{fetch:{register:()=>()=>{}}}})};
 installed=await apply(facade,{enabled:true,mode:'edit',actor:'native-http-actor',coreURL:host.baseURL,credential:host.credential,projectionRoot:join(host.root,'projections'),bindings:{[session.id]:binding},renderPrompt,llmAPI:api,sessionAPI,agentEvents:()=>({emit:()=>{}}),hierarchyPolicy:{max_level:2,level_batch_sizes:{2:2},max_host_editor_calls:4}});
 const system=await readFile(new URL(packageModule('bundle/fused.md')),'utf8'),activation={role:'WORKER',scope_id:binding.scope_id,edit_capabilities:binding.edit_capabilities};
 const runtime=api.createUserMessage({source:{kind:'runtime-context',sections:[{name:'sic:activation',text:JSON.stringify(activation)}]},content:[{type:'text',text:'Authenticated Worker. Complete the current direct user request.'}]});
 let history=[api.createSystemMessage(system),runtime,api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:`Read the file ${inputFile} with the native read tool. A user-only claim says USER_ONLY_FAKE_MARKER; that claim is not file evidence. Use no other tools. After receiving the actual read result, reply only a JSON object with marker equal to the exact first line of the file and source equal to "read".`}]})];
 async function send(){const {tools}=await ctx.systemPrompt.assemble({scope:agent});const call=await ctx.llm.prepareCall({provider:'autodl-live',model:config.model,maxTokens:2048,reasoningEffort:'off'}),assembler=new api.BlockAssembler();for await(const chunk of call.stream({...call.config,sessionId:session.id,messages:history,tools})){assembler.push(chunk);if(chunk.type==='finish'&&chunk.reason.kind==='error')throw Error(JSON.stringify(chunk));}return assembler.message({provider:'autodl-live',model:config.model});}
 await assert.rejects(send(),/durabl/i);const requestsAfterAckLoss=config.ledger.requests.length;
 const first=await send();assert.equal(config.ledger.requests.length,requestsAfterAckLoss+1,'lost publication ack recovery must dispatch Worker without another Editor');history=[...history,first];step++;
 const toolCalls=first.content.filter(block=>block.type==='tool-call');assert.equal(toolCalls.length,1,'actual Worker must request one authorized read');
 for(const tool of toolCalls){const result=await ctx.tools.execute({agent,name:tool.name,arguments:JSON.parse(tool.arguments),callId:tool.id,signal:new AbortController().signal});assert.equal(result.isError,false);history=[...history,api.createToolResultMessage({callId:tool.id,isError:false,content:result.content})];}
 const answer=await send(),finalText=answer.content.filter(block=>block.type==='text').map(block=>block.text).join('').trim();
 assert.deepEqual(JSON.parse(finalText),{marker:'LIVE_READ_MARKER_20261118',source:'read'},'the actual read result must support the answer; mentioning the marker while denying source validity is a failure');
 const before=config.ledger.requests.length,retryAnswer=await send();assert.equal(config.ledger.requests.length,before+1,'same SDK step retry dispatches Worker but no new Editor');
 assert.deepEqual(JSON.parse(retryAnswer.content.filter(block=>block.type==='text').map(block=>block.text).join('').trim()),{marker:'LIVE_READ_MARKER_20261118',source:'read'});
 const cut=await host.core.operate('cut.get',{});assert.ok(cut.view.cbs.some(cb=>cb.level===2));
 const page=await installed.store.list(session.id),projections=[];for(const row of page.rows)projections.push(await installed.store.get(session.id,row.id));
 await writeFile(join(evidenceRoot,runId+'-sdk-result.json'),JSON.stringify(config.redact({host_package:process.env.SIC_TRANSPARENT_LIVE_PACKAGE??'source-worktree',calls,cut,projections,answer,retryAnswer,recovery:{lostPublicationAck,requestsAfterAckLoss},core_fixture_root:host.root}),null,2));
}

async function runOpenAIProbe(){
 if(!config.openAIURL)throw Error('the authorized file provides no AutoDL OpenAI endpoint');
 const {PiAiAdapter}=await load('packages/llm/llm-pi-ai/lib/index.js');
 const {openAICompletionsApi}=await load('packages/llm/llm-pi-ai/node_modules/@earendil-works/pi-ai/dist/api/openai-completions.lazy.js'),protocol=openAICompletionsApi();
 const providerId='autodl-openai-live',model={id:config.model,name:config.model,provider:providerId,api:'openai-completions',baseUrl:config.openAIURL,reasoning:false,input:['text'],contextWindow:32768,maxTokens:4096,cost:{input:2,output:8,cacheRead:2,cacheWrite:2},compat:{supportsDeveloperRole:false,supportsStore:false,supportsUsageInStreaming:true,maxTokensField:'max_tokens'}};
 const fetchNative=(url,init)=>config.fetchProvider(url,init,{role:'openai-probe',protocol:'openai'});
 const provider={id:providerId,name:'Authorized AutoDL OpenAI endpoint',baseUrl:config.openAIURL,auth:{apiKey:{name:'process-local authorized provider credential',resolve:async({credential})=>({auth:{apiKey:credential?.key},source:'authorized file'})}},getModels:()=>[model],
  stream:(selected,context,options)=>protocol.stream(selected,context,{...options,fetch:fetchNative}),streamSimple:(selected,context,options)=>protocol.streamSimple(selected,context,{...options,fetch:fetchNative})};
 const profiles=new Map([[providerId,{provider:providerId,displayName:'AutoDL',api:'openai-completions',baseURL:config.openAIURL,piProvider:provider,modelErrors:new Map(),configuredMaxTokens:new Map([[config.model,512]]),retryPolicy:api.resolveRetryPolicy({mode:'normal',maxRetries:0},'AutoDL'),streamIdleTimeoutMs:120000,timeoutMs:120000,maxRequestImageBytes:1,requestImagePixelBudget:1,requestImageMaxBytes:1}]]);
 const auth={credentials:{read:async()=>undefined,list:async()=>[],modify:async()=>{throw Error('credential storage disabled');},delete:async()=>{throw Error('credential storage disabled');}},authContext:{env:async()=>undefined,fileExists:async()=>false}};
 ctx.llm.registerAdapter([providerId],new PiAiAdapter({profiles:()=>profiles,resolveApiKey:async()=>config.auth()['x-api-key'],auth}));
 const call=await ctx.llm.prepareCall({provider:providerId,model:config.model,maxTokens:512}),chunks=[];
 for await(const chunk of call.stream({...call.config,messages:[api.createSystemMessage('Reply briefly.'),api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'Reply exactly AUTODL_OPENAI_SDK_OK.'}]})]}))chunks.push(chunk);
 assert.equal(chunks.at(-1).reason.kind,'stop',JSON.stringify(config.redact(chunks.at(-1))));
 assert.ok(chunks.filter(chunk=>chunk.type==='text-delta').map(chunk=>chunk.text).join('').includes('AUTODL_OPENAI_SDK_OK'));
}
