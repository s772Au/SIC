import {dshSourcePath} from './test-paths.mjs';
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {pathToFileURL} from 'node:url';
import {join} from 'node:path';
import {setTimeout as wait} from 'node:timers/promises';
import {createHarnessEditorDriver} from '../transparent/editor-driver.mjs';
import {getEditorInvocation} from '../transparent/editor-invocation.mjs';
const dsh=dshSourcePath();
const api=await import(pathToFileURL(join(dsh,'packages/llm/llm/lib/index.js')).href);
const {Context}=await import(pathToFileURL(join(dsh,'vendor/cordis/lib/index.js')).href);
const options={provider:'same-host-route',model:'same-host-model',temperature:0.2,maxTokens:300,stop:['END'],tools:[
  {name:'read',description:'Native read',parameters:{type:'object',properties:{file_path:{type:'string'}},required:['file_path']}},
  {name:'sic_edit_blocks',description:'Deliver edits',parameters:{type:'object',properties:{ops:{type:'array'},pvi:{type:'object'}},required:['ops','pvi']}},
],messages:[],vendor_opaque:{keep:true}};
const source={source_id:'s1',source_digest:'digest-1',version:0,binding:{editable_unit_ids:['u2']},units:[
  {unit_id:'u1',text:'published CB',anchor:['messages',0,'content']},{unit_id:'u2',text:'new WB',anchor:['messages',1,'content']},
]};
const auth={role:'EDITOR',scope_id:'wb-s1',source_id:'s1',source_digest:'digest-1',expected_version:0,assigned_unit_ids:['u2'],allowed_tool_names:['read','sic_edit_blocks']};
const shared=[{id:'system-id',role:'system',source:{kind:'system-prompt'},content:[{type:'text',text:'FROZEN FUSED SYSTEM'}]},
  {id:'cb-id',role:'user',source:{kind:'sic-cb'},content:[{type:'text',text:'published CB'}]}];
const assignment=()=>({source,invocation:{session_id:'s',request_id:'r'},originalOptions:options,shared_messages:shared,signal:new AbortController().signal});

test('actual Harness prepared runtime and assembler execute Editor native read then deliver while preserving shared prefix/route',async()=>{
  const ctx=new Context(),requests=[],trace=[];
  class Adapter extends api.LlmAdapter{
    async *stream(request){
      requests.push(request);
      assert.equal(getEditorInvocation()?.role,'EDITOR');
      getEditorInvocation().verifyNativeOptions(request);
      assert.throws(()=>getEditorInvocation().verifyNativeOptions({...request,tools:[]}),/final native request changed/);
      assert.equal(request.provider,options.provider);assert.equal(request.model,options.model);
      assert.equal(request.maxTokens,options.maxTokens);assert.deepEqual(request.stop,options.stop);
      assert.deepEqual(request.tools,options.tools);assert.deepEqual(request.vendor_opaque,{keep:true});
      assert.deepEqual(request.messages.slice(0,2),shared);
      const material=JSON.parse(request.messages.find(m=>m.source?.kind==='sic-editor-wb').content[0].text);
      assert.deepEqual(material.units.map(u=>u.unit_id),['u2']);
      const control=request.messages.find(m=>m.source?.kind==='runtime-context');
      assert.equal(JSON.parse(control.source.sections.find(s=>s.name==='sic:activation').text).role,'EDITOR');
      assert.match(control.content[0].text,/authenticated role for this invocation is EDITOR/);
      const name=requests.length===1?'read':'sic_edit_blocks';
      const args=name==='read'?{file_path:'/sic/projection/reference.txt'}:{ops:[{op:'COMPRESS',target:'u2',content:'condensed'}],pvi:{}};
      yield{type:'tool-call-delta',index:0,id:'call-'+requests.length,name,argumentsDelta:JSON.stringify(args)};
      yield{type:'finish',reason:{kind:'tool-calls'}};
    }
  }
  try{
    await ctx.plugin(api.default);ctx.llm.registerAdapter([options.provider],new Adapter());
    const driver=await createHarnessEditorDriver({llm:ctx.llm,llmAPI:api,authorizeEditor:async()=>auth,record:async event=>{trace.push(event.stage);},
      executeNativeTool:async call=>{assert.equal(call.name,'read');assert.equal(call.arguments.file_path,'/sic/projection/reference.txt');return{isError:false,content:[{type:'text',text:'exact reference'}]};}});
    assert.deepEqual(await driver(assignment()),{ops:[{op:'COMPRESS',target:'u2',content:'condensed'}],pvi:{}});
    assert.equal(requests.length,2);assert.equal(requests[1].messages.at(-1).role,'tool');
    assert.equal(getEditorInvocation(),undefined);assert.ok(trace.includes('editor-tool-result'));
    assert.deepEqual(source.units[0].text,'published CB');
  }finally{await ctx.fiber.dispose();}
});

