// Provider-owned wire hook. Installs no routes, models or admission policy.
import { createHash, randomUUID } from 'node:crypto';
import { isDeepStrictEqual } from 'node:util';
import {applyRawTextEdits} from './native-json.mjs';
export const digest = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');
export const wireDigest = raw => createHash('sha256').update(raw,'utf8').digest('hex');

export function validateProjection(before, after, units, edits) {
  const expected=structuredClone(before), byId=new Map(units.map(u=>[u.unit_id,u])), seen=new Set();
  for(const edit of edits) {
    if(seen.has(edit.unit_id))throw Error('duplicate native edit');
    seen.add(edit.unit_id);
    const unit=byId.get(edit.unit_id);
    if(!unit||!Array.isArray(unit.anchor)||!unit.anchor.length)throw Error('unknown native anchor');
    let target=expected;
    for(const key of unit.anchor.slice(0,-1)) {
      if(!Object.hasOwn(target??{},key))throw Error('native anchor missing');
      target=target[key];
    }
    const key=unit.anchor.at(-1);
    if(typeof target?.[key]!=='string'||target[key]!==unit.text||target[key]!==edit.expected_text
      ||typeof edit.replacement!=='string')throw Error('native text conflict');
    target[key]=edit.replacement;
  }
  if(!isDeepStrictEqual(expected,after))throw Error('native fields changed outside authorized text');
}

// Protocol-native transaction shared by HTTP hooks and the Harness SDK port.
// No HTTP dispatch, response parsing or Provider policy exists in this helper.
export async function editNativeContext({core,invocation,rawOriginal,signal=new AbortController().signal,selectEdits,publishProjection,admit,adoptPublished,publicationRequired=true}){
    if(!invocation.session_id||!invocation.actor||!invocation.format||!invocation.request_id)throw Error('host invocation incomplete');
    const readonly=Array.isArray(invocation.editable_anchors)&&invocation.editable_anchors.length===0;
    if(publicationRequired&&!readonly&&(typeof core.publish!=='function'||!Number.isSafeInteger(invocation.expected_cut)||invocation.expected_cut<0
      ||typeof invocation.idempotency_key!=='string'||!invocation.idempotency_key))throw Error('chain-bound native publication metadata required');
    signal.throwIfAborted();
    const body=JSON.parse(rawOriginal);
    const scope={wire_sha256:wireDigest(rawOriginal),...(invocation.editable_anchors?{editable_anchors:invocation.editable_anchors}:{}),
      ...(invocation.protected_cb_bindings?{protected_cb_bindings:invocation.protected_cb_bindings}:{}),...(invocation.idempotency_key?{idempotency_key:invocation.idempotency_key}:{}),
      ...(invocation.adopted_i?{adopted_i:invocation.adopted_i}:{}),...(invocation.host_message_origins?{host_message_origins:invocation.host_message_origins}:{}),
      ...(invocation.host_opaque_origins?.length?{host_opaque_origins:invocation.host_opaque_origins}:{}),
      ...(invocation.provenances?.length?{provenances:invocation.provenances}:{}),...(invocation.volatile_source_bindings?.length?{volatile_source_bindings:invocation.volatile_source_bindings}:{})};
    if(invocation.native_step_id)scope.native_step_id=invocation.native_step_id;
    if(invocation.tool_wait)scope.tool_wait=invocation.tool_wait;
    const begun=typeof core.beginRaw==='function'
      ?await core.beginRaw(invocation.format,rawOriginal,signal,scope)
      :await core.begin({format:invocation.format,request:body,...scope},signal);
    if(!Array.isArray(begun?.units)||typeof begun.source_digest!=='string'||!Number.isSafeInteger(begun.version))throw Error('invalid native.begin receipt');
    const reused=!!begun.publication;
    const recovering=!reused&&begun.version>0;
    if(recovering&&(!begun.current_delivery||begun.delivery_version!==begun.version))throw Error('committed native candidate has no recoverable delivery; explicit recovery required');
    const originalUnits=begun.units.map(u=>({...u,text:begun.source_request?u.anchor.reduce((v,k)=>v[k],begun.source_request):u.text}));
    const candidateRequest=begun.candidate_request??begun.request;
    const recoveredEdits=()=>originalUnits.filter(u=>u.text!==u.anchor.reduce((v,k)=>v[k],candidateRequest)).map(u=>({unit_id:u.unit_id,expected_text:u.text,replacement:u.anchor.reduce((v,k)=>v[k],candidateRequest)}));
    const delivery=reused?{edits:recoveredEdits()}:recovering?{...begun.current_delivery,edits:recoveredEdits()}:readonly?[]:await selectEdits({invocation:structuredClone(invocation),source:structuredClone(begun),request:structuredClone(body),signal:signal});
    const edits=Array.isArray(delivery)?delivery:delivery?.edits;
    if(!Array.isArray(edits))throw Error('edit array required');
    signal.throwIfAborted();
    if(invocation.editable_anchors){const allowed=new Set(invocation.editable_anchors.map(JSON.stringify));for(const e of edits){const u=begun.units.find(u=>u.unit_id===e.unit_id);if(!u||!allowed.has(JSON.stringify(u.anchor)))throw Error('edit outside host-assigned current WB');}}
    const committed=reused||recovering?{...begun,request:candidateRequest}:readonly?{version:begun.version,receipt:null,request:body}:await core.commit({source_id:begun.source_id,expected_source_digest:begun.source_digest,expected_version:begun.version,edits,
      ...(Array.isArray(delivery?.ops)?{delivery:{ops:delivery.ops,pvi:delivery.pvi??{}}}:{})},signal);
    if((!readonly&&!committed?.receipt)||!committed.request)throw Error('invalid native.commit receipt');
    validateProjection(body,committed.request,originalUnits,edits);
    let publication=begun.publication??null;
    if(publicationRequired&&!readonly&&!reused){
      const published=await core.publish({source_id:begun.source_id,expected_source_digest:begun.source_digest,
        expected_version:committed.version??committed.receipt.version,expected_cut:invocation.expected_cut,idempotency_key:invocation.idempotency_key,
        ...(Array.isArray(delivery?.ops)?{ops:delivery.ops}:{}),pvi:delivery?.pvi??{}},signal);
      if(!published?.publication?.publication_id||!published.publication.cb_id||!published.request)throw Error('native CB publication receipt missing');
      validateProjection(body,published.request,originalUnits,edits);publication=published.publication;
    }
    let rawAdopted=applyRawTextEdits(rawOriginal,originalUnits,edits),adopted=committed.request,layout=null;
    if(publication&&adoptPublished){
      const effective=await adoptPublished({invocation,source:begun,candidate_request:committed.request,candidate_version:committed.version??committed.receipt.version,raw_candidate:rawAdopted,publication,signal:signal});
      if(!effective?.layout_receipt||typeof effective.raw_request!=='string'||!isDeepStrictEqual(JSON.parse(effective.raw_request),effective.request))throw Error('post-publication native layout proof required');
      rawAdopted=effective.raw_request;adopted=effective.request;layout=effective.layout_receipt;
    }
    await admit({invocation,original:body,request:adopted,candidate_request:committed.request,receipt:committed.receipt,layout_receipt:layout,signal:signal});
    if(!isDeepStrictEqual(JSON.parse(rawAdopted),adopted))throw Error('admission changed the adopted context without a native layout receipt');
    const projection={id:digest({session:invocation.session_id,request:invocation.request_id,source:begun.source_id}),session_id:invocation.session_id,request_id:invocation.request_id,
      actor:invocation.actor,role:invocation.role,format:invocation.format,source_id:begun.source_id,
      source_digest:begun.source_digest,units:originalUnits,edits,receipt:committed.receipt,
      original:body,candidate_request:committed.request,adopted,raw_original:rawOriginal,raw_adopted:rawAdopted,layout_receipt:layout,
      adopted_digest:wireDigest(rawAdopted),publication,host_binding:invocation.host_binding??null,
      stage:invocation.tool_wait?'tool-wait-published':readonly?'admitted-readonly':publicationRequired?'admitted':'candidate-only-fixture'};
    const freeze=value=>{if(value&&typeof value==='object'){for(const child of Object.values(value))freeze(child);Object.freeze(value);}return value;};freeze(projection);
    const ack=await publishProjection(projection,signal);
    if(ack?.recorded!==true||ack.adopted_digest!==projection.adopted_digest)throw Error('actual adopted projection not acknowledged by host');
    signal.throwIfAborted();
    return{projection,request:adopted,raw_request:rawAdopted};
}

