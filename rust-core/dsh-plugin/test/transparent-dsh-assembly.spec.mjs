import {dshSourcePath,evidencePath} from './test-paths.mjs';
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';
import {join} from 'node:path';
import {createAdmissionGuard} from '../transparent/prompt.mjs';
import {createTransparentFetch,digest} from '../transparent/transport.mjs';
import {createGoalFacade,installGoalCompatibility} from '../transparent/harness-facade.mjs';

const dsh=dshSourcePath();
const artifacts=process.env.SIC_PROMPT_ARTIFACTS??evidencePath('dsh-sic-transparent-comparison-20261117/adaptation');
const load=p=>import(pathToFileURL(join(dsh,p)).href);
const {Context}=await load('vendor/cordis/lib/index.js');
const {default:SystemPrompt,renderPrompt,renderContextSections}=await load('packages/core/system-prompt/lib/index.js');
const {default:LlmRuntime,LlmAdapter}=await load('packages/llm/llm/lib/index.js');
const {default:ToolRuntime}=await load('packages/core/tools/lib/index.js');
const {createScope}=await load('packages/core/scope/lib/index.js');
const {apply:applyGoalTools}=await load('packages/goal/tool-goal/lib/index.js');

test('real DSH registry assembles all three actual baselines; final guard sees restored complete and refreshed activation',async()=>{
  const tools=JSON.parse(await readFile(join(artifacts,'contracts/native-tools.json'),'utf8'));
  for(const baseline of ['native','append-only','fused']){
    const ctx=new Context();
    try{
      await ctx.plugin(SystemPrompt,{includeHarnessIdentity:false});
      const system=await readFile(join(artifacts,baseline,'system.md'),'utf8');
      ctx.systemPrompt.section({name:'sic:reviewed-complete-fixture',order:0,text:system,interpolate:false,complete:true});
      ctx.systemPrompt.tools(()=>({schemas:tools}));
      let auth={role:'WORKER',scope_id:'fixture-main',revision:0};
      ctx.systemPrompt.context({name:'sic:activation',order:9000,required:true,interpolate:false,text:()=>JSON.stringify(auth)});
      const guard=createAdmissionGuard({system,tools,authorization:()=>auth});
      const accepted=await ctx.systemPrompt.assemble();
      auth={role:'EDITOR',scope_id:'wb-current',revision:1};
      const final=ctx.systemPrompt.refreshContext(accepted);
      assert.equal(guard({renderedSystem:renderPrompt(final),tools:final.tools,contexts:renderContextSections(final)}).role,'EDITOR');
      assert.equal(renderPrompt(final),system);assert.deepEqual(final.tools,tools);
      // A watermark string cannot self-authorize a role; trusted resolver wins.
      assert.throws(()=>guard({renderedSystem:system,tools,contexts:[{name:'sic:activation',text:JSON.stringify({role:'WORKER'})}]}),/activation changed/);
      const mutated=structuredClone(tools);mutated.find(t=>t.name==='read').parameters.required=[];
      assert.throws(()=>guard({renderedSystem:system,tools:mutated,contexts:renderContextSections(final)}),/missing core|schema changed/);
      assert.throws(()=>guard({renderedSystem:system+'\nlate conflicting policy',tools,contexts:renderContextSections(final)}),/system prompt changed/);
    }finally{await ctx.fiber.dispose();}
  }
});