test('Editor subset delivery and mutable or revoked host authorization fail closed in real Harness calls',async()=>{
  for(const mode of ['subset','revoke-in-stream','mutate-after-tool']){
    const ctx=new Context(),current=structuredClone(auth),candidate=structuredClone(source);let dispatches=0;
    candidate.binding.editable_unit_ids.push('u3');candidate.units.push({unit_id:'u3',text:'another Editor unit',anchor:['messages',2,'content']});
    class Adapter extends api.LlmAdapter{async *stream(request){
      dispatches++;
      if(mode==='subset'&&dispatches===2){
        assert.equal(request.messages.at(-1).isError,true);
        assert.match(request.messages.at(-1).content[0].text,/outside assigned units/);
      }
      if(mode==='revoke-in-stream')current.assigned_unit_ids=['u3'];
      const name=mode==='mutate-after-tool'?'read':'sic_edit_blocks';
      const args=name==='read'?{file_path:'/sic/reference'}:{ops:[{op:'REPLACE',target:mode==='subset'?'u3':'u2',content:'changed'}],pvi:{}};
      yield{type:'tool-call-delta',index:0,id:'scoped-call',name,argumentsDelta:JSON.stringify(args)};
      yield{type:'finish',reason:{kind:'tool-calls'}};
    }}
    try{
      await ctx.plugin(api.default);ctx.llm.registerAdapter([options.provider],new Adapter());
      const driver=await createHarnessEditorDriver({llm:ctx.llm,llmAPI:api,authorizeEditor:async()=>current,record:async()=>{},
        executeNativeTool:async()=>{current.allowed_tool_names.push('bash');return{isError:false,content:[{type:'text',text:'observed'}]};}});
      await assert.rejects(()=>driver({...assignment(),source:candidate}),mode==='subset'?/outside assigned units/:/authority changed/);
      assert.equal(dispatches,mode==='subset'?2:1);assert.equal(getEditorInvocation(),undefined);
    }finally{await ctx.fiber.dispose();}
  }
});

test('Editor ignores archived decision writes while preserving other edits and the original response',async()=>{
  const ctx=new Context(),trace=[];
  const views={'Working on':['Retain this lead']};
  const raw={ops:[{op:'COMPRESS',target:'u2',content:'condensed'}],pvi:{P:['persistent'],I:{
    context_view:'keep this view',decision_tree:{views,revision:76,discovery_prompt:'model proposed text',event_ids:['fake receipt']}}}};
  let proposed=raw;
  class Adapter extends api.LlmAdapter{
    async *stream(request){
      assert.deepEqual(request.messages.slice(0,shared.length),shared);
      const control=request.messages.find(item=>item.source?.kind==='runtime-context');
      assert.doesNotMatch(control.content[0].text,/decision_tree|discovery_prompt/);
      yield{type:'tool-call-delta',index:0,id:'decision-delivery',name:'sic_edit_blocks',argumentsDelta:JSON.stringify(proposed)};
      yield{type:'finish',reason:{kind:'tool-calls'}};
    }
  }
  try{
    await ctx.plugin(api.default);ctx.llm.registerAdapter([options.provider],new Adapter());
    const driver=await createHarnessEditorDriver({llm:ctx.llm,llmAPI:api,
      authorizeEditor:async()=>({...auth,edit_capabilities:{ops:['COMPRESS'],pvi:['I','P']}}),record:async event=>trace.push(event)});
    const actual=await driver(assignment());
    const accepted={...raw,pvi:{...raw.pvi,I:{context_view:'keep this view'}}};
    assert.deepEqual(actual,accepted);
    const response=trace.find(event=>event.stage==='editor-response').message.content.find(block=>block.type==='tool-call');
    assert.deepEqual(JSON.parse(response.arguments),raw);
    proposed=structuredClone(raw);
    proposed.pvi.I.decision_tree.unknown='legacy field';
    proposed.ops.push({op:'FLAG',target:'u2',content:'legacy',lifecycle:'I',view_key:'decision_tree'});
    assert.deepEqual(await driver(assignment()),accepted);
    proposed={ops:raw.ops};
    assert.deepEqual(await driver(assignment()),{ops:raw.ops,pvi:{}});
    proposed={ops:raw.ops,pvi:{status:'Need to retain this view.'}};
    assert.deepEqual(await driver(assignment()),{ops:raw.ops,pvi:{I:{status:'Need to retain this view.'}}});
    proposed={ops:raw.ops,pvi:{P:raw.pvi.P,I:{decision_tree:{views}},status:'Retain the instant status.'}};
    assert.deepEqual(await driver(assignment()),{ops:raw.ops,pvi:{P:raw.pvi.P,I:{status:'Retain the instant status.'}}});
  }finally{await ctx.fiber.dispose();}
});

