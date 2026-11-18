import {test} from 'node:test';import assert from 'node:assert/strict';
import {load} from './native-api-fixture.mjs';
import {projectHarnessContext} from '../transparent/context-project.mjs';
import {repackageNativeContext} from '../transparent/native-layout.mjs';
const api=await load('packages/llm/llm/lib/index.js');
const runtime=api.createUserMessage({source:{kind:'runtime-context',sections:[{name:'sic:activation',text:'{"role":"WORKER"}'}]},content:[{type:'text',text:'runtime'}]});
const ref={attachmentId:'sha256:media',mediaType:'image/png',bytes:1,width:1,height:1},image={type:'image',attachment:ref};
const emptyCut={cut_id:null,sequence:0,view:{cbs:[],cb_origin_bindings:[]}};
const project=(history,cut=emptyCut)=>projectHarnessContext({history,options:{},cut,requestId:'media-test',api});
test('shared media keeps original block order/identity, stable carrier identity, user provenance and survives text DELETE',async()=>{
 const original=api.createUserMessage({source:{kind:'user',caption:'success',claimed_source:'tool'},content:[{type:'text',text:'private text to DELETE'},image,{...image,attachment:{...ref,attachmentId:'sha256:later'}}]});
 const cut={cut_id:'cut',sequence:1,view:{cbs:[{cb_id:'cb',body:'',level:2}],cb_origin_bindings:[{cb_id:'cb',missing_source_ids:[],origins:[{message_id:original.id,block_index:0,retained_in_p:false}]}]}};
 const first=await project([runtime,original],cut),second=await project([runtime,original],cut),media=first.shared_messages.find(m=>m.source?.kind==='sic-media-view');
 assert.equal(media.id,second.shared_messages.find(m=>m.source?.kind==='sic-media-view').id);assert.strictEqual(media.content[1],original.content[1]);assert.deepEqual(media.content.slice(1),original.content.slice(1));
 assert.ok(media.source.origins.every(o=>o.original_role==='user'&&!o.tool));assert.ok(!JSON.stringify(first.projected_history).includes('private text to DELETE'));
 assert.equal(first.projected_history.flatMap(m=>m.content).filter(b=>b.type==='image').length,2);assert.equal(original.content[0].text,'private text to DELETE');
 assert.ok(first.masked_origins.filter(o=>o.kind==='media-pointer').length===2);
});
test('tool image-only result preserves true call linkage and exact native image/signature raw slices during media relocation',async()=>{
 const caller=api.createAssistantMessage({source:{kind:'model',provider:'p',model:'m'},content:[{type:'reasoning',text:'thought'},{type:'tool-call',id:'call',name:'read',arguments:'{}'}]}),result=api.createToolResultMessage({callId:'call',isError:false,content:[image]});
 const history=[runtime,caller,result],plan=await project(history),media=plan.shared_messages.find(m=>m.source?.kind==='sic-media-view');
 assert.equal(media.source.origins[0].tool.tool_call_message_id,caller.id);assert.equal(media.source.origins[0].tool.tool_name,'read');assert.equal(media.source.origins[0].tool.is_error,false);
 const imageWire={type:'image',source:{type:'base64',media_type:'image/png',data:'AA=='}};
 function serialize(messages){const groups=[];for(const m of messages){const blocks=m.content.flatMap(b=>b.type==='text'&&b.text?[{type:'text',text:b.text}]:b.type==='image'?[{type:'text',text:'native image handle'},imageWire]:b.type==='reasoning'?[{type:'thinking',thinking:b.text,signature:'signature'}]:b.type==='tool-call'?[{type:'tool_use',id:b.id,name:b.name,input:JSON.parse(b.arguments)}]:[]),role=m.role==='tool'?'user':m.role,content=m.role==='tool'?[{type:'tool_result',tool_use_id:m.toolCallId,is_error:m.isError,content:blocks}]:blocks;if(!content.length)continue;if(groups.at(-1)?.role===role)groups.at(-1).content.push(...content);else groups.push({role,content});}for(const g of groups)if(g.role==='user')g.content=[...g.content.filter(b=>b.type==='tool_result'),...g.content.filter(b=>b.type!=='tool_result')];return{model:'native',stream:true,messages:groups};}
 const before=serialize(history),after=serialize(plan.projected_history),fragment='{ "type" : "image", "source" : {"type":"base64","media_type":"image/png","data":"\\u0041A=="} }';
 const raw=JSON.stringify(before).replace(JSON.stringify(imageWire),fragment).replace('"signature":"signature"','"signature" : "sig\\u006eature"');
 const repack=repackageNativeContext(raw,after,history,plan.projected_history,plan.media_bindings);
 assert.ok(repack.includes(fragment));assert.ok(repack.includes('"signature" : "sig\\u006eature"'));assert.deepEqual(JSON.parse(repack),after);
 const afterPublication=await project(history,{...emptyCut,view:{cbs:[{cb_id:'cb-with-media',body:'edited',level:1}],cb_origin_bindings:[{cb_id:'cb-with-media',origins:[]}]}});
 const published=serialize(afterPublication.projected_history);
 const adopted=repackageNativeContext(repack,published,plan.projected_history,afterPublication.projected_history,afterPublication.media_bindings);
 assert.ok(adopted.includes(fragment));assert.deepEqual(JSON.parse(adopted),published);
 assert.throws(()=>repackageNativeContext(raw,after,history,plan.projected_history,plan.media_bindings.map(b=>({...b,block_index:9}))),/trusted media relocation/);
});
test('Host offloaded media keeps its original offload policy flag and is not reclassified as editable text',async()=>{
 const offloaded={...image,offloaded:true},message=api.createUserMessage({source:{kind:'user'},content:[offloaded]});
 const plan=await project([runtime,message]),carrier=plan.shared_messages.find(m=>m.source?.kind==='sic-media-view');assert.strictEqual(carrier.content[1],message.content[0]);assert.equal(carrier.content[1].offloaded,true);assert.equal(plan.media_bindings.length,1);assert.equal(plan.masked_origins[0].kind,'media-pointer');
});
