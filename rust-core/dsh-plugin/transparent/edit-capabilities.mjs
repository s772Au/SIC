import{editorDelivery}from'./editor-delivery.mjs';
export const LEGACY_TEXT_CAPABILITIES={ops:['KEEP','REPLACE','COMPRESS'],pvi:['I']};
const HOST_TEXT_OPS=['KEEP','REPLACE','COMPRESS','DELETE','ADD','REORDER','FLAG'],HOST_TEXT_PVI=['P','I'];
export function editorCapabilities(binding,source){
 const actual=source.binding?.edit_capabilities??LEGACY_TEXT_CAPABILITIES,vInstalled=binding?.installed_native_bindings?.includes('native-volatile-next-WB-v1'),maximumPvi=[...HOST_TEXT_PVI,...vInstalled?['V']:[]],supportedPvi=[...actual.pvi,...vInstalled&&(actual.deferred_pvi??[]).includes('V')?['V']:[]],host=binding?.edit_capabilities??{ops:HOST_TEXT_OPS,pvi:maximumPvi};
 const ops=actual.ops.filter(op=>HOST_TEXT_OPS.includes(op)&&host.ops?.includes(op)),pvi=[...new Set(supportedPvi)].filter(kind=>maximumPvi.includes(kind)&&host.pvi?.includes(kind));
 return{ops,pvi,...actual.constraints?{constraints:actual.constraints}:{},deferred_pvi:[...new Set([...(actual.deferred_pvi??[]).filter(kind=>!pvi.includes(kind)),...actual.pvi.filter(kind=>!pvi.includes(kind))])]};
}
export async function materializeEditorDelivery({core,source,args,allowedAnchors,binding,signal}){
 const capabilities=editorCapabilities(binding,source);
 if(!Array.isArray(args?.ops)||!args.pvi||typeof args.pvi!=='object'||Array.isArray(args.pvi))throw Error('invalid sic_edit_blocks delivery');
 if(args.ops.some(op=>!capabilities.ops.includes(op.op)||op.lifecycle&&op.lifecycle!=='P'&&!capabilities.pvi.includes(op.lifecycle)))throw Error('Editor operation is outside the current Core/host capability intersection');
 if(Object.entries(args.pvi).some(([kind,value])=>!capabilities.pvi.includes(kind)&&(Array.isArray(value)?value.length: value&&Object.keys(value).length)))throw Error('Editor PVI channel is not activated by the current host adapter');
 if(!source.binding?.edit_capabilities)return editorDelivery(source,args,allowedAnchors);
 if(typeof core.preview!=='function')throw Error('Core native preview binding required for activated edit capabilities');
 const preview=await core.preview({source_id:source.source_id,expected_source_digest:source.source_digest,expected_version:source.version,ops:args.ops,pvi:args.pvi},signal);
 if(!Array.isArray(preview.edits))throw Error('invalid actual Core native.preview receipt');
 const allowed=new Set(allowedAnchors.map(JSON.stringify));for(const edit of preview.edits){const unit=source.units.find(u=>u.unit_id===edit.unit_id);if(!unit||!allowed.has(JSON.stringify(unit.anchor)))throw Error('Core preview escaped host-assigned text anchors');}
 return{edits:preview.edits,ops:args.ops,pvi:args.pvi};
}