test('actual DSH native goal definitions keep exact schema/output while scoped bodies use only SIC lifecycle',async()=>{
  const ctx=new Context();let calls=0;
  try{
    await ctx.plugin(SystemPrompt);await ctx.plugin(ToolRuntime);applyGoalTools(ctx,{});
    const native=ctx.tools.schemas().filter(t=>['create_goal','get_goal','update_goal'].includes(t.name));
    const key={id:'fixture-agent'},scope=createScope(ctx,key);
    const value={goal:{id:'goal-from-sic',revision:1,objective:'task',phase:'active',roundsStarted:0,maxGoalRounds:10},activation:'armed'};
    const facade=createGoalFacade({resolveGrant:async()=>({scope_id:'native-scope',role:'WORKER',allowed_calls:['get_goal','create_goal','update_goal'],grant_id:'trusted-grant'}),
      operate:async(operation,payload)=>{calls++;assert.equal(operation,'harness.call');assert.equal(payload.name,'create_goal');assert.equal(payload.arguments.objective,'task');return{result:{value}};}});
    await new Promise((resolve,reject)=>scope.ctx.inject(['tools'],child=>{try{installGoalCompatibility({tools:child.tools,scope:key,facade});resolve();}catch(e){reject(e);}}));
    assert.deepEqual(ctx.tools.schemas(key).filter(t=>['create_goal','get_goal','update_goal'].includes(t.name)),native);
    const result=await ctx.tools.execute({agent:key,name:'create_goal',arguments:{objective:'task'},callId:'native-goal-call',signal:new AbortController().signal});
    assert.equal(result.isError,false);assert.equal(calls,1);assert.match(JSON.stringify(result),/goal-from-sic/);
    await scope.dispose();
  }finally{await ctx.fiber.dispose();}
});

test('actual DSH prepared call keeps its original adapter/model while native fetch edits and streams',async()=>{
  const ctx=new Context();
  const trace=[];
  const body={model:'original-model',messages:[{role:'user',content:'before'}],stream:true,n:2,reasoning_signature:'opaque'};
  const wire=createTransparentFetch({publicationRequired:false,
    core:{begin:async()=>({source_id:'src',source_digest:digest(body),version:0,units:[{unit_id:'u1',text:'before',anchor:['messages',0,'content']}]}),
      commit:async()=>({receipt:{version:1},request:{...body,messages:[{role:'user',content:'after'}]}})},
    resolveInvocation:async()=>({actor:'host',session_id:'session',request_id:'request',format:'openai-chat'}),
    selectEdits:async()=>[{unit_id:'u1',expected_text:'before',replacement:'after'}],
    admit:async({request})=>{trace.push('admit');assert.equal(request.model,'original-model');},
    publishProjection:async p=>{trace.push('projection');return{recorded:true,adopted_digest:p.adopted_digest};},
    fetchImpl:async request=>{
      trace.push('dispatch');const sent=await request.json();assert.equal(sent.n,2);assert.equal(sent.stream,true);assert.equal(sent.messages[0].content,'after');
      assert.equal(sent.reasoning_signature,'opaque');
      return new Response('data: {"choices":[{"index":0},{"index":1}]}\n\n',{headers:{'content-type':'text/event-stream'}});
    },
  });
  class ExistingHostAdapter extends LlmAdapter{
    async *stream(options){
      assert.equal(options.provider,'host-route');assert.equal(options.model,'original-model');
      const response=await wire('https://fixture.invalid/provider',{method:'POST',body:JSON.stringify(body),signal:options.signal});
      const chunk=await response.body.getReader().read();
      // Native adapter owns parsing/choice policy; SIC transport did not do it.
      assert.match(new TextDecoder().decode(chunk.value),/"index":1/);
      yield{type:'text-delta',index:0,text:'host selected branch'};
      yield{type:'finish',reason:{kind:'stop'}};
    }
  }
  try{
    await ctx.plugin(LlmRuntime);ctx.llm.registerAdapter(['host-route'],new ExistingHostAdapter());
    const prepared=await ctx.llm.prepareCall({provider:'host-route',model:'original-model'});
    const chunks=[];for await(const c of prepared.stream({provider:'host-route',model:'original-model',messages:[]}))chunks.push(c);
    assert.deepEqual(trace,['admit','projection','dispatch']);assert.equal(chunks[0].text,'host selected branch');
  }finally{await ctx.fiber.dispose();}
});
