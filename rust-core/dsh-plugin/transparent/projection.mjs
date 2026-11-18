import { digest,wireDigest } from './transport.mjs';

// Host-owned durable persistence and native-session event binding are mandatory.
export function createProjectionPublisher({save,getSession,appendEvent}) {
  if(![save,getSession,appendEvent].every(f=>typeof f==='function'))throw TypeError('durable projection/session bindings required');
  return async(record,signal)=>{
    signal?.throwIfAborted();
    const session=getSession(record.session_id);
    if(session?.id!==record.session_id||record.adopted_digest!==(record.raw_adopted===undefined?digest(record.adopted):wireDigest(record.raw_adopted)))throw Error('projection session/digest mismatch');
    await save(record,signal);
    await appendEvent(session,{type:'sic/native-projection',data:{projection_id:record.id,request_id:record.request_id,
      source_id:record.source_id,adopted_digest:record.adopted_digest,format:record.format,stage:record.stage}},signal);
    return{recorded:true,projection_id:record.id,adopted_digest:record.adopted_digest};
  };
}

export function createNativeProjectionHandler({authorize,list,get,history,image}) {
  if(![authorize,list,get].every(f=>typeof f==='function'))throw TypeError('projection authorization/readers required');
  return async request=>{
    const reply=(body,status=200)=>Response.json(body,{status,headers:{'cache-control':'no-store'}});
    let b;
    try{const raw=await request.text();if(Buffer.byteLength(raw)>4096)return reply({error:'invalid_request'},413);b=JSON.parse(raw);}
    catch{return reply({error:'invalid_request'},400);}
    if(!b||typeof b.sessionId!=='string'||!b.sessionId.trim()||!['list','get','history','image'].includes(b.action)
      ||Object.keys(b).some(k=>!['sessionId','action','projectionId','offset','limit','after_version','after_i_revision','assetId'].includes(k))
      ||(Object.hasOwn(b,'assetId')&&b.action!=='image'))return reply({error:'invalid_request'},400);
    if(!await authorize(request,b.sessionId))return reply({error:'forbidden'},403);
    const offset=b.offset??0,limit=b.limit??20;
    if(!Number.isSafeInteger(offset)||offset<0||!Number.isSafeInteger(limit)||limit<1||limit>100)return reply({error:'invalid_request'},400);
    const cursors={};
    for(const key of ['after_version','after_i_revision'])if(Object.hasOwn(b,key)){
      if(b.action!=='history'||!Number.isSafeInteger(b[key])||b[key]<0)return reply({error:'invalid_request'},400);
      cursors[key]=b[key];
    }
    if(b.action==='get'||b.action==='history'||b.action==='image'){
      if(typeof b.projectionId!=='string'||!b.projectionId)return reply({error:'invalid_request'},400);
      const record=await get(b.sessionId,b.projectionId);if(!record)return reply({error:'not_found'},404);
      if(b.action==='history'){if(typeof history!=='function')return reply({error:'history_unavailable'},501);return reply({ok:true,value:await history(b.sessionId,record,offset,limit,cursors)});}
      if(b.action==='image'){
        if(typeof b.assetId!=='string'||!/^[a-f0-9]{64}$/u.test(b.assetId))return reply({error:'invalid_request'},400);
        if(typeof image!=='function')return reply({error:'image_unavailable'},501);
        const asset=await image(b.sessionId,record,b.assetId,request.signal);
        if(!asset)return reply({error:'not_found'},404);
        if(!['image/png','image/jpeg','image/webp','image/gif','image/avif'].includes(asset.mediaType))return reply({error:'unsupported_image_type'},415);
        return new Response(asset.data,{headers:{'content-type':asset.mediaType,'cache-control':'no-store','x-content-type-options':'nosniff'}});
      }
      return reply({ok:true,value:record});
    }
    return reply({ok:true,value:await list(b.sessionId,offset,limit)});
  };
}

export function mountNativeProjectionRoute(ctx,dependencies){
  const handler=createNativeProjectionHandler(dependencies);
  return ctx.inject(['connection'],child=>child.effect(()=>child.connection.fetch.register({
    path:'/api/sic-core/native-projections',methods:['POST'],requestBody:'buffered',fetch:handler,
  }),'sic-core: native adopted projections'));
}

// Native version and I cursors belong to Core; offset belongs only to host Editor traces.
export function createProjectionHistoryReader({clientFor,traces}) {
  return async(id,record,offset,limit,cursors={})=>({
    core:record.source_kind==='host-hierarchy'?record.hierarchy:
      await clientFor(id).history({source_id:record.source_id,limit,...cursors}),
    editor:await traces(id,record.source_id,offset,limit),
  });
}
