// Compile the original sic_edit_blocks arguments into fixed native text edits.
// Operation purpose stays distinct from actual protocol-mapped capabilities.
export function editorDelivery(source,args,allowedAnchors){
  if(!Array.isArray(args?.ops)||!args.pvi||typeof args.pvi!=='object'||Array.isArray(args.pvi))throw Error('invalid sic_edit_blocks delivery');
  if(Object.keys(args.pvi).some(k=>k!=='I'))throw Error('P/V native projection requires an adapter');
  const allowed=new Set(allowedAnchors.map(JSON.stringify)),units=new Map(source.units.map(u=>[u.unit_id,u])),final=new Map();
  for(const op of args.ops){
    if(!['KEEP','REPLACE','COMPRESS'].includes(op.op)||op.lifecycle&&op.lifecycle!=='P')throw Error('operation not mapped to native fixed text anchors');
    const unit=units.get(op.target);
    if(!unit||!allowed.has(JSON.stringify(unit.anchor)))throw Error('operation outside assigned current WB');
    if(op.op!=='KEEP'){
      if(typeof op.content!=='string')throw Error('native replacement content required');
      final.set(unit.unit_id,{unit_id:unit.unit_id,expected_text:unit.text,replacement:op.content});
    }
  }
  return{edits:[...final.values()],ops:args.ops,pvi:args.pvi};
}
