import{test}from'node:test';import assert from'node:assert/strict';import{createHash}from'node:crypto';import{nativeCollectionProvenances}from'../transparent/collection-provenance.mjs';
const hash=s=>createHash('sha256').update(s).digest('hex');
test('collection receipts bind exact SDK tool-result identity and UTF8 byte range, allowing several distinct jobs in one native carrier',()=>{
 const a={message_id:'tool-message-a',block_index:0,call_id:'native-call-a',source_kind:'tool',text:'结果甲\nstatus',anchor:['messages',2,'content',0,'content',0,'text']},b={message_id:'tool-message-b',block_index:0,call_id:'native-call-b',source_kind:'tool',text:'same words\nstatus',anchor:['messages',2,'content',1,'content',0,'text']};
 const record=(o,body,id)=>({lifecycle_object_id:'task-'+id,collection_receipt_id:'receipt-'+id,collection_digest:hash(body),chain_id:'chain',message_id:o.message_id,call_id:o.call_id,material:{block_index:0,utf8_start:0,utf8_end:Buffer.byteLength(body)}});
 const records=[record(a,'结果甲','a'),record(b,'same words','b')],result=nativeCollectionProvenances(records,[a,b],[a,b],'chain');assert.equal(result.length,2);assert.equal(result[0].utf8_end,9);assert.deepEqual(result[1].native_anchor,b.anchor);
 assert.deepEqual(nativeCollectionProvenances(records,[{...a,message_id:'different-id'},b],[b],'chain'),[result[1]]);
 assert.throws(()=>nativeCollectionProvenances([{...records[0],call_id:'forged'}],[a],[a],'chain'),/not bound/);
 assert.throws(()=>nativeCollectionProvenances([{...records[0],material:{...records[0].material,utf8_end:1}}],[a],[a],'chain'),/range/);
 assert.throws(()=>nativeCollectionProvenances([records[0],records[0]],[a],[a],'chain'),/duplicate/);
 assert.throws(()=>nativeCollectionProvenances([{...records[0],chain_id:'another-chain'}],[a],[a],'chain'),/not bound/);
 assert.deepEqual(nativeCollectionProvenances(records,[a,b],[],'chain'),[]);
});
