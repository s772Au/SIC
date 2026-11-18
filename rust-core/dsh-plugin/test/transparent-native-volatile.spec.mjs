import test from 'node:test';
import assert from 'node:assert/strict';
import {collectPendingNativeVolatile,bindPendingNativeVolatile} from '../transparent/native-volatile.mjs';
function fixture(){
 let next=0;const reads=[];
 const sources={v1:{origin:{kind:'volatile-continuation'},units:[{unit_id:'u1',text:'summary A'},{unit_id:'u2',text:''}],
  collection_provenances:[{lifecycle_object_id:'job-A',collection_receipt_id:'receipt-A',unit_ids:['u1']}]},
  v2:{origin:{kind:'volatile-continuation'},units:[{unit_id:'u1',text:'summary B'}],collection_provenance:{lifecycle_object_id:'job-B',collection_receipt_id:'receipt-B',unit_ids:['u1']}},
  ordinary:{origin:{kind:'worker-output'},units:[{unit_id:'u1',text:'other'}]},v3:{origin:{kind:'volatile-continuation'},units:[{unit_id:'u1',text:'later'}]}};
 const core={operate:async(op,p)=>{reads.push([op,p]);return op==='chain.get'?{chain_id:'chain',contexts:{main:{levels:{'1':{pending:['v1','v2','ordinary','v3']},'3':{pending:['not-first-order']}}}}}:{source:sources[p.source_id]};}};
 const api={createUserMessage:value=>({id:'sdk-'+(++next),role:'user',...value})};
 return{core,api,reads,sources};
}
function origins(plan){return plan.messages.flatMap((message,m)=>message.content.map((block,b)=>({message_id:message.id,block_index:b,source_kind:message.source.kind,role:message.role,text:block.text,anchor:['messages',m+2,'content',b,'text']})));}
test('pending V reads main1 in order and stops before unrelated pending material',async()=>{
 const f=fixture(),plan=await collectPendingNativeVolatile({...f,chainId:'chain'});
 assert.deepEqual(plan.sources.map(s=>s.source_id),['v1','v2']);
 assert.deepEqual(f.reads.filter(([op])=>op==='source.read').map(([,p])=>p.source_id),['v1','v2','ordinary']);
 assert.equal(plan.messages[0].content[1].text,'');
 assert.equal(plan.messages[0].source.kind,'sic-volatile');
 assert.deepEqual(JSON.parse(JSON.stringify(plan)),plan,'persistable identity plan required before native.begin');
});
test('bindings use actual SDK origins and retain original V source/unit and collection proofs',async()=>{
 const f=fixture(),plan=await collectPendingNativeVolatile({...f,chainId:'chain'}),actual=origins(plan),bound=bindPendingNativeVolatile(plan,actual);
 assert.deepEqual(bound.volatile_source_bindings,[{source_id:'v1',native_anchors:[actual[0].anchor,actual[1].anchor]},{source_id:'v2',native_anchors:[actual[2].anchor]}]);
 assert.deepEqual(bound.provenances,[{lifecycle_object_id:'job-A',collection_receipt_id:'receipt-A',native_anchors:[actual[0].anchor],via_volatile_source_id:'v1'},
  {lifecycle_object_id:'job-B',collection_receipt_id:'receipt-B',native_anchors:[actual[2].anchor],via_volatile_source_id:'v2'}]);
 assert.equal(bound.editable_origins.length,3);
 assert.throws(()=>bindPendingNativeVolatile(plan,actual.map(o=>({...o,message_id:'equal-text-other-identity'}))),/exact native SDK origin/);
 assert.throws(()=>bindPendingNativeVolatile(plan,[...actual,actual[0]]),/exact native SDK origin/);
 assert.throws(()=>bindPendingNativeVolatile(plan,actual.map((o,index)=>index?o:{...o,text:'changed'})),/changed or overlaps/);
});
test('cross-chain, failed dependency read and unordered collection claims fail closed',async()=>{
 const f=fixture();await assert.rejects(collectPendingNativeVolatile({...f,chainId:'another'}),/differs from host binding/);
 const core={operate:async(op,p)=>op==='source.read'?Promise.reject(Error('Core revoked dependency')):f.core.operate(op,p)};
 await assert.rejects(collectPendingNativeVolatile({...f,core,chainId:'chain'}),/revoked dependency/);
 const plan=await collectPendingNativeVolatile({...f,chainId:'chain'});
 plan.sources[0].collection_provenances[0].unit_ids=['u2','u1'];
 assert.throws(()=>bindPendingNativeVolatile(plan,origins(plan)),/source order/);
});
