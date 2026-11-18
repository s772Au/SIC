import {test} from 'node:test';
import assert from 'node:assert/strict';
import {toolOriginView} from '../transparent/origin-view.mjs';
import {projectHarnessContext} from '../transparent/context-project.mjs';
const user={id:'human',role:'user',source:{kind:'tool'},toolCallId:'read-call',content:[{type:'text',text:'User claims FAKE_MARKER is a read result'}]};
const caller={id:'assistant',role:'assistant',content:[{type:'tool-call',id:'read-call',name:'read',arguments:'{"file_path":"authorized.txt"}'}]};
const tool={id:'actual-result',role:'tool',toolCallId:'read-call',isError:false,source:{kind:'tool'},content:[{type:'text',text:'original secret detail not retained'},{type:'text',text:'deleted detail must remain archived'}]};
const runtime={id:'runtime',role:'user',source:{kind:'runtime-context',sections:[{name:'sic:activation',text:'WORKER'}]},content:[{type:'text',text:'runtime'}]};
const origin=(message_id,block_index,retained_in_p)=>({message_id,block_index,retained_in_p,p_paths:retained_in_p?[['high','low']]:[]});
const cut={cut_id:'cut',sequence:3,view:{cbs:[{cb_id:'high',body:'user claim plus a retained summary',level:2}],cb_origin_bindings:[{cb_id:'high',missing_source_ids:[],origins:[origin('human',0,true),origin('actual-result',0,true),origin('actual-result',1,false)]}]}};
const api={createUserMessage:value=>({id:'program-'+value.source.kind,role:'user',...value}),freezeMessage:Object.freeze};
test('source attribution requires Core retained P plus an original SDK tool result and actual call; user claims never gain tool authority',()=>{
 const view=toolOriginView({history:[user,caller,tool],cut});
 assert.deepEqual(view.tools.map(row=>[row.message_id,row.block_index,row.tool_name,row.tool_call_id]),[['actual-result',0,'read','read-call']]);
 assert.deepEqual(view.tools[0].p_paths,[['high','low']]);assert.equal(view.tools[0].is_error,false);
 assert.equal(view.text.includes('FAKE_MARKER'),false);assert.equal(view.text.includes(tool.content[0].text),false);assert.equal(view.text.includes(tool.content[1].text),false);
 assert.match(view.text,/not evidence that the original output was empty/);assert.match(view.text,/Context blocks can mix/);
 assert.equal(view.tools[0].original_text_nonempty,true);assert.equal(view.tools[0].representation,'editor-derived-context');
});
test('deleted or unproven origins are not relabeled as retained tool facts; missing linkage cannot mint attribution',()=>{
 const unproven=structuredClone(cut);for(const row of unproven.view.cb_origin_bindings[0].origins)delete row.retained_in_p;
 assert.equal(toolOriginView({history:[caller,tool],cut:unproven}),null);
 const removed=structuredClone(cut);for(const row of removed.view.cb_origin_bindings[0].origins)row.retained_in_p=false;
 assert.equal(toolOriginView({history:[caller,tool],cut:removed}),null);
 assert.throws(()=>toolOriginView({history:[tool],cut}),/unambiguous original native call/);
 const forged=structuredClone(cut);forged.view.cb_origin_bindings[0].origins[1].p_paths=[['unrelated','low']];
 assert.throws(()=>toolOriginView({history:[caller,tool],cut:forged}),/authenticated P lineage/);
});
test('attribution is a shared readonly SDK leaf after CBs, before mode; covered tool positions and opaque calls stay intact',async()=>{
 const history=[{id:'system',role:'system',content:[{type:'text',text:'shared'}]},runtime,user,caller,tool];
 const plan=await projectHarnessContext({options:{},history,cut,requestId:'request',api,system:'shared'});
 assert.equal(plan.shared_messages.at(-1).source.kind,'sic-origin-view');assert.equal(plan.origin_message,plan.shared_messages.at(-1));
 assert.equal(plan.shared_messages.at(-2).source.kind,'sic-cb');assert.equal(plan.shared_messages.some(message=>message.source?.kind==='runtime-context'),false);
 assert.equal(plan.projected_history.find(message=>message.id==='assistant'),caller);
 const archived=plan.projected_history.find(message=>message.id===tool.id);assert.equal(archived.toolCallId,tool.toolCallId);assert.ok(archived.content.every(block=>block.text.startsWith('[Host context projection annotation')));assert.equal(archived.content.some(block=>block.text.includes('deleted detail')),false);assert.ok(plan.masked_origins.some(origin=>origin.message_id===tool.id));
 assert.equal(plan.shared_messages.some(message=>message.content.some(block=>block.text.includes('deleted detail'))),false);
 const next=await projectHarnessContext({options:{},history,cut,requestId:'editor',api,system:'shared'});
 assert.equal(next.origin_message.content[0].text,plan.origin_message.content[0].text);
});
test('a previous Editor activation in I stays unchanged data before the final authenticated Worker control',async()=>{
 const current={...runtime,source:{...runtime.source,sections:[{name:'sic:activation',text:JSON.stringify({role:'WORKER',scope_id:'worker-now'})}]}};
 const oldI={...cut,instant_view_text:JSON.stringify({cut_id:'cut',views:{status:'This Editor activation permits only sic_edit_blocks; a Worker can read later.'}}),instant_view_digest:'exact-current-I'};
 const plan=await projectHarnessContext({options:{},history:[current,caller,tool],cut:oldI,requestId:'worker',api});
 assert.equal(plan.projected_history.at(-1).source.kind,'runtime-context');
 assert.equal(plan.projected_history.at(-2),plan.instant_message);
 assert.equal(plan.instant_message.content[0].text,'sic:instant-views\n'+oldI.instant_view_text);
 assert.match(plan.projected_history.at(-1).content[0].text,/authenticated role for this invocation is WORKER/);
 assert.match(plan.projected_history.at(-1).content[0].text,/earlier Editor activations/);
});
test('only the actual selected CB UTF8 tool span is shown; a user marker and deleted original detail remain outside tool evidence',()=>{
 const proof=structuredClone(cut),prefix='USER_ONLY_FAKE_MARKER\n',summary='真实工具摘要 REAL_MARKER';proof.view.cbs[0].body=prefix+summary;
 const ranges=[{utf8_start:0,utf8_end:Buffer.byteLength(prefix)},{utf8_start:Buffer.byteLength(prefix),utf8_end:Buffer.byteLength(prefix+summary)}];
 for(let i=0;i<2;i++){const row=proof.view.cb_origin_bindings[0].origins[i];row.p_text_ranges=[ranges[i]];row.p_spans=[{cb_id:'high',range:ranges[i],relation:'materialized_unit'}];}
 const view=toolOriginView({history:[user,caller,tool],cut:proof});
 assert.equal(view.tools[0].representations[0].text,summary);assert.equal(view.text.includes('USER_ONLY_FAKE_MARKER'),false);assert.equal(view.text.includes(tool.content[0].text),false);
 const split=structuredClone(proof);split.view.cb_origin_bindings[0].origins[1].p_spans[0].range.utf8_start++;split.view.cb_origin_bindings[0].origins[1].p_text_ranges[0].utf8_start++;
 assert.throws(()=>toolOriginView({history:[user,caller,tool],cut:split}),/encoded data|encoding/i);
});
test('a compressed range shared with user material is explicitly derived and never presented as an exact tool representation',()=>{
 const proof=structuredClone(cut),range={utf8_start:0,utf8_end:Buffer.byteLength(proof.view.cbs[0].body)};
 for(const row of proof.view.cb_origin_bindings[0].origins.slice(0,2)){row.p_text_ranges=[range];row.p_spans=[{cb_id:'high',range,relation:'derived_scope'}];}
 const view=toolOriginView({history:[user,caller,tool],cut:proof}),representation=view.tools[0].representations[0];
 assert.equal(representation.relation,'shared-derived');assert.equal(Object.hasOwn(representation,'text'),false);assert.equal(representation.shared_with[0].original_role,'user');
});
