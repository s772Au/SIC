import {dshSourcePath,fixturePath,evidencePath} from './test-paths.mjs';
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile,writeFile,mkdir,cp,readdir,open,unlink} from 'node:fs/promises';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {randomUUID,createHash} from 'node:crypto';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {fileURLToPath} from 'node:url';
import {createRequire} from 'node:module';
import {dirname} from 'node:path';
import {isolated} from './isolated-core-fixture.mjs';
import {bindNativeSession,declarativeHostConfig,readHostBindings,hostControlClient} from '../transparent/binding-store.mjs';
import {prepareSdkRuntime} from '../transparent/prepare-sdk-runtime.mjs';
const dsh=dshSourcePath(),load=path=>import(pathToFileURL(join(dsh,path)).href);
const {Context}=await load('vendor/cordis/lib/index.js'),api=await load('packages/llm/llm/lib/index.js'),sessionAPI=await load('packages/core/session/lib/index.js');
const {PluginPackages}=await load('packages/boot/app-boot/lib/index.js'),{Loader}=await load('vendor/loader/lib/index.js');
const {default:SystemPrompt}=await load('packages/core/system-prompt/lib/index.js'),{default:ToolRuntime}=await load('packages/core/tools/lib/index.js'),{createScope}=await load('packages/core/scope/lib/index.js');
const {default:LocalAttachmentStore}=await load('packages/attachment/attachment-local/lib/index.js');
const runtimePath=await prepareSdkRuntime(dsh,fixturePath('declarative-sdk-fixture')),{default:Runtime}=await import(pathToFileURL(runtimePath).href);
async function installedModules(){
 // Copy the package byte-for-byte. The real DSH PluginPackages/Loader resolves
 // its declared peers; no source import rewriting or duplicate npm install.
 const folder=process.env.SIC_TRANSPARENT_TEST_PACKAGE??join(fixturePath(),'declarative-entry-'+randomUUID());
 if(!process.env.SIC_TRANSPARENT_TEST_PACKAGE){await mkdir(folder,{recursive:true});await cp(new URL('../transparent/',import.meta.url),folder,{recursive:true});}
 return{folder};
}
async function mountOriginalLoader(ctx,modules){
 const profile=join(fixturePath(),'native-loader-'+randomUUID(),'profiles','isolated'),peers={'@deepseek-ai/dsh-llm':'packages/llm/llm','@deepseek-ai/dsh-session':'packages/core/session','@deepseek-ai/dsh-system-prompt':'packages/core/system-prompt','@deepseek-ai/dsh-agent':'packages/core/agent','@deepseek-ai/dsh-jobs':'packages/jobs/jobs'};
 await mkdir(profile,{recursive:true});await writeFile(join(profile,'package.json'),'{"name":"isolated-native-loader-profile","private":true}');
 const entries=await Promise.all(Object.entries(peers).map(async([name,path])=>{const packageDir=join(dsh,path),manifest=JSON.parse(await readFile(join(packageDir,'package.json'),'utf8'));return{name,packageDir,version:manifest.version,declarer:join(packageDir,'package.json'),scope:'installation'};}));
 const sharpEntry=createRequire(join(dsh,'packages/attachment/attachment-local/package.json')).resolve('sharp');
 const sharpManifest=join(dirname(sharpEntry),'..','package.json');
 entries.push({name:'sharp',packageDir:dirname(sharpManifest),version:JSON.parse(await readFile(sharpManifest,'utf8')).version,declarer:sharpManifest,scope:'installation'});
 entries.push({name:'sic-transparent-host',packageDir:modules.folder,version:'0.2.0',declarer:join(modules.folder,'package.json'),scope:'profile'});
 await ctx.plugin(PluginPackages,{resolution:{profilesDir:join(profile,'..'),profileDir:profile,localPackageNames:[],entries,linkedRoots:[{name:'sic-transparent-host',realPath:modules.folder}]}});
 ctx.baseUrl=pathToFileURL(join(profile,'package.json')).href;
 await ctx.plugin(Loader,{baseUrl:ctx.baseUrl});
 modules.entry=await ctx.loader.import('sic-transparent-host');modules.editor=await ctx.loader.import(pathToFileURL(join(modules.folder,'editor-invocation.mjs')).href);
 const parent=pathToFileURL(join(modules.folder,'cordis-entry.mjs')).href,peer=name=>ctx.loader.internal.import(name,parent,{});
 assert.equal((await peer('@deepseek-ai/dsh-session')).Session,sessionAPI.Session);assert.equal((await peer('@deepseek-ai/dsh-llm')).LlmAdapter,api.LlmAdapter);
 assert.equal((await peer('@deepseek-ai/dsh-agent')).agentEvents,(await load('packages/core/agent/lib/index.js')).agentEvents);
 assert.equal((await peer('@deepseek-ai/dsh-system-prompt')).renderPrompt,(await load('packages/core/system-prompt/lib/index.js')).renderPrompt);
 if(!process.env.SIC_TRANSPARENT_TEST_PACKAGE)assert.equal(await readFile(join(modules.folder,'cordis-entry.mjs'),'utf8'),await readFile(new URL('../transparent/cordis-entry.mjs',import.meta.url),'utf8'));
}
test('declarative Cordis JSON configuration loads the primary SDK plugin, persists binding and reopens without creating a second chain or Editor',{timeout:20000},async()=>{
 const host=await isolated({evidenceRoot:evidencePath('production-readiness-20261118/sdk-live/declarative')}),modules=await installedModules(),envName='SIC_DECLARATIVE_FIXTURE_'+randomUUID().replaceAll('-','');
 process.env[envName]=host.credential();const config={enabled:true,imageCompression:true,actor:'native-http-actor',coreURL:host.baseURL,credentialEnv:envName,projectionRoot:join(host.root,'projections'),bindingsFile:join(host.root,'bindings.json')};
 const human=api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'a durable native user task'}]});let events,editors=0,workers=0;
 async function run(resume,legacy=false){
  const ctx=new Context();let scope;try{
   await ctx.plugin(Runtime);await ctx.plugin(SystemPrompt,{includeHarnessIdentity:false});await ctx.plugin(ToolRuntime);
   await ctx.plugin(LocalAttachmentStore,{dshHome:join(host.root,'attachments-home')});
   const native=await readFile(new URL('../transparent/bundle/native.md',import.meta.url),'utf8');ctx.systemPrompt.section({name:'reviewed-native',order:0,text:native,interpolate:false});
   for(const schema of JSON.parse(await readFile(new URL('../transparent/bundle/native-tools.json',import.meta.url),'utf8')))ctx.tools.register({...schema,output:{schema:{},render:()=>[]},execute:async()=>{throw Error('unexpected tool');}});
   const session=resume?sessionAPI.Session.create('declarative-session',events):sessionAPI.Session.create('declarative-session'),agent={id:session.id,status:'idle',session};scope=createScope(ctx,agent);agent.ctx=scope.ctx;
   if(legacy)session.append('user/message',human,{surfaceOp:'append'});
   ctx.provide('agents',{list:()=>[agent],get:id=>id===agent.id?agent:null,roots:()=>[agent],currentInitiator:()=>agent});
   ctx.provide('jobs',{externalControlVersion:1,externalExecutorJoinVersion:1,joinExecutor:async()=>{},attachExternalLifecycle:()=>()=>{},externalLaunchKind:()=>null});
   ctx.provide('goals',{externalDriverVersion:1,defaultMaxGoalRounds:8,attachExternalState:()=>()=>{}});
   ctx.provide('connection',{fetch:{register:()=>()=>{}}});ctx.provide('sessionProjections',{stateOf:()=>({openTurnStartSeq:0,lastStepStartSeq:1})});
   ctx.provide('sessions',{get:id=>id===session.id?session:null,flush:async()=>{const file=await open(join(host.root,'session.jsonl'),'w');try{await file.writeFile(session.snapshotEvents().map(row=>JSON.stringify(row)).join('\n'));await file.sync();}finally{await file.close();}return true;}});
   await mountOriginalLoader(ctx,modules);
   class Model extends api.LlmAdapter{async *stream(options){
    if(modules.editor.getEditorInvocation()){editors++;yield{type:'tool-call-delta',index:0,id:'delivery',name:'sic_edit_blocks',argumentsDelta:'{"ops":[],"pvi":{}}'};yield{type:'finish',reason:{kind:'tool-calls'}};return;}
    workers++;assert.equal(options.provider,'original-route');yield{type:'text-delta',text:'declarative SDK works'};yield{type:'finish',reason:{kind:'stop'}};
   }}ctx.llm.registerAdapter(['original-route'],new Model());
   if(legacy){const before=session.snapshotEvents();await assert.rejects(()=>modules.entry.apply(ctx,config),/independent explicit SIC migration/);assert.deepEqual(session.snapshotEvents(),before);return;}
   await ctx.loader.create({name:'sic-transparent-host',config:JSON.parse(JSON.stringify(config))});await ctx.loader.await();assert.ok(ctx.get('sicTransparent'));
   if(!resume)session.append('user/message',human,{surfaceOp:'append'});
   const system=await readFile(new URL('../transparent/bundle/fused.md',import.meta.url),'utf8'),activation=api.createUserMessage({source:{kind:'runtime-context',sections:[{name:'sic:activation',text:JSON.stringify({role:'WORKER',scope_id:'dsh:declarative-session'})}]},content:[{type:'text',text:'current'}]});
   const call=await ctx.llm.prepareCall({provider:'original-route',model:'original-model',maxTokens:128});
   for await(const chunk of call.stream({...call.config,sessionId:session.id,messages:[api.createSystemMessage(system),activation,human],tools:ctx.tools.schemas(agent)}))if(chunk.type==='finish')assert.equal(chunk.reason.kind,'stop',JSON.stringify(chunk));
   assert.equal(await ctx.sessions.flush(session),true);events=(await readFile(join(host.root,'session.jsonl'),'utf8')).split('\n').map(JSON.parse);
  }finally{if(scope)await scope.dispose();await ctx.fiber.dispose();}
 }
 try{
  const first=await bindNativeSession(config,{sessionId:'declarative-session',create:true});assert.equal(first.replayed,false);
  const binding=await readHostBindings(declarativeHostConfig(config));assert.equal(binding.sessions['declarative-session'].chain_id,first.chain_id);
  const before=(await hostControlClient(declarativeHostConfig(config)).operate('chain.list',{})).routes.length;
  await run(false,true);assert.equal(editors,0);assert.equal(workers,0);await run(false);assert.equal(editors,1);await run(true);assert.equal(editors,1);assert.equal(workers,2);
  const replay=await bindNativeSession(config,{sessionId:'declarative-session',create:true});assert.equal(replay.replayed,true);assert.equal(replay.chain_id,first.chain_id);
  assert.equal((await hostControlClient(declarativeHostConfig(config)).operate('chain.list',{})).routes.length,before);
  const configFile=join(host.root,'host-config.json');await writeFile(configFile,JSON.stringify(config));
  const cli=await promisify(execFile)(process.execPath,[fileURLToPath(new URL('../transparent/bind-session.mjs',import.meta.url)),'--config',configFile,'--session','declarative-session','--chain',first.chain_id],
   {windowsHide:true,env:{PATH:process.env.PATH,SystemRoot:process.env.SystemRoot,TEMP:host.root,TMP:host.root,[envName]:host.credential()}});
  assert.equal(JSON.parse(cli.stdout).chain_id,first.chain_id);assert.equal(cli.stdout.includes(host.credential()),false);
  assert.equal(JSON.parse(cli.stdout).history_admission,'fresh-only');assert.equal(JSON.parse(cli.stdout).migration_performed,false);
  await assert.rejects(()=>promisify(execFile)(process.execPath,[fileURLToPath(new URL('../transparent/bind-session.mjs',import.meta.url)),'--config',configFile,'--session','declarative-session','--create','--migrate'],{windowsHide:true,env:{PATH:process.env.PATH,SystemRoot:process.env.SystemRoot,TEMP:host.root,TMP:host.root,[envName]:host.credential()}}),error=>error.stderr.includes('historical SIC migration is not implemented'));
  assert.equal((await readFile(config.bindingsFile,'utf8')).includes(host.credential()),false);
  await bindNativeSession(config,{sessionId:'__proto__',chainId:first.chain_id});const opaque=await readHostBindings(declarativeHostConfig(config));
  assert.equal(Object.hasOwn(opaque.sessions,'__proto__'),true);assert.equal(opaque.sessions.__proto__.chain_id,first.chain_id);
  await writeFile(config.bindingsFile+'.create-intent',JSON.stringify({sessionId:'uncertain-session',state:'creating'}));
  await assert.rejects(()=>bindNativeSession(config,{sessionId:'uncertain-session',create:true}),/uncertain Core chain creation/);
  assert.equal((await hostControlClient(declarativeHostConfig(config)).operate('chain.list',{})).routes.length,before,'an uncertain create is not blindly redispatched');
  await assert.rejects(readHostBindings(declarativeHostConfig({...config,actor:'other-host'})),/actor\/Core\/version/);
  assert.throws(()=>declarativeHostConfig({...config,credential:()=> 'not JSON'}),/unsupported declarative/);
  await writeFile(join(host.root,'native-loader-evidence.json'),JSON.stringify({package_root:modules.folder,source_rewritten:false,npm_duplicate_installed:false,peer_singletons:['Session','LlmAdapter','agentEvents','renderPrompt'],entry_sha256:createHash('sha256').update(await readFile(join(modules.folder,'cordis-entry.mjs'))).digest('hex'),editors,workers},null,2));
 }finally{delete process.env[envName];await host.close();}
});

