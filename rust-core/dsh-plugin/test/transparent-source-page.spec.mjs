import {dshSourcePath} from './test-paths.mjs';
import{test}from'node:test';import assert from'node:assert/strict';import{join}from'node:path';import{pathToFileURL}from'node:url';import{composeEditorDriver}from'../transparent/driver-composition.mjs';
import{protectedWorkerText,readNativeSourcePage}from'../transparent/source-page.mjs';
const dsh=dshSourcePath(),load=p=>import(pathToFileURL(join(dsh,p)).href),api=await load('packages/llm/llm/lib/index.js'),{Context}=await load('vendor/cordis/lib/index.js');
test('denied Editor paging returns a tool error and the same assignment can finish after an authorized read',async()=>{
 for(const requested of [{source_id:'private-other-native',unit_id:'u1'},{source_id:'assigned-native',unit_id:'u2'},{source_id:'core-source-archive',unit_id:'u1'}]){
  const ctx=new Context();let foreignReads=0,round=0;const outcomes=[];
  try{await ctx.plugin(api.default);class Model extends api.LlmAdapter{async*stream(request){
    const index=round++;
    if(index===1){assert.equal(request.messages.at(-1).isError,true);assert.match(request.messages.at(-1).content[0].text,/outside the authenticated read scope/);}
    if(index===2){assert.equal(request.messages.at(-1).isError,false);assert.equal(JSON.parse(request.messages.at(-1).content[0].text).text,'authorized');}
    const args=[requested,{source_id:'assigned-native',unit_id:'u1'},{ops:[],pvi:{}}][index];
    yield{type:'tool-call-delta',index:0,id:'page-'+index,name:index===2?'sic_edit_blocks':'sic_source_text_page',argumentsDelta:JSON.stringify(args)};
    yield{type:'finish',reason:{kind:'tool-calls'}};
   }}ctx.llm.registerAdapter(['original'],new Model());
   const source={source_id:'assigned-native',source_digest:'digest',version:0,binding:{editable_unit_ids:['u1']},units:[{unit_id:'u1',text:'authorized',anchor:['messages',0,'content',0,'text']},{unit_id:'u2',text:'private branch',anchor:['messages',0,'content',1,'text']}],source_request:{messages:[{role:'user',content:[{type:'text',text:'authorized'},{type:'text',text:'private branch'}]}]}},core={get:async p=>{if(p.source_id!==source.source_id)foreignReads++;return source;}};
   const driver=await composeEditorDriver({ctx:{llm:ctx.llm,sessions:{get:()=>({id:'s'})}},cfg:{llmAPI:api},store:{recordTrace:async(_session,_source,event)=>{if(event.stage==='editor-tool-result')outcomes.push(event.result.isError);}},recordSession:async()=>{},clientFor:()=>core,bindingFor:()=>({chain_id:'chain',scope_id:'scope'})});
   assert.deepEqual(await driver({source,invocation:{session_id:'s'},originalOptions:{provider:'original',model:'m',tools:[{name:'sic_source_text_page',parameters:{}},{name:'sic_edit_blocks',parameters:{}}],messages:[]},shared_messages:[]}),{ops:[],pvi:{}});
   assert.deepEqual(outcomes,[true,false]);assert.equal(foreignReads,0);
  }finally{await ctx.fiber.dispose();}
 }
});

test('authorized source pages count Unicode scalars without splitting an emoji',async()=>{
 const source={source_digest:'digest',version:0,binding:{editable_unit_ids:['u1']},source_request:{text:'A😀中文'},units:[{unit_id:'u1',anchor:['text']}]};
 const read=(offset,limit)=>readNativeSourcePage({core:{get:async()=>source},authority:{source_id:'native',assigned_unit_ids:['u1'],source_digest:'digest',expected_version:0},args:{source_id:'native',unit_id:'u1',offset,limit}});
 assert.deepEqual(await read(1,1),{source_id:'native',unit_id:'u1',offset:1,text:'😀',next_offset:2});
 assert.equal((await read(2,2)).text,'中文');assert.equal((await read(2,2)).next_offset,null);
});

