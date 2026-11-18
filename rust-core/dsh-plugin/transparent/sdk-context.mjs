import{isDeepStrictEqual}from'node:util';
import{digest,editNativeContext}from'./transport.mjs';
import{projectHarnessContext,contextRequestId}from'./context-project.mjs';
import{materializeEditorDelivery}from'./edit-capabilities.mjs';
import{getEditorInvocation}from'./editor-invocation.mjs';
import{withSdkAdoption}from'./sdk-invocation.mjs';
import{collectPendingNativeVolatile,bindPendingNativeVolatile}from'./native-volatile.mjs';
import{nativeCollectionProvenances}from'./collection-provenance.mjs';
import{assertImageDeliveryDisabled}from'./image-compression.mjs';
import{adoptedImageView,imageCallsFromMessage}from'./image-source-tool.mjs';

const json=value=>JSON.parse(JSON.stringify(value,(_key,v)=>v instanceof AbortSignal?undefined:v));
function originsOf(options){const rows=[];for(let m=0;m<options.messages.length;m++){const message=options.messages[m];for(let b=0;b<message.content.length;b++){const block=message.content[b];if(block.type==='text'&&typeof block.text==='string'&&!['system','developer'].includes(message.role))rows.push({message_id:message.id,block_index:b,source_kind:message.source?.kind??(message.role==='assistant'?'model':'unknown'),role:message.role,call_id:message.toolCallId,text:block.text,anchor:['messages',m,'content',b,'text']});}}return rows;}

