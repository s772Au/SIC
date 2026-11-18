import {dshSourcePath} from './test-paths.mjs';
import{test}from'node:test';import assert from'node:assert/strict';import{join}from'node:path';import{pathToFileURL}from'node:url';import{composeEditorDriver}from'../transparent/driver-composition.mjs';
import{readNativeSourcePage}from'../transparent/source-page.mjs';
const dsh=dshSourcePath(),load=p=>import(pathToFileURL(join(dsh,p)).href),api=await load('packages/llm/llm/lib/index.js'),{Context}=await load('vendor/cordis/lib/index.js');
test('actual default Harness Editor cannot page another candidate, unassigned unit or a Core-source ID as native archive',async()=>{
 for(const requested of [{source_id:'private-other-native',unit_id:'u1'},{source_id:'assigned-native',unit_id:'u2'},{source_id:'core-source-archive',unit_id:'u1'}]){
  const ctx=new Context();let foreignReads=0;
  try{await ctx.plugin(api.default);class Model extends api.LlmAdapter{async*stream(){yield{type:'tool-call-delta',index:0,id:'page',name:'sic_source_text_page',argumentsDelta:JSON.stringify(requested)};yield{type:'finish',reason:{kind:'tool-calls'}};}}ctx.llm.registerAdapter(['original'],new Model());
   const source={source_id:'assigned-native',source_digest:'digest',version:0,binding:{editable_unit_ids:['u1']},units:[{unit_id:'u1',text:'authorized',anchor:['messages',0,'content',0,'text']},{unit_id:'u2',text:'private branch',anchor:['messages',0,'content',1,'text']}],source_request:{messages:[{role:'user',content:[{type:'text',text:'authorized'},{type:'text',text:'private branch'}]}]}},core={get:async p=>{if(p.source_id!==source.source_id)foreignReads++;return source;}};
   const driver=await composeEditorDriver({ctx:{llm:ctx.llm,sessions:{get:()=>({id:'s'})}},cfg:{llmAPI:api},store:{recordTrace:async()=>{}},recordSession:async()=>{},clientFor:()=>core,bindingFor:()=>({chain_id:'chain',scope_id:'scope'})});
   await assert.rejects(driver({source,invocation:{session_id:'s'},originalOptions:{provider:'original',model:'m',tools:[{name:'sic_source_text_page',parameters:{}},{name:'sic_edit_blocks',parameters:{}}],messages:[]},shared_messages:[]}),/outside the authenticated read scope/);assert.equal(foreignReads,0);
  }finally{await ctx.fiber.dispose();}
 }
});

test('authorized source pages count Unicode scalars without splitting an emoji',async()=>{
 const source={source_digest:'digest',version:0,binding:{editable_unit_ids:['u1']},source_request:{text:'A😀中文'},units:[{unit_id:'u1',anchor:['text']}]};
 const read=(offset,limit)=>readNativeSourcePage({core:{get:async()=>source},authority:{source_id:'native',assigned_unit_ids:['u1'],source_digest:'digest',expected_version:0},args:{source_id:'native',unit_id:'u1',offset,limit}});
 assert.deepEqual(await read(1,1),{source_id:'native',unit_id:'u1',offset:1,text:'😀',next_offset:2});
 assert.equal((await read(2,2)).text,'中文');assert.equal((await read(2,2)).next_offset,null);
});
