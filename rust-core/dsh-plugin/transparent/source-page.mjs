export function pageSourceText(text,offset,limit){
 const scalars=Array.from(text);
 return{text:scalars.slice(offset,offset+limit).join(''),next_offset:offset+limit<scalars.length?offset+limit:null};
}

export async function readNativeSourcePage({core,authority,args,signal}){
 const offset=args.offset??0,limit=args.limit??4096;
 if(typeof args.source_id!=='string'||typeof args.unit_id!=='string'||!Number.isSafeInteger(offset)||offset<0||!Number.isSafeInteger(limit)||limit<1||limit>8192)throw Error('invalid source page arguments');
 const own=args.source_id===authority?.source_id&&authority.assigned_unit_ids?.includes(args.unit_id);
 const extra=(authority?.readable_sources??[]).some(source=>source.kind==='native-source'&&source.source_id===args.source_id&&source.unit_ids?.includes(args.unit_id));
 if(!own&&!extra)throw Error('source page is outside the authenticated read scope');
 const source=await core.get({source_id:args.source_id},signal);
 if(own){
  if(Array.isArray(source.binding?.editable_unit_ids)&&!source.binding.editable_unit_ids.includes(args.unit_id))throw Error('source page is outside the persisted candidate scope');
  if(authority.source_digest!==undefined&&authority.source_digest!==source.source_digest||authority.expected_version!==undefined&&authority.expected_version!==source.version)throw Error('source page authority is stale');
 }
 const unit=source.units.find(u=>u.unit_id===args.unit_id);if(!unit)throw Error('authorized source unit missing');
 const text=unit.anchor.reduce((v,k)=>v[k],source.source_request);
 if(typeof text!=='string')throw Error('authorized native source text missing');
 return{source_id:args.source_id,unit_id:args.unit_id,offset,...pageSourceText(text,offset,limit)};
}
