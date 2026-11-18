// Trusted Core pending material -> distinct original Harness SDK messages.
// The caller persists this plan with its request identity BEFORE native.begin.
const text=(value,label)=>{if(typeof value!=='string'||!value)throw Error(label+' required');return value;};
export async function collectPendingNativeVolatile({core,api,chainId,signal}){
 const chain=await core.operate('chain.get',{},signal);
 if(chain.chain_id!==chainId)throw Error('volatile source chain differs from host binding');
 const pending=chain.contexts?.main?.levels?.['1']?.pending;
 if(!Array.isArray(pending))throw Error('Core main level-1 pending view missing');
 const sources=[];
 // Only the earliest consecutive prefix is admissible, never a filtered tail.
 for(const source_id of pending){
  const read=await core.operate('source.read',{source_id},signal),source=read.source;
  if(source?.origin?.kind!=='volatile-continuation')break;
  if(!Array.isArray(source.units)||!source.units.length)throw Error('volatile source text units missing');
  const units=source.units.map(unit=>{if(typeof unit.text!=='string')throw Error('volatile source contains a nontext unit');return{unit_id:text(unit.unit_id,'volatile unit identity'),text:unit.text};});
  if(new Set(units.map(unit=>unit.unit_id)).size!==units.length)throw Error('volatile unit identities repeated');
  const message=api.createUserMessage({source:{kind:'sic-volatile',form:'continuation',source_id},content:units.map(unit=>({type:'text',text:unit.text}))});
  text(message.id,'volatile SDK message identity');
  if(message.role!=='user'||!Array.isArray(message.content)||message.content.length!==units.length||message.content.some((block,index)=>block.type!=='text'||block.text!==units[index].text))throw Error('Harness changed volatile native message');
  const proofs=Array.isArray(source.collection_provenances)?source.collection_provenances:source.collection_provenance?[source.collection_provenance]:[];
  sources.push({source_id,message,units,collection_provenances:proofs});
 }
 return{chain_id:chainId,sources,messages:sources.map(source=>source.message)};
}

// Origins come from the actual SDK snapshot AFTER insertion, never text search.
export function bindPendingNativeVolatile(plan,origins){
 const volatile_source_bindings=[],provenances=[],editable_origins=[];
 const usedOrigins=new Set(),usedMessages=new Set();
 for(const source of plan.sources??[]){
  if(usedMessages.has(source.message.id))throw Error('volatile SDK message identities repeated');usedMessages.add(source.message.id);
  const bound=source.units.map((unit,index)=>{
   const matches=origins.filter(origin=>origin.message_id===source.message.id&&origin.block_index===index);
   if(matches.length!==1)throw Error('volatile source needs one exact native SDK origin per unit');
   const origin=matches[0],key=JSON.stringify(origin.anchor);
   if(origin.text!==unit.text||origin.role!=='user'||origin.source_kind!=='sic-volatile'||usedOrigins.has(key))throw Error('volatile source origin changed or overlaps');
   usedOrigins.add(key);editable_origins.push(origin);return origin;
  });
  volatile_source_bindings.push({source_id:source.source_id,native_anchors:bound.map(origin=>origin.anchor)});
  for(const proof of source.collection_provenances??[]){
   const ids=proof.unit_ids;if(!Array.isArray(ids)||!ids.length||new Set(ids).size!==ids.length)throw Error('volatile collection unit provenance invalid');
   const selected=source.units.map((unit,index)=>({unit,index})).filter(({unit})=>ids.includes(unit.unit_id));
   if(selected.length!==ids.length||selected.some(({unit},index)=>unit.unit_id!==ids[index]))throw Error('volatile collection units are not in source order');
   provenances.push({lifecycle_object_id:text(proof.lifecycle_object_id,'collection object'),collection_receipt_id:text(proof.collection_receipt_id,'collection receipt'),
    native_anchors:selected.map(({index})=>bound[index].anchor),via_volatile_source_id:source.source_id});
  }
 }
 return{volatile_source_bindings,provenances,editable_origins};
}
