// Inverse of reviewed DSH Messages serializer structure. Immutable Message.id
// and block index supply identity; equal text alone never grants authority.
export function messagesOrigins(history,native){
  const groups=[];
  for(const message of history){
    if(['system','developer'].includes(message.role))continue;
    if(!['user','assistant','tool'].includes(message.role)||!message.id)throw Error('unsupported native provenance');
    const role=message.role==='tool'?'user':message.role,blocks=[];
    if(message.role==='tool'){
      const children=[];
      for(let index=0;index<message.content.length;index++){
        const b=message.content[index];
        if(b.type==='text'&&b.text)children.push({type:'text',origin:{message_id:message.id,block_index:index,source_kind:'tool',role:'tool',call_id:message.toolCallId,text:b.text}});
        else if(b.type==='image')children.push({type:'text'},{type:'image'});
        else if(!['text','reasoning','tool-call'].includes(b.type))throw Error('unsupported tool result provenance');
      }
      blocks.push({type:'tool_result',children,call_id:message.toolCallId,is_error:message.isError});
    }
    else for(let index=0;index<message.content.length;index++){
      const b=message.content[index];
      if(b.type==='text'&&b.text)blocks.push({type:'text',origin:{message_id:message.id,block_index:index,
        source_kind:message.source?.kind??(role==='assistant'?'model':'unknown'),role:message.role,text:b.text}});
      else if(role==='assistant'&&b.type==='reasoning')blocks.push({type:'thinking'});
      else if(role==='assistant'&&b.type==='tool-call')blocks.push({type:'tool_use'});
      else if(role==='user'&&b.type==='image')blocks.push({type:'text'},{type:'image'});
      else if(!['text','reasoning','tool-call'].includes(b.type))throw Error('unsupported native provenance block');
    }
    if(!blocks.length)continue;
    if(groups.at(-1)?.role===role)groups.at(-1).blocks.push(...blocks);else groups.push({role,blocks});
  }
  for(const group of groups)if(group.role==='user')group.blocks=[...group.blocks.filter(b=>b.type==='tool_result'),...group.blocks.filter(b=>b.type!=='tool_result')];
  const wire=native.messages.map((message,index)=>({message,index})).filter(({message})=>message.role!=='system');
  if(wire.length!==groups.length)throw Error('native provenance group mismatch');
  const result=[];
  for(let g=0;g<groups.length;g++){
    const {message,index}=wire[g],expected=groups[g];
    if(message.role!==expected.role||message.content.length!==expected.blocks.length)throw Error('native provenance shape mismatch');
    for(let b=0;b<expected.blocks.length;b++){
      const actual=message.content[b],source=expected.blocks[b];
      if(actual.type!==source.type)throw Error('native provenance block mismatch');
      if(source.origin){if(actual.text!==source.origin.text)throw Error('source serialization integrity failure');result.push({...source.origin,anchor:['messages',index,'content',b,'text']});}
      if(source.children){
        if(actual.tool_use_id!==source.call_id||actual.is_error!==source.is_error)throw Error('tool result identity mismatch');
        if(!Array.isArray(actual.content)||actual.content.length!==source.children.length)throw Error('tool result provenance shape mismatch');
        for(let child=0;child<source.children.length;child++){
          const expected=source.children[child],wire=actual.content[child];if(wire.type!==expected.type)throw Error('tool result provenance block mismatch');
          if(expected.origin){if(wire.text!==expected.origin.text)throw Error('tool result source serialization integrity failure');result.push({...expected.origin,anchor:['messages',index,'content',b,'content',child,'text']});}
        }
      }
    }
  }
  return result;
}

export function reconcileFrozenOrigins(native,origins,frozen){
  const request=structuredClone(native),identity=o=>`${o.message_id}:${o.block_index}`;
  const byIdentity=new Map(origins.map(o=>[identity(o),o])),protectedIds=new Set(),protected_cb_bindings=[];
  for(const binding of frozen){
    const native_anchors=[];
    for(const leaf of binding.leaves){
      const key=identity(leaf),origin=byIdentity.get(key);
      if(!origin)throw Error('selected CB source identity missing');
      if(origin.text!==leaf.original_text)throw Error('immutable Harness source changed');
      let target=request;for(const key of origin.anchor.slice(0,-1))target=target[key];target[origin.anchor.at(-1)]=leaf.adopted_text;
      native_anchors.push(origin.anchor);protectedIds.add(key);
    }
    protected_cb_bindings.push({cb_id:binding.cb_id,native_anchors});
  }
  const editable_origins=origins.filter(o=>!protectedIds.has(identity(o))&&['user','model'].includes(o.source_kind));
  return{request,editable_anchors:editable_origins.map(o=>o.anchor),editable_origins,protected_cb_bindings};
}

