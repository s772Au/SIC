const object=value=>value!==null&&typeof value==='object'&&!Array.isArray(value);

export function normalizeEditorDelivery(delivery){
 const imageView='sic-image-compression-v1';
 const originalPvi=delivery?.pvi;
 const image=originalPvi?.[imageView];
 if(object(originalPvi)&&object(image)&&Array.isArray(image.crops)&&Array.isArray(image.description)
   &&(!Object.hasOwn(originalPvi,'I')||object(originalPvi.I)&&!Object.hasOwn(originalPvi.I,imageView))){
  const {[imageView]:recipe,...pvi}=originalPvi;
  delivery={...delivery,pvi:{...pvi,I:{...pvi.I,[imageView]:recipe}}};
 }
 const instant=delivery?.pvi?.I;
 if(object(instant)&&Object.hasOwn(instant,'decision_tree')){
  const {decision_tree,...views}=instant;
  delivery={...delivery,pvi:{...delivery.pvi,I:views}};
 }
 if(!Array.isArray(delivery?.ops))return delivery;
 const isDecision=op=>op?.lifecycle==='I'&&op.view_key==='decision_tree';
 const canonical=delivery.ops.some(isDecision)?{...delivery,ops:delivery.ops.filter(op=>!isDecision(op))}:delivery;
 let pvi=canonical.pvi;
 if(!Object.hasOwn(canonical,'pvi'))pvi={};
 else if(object(pvi)&&typeof pvi.status==='string'&&(!Object.hasOwn(pvi,'I')||object(pvi.I)&&!Object.hasOwn(pvi.I,'status'))){
  const {status,...channels}=pvi;
  pvi={...channels,I:{...channels.I,status}};
 }
 if(object(pvi)){
  for(const channel of ['P','V']){
   if(typeof pvi[channel]==='string')pvi={...pvi,[channel]:[pvi[channel]]};
  }
 }
 const ops=canonical.ops.map(op=>{
  if(op?.op!=='ADD'||Object.hasOwn(op,'anchor')||typeof op.target!=='string'||!['before','after'].includes(op.position))return op;
  const {target,position,...fields}=op;
  return {...fields,anchor:{target,position}};
 });
 return pvi!==canonical.pvi||ops.some((op,index)=>op!==canonical.ops[index])?{...canonical,ops,pvi}:canonical;
}

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
