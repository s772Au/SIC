import{createHash}from'node:crypto';
const identity=o=>JSON.stringify([o.message_id,o.block_index]);

// Inputs are committed native PluginRecords, frozen with the SDK call before
// serialization. Receipt IDs never come from text search or model arguments.
export function nativeCollectionProvenances(records,origins,editable,chainId){
 const allowed=new Set(editable.map(o=>identity(o))),byOrigin=new Map(origins.map(o=>[identity(o),o])),ranges=new Map(),receipts=new Set(),result=[];
 for(const record of records??[]){
  const material=record.material,origin=byOrigin.get(identity({message_id:record.message_id,block_index:material?.block_index}));
  if(!origin||!allowed.has(identity(origin)))continue;
  if(origin.source_kind!=='tool'||record.chain_id!==chainId||record.call_id!==origin.call_id)throw Error('collection is not bound to this native tool result/chain');
  if(typeof record.lifecycle_object_id!=='string'||typeof record.collection_receipt_id!=='string'||!/^[a-f0-9]{64}$/.test(record.collection_digest))throw Error('invalid native collection receipt');
  const start=material.utf8_start,end=material.utf8_end,bytes=Buffer.from(origin.text,'utf8');
  if(!Number.isSafeInteger(start)||!Number.isSafeInteger(end)||start<0||end<start||end>bytes.length)throw Error('invalid native collection UTF8 range');
  const slice=bytes.subarray(start,end);
  if(!Buffer.from(slice.toString('utf8'),'utf8').equals(slice)||createHash('sha256').update(slice).digest('hex')!==record.collection_digest)throw Error('native renderer collection range no longer matches its receipt');
  if(receipts.has(record.collection_receipt_id))throw Error('duplicate native collection receipt');receipts.add(record.collection_receipt_id);
  const key=JSON.stringify(origin.anchor),prior=ranges.get(key)??[];
  if(prior.some(r=>start<r.end&&end>r.start))throw Error('overlapping native collection ranges');prior.push({start,end});ranges.set(key,prior);
  result.push({lifecycle_object_id:record.lifecycle_object_id,collection_receipt_id:record.collection_receipt_id,native_anchor:origin.anchor,utf8_start:start,utf8_end:end});
 }
 return result;
}