test('a queued user message does not consume the page and the next Worker response releases it',()=>{
 const ask={id:'q',role:'user',source:{kind:'user'},content:[]};
 const post={id:'followup',role:'user',source:{kind:'user'},content:[]};
 const request={id:'request',role:'assistant',source:{kind:'model'},content:[
   {type:'tool-call',name:'sic_source_text_page',id:'p1'},{type:'tool-call',name:'sic_hierarchy_expand',id:'p2'}]};
 const page={id:'page',role:'tool',toolCallId:'p1',isError:false,content:[]};
 const denial={id:'denial',role:'tool',toolCallId:'p2',isError:true,content:[]};
 const history=[ask,request,page,post,denial];
 assert.deepEqual([...protectedWorkerText(history,true)],['followup','page','denial']);
 const consumed={id:'worker-continuation',role:'assistant',source:{kind:'model'},content:[]};
 assert.deepEqual([...protectedWorkerText([...history,consumed],true)],['followup']);
});

test('real Editor dispatch reads shared fixed operations and has no Worker archive privileges',async()=>{
 const ctx=new Context();let calls=0,round=0;
 const source={source_id:'assigned-native',source_digest:'digest',version:0,binding:{editable_unit_ids:['u1']},
   units:[{unit_id:'u1',text:'editable',anchor:['messages',0,'content',0,'text']}],
   source_request:{messages:[{role:'user',content:[{type:'text',text:'editable'}]}]}};
 const shared=api.createUserMessage({source:{kind:'sic-cb',cb_id:'shared-cb'},content:[{type:'text',text:'fixed background'}]});
 try{
  await ctx.plugin(api.default);
  class Model extends api.LlmAdapter{async*stream(request){
   const index=round++;
   const runtime=request.messages.find(message=>message.source?.kind==='runtime-context');
   assert.doesNotMatch(runtime.content[0].text,/Worker-only navigation text/);
   const trusted=JSON.parse(runtime.source.sections.find(section=>section.name==='sic:activation').text);
   assert.deepEqual(trusted.read_only_cb_ids,['shared-cb']);
   assert.deepEqual(trusted.assigned_unit_ids,['u1']);
   if(index===1){
    const denied=request.messages.at(-1).content[0].text;
    assert.match(denied,/outside the shared CB background.*read_only_cb_ids/);
    assert.doesNotMatch(denied,/assigned unit ID/);
   }
   if(index===2)assert.equal(JSON.parse(request.messages.at(-1).content[0].text).value,'[{"op":"KEEP"}]');
   const args=index===2?{ops:[],pvi:{}}:{kind:'ops',object_id:index===0?'private-cb':'shared-cb'};
   yield{type:'tool-call-delta',index:0,id:'read-ops-'+index,name:index===2?'sic_edit_blocks':'sic_memory_page',argumentsDelta:JSON.stringify(args)};
   yield{type:'finish',reason:{kind:'tool-calls'}};
  }}
  ctx.llm.registerAdapter(['original'],new Model());
  const core={get:async()=>source,operate:async(operation,args)=>{
   calls++;assert.equal(operation,'memory.archive_ops');assert.deepEqual(args,{kind:'ops',object_id:'shared-cb'});
   return {value:'[{"op":"KEEP"}]'};
  }};
  const driver=await composeEditorDriver({ctx:{llm:ctx.llm,sessions:{get:()=>({id:'s'})}},cfg:{llmAPI:api},
   store:{recordTrace:async()=>{}},recordSession:async()=>{},clientFor:()=>core,bindingFor:()=>({chain_id:'chain',scope_id:'scope'})});
  const tools=['sic_edit_blocks','sic_memory_page'].map(name=>({name,parameters:{}}));
  const delivery=await driver({source,invocation:{session_id:'s'},
   originalOptions:{provider:'original',model:'m',tools,messages:[]},shared_messages:[shared],
   runtime_sections:[{name:'sic:archive-text',text:'Worker-only navigation text'}]});
  assert.deepEqual(delivery,{ops:[],pvi:{}});assert.equal(calls,1);assert.equal(round,3);
 }finally{await ctx.fiber.dispose();}
});
