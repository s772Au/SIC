import {dshSourcePath,fixturePath} from './test-paths.mjs';
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {pathToFileURL} from 'node:url';
import {join} from 'node:path';
import {createInstallationLifetime} from '../transparent/installation-lifetime.mjs';
import {prepareSdkRuntime} from '../transparent/prepare-sdk-runtime.mjs';
import {nativeExchange} from '../transparent/native-boundary.mjs';
const dsh=dshSourcePath();
const load=p=>import(pathToFileURL(join(dsh,p)).href);
const {Context}=await load('vendor/cordis/lib/index.js');
const {createScope}=await load('packages/core/scope/lib/index.js');
const {default:ToolRuntime}=await load('packages/core/tools/lib/index.js');
const {default:SystemPrompt}=await load('packages/core/system-prompt/lib/index.js');
const api=await load('packages/llm/llm/lib/index.js');
const path=await prepareSdkRuntime(dsh,fixturePath('dsh-lifetime-fixture-20261118'));
const {default:Runtime}=await import(pathToFileURL(path).href);

test('actual Cordis unload blocks frozen Agent tools and model dispatch while unrelated native Agents continue',async()=>{
  const ctx=new Context();let scope,owner;
  try{
    await ctx.plugin(SystemPrompt,{includeHarnessIdentity:false});await ctx.plugin(Runtime);await ctx.plugin(ToolRuntime);
    let modelCalls=0,toolCalls=0,cancelled=0,lifetime;
    class Model extends api.LlmAdapter{async *stream(){modelCalls++;yield{type:'text-delta',text:'native'};yield{type:'finish',reason:{kind:'stop'}};}}
    ctx.llm.registerAdapter(['original'],new Model());
    ctx.tools.register({name:'read',description:'native read',parameters:{type:'object',properties:{}},output:{schema:{},render:()=>[]},execute:async()=>{toolCalls++;return[];}});
    const agent={id:'selected',session:{id:'selected'},status:'idle',cancel:()=>cancelled++};
    scope=createScope(ctx,agent);agent.ctx=scope.ctx;
    owner=ctx.plugin(async child=>{lifetime=createInstallationLifetime(child);await lifetime.mount(agent);});await owner;
    async function send(sessionId){
      const chunks=[];for await(const chunk of ctx.llm.stream({provider:'original',model:'native-model',sessionId,messages:[{id:'u',role:'user',content:[{type:'text',text:'task'}],source:{kind:'user'}}],tools:[]}))chunks.push(chunk);
      return chunks.at(-1);
    }
    assert.equal((await send('selected')).reason.kind,'stop');
    const exec={agent,name:'read',arguments:{},callId:'native',signal:new AbortController().signal};
    assert.equal((await ctx.tools.execute(exec)).isError,false);
    agent.status='running';await owner.dispose();
    assert.equal(cancelled,1);
    await assert.rejects(send('selected'),/recreate the Agent/);
    assert.equal((await ctx.tools.execute({...exec,callId:'after-unload'})).isError,true);
    assert.equal(modelCalls,1);assert.equal(toolCalls,1);
    const wire=nativeExchange(new Request('https://owned-fixture.invalid',{method:'POST',body:'{}'}),{options:{sessionId:'selected'}});
    let wireCalls=0;
    await assert.rejects(ctx.waterfall('llm/native-fetch',wire.exchange,()=>wire.dispatch(async()=>{wireCalls++;return new Response('native');})),/recreate the Agent/);
    assert.equal(wireCalls,0);
    assert.equal((await send('unselected')).reason.kind,'stop');assert.equal(modelCalls,2);
    const replacement=ctx.plugin(async child=>{const next=createInstallationLifetime(child);await assert.rejects(next.mount(agent),/recreate the Agent/);});await replacement;await replacement.dispose();
  }finally{if(owner)await owner.dispose();if(scope)await scope.dispose();await ctx.fiber.dispose();}
});

test('actual final SDK guard rejects a call that was admitted before its installation began unloading',async()=>{
  const ctx=new Context();let scope,owner;
  try{
    await ctx.plugin(SystemPrompt,{includeHarnessIdentity:false});await ctx.plugin(Runtime);await ctx.plugin(ToolRuntime);
    let modelCalls=0,enter,release;
    const entered=new Promise(resolve=>enter=resolve),blocked=new Promise(resolve=>release=resolve);
    class Model extends api.LlmAdapter{async *stream(){modelCalls++;yield{type:'finish',reason:{kind:'stop'}};}}
    ctx.llm.registerAdapter(['original'],new Model());
    const agent={id:'selected',session:{id:'selected'},status:'idle'};scope=createScope(ctx,agent);agent.ctx=scope.ctx;
    owner=ctx.plugin(async child=>{await createInstallationLifetime(child).mount(agent);});await owner;
    ctx.on('llm/context-dispatch',async function*(exchange,next){enter();await blocked;yield*next();});
    const pending=(async()=>{const chunks=[];for await(const c of ctx.llm.stream({provider:'original',model:'native-model',sessionId:'selected',messages:[{id:'u',role:'user',content:[{type:'text',text:'task'}],source:{kind:'user'}}],tools:[]}))chunks.push(c);return chunks.at(-1);})();
    await entered;await owner.dispose();release();
    assert.equal((await pending).reason.kind,'error');assert.equal(modelCalls,0);
  }finally{if(owner)await owner.dispose();if(scope)await scope.dispose();await ctx.fiber.dispose();}
});