test('Editor scope mismatch, unauthorized task tool and interrupted stream cannot produce a delivered batch',async()=>{
  const ctx=new Context();let dispatches=0;
  class Adapter extends api.LlmAdapter{async *stream(){dispatches++;yield{type:'tool-call-delta',index:0,id:'bad',name:'bash',argumentsDelta:'{"command":"do material task"}'};yield{type:'finish',reason:{kind:'tool-calls'}};}}
  try{
    await ctx.plugin(api.default);ctx.llm.registerAdapter([options.provider],new Adapter());
    const factory=authorizeEditor=>createHarnessEditorDriver({llm:ctx.llm,llmAPI:api,authorizeEditor,record:async()=>{}});
    const badScope=await factory(async()=>({...auth,assigned_unit_ids:['u1']}));
    await assert.rejects(()=>badScope(assignment()),/persistent native scope/);assert.equal(dispatches,0);
    const driver=await factory(async()=>auth);
    await assert.rejects(()=>driver(assignment()),/unauthorized native tool/);assert.equal(dispatches,2);
    const aborted=assignment();const controller=new AbortController();controller.abort();aborted.signal=controller.signal;
    await assert.rejects(()=>driver(aborted),/abort/i);assert.equal(dispatches,2);
    assert.equal(getEditorInvocation(),undefined);
    await assert.rejects(()=>driver({...assignment(),shared_messages:undefined}),/actual shared message projection/);
  }finally{await ctx.fiber.dispose();}
});

test('Editor retries one provider output exhaustion on the same assignment and budget',async()=>{
  const ctx=new Context();
  const requests=[];
  let exhaustAgain=false;
  const edit={ops:[{op:'COMPRESS',target:'u2',content:'condensed'}],pvi:{}};
  class Adapter extends api.LlmAdapter{
    async *stream(request){
      requests.push(request);
      if(requests.length===1||exhaustAgain){
        yield{type:'finish',reason:{kind:'max-tokens'}};
        return;
      }
      yield{type:'tool-call-delta',index:0,id:'complete-batch',name:'sic_edit_blocks',argumentsDelta:JSON.stringify(edit)};
      yield{type:'finish',reason:{kind:'tool-calls'}};
    }
  }
  try{
    await ctx.plugin(api.default);
    ctx.llm.registerAdapter([options.provider],new Adapter());
    const driver=await createHarnessEditorDriver({llm:ctx.llm,llmAPI:api,authorizeEditor:async()=>auth,record:async()=>{}});
    assert.deepEqual(await driver(assignment()),edit);
    assert.equal(requests.length,2);
    assert.deepEqual(requests[1].messages.slice(0,-1),requests[0].messages);
    assert.equal(requests[1].messages.at(-1).source.sections[0].name,'sic:output-recovery');
    assert.deepEqual(requests.map(request=>request.maxTokens),[options.maxTokens,options.maxTokens]);
    requests.length=0;
    exhaustAgain=true;
    await assert.rejects(driver(assignment()),/Editor exhausted the native output allowance/);
    assert.equal(requests.length,2);
  }finally{await ctx.fiber.dispose();}
});

test('Editor preserves the archived provider stream error without requesting a delivery correction',async()=>{
  const ctx=new Context(),responses=[];
  const failure={message:'[code=upstream_error] The request could not be completed.',code:'PI_AI_ERROR'};
  let calls=0;
  class Adapter extends api.LlmAdapter{
    async *stream(){calls++;yield{type:'finish',reason:{kind:'error',failure}};}
  }
  try{
    await ctx.plugin(api.default);ctx.llm.registerAdapter([options.provider],new Adapter());
    const driver=await createHarnessEditorDriver({llm:ctx.llm,llmAPI:api,authorizeEditor:async()=>auth,
      record:async event=>{if(event.stage==='editor-response')responses.push(event.finish);}});
    await assert.rejects(()=>driver(assignment()),error=>error instanceof api.LlmError&&error.failure.code===failure.code&&error.failure.message===failure.message);
    assert.deepEqual(responses,[{kind:'error',failure}]);
    assert.equal(calls,1);
  }finally{await ctx.fiber.dispose();}
});