test('actual binding CLI process crashes retain atomic intents and replay cleans a committed binding without a second chain',{timeout:20000},async()=>{
 const host=await isolated({evidenceRoot:evidencePath('production-readiness-20261118/sdk-live/declarative-faults')}),envName='SIC_BIND_FAULT_TOKEN';
 process.env[envName]=host.credential();const config={enabled:true,actor:'native-http-actor',coreURL:host.baseURL,credentialEnv:envName,projectionRoot:join(host.root,'projections'),bindingsFile:join(host.root,'bindings.json')};
 const configFile=join(host.root,'config.json'),hook=join(host.root,'fault-hook.mjs');await writeFile(configFile,JSON.stringify(config));
 await writeFile(hook,`import fs from 'node:fs';import{syncBuiltinESMExports}from'node:module';const rename=fs.promises.rename;fs.promises.rename=async(source,target)=>{if(process.env.BIND_FAULT==='before-created'&&target.endsWith('.create-intent')&&JSON.parse(await fs.promises.readFile(source,'utf8')).state==='created')process.exit(91);await rename(source,target);if(process.env.BIND_FAULT==='after-store'&&target===process.env.BINDING_PATH)process.exit(92);};syncBuiltinESMExports();`);
 const cli=(session,args,phase)=>promisify(execFile)(process.execPath,[...phase?['--import',pathToFileURL(hook).href]:[],fileURLToPath(new URL('../transparent/bind-session.mjs',import.meta.url)),'--config',configFile,'--session',session,...args],
  {windowsHide:true,env:{PATH:process.env.PATH,SystemRoot:process.env.SystemRoot,TEMP:host.root,TMP:host.root,[envName]:host.credential(),BIND_FAULT:phase??'',BINDING_PATH:config.bindingsFile}});
 try{
  const before=(await hostControlClient(declarativeHostConfig(config)).operate('chain.list',{})).routes.map(row=>row.chain_id);
  await assert.rejects(()=>cli('interrupted-intent',['--create'],'before-created'),error=>error.code===91);
  // The child has exited; releasing only its own known stale lock is safe.
  await unlink(config.bindingsFile+'.lock');const intent=JSON.parse(await readFile(config.bindingsFile+'.create-intent','utf8'));assert.equal(intent.state,'creating');
  const created=(await hostControlClient(declarativeHostConfig(config)).operate('chain.list',{})).routes.map(row=>row.chain_id).filter(id=>!before.includes(id));assert.equal(created.length,1);
  await assert.rejects(()=>bindNativeSession(config,{sessionId:'interrupted-intent',create:true}),/uncertain Core chain creation/);
  await cli('interrupted-intent',['--chain',created[0]]);await assert.rejects(readFile(config.bindingsFile+'.create-intent'),error=>error.code==='ENOENT');
  await assert.rejects(()=>cli('committed-before-cleanup',['--create'],'after-store'),error=>error.code===92);await unlink(config.bindingsFile+'.lock');
  const persisted=await readHostBindings(declarativeHostConfig(config)),chain=persisted.sessions['committed-before-cleanup'].chain_id,count=(await hostControlClient(declarativeHostConfig(config)).operate('chain.list',{})).routes.length;
  const replay=JSON.parse((await cli('committed-before-cleanup',['--create'])).stdout);assert.equal(replay.chain_id,chain);assert.equal(replay.replayed,true);
  await assert.rejects(readFile(config.bindingsFile+'.create-intent'),error=>error.code==='ENOENT');assert.equal((await hostControlClient(declarativeHostConfig(config)).operate('chain.list',{})).routes.length,count);
  await writeFile(config.bindingsFile+'.create-intent','{"broken":');await cli('legacy-corrupt-intent',['--chain',created[0]]);
  await assert.rejects(readFile(config.bindingsFile+'.create-intent'),error=>error.code==='ENOENT');
  await cli('next-session',['--create']);assert.equal((await hostControlClient(declarativeHostConfig(config)).operate('chain.list',{})).routes.length,count+1);
 }finally{delete process.env[envName];await host.close();}
});