export function sharedSdkPrefix(history,frozen,adoptedMessageIds){
  const leaves=new Map(frozen.flatMap(b=>b.leaves).map(l=>[`${l.message_id}:${l.block_index}`,l]));
  const messages=[],pending=new Set();let safeLength=0;
  for(const original of history){
    if(original.source?.kind==='runtime-context')continue;
    if(!['system','developer'].includes(original.role)&&!adoptedMessageIds.has(original.id))break;
    const message=structuredClone(original);
    for(let i=0;i<message.content.length;i++){
      const leaf=leaves.get(`${message.id}:${i}`);if(!leaf)continue;
      if(message.content[i].type!=='text'||message.content[i].text!==leaf.original_text)throw Error('shared SDK source identity changed');
      message.content[i].text=leaf.adopted_text;
    }
    messages.push(message);
    if(message.role==='assistant')for(const b of message.content)if(b.type==='tool-call')pending.add(b.id);
    if(message.role==='tool')pending.delete(message.toolCallId);
    if(!pending.size)safeLength=messages.length;
  }
  return{messages:messages.slice(0,safeLength),dropped_incomplete_tool_group:safeLength!==messages.length};
}
// Every native block retains a host SDK identity through the reviewed serializer.
// This map is used only for preserving original opaque raw fragments during
// host-owned canonical CB layout, never for granting an Editor text scope.
export function messagesBlockOrigins(history,native,mediaBindings=[]){
 const mediaByCarrier=new Map(mediaBindings.map(b=>[JSON.stringify([b.carrier_message_id,b.carrier_block_index]),b]));
 const groups=[];
 for(const message of history){
  if(['system','developer'].includes(message.role))continue;
  const role=message.role==='tool'?'user':message.role,blocks=[];
  const add=(type,index,part)=>{const alias=mediaByCarrier.get(JSON.stringify([message.id,index]));blocks.push({type,identity:JSON.stringify(alias?[alias.message_id,alias.block_index,alias.original_role==='tool'?'result-'+part:part]:[message.id,index,part])});};
  if(message.role==='tool'){
   add('tool_result',null,'result');const parent=blocks[0];parent.children=[];
   for(let i=0;i<message.content.length;i++){
    const b=message.content[i],addChild=(type,part)=>parent.children.push({type,identity:JSON.stringify([message.id,i,part]),parent_identity:parent.identity});
    if(b.type==='text'&&b.text)addChild('text','result-text');
    else if(b.type==='image'){addChild('text','result-image-handle');addChild('image','result-image');}
    else if(!['text','reasoning','tool-call'].includes(b.type))throw Error('unsupported tool result layout');
   }
  }
  else for(let i=0;i<message.content.length;i++){
   const b=message.content[i];
   if(b.type==='text'&&b.text)add('text',i,'text');
   else if(role==='assistant'&&b.type==='reasoning')add('thinking',i,'reasoning');
   else if(role==='assistant'&&b.type==='tool-call')add('tool_use',i,'call');
   else if(role==='user'&&b.type==='image'){add('text',i,'image-handle');add('image',i,'image');}
   else if(!['text','reasoning','tool-call'].includes(b.type))throw Error('unsupported block layout provenance');
  }
  if(!blocks.length)continue;
  if(groups.at(-1)?.role===role)groups.at(-1).blocks.push(...blocks);else groups.push({role,blocks});
 }
 for(const g of groups)if(g.role==='user')g.blocks=[...g.blocks.filter(b=>b.type==='tool_result'),...g.blocks.filter(b=>b.type!=='tool_result')];
 if(native.messages.length!==groups.length)throw Error('native layout group mismatch');
 const result=[];
 for(let m=0;m<groups.length;m++){
  const g=groups[m],wire=native.messages[m];
  if(wire.role!==g.role||wire.content.length!==g.blocks.length||Object.keys(wire).some(k=>!['role','content'].includes(k)))throw Error('unsupported opaque native message layout');
  for(let b=0;b<g.blocks.length;b++){
   if(wire.content[b].type!==g.blocks[b].type)throw Error('native layout block mismatch');
   result.push({...g.blocks[b],anchor:['messages',m,'content',b]});
   const children=g.blocks[b].children;
   if(children){
    if(!Array.isArray(wire.content[b].content)||wire.content[b].content.length!==children.length)throw Error('native tool result layout mismatch');
    children.forEach((child,n)=>{if(wire.content[b].content[n].type!==child.type)throw Error('native tool result child mismatch');result.push({...child,anchor:['messages',m,'content',b,'content',n]});});
   }
  }
 }
 return result;
}