// The HTTP caller explicitly binds this hook to its existing Provider transport.
// Auth headers never go to SIC. resolveInvocation is host-bound, not body-derived.
export function createTransparentFetch({fetchImpl=globalThis.fetch,core,resolveInvocation,selectEdits,publishProjection,admit,adoptPublished,publicationRequired=true,observeDispatch=async()=>{},onObservationError=async()=>{}}) {
  if(![resolveInvocation,selectEdits,publishProjection,admit,core?.begin,core?.commit].every(f=>typeof f==='function'))throw TypeError('complete host bindings required');
  const observe=async event=>{try{await observeDispatch(event);}catch(error){try{await onObservationError(error,event);}catch{}}};
  return async(input,init)=>{
    const original=new Request(input,init), invocation=await resolveInvocation(original);
    if(invocation==null)return fetchImpl(original);
    const rawOriginal=await original.clone().text();
    const transaction=await editNativeContext({core,invocation,rawOriginal,signal:original.signal,selectEdits,publishProjection,admit,adoptPublished,publicationRequired});
    const{projection,raw_request:rawAdopted}=transaction;
    const headers=new Headers(original.headers);headers.delete('content-length');
    // Same URL, headers/credentials, method, signal and controls; only body text
    // anchors change. Return the ORIGINAL response without reading/buffering it.
    let response;
    try{response=await fetchImpl(new Request(original,{headers,body:rawAdopted}));}
    catch(error){await observe({projection_id:projection.id,request_id:invocation.request_id,stage:'dispatch-failed'});throw error;}
    await observe({projection_id:projection.id,request_id:invocation.request_id,stage:'response-returned',status:response.status,
      content_type:response.headers.get('content-type'),provider_request_id:response.headers.get('request-id')??response.headers.get('x-request-id')});
    return response;
  };
}

// Operate is the existing authenticated Core client; actor stays out-of-band.
export function createNativeCoreClient(operate) {
  return Object.fromEntries(['begin','commit','get','publish'].map(name=>[name,async(params,signal)=>{
    const value=await operate(`native.${name}`,params,signal);
    if(value?.error)throw Error(`native.${name}: ${value.error.code??'rejected'}`);
    return value.result??value;
  }]));
}
