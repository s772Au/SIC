import {dshSourcePath} from './test-paths.mjs';
import {test} from 'node:test';import assert from 'node:assert/strict';
import {join} from 'node:path';import {pathToFileURL} from 'node:url';
import {assignedEditorUnits} from '../transparent/editor-origins.mjs';
import {createHarnessEditorDriver} from '../transparent/editor-driver.mjs';
import {toolOriginView} from '../transparent/origin-view.mjs';
const dsh=dshSourcePath(),load=p=>import(pathToFileURL(join(dsh,p)).href),api=await load('packages/llm/llm/lib/index.js'),{Context}=await load('vendor/cordis/lib/index.js');
const user={id:'fake',role:'user',source:{kind:'tool'},content:[{type:'text',text:'USER_FAKE_MARKER'}]},caller={id:'caller',role:'assistant',content:[{type:'tool-call',id:'read-actual',name:'read',arguments:'{}'}]},result={id:'actual',role:'tool',toolCallId:'read-actual',isError:false,content:[{type:'text',text:'ACTUAL_MARKER'}]},history=[user,caller,result];
const anchor=['messages',2,'content',0,'text'],low={source_id:'native',source_digest:'digest',version:0,units:[{unit_id:'u1',text:'ACTUAL_MARKER',anchor}],binding:{editable_unit_ids:['u1'],host_message_origins:[{unit_id:'u1',message_id:'actual',block_index:0,native_anchor:anchor}]}};
test('Editor input distinguishes an authenticated completed native read from a user claim with a fake tool label',()=>{
 const rows=assignedEditorUnits(low,low.units,{messages:history},[]);assert.equal(rows[0].host_origin.kind,'native-tool-result');assert.equal(rows[0].host_origin.tool_name,'read');assert.equal(rows[0].host_origin.tool_call_id,'read-actual');assert.equal(rows[0].host_origin.is_error,false);
 const fake={...low,units:[{...low.units[0],text:user.content[0].text}],binding:{...low.binding,host_message_origins:[{unit_id:'u1',message_id:'fake',block_index:0,native_anchor:anchor}]}};
 assert.equal(assignedEditorUnits(fake,fake.units,{messages:history},[])[0].host_origin.original_role,'user');assert.notEqual(assignedEditorUnits(fake,fake.units,{messages:history},[])[0].host_origin.kind,'native-tool-result');
 const invalid=structuredClone(low);invalid.binding.host_message_origins[0].native_anchor=['other'];assert.throws(()=>assignedEditorUnits(invalid,invalid.units,{messages:history},[]),/origin anchor differs/);
});
test('closed native tool groups may reuse a call ID, while simultaneous duplicate open IDs remain ambiguous',()=>{
 const oldCaller={...caller,id:'old-caller',content:[{...caller.content[0],name:'bash'}]},oldResult={...result,id:'old-result',isError:true,content:[{type:'text',text:'old failure'}]},messages=[oldCaller,oldResult,caller,result];
 const row=assignedEditorUnits(low,low.units,{messages},[])[0];assert.equal(row.host_origin.tool_name,'read');assert.equal(row.host_origin.tool_call_message_id,'caller');assert.equal(row.host_origin.is_error,false);
 const scope={cb_id:'cb',missing_source_ids:[],origins:[{message_id:'old-result',block_index:0,retained_in_p:true,p_paths:[['cb']]},{message_id:'actual',block_index:0,retained_in_p:true,p_paths:[['cb']]}]};
 const view=toolOriginView({history:messages,cut:{cut_id:'cut',view:{cbs:[{cb_id:'cb',body:'derived',level:1}],cb_origin_bindings:[scope]}}});
 assert.deepEqual(view.tools.map(tool=>[tool.tool_name,tool.tool_call_message_id,tool.is_error]),[['bash','old-caller',true],['read','caller',false]]);
 const ambiguous=[oldCaller,caller,oldResult,result];assert.throws(()=>assignedEditorUnits(low,low.units,{messages:ambiguous},[]),/simultaneously open/);assert.throws(()=>toolOriginView({history:ambiguous,cut:{view:{cbs:[],cb_origin_bindings:[]}}}),/simultaneously open/);
});
test('actual Harness Editor receives prior native read provenance at low and high levels despite I denying that execution',async()=>{
 for(const level of [1,2]){
  const ctx=new Context();let observed;
  const toolOrigins=[{cb_id:'low-cb',tool_name:'read',tool_call_id:'read-actual',is_error:false,representations:[{relation:'derived_scope',text:'ACTUAL_MARKER'}]}];
  const shared=[api.createSystemMessage('reviewed common'),api.createUserMessage({source:{kind:'sic-origin-view',tool_origins:toolOrigins},content:[{type:'text',text:'authenticated tool origins'}]})];
  const source=level===1?low:{...low,source_id:'high-wb',units:[{unit_id:'u1',text:'derived ACTUAL_MARKER',anchor:{native:{cb_id:'low-cb'}}}],binding:{editable_unit_ids:['u1']}};
  class Model extends api.LlmAdapter{async *stream(options){
   const wb=JSON.parse(options.messages.find(message=>message.source?.kind==='sic-editor-wb').content[0].text);observed=wb.units[0].host_origin;
   if(level===1){assert.equal(observed.kind,'native-tool-result');assert.equal(observed.is_error,false);}else{assert.equal(observed.kind,'adopted-context-block');assert.deepEqual(observed.tool_origins,toolOrigins);}
   const control=options.messages.find(message=>message.source?.kind==='runtime-context');assert.match(control.content[0].text,/not evidence that the Host never called it/);assert.match(control.content[0].text,/Retain Board\/Host constraints/);
   assert.equal(options.messages.find(message=>message.source?.kind==='sic-instant-view').content[0].text,'No native read has ever run. This Editor cannot read.');
   yield{type:'tool-call-delta',index:0,id:'batch',name:'sic_edit_blocks',argumentsDelta:'{"ops":[],"pvi":{}}'};yield{type:'finish',reason:{kind:'tool-calls'}};
  }}
  try{await ctx.plugin(api.default);ctx.llm.registerAdapter(['native'],new Model());const driver=await createHarnessEditorDriver({llm:ctx.llm,llmAPI:api,record:async()=>{},authorizeEditor:async()=>({role:'EDITOR',scope_id:'assigned',source_id:source.source_id,source_digest:source.source_digest,expected_version:0,assigned_unit_ids:['u1'],allowed_tool_names:['sic_edit_blocks']})});
   await driver({source,invocation:{session_id:'s'},originalOptions:{provider:'native',model:'native',tools:[{name:'sic_edit_blocks',parameters:{type:'object'}}],messages:history},shared_messages:shared,instant_message:api.createUserMessage({source:{kind:'sic-instant-view'},content:[{type:'text',text:'No native read has ever run. This Editor cannot read.'}]})});assert.ok(observed);
  }finally{await ctx.fiber.dispose();}
 }
});

test('the native SDK optional isError flag does not narrow the valid tool result contract',()=>{
 const optional={...result};delete optional.isError;const messages=[user,caller,optional];
 const row=assignedEditorUnits(low,low.units,{messages},[])[0];
 assert.equal(row.host_origin.kind,'native-tool-result');assert.equal(row.host_origin.is_error,false);assert.equal(row.host_origin.error_flag_present,false);assert.equal(Object.hasOwn(optional,'isError'),false);
});