export function installSdkContext(ctx,{cfg,api,system,mode,store,clientFor,bindingFor,guards,publisher,recordSession,sessionAPI,editorDriver,adoptPublication,drainHigher,imageCompression,imageSource}){
 if(ctx.llm.contextDispatchVersion!==1)throw Error('primary transparent integration requires the common native LlmRuntime SDK context-dispatch source patch');
 async function* middleware(exchange,next){
  const editor=getEditorInvocation();if(editor){editor.verifyNativeOptions(exchange.options);exchange.addFinalGuard(actual=>editor.verifyNativeOptions(actual));yield*next();return;}
  const options=exchange.options,sessionId=options?.sessionId,binding=bindingFor(sessionId);
  if(!binding||options.purpose){yield*withSdkAdoption(null,()=>next());return;}
  const agent=ctx.agents.get(sessionId),guard=guards.get(agent);if(!agent||!guard)throw Error('reviewed exact live SDK Agent required');
  imageSource?.begin(agent);
  const systemMessage=options.messages.findLast(m=>m.role==='system'),runtime=options.messages.findLast(m=>m.source?.kind==='runtime-context');
  guard({renderedSystem:options.system??systemMessage?.content.filter(b=>b.type==='text').map(b=>b.text).join(''),tools:options.tools??[],contexts:runtime?.source.sections??[],scope:agent});
  const {openTurnStartSeq,lastStepStartSeq}=ctx.sessionProjections.stateOf(agent.session,'turnBoundary')??{};
  const boundary={openTurnStartSeq,lastStepStartSeq,contentGeneration:agent.session.surface?.contentGeneration};
  const requestId=contextRequestId(sessionId,boundary,options),core=clientFor(binding);
  const projected=await store.context(sessionId,requestId,async()=>{
   const cut=await core.operate('cut.get',{},options.signal),plan=await projectHarnessContext({options,history:options.messages,cut,requestId,api,system,imageCompression});
   plan.collection_records=agent.session.snapshotEvents().map(sessionAPI.pluginRecordOf).filter(r=>r?.type==='plugin:sic/jobs/collection').map(r=>r.data);
   plan.volatile_plan=mode==='edit'?await collectPendingNativeVolatile({core,api,chainId:binding.chain_id,signal:options.signal}):{chain_id:binding.chain_id,sources:[],messages:[]};
   plan.projected_history=[...plan.shared_messages,...plan.volatile_plan.messages,...plan.projected_history.slice(plan.shared_messages.length)];plan.context_digest=digest(plan.projected_history);return plan;
  });
  const masked=new Set(projected.masked_origins.map(o=>JSON.stringify([o.message_id,o.block_index])));
  const request=json({...options,messages:projected.projected_history}),origins=originsOf(request),volatile=bindPendingNativeVolatile(projected.volatile_plan??{sources:[]},origins),editable=mode==='passthrough'?[]:[...volatile.editable_origins,...origins.filter(o=>o.text.length>0&&['user','model','tool'].includes(o.source_kind)&&!masked.has(JSON.stringify([o.message_id,o.block_index])))];
  const protected_cb_bindings=projected.cb_sdk_bindings.map(b=>{const origin=origins.find(o=>o.message_id===b.message_id&&o.block_index===b.block_index);if(!origin)throw Error('SDK selected CB text anchor missing');return{cb_id:b.cb_id,native_anchors:[origin.anchor]};});
  let adopted_i;if(projected.instant){const leaf=origins.find(o=>o.message_id===projected.instant_message.id&&o.block_index===0);if(!leaf||leaf.text!==projected.instant.section_text)throw Error('SDK instant view proof missing');adopted_i={...projected.instant,native_anchor:leaf.anchor};}
  const mediaPointers=new Set(projected.masked_origins.filter(o=>o.kind==='media-pointer').map(o=>JSON.stringify([o.message_id,o.block_index])));
  const host_opaque_origins=[];
  for(let m=0;m<request.messages.length;m++)for(let b=0;b<request.messages[m].content.length;b++){
   if(!['image','file'].includes(request.messages[m].content[b].type))continue;
   const relocation=projected.media_bindings?.find(o=>o.carrier_message_id===request.messages[m].id&&o.carrier_block_index===b);
   host_opaque_origins.push({message_id:relocation?.message_id??request.messages[m].id,block_index:relocation?.block_index??b,native_anchor:['messages',m,'content',b]});
  }
  const routes=await core.operate('chain.list',{},options.signal),cut=(routes.routes??[]).find(r=>r.chain_id===binding.chain_id)?.cut_seq;
  const invocation={actor:cfg.actor,session_id:sessionId,request_id:requestId,role:binding.role??'WORKER',format:'dsh-messages',expected_cut:cut,idempotency_key:'dsh-sdk:'+requestId,
   editable_anchors:editable.map(o=>o.anchor),protected_cb_bindings,adopted_i,host_opaque_origins,host_message_origins:origins.filter(o=>!mediaPointers.has(JSON.stringify([o.message_id,o.block_index]))).map(o=>({message_id:o.message_id,block_index:o.block_index,native_anchor:o.anchor})),volatile_source_bindings:volatile.volatile_source_bindings,provenances:[...volatile.provenances,...nativeCollectionProvenances(projected.collection_records,origins,editable,binding.chain_id)],host_binding:{origins,context_projection:projected}};
  const selectEdits=async assigned=>{
   const history=projected.original_history;
   const delivery=await editorDriver({...assigned,role:'EDITOR',system,originalOptions:options,shared_messages:projected.shared_messages,
     runtime_sections:projected.runtime_sections,instant_message:projected.instant_message,image_compression:imageCompression?.offer(history)});
   const args=imageCompression?await imageCompression.prepare({args:delivery,history,sessionId,signal:assigned.signal}):assertImageDeliveryDisabled(delivery);
   return materializeEditorDelivery({core,source:assigned.source,args,allowedAnchors:invocation.editable_anchors,binding:bindingFor(sessionId),signal:assigned.signal});
  };
  const transaction=await editNativeContext({core,invocation,rawOriginal:JSON.stringify(request),signal:options.signal,selectEdits,
   adoptPublished:async({source,candidate_request,candidate_version,publication,signal})=>{
    const id=digest({request_id:requestId,publication_id:publication.publication_id,kind:'sdk-published-context'}),effective=await store.context(sessionId,id,async()=>{
     const hierarchy=drainHigher?await drainHigher({agent,core,options,requestId,signal,trace_source_id:source.source_id}):null;
     const cut=await core.operate('cut.get',{},signal);let i_projection=null;
     if(projected.instant&&source.binding?.edit_capabilities)i_projection=(await core.refreshI({source_id:source.source_id,expected_version:candidate_version,cut_id:cut.cut_id,idempotency_key:'sdk-i:'+requestId+':'+cut.cut_id},signal)).active_i_projection;
     const plan=await projectHarnessContext({options,history:projected.original_history,cut,requestId,api,system,imageCompression,instantMessageId:projected.instant_message?.id});plan.active_i_projection=i_projection;plan.hierarchy=hierarchy;return plan;
    });
    const adopted={...request,messages:effective.projected_history};return{request:json(adopted),raw_request:JSON.stringify(adopted),layout_receipt:{kind:'harness-sdk-canonical-cut-adoption',layout_id:id,publication_id:publication.publication_id,selected_cut_id:effective.selected_cut_id,selected_sequence:effective.selected_sequence,candidate_digest:digest(candidate_request),projected_history:effective.projected_history,shared_messages:effective.shared_messages,media_bindings:effective.media_bindings,instant:effective.instant,image_view:effective.image_view,active_i_projection:effective.active_i_projection,hierarchy:effective.hierarchy}};
   },admit:cfg.admitFinal??(async()=>{}),publishProjection:record=>publisher(record,options.signal)});
  const adopted=Object.freeze({...options,messages:transaction.request.messages.map(api.freezeMessage)}),expected=json(adopted),signal=options.signal;
  if(transaction.projection.publication&&adoptPublication){
   const decision=await adoptPublication({agent,publication:transaction.projection.publication,request_id:requestId,adopted_cut_id:transaction.projection.layout_receipt?.selected_cut_id});
   if(decision.adoptions?.length||decision.skipped?.length)await store.observe(sessionId,transaction.projection.id,{stage:'collection-adoption',...decision});
  }
  const verify=actual=>{
   if(actual.signal!==signal||!isDeepStrictEqual(json(actual),expected))throw Error('SDK context changed after durable host adoption');
   if(imageSource&&agent.session.surface.contentGeneration!==boundary.contentGeneration)throw Error('native image surface changed before dispatch');
  };
  exchange.options=adopted;exchange.addFinalGuard(verify);
  const observe=async event=>{try{const record={projection_id:transaction.projection.id,request_id:requestId,...event};await store.observe(sessionId,transaction.projection.id,record);await recordSession(agent.session,'sic/sdk-dispatch',record);}catch{try{ctx.logger?.warn('SIC dispatch observation could not be persisted');}catch{}}};
  const authority=Object.freeze({source_id:transaction.projection.source_id,session_id:sessionId,request_id:requestId,projection_id:transaction.projection.id,verifyNativeOptions:verify,observe});
  try{
   let finish;
   const assembler=imageSource&&adoptedImageView(transaction.projection)?.active?new api.BlockAssembler():null;
   for await(const chunk of withSdkAdoption(authority,()=>next())){
    if(chunk.type==='finish')finish=chunk.reason;
    assembler?.push(chunk);
    yield chunk;
   }
   if(assembler&&finish?.kind==='tool-calls'){
    const calls=imageCallsFromMessage(assembler.message({provider:options.provider,model:options.model}));
    imageSource.finish(agent,transaction.projection,projected.original_history,boundary,calls);
   }
   await observe({stage:finish?.kind==='error'||finish?.kind==='aborted'?'adapter-stream-failed':'adapter-stream-finished',...(finish?{finish}:{})});
  }
  catch(error){await observe({stage:'adapter-stream-failed'});throw error;}
 }
 const dispose=ctx.on('llm/context-dispatch',middleware);return{middleware,dispose};
}
