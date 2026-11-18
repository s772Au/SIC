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
    class Adapter extends api.LlmAdapter{async *stream(){
      dispatches++;
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
      assert.equal(dispatches,1);assert.equal(getEditorInvocation(),undefined);
    }finally{await ctx.fiber.dispose();}
  }
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
    await assert.rejects(()=>driver(assignment()),/unauthorized native tool/);assert.equal(dispatches,1);
    const aborted=assignment();const controller=new AbortController();controller.abort();aborted.signal=controller.signal;
    await assert.rejects(()=>driver(aborted),/abort/i);assert.equal(dispatches,1);
    assert.equal(getEditorInvocation(),undefined);
    await assert.rejects(()=>driver({...assignment(),shared_messages:undefined}),/actual shared message projection/);
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