test('pressure Editor references exact shared CB text once and corrects a no-op before returning a shorter edit',async()=>{
  const ctx=new Context(),requests=[];
  const unit={unit_id:'u2',text:'published CB',anchor:{native:{cb_id:'cb1'}}};
  const prefix=[shared[0],{...shared[1],source:{...shared[1].source,cb_id:'cb1'}}];
  const compressed={ops:[{op:'COMPRESS',target:'u2',content:'short'}],pvi:{}};
  class Adapter extends api.LlmAdapter{
    async *stream(request){
      assert.deepEqual(request.messages.slice(0,2),prefix);
      const wb=request.messages.find(message=>message.source?.kind==='sic-editor-wb');
      const [assigned]=JSON.parse(wb.content[0].text).units;
      assert.deepEqual(assigned.text_ref,{kind:'shared-cb',cb_id:'cb1',ordinal:1});
      assert.equal(Object.hasOwn(assigned,'text'),false);
      requests.push(request);
      if(requests.length===2){
        assert.equal(request.messages.at(-1).isError,true);
        assert.match(request.messages.at(-1).content[0].text,/did not shorten/);
      }
      yield{type:'tool-call-delta',index:0,id:'pressure-'+requests.length,name:'sic_edit_blocks',
        argumentsDelta:JSON.stringify(requests.length===1?{ops:[],pvi:{}}:compressed)};
      yield{type:'finish',reason:{kind:'tool-calls'}};
    }
  }
  try{
    await ctx.plugin(api.default);ctx.llm.registerAdapter([options.provider],new Adapter());
    const driver=await createHarnessEditorDriver({llm:ctx.llm,llmAPI:api,authorizeEditor:async()=>auth,record:async()=>{}});
    assert.deepEqual(await driver({...assignment(),source:{...source,units:[unit]},shared_messages:prefix,capacity_pressure:true}),compressed);
    assert.equal(requests.length,2);
    assert.deepEqual(await driver({...assignment(),source:{...source,units:[unit]},shared_messages:prefix,reference_shared:true}),compressed);
    assert.equal(requests.length,3);
    assert.ok(requests[2].messages.some(message=>message.source?.sections?.some(section=>section.name==='sic:shared-source')));
  }finally{await ctx.fiber.dispose();}
});

test('host Editor bounds tools and cancels an active native stream without delivering a partial edit',async()=>{
  for(const mode of ['rounds','deadline']){
    const ctx=new Context();let dispatched=0,executed=0,reason;
    class Adapter extends api.LlmAdapter{async *stream(request){
      dispatched++;
      if(mode==='deadline'){
        await wait(60000,null,{signal:request.signal}).catch(()=>{reason=request.signal.reason;});
        request.signal.throwIfAborted();
      }
      yield{type:'tool-call-delta',index:0,id:'read-'+dispatched,name:'read',argumentsDelta:'{"file_path":"owned"}'};
      yield{type:'finish',reason:{kind:'tool-calls'}};
    }}
    try{
      await ctx.plugin(api.default);ctx.llm.registerAdapter([options.provider],new Adapter());
      const driver=await createHarnessEditorDriver({llm:ctx.llm,llmAPI:api,authorizeEditor:async()=>auth,record:async()=>{},
        limits:{maxToolRounds:1,timeoutMs:mode==='deadline'?100:10000},executeNativeTool:async()=>{
          executed++;return{isError:false,content:[{type:'text',text:'owned'}]};
        }});
      await assert.rejects(()=>driver(assignment()),mode==='rounds'?/tool round limit/:error=>error?.name==='TimeoutError');
      assert.equal(dispatched,mode==='rounds'?2:1);
      assert.equal(executed,mode==='rounds'?1:0);
      if(mode==='deadline')assert.equal(reason?.name,'TimeoutError');
    }finally{await ctx.fiber.dispose();}
  }
});

test('host Editor can inherit cancellation for a full-capacity long task',async()=>{
  const ctx=new Context(),controller=new AbortController(),entered=Promise.withResolvers();
  const cancellation=new Error('long task cancelled by host');
  class Adapter extends api.LlmAdapter{
    async resolveModel(provider,id){
      return{provider,id,name:id,context:{contextWindow:1000000},defaultMaxTokens:393216,
        reasoning:{efforts:[{id:'max',name:'Max'}]}};
    }
    async *stream(request){
      assert.equal(request.maxTokens,393216);
      assert.equal(request.reasoningEffort,'max');
      entered.resolve();
      await wait(60000,null,{signal:request.signal}).catch(()=>{});
      request.signal.throwIfAborted();
    }
  }
  try{
    await ctx.plugin(api.default);ctx.llm.registerAdapter([options.provider],new Adapter());
    const driver=await createHarnessEditorDriver({llm:ctx.llm,llmAPI:api,authorizeEditor:async()=>auth,
      record:async()=>{},limits:{timeoutMs:null}});
    const pending=driver({...assignment(),signal:controller.signal,
      originalOptions:{...options,maxTokens:393216,reasoningEffort:'max'}});
    const stopped=assert.rejects(pending,error=>error===cancellation);
    await Promise.race([entered.promise,stopped.then(()=>assert.fail('native Editor did not dispatch'))]);
    controller.abort(cancellation);
    await stopped;
  }finally{
    controller.abort(cancellation);
    await ctx.fiber.dispose();
  }
});
