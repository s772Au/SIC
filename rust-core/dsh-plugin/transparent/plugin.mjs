import {readFile} from 'node:fs/promises';
import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual} from 'node:util';
import {createCoreClient} from './core-client.mjs';
import {createTransparentFetch,digest,wireDigest} from './transport.mjs';
import {createFileProjectionStore} from './file-store.mjs';
import {createProjectionPublisher,createProjectionHistoryReader,mountNativeProjectionRoute} from './projection.mjs';
import {createAdmissionGuard,registerReviewedPrompt} from './prompt.mjs';
import {installCoreGoalRuntime} from './goal-runtime.mjs';
import {installGoalCompatibility} from './harness-facade.mjs';
import {createTurnGrantResolver} from './turn-authority.mjs';
import {messagesOrigins,messagesBlockOrigins} from './origins.mjs';
import {projectHarnessContext,contextRequestId} from './context-project.mjs';
import {createSessionRecorder} from './session-records.mjs';
import {materializeEditorDelivery,materializeWithEditor} from './edit-capabilities.mjs';
import {composeEditorDriver} from './driver-composition.mjs';
import {getEditorInvocation} from './editor-invocation.mjs';
import {repackageNativeContext} from './native-layout.mjs';
import {nativeCollectionProvenances} from './collection-provenance.mjs';
import {installSdkContext} from './sdk-context.mjs';
import {getSdkAdoption} from './sdk-invocation.mjs';
import {ARCHIVE_EXPAND,ARCHIVE_OPS,hasNativePageGrant,protectedWorkerText,readNativeSourcePage,SOURCE_PAGE,SOURCE_SEARCH} from './source-page.mjs';
import {assertNativeJobsRuntime,installCoreJobsRuntime} from './jobs-runtime.mjs';
import {createHostHierarchyRuntime} from './host-hierarchy.mjs';
import {installCoreCompactionRuntime} from './compaction-runtime.mjs';
import {createInstallationLifetime} from './installation-lifetime.mjs';
import {createSessionAdmission} from './session-admission.mjs';
import {assertImageDeliveryDisabled,createImageCompression} from './image-compression.mjs';
import {adoptedImageView,createImageSourceTool,IMAGE_SOURCE_DESCRIPTOR} from './image-source-tool.mjs';
import {editorLimits} from './editor-driver.mjs';

export const name='sic-transparent-host';
export const inject=['llm','systemPrompt','tools','agents','sessions','sessionProjections','connection','goals','jobs'];
const valueAt=(obj,path)=>path.reduce((v,k)=>v[k],obj);
const bundle=filename=>readFile(new URL('./bundle/'+filename,import.meta.url),'utf8');

// Explicit opt-in plugin, independently installable by this module path. It
// registers no Provider/model and never changes an existing profile by itself.
export async function apply(ctx,cfg={}){
  if(cfg.enabled!==true)return;
  const mode=cfg.mode??'edit';if(!['edit','passthrough'].includes(mode))throw Error('invalid transparent host mode');
  const transport=cfg.transport??'sdk';if(!['sdk','native-wire'].includes(transport))throw Error('invalid transparent transport');
  if(transport==='sdk'&&ctx.llm.contextDispatchVersion!==1)throw Error('primary transparent integration requires the common LlmRuntime SDK source patch');
  if(!cfg.actor||!cfg.coreURL||!cfg.projectionRoot||typeof cfg.credential!=='function'||(transport==='native-wire'&&typeof cfg.admitFinal!=='function'))throw Error('transparent host requires actor/Core/persistence/credential and original host bindings');
  editorLimits(cfg.editorLimits);
  assertNativeJobsRuntime(ctx);
  const baseline=cfg.baseline??'fused';if(!['native','append-only','fused'].includes(baseline))throw Error('invalid reviewed baseline');
  const system=await bundle(baseline+'.md'),expectedNative=await bundle('native.md');
  const extras=JSON.parse(await bundle('sic-extra-tools.json'));
  const renderPrompt=cfg.renderPrompt??(await import('@deepseek-ai/dsh-system-prompt')).renderPrompt;
  const resolveBinding=(sessionId,agent)=>{const binding=cfg.bindingFor?.(sessionId,agent)??cfg.bindings?.[sessionId];return binding&&transport==='sdk'?{...binding,installed_native_bindings:[...(binding.installed_native_bindings??[]),'native-volatile-next-WB-v1']}:binding;};
  const store=createFileProjectionStore(cfg.projectionRoot),guards=new WeakMap();
  if(cfg.imageCompression===true&&(transport!=='sdk'||mode!=='edit'))throw Error('image compression requires native SDK edit mode');
  const imageCompression=cfg.imageCompression===true?createImageCompression({
    store,attachments:cfg.attachments??ctx.get('attachments'),sharp:cfg.imageSharp??(await import('sharp')).default,
  }):null;
  const lifetime=createInstallationLifetime(ctx);
  const clientFor=b=>createCoreClient({baseURL:cfg.coreURL,credential:cfg.credential,
    requestTimeoutMs:cfg.coreRequestTimeoutMs,
    operationTimeoutMs:cfg.coreOperationTimeoutMs,
    context:()=>({actor:cfg.actor,chain_id:b?.chain_id??null,allow_write:true,allow_model:false,allow_revoke:false}),fetchImpl:cfg.coreFetch??globalThis.fetch});
  const sessionAPI=cfg.sessionAPI??await import('@deepseek-ai/dsh-session');
  const lifecycleClient=clientFor(null),recordSession=await createSessionRecorder(ctx,sessionAPI);
  const sessionAdmission=createSessionAdmission({ctx,actor:cfg.actor,coreURL:cfg.coreURL,root:cfg.projectionRoot,clientFor,recordSession,sessionAPI,resolveBinding});
  const bindingFor=(id,agent=ctx.agents.get(id))=>sessionAdmission.bindingFor(id,agent,resolveBinding(id,agent));
  const imageSource=imageCompression&&createImageSourceTool({imageCompression,bindingFor,clientFor,store,liveAgent:id=>ctx.agents.get(id),
    stepOf:session=>{
      const {openTurnStartSeq,lastStepStartSeq}=ctx.sessionProjections.stateOf(session,'turnBoundary')??{};
      return {openTurnStartSeq,lastStepStartSeq,contentGeneration:session.surface.contentGeneration};
    }});
  // Reject existing native history before any lifecycle module projects a
  // receipt or takes ownership of its original services.
  for(const agent of ctx.agents.list()){
    const binding=resolveBinding(agent.session.id,agent);
    if(binding&&agent.status==='running')throw Error('install transparent host before selected Agent starts');
    await sessionAdmission.admit(agent,binding);
  }
  // AgentManager awaits these listeners serially before releasing queued work.
  // Enroll before Goal/Jobs/compaction subscribe to the same creation boundary.
  ctx.on('agent/created',({agent})=>sessionAdmission.admit(agent,resolveBinding(agent.session.id,agent)));
  const api=cfg.llmAPI??await import('@deepseek-ai/dsh-llm');
  const editorDriver=mode==='edit'?await composeEditorDriver({ctx,cfg,store,clientFor,bindingFor,recordSession}):null;
  const authority=createTurnGrantResolver({ctx,client:lifecycleClient,roleFor:a=>bindingFor(a.session.id,a)?.role??'WORKER',
    scopeFor:a=>bindingFor(a.session.id,a)?.scope_id,allowedCallsFor:a=>bindingFor(a.session.id,a)?.lifecycle_calls??['get_goal','create_goal','update_goal'],
    chainFor:a=>bindingFor(a.session.id,a)?.chain_id??null,maxRoundsFor:a=>bindingFor(a.session.id,a)?.max_goal_rounds??cfg.defaultMaxGoalRounds??ctx.goals.defaultMaxGoalRounds??null,
    goalFor:async(a,signal)=>{const value=await lifecycleClient.operate('harness.call',{scope_id:bindingFor(a.session.id,a)?.scope_id,name:'get_goal',arguments:{}},signal);return value.value.goal;}});
  const agentEvents=cfg.agentEvents??(await import('@deepseek-ai/dsh-agent')).agentEvents;
  const goalRuntime=await installCoreGoalRuntime(ctx,{client:lifecycleClient,bindingFor,resolveGrant:authority,
    appendRecord:sessionAPI.appendPluginRecord,
    emitGoalChanged:(agent,change)=>agentEvents(ctx,agent).emit('goal/changed',{change})});
  const jobsRuntime=await installCoreJobsRuntime(ctx,{client:lifecycleClient,bindingFor,appendRecord:sessionAPI.appendPluginRecord,onCollection:cfg.onCollection});
  const publisher=createProjectionPublisher({save:store.save,getSession:id=>ctx.sessions.get(id),
    appendEvent:async(session,event)=>recordSession(session,event.type,event.data,{key:event.data.projection_id})});
  const hierarchy=createHostHierarchyRuntime({ctx,cfg,api,system,store,clientFor,bindingFor,recordSession,publisher,imageCompression});
  const drainHigher=cfg.drainHigher??hierarchy.drainHigher;
  const compactions=new WeakMap(),compactionRuns=[];
  async function mountCompaction(child){
    if(mode!=='edit'||!child.compaction)return;
    if(compactions.has(child.compaction))return compactions.get(child.compaction);
    const run=installCoreCompactionRuntime(child,{bindingFor,schedule:hierarchy.scheduleCompaction,appendRecord:sessionAPI.appendPluginRecord});
    compactions.set(child.compaction,run);compactionRuns.push(run);return run;
  }
  if(mode==='edit'){
    if(ctx.get('compaction'))await mountCompaction(ctx);
    ctx.inject(['compaction'],mountCompaction);
  }
  mountNativeProjectionRoute(ctx,{authorize:async(request,id)=>!!bindingFor(id)&&!!ctx.sessions.get(id)
    &&(cfg.authorizeSession?await cfg.authorizeSession(request,id):true),list:store.list,
    get:async(id,projectionId)=>{const record=await store.get(id,projectionId);if(record){const core=clientFor(bindingFor(id));if(record.source_kind==='host-hierarchy')await hierarchy.authorizeRecord(record,core);else await core.get({source_id:record.source_id});}return record;},
    history:createProjectionHistoryReader({clientFor: id=>clientFor(bindingFor(id)),traces:store.traces}),
    image: imageCompression&&((id,record,assetId,signal)=>{
      const view=adoptedImageView(record);
      return view?imageCompression.readAsset(id,view.pointer,assetId,signal):null;
    })});

  async function mountAgent(agent){
    const binding=resolveBinding(agent.session.id,agent);if(!binding)return;
    if(!binding.chain_id||!binding.scope_id)throw Error('selected session requires an existing native chain/scope binding');
    await sessionAdmission.admit(agent,binding);
    await lifetime.mount(agent);
    await new Promise((resolve,reject)=>agent.ctx.inject(['tools','systemPrompt'],child=>{void(async()=>{
      const auth=()=>({role:bindingFor(agent.session.id,agent)?.role??'WORKER',scope_id:binding.scope_id,
        ...(bindingFor(agent.session.id,agent)?.edit_capabilities?{edit_capabilities:bindingFor(agent.session.id,agent).edit_capabilities}:{})});
      // Compare before adding any new section/tool, against the archived native source.
      await registerReviewedPrompt({systemPrompt:child.systemPrompt,expectedNative,renderPrompt,system,authorization:auth,scope:agent});
      installGoalCompatibility({tools:child.tools,scope:agent,facade:goalRuntime.facade});
      if(imageSource)child.tools.register({...IMAGE_SOURCE_DESCRIPTOR,output:{schema:{},render:imageSource.render},execute:imageSource.execute});
      for(const descriptor of extras)child.tools.register({...descriptor,output:{schema:{},render:(_args,value)=>[{type:'text',text:JSON.stringify(value)}]},execute:async(args,exec)=>{
        const current=bindingFor(agent.session.id,agent),core=clientFor(current);
        if(descriptor.name===SOURCE_SEARCH){
          if((current.role??'WORKER')!=='WORKER')throw Error('published archive search requires an authorized Worker');
          return core.operate('memory.archive_search',args,exec.signal);
        }
        if(descriptor.name===ARCHIVE_EXPAND||descriptor.name===ARCHIVE_OPS){
          if((current.role??'WORKER')!=='WORKER')throw Error('published archive navigation requires an authorized Worker');
          return core.operate(descriptor.name===ARCHIVE_EXPAND?'memory.archive_expand':'memory.archive_ops',args,exec.signal);
        }
        if(descriptor.name===SOURCE_PAGE){
          const scope=current.role==='EDITOR'?current.editor_scope:null;
          if(!scope&&current.role!=='EDITOR'&&!hasNativePageGrant(current.readable_sources,args)){
            return core.operate('memory.source_text_page',args,exec.signal);
          }
          return readNativeSourcePage({core,authority:{source_id:scope?.source_id,source_digest:scope?.source_digest,expected_version:scope?.version??scope?.expected_version,
            assigned_unit_ids:scope?.assigned_unit_ids??[],readable_sources:scope?.readable_sources??(current.role==='EDITOR'?current.editor_readable_sources:current.readable_sources)??[]},args,signal:exec.signal});
        }
        if(current.role!=='EDITOR'||!current.editor_scope)throw Error('Editor candidate not activated');
        const s=current.editor_scope,source=await core.get({source_id:s.source_id},exec.signal),delivery=await materializeEditorDelivery({core,source,args,allowedAnchors:s.editable_anchors,binding:current,signal:exec.signal});
        assertImageDeliveryDisabled(args);
        const {edits,ops,pvi}=delivery;
        const committed=await core.commit({source_id:s.source_id,expected_source_digest:s.source_digest,expected_version:s.version,edits,delivery:{ops,pvi},native_tool:{name:'sic_edit_blocks',arguments_sha256:digest(args)}},exec.signal);
        return core.publish({source_id:s.source_id,expected_source_digest:s.source_digest,expected_version:committed.version,expected_cut:s.expected_cut,idempotency_key:s.idempotency_key,ops,pvi},exec.signal);
      }});
      child.tools.guard(exec=>{const role=auth().role;if(role==='WORKER'&&exec.name==='sic_edit_blocks')return'Editor-only candidate submission';
        if(role==='EDITOR'&&!['sic_edit_blocks','sic_source_text_page','read'].includes(exec.name))return'not authorized for Editor';
        if(role==='EDITOR'&&exec.name==='read'&&!bindingFor(agent.session.id,agent)?.editor_scope?.readable_paths?.includes(exec.arguments.file_path))return'outside Editor source projection';});
      const {tools}=await child.systemPrompt.assemble({scope:agent});
      guards.set(agent,createAdmissionGuard({system,tools,authorization:auth}));
      resolve();
    })().catch(reject);}));
  }
  ctx.on('agent/created',async({agent})=>{await mountAgent(agent);});
  for(const agent of ctx.agents.list()){
    if(bindingFor(agent.session.id,agent)){if(agent.status==='running')throw Error('install transparent host before selected Agent starts');await mountAgent(agent);}
  }
  const adoptPublication=async args=>{
    if(!(args.publication?.publication?.collection_adoptions??[]).length)return{recorded:true,adoptions:[],skipped:[]};
    const ack=await jobsRuntime.adoptPublication(args);if(ack?.recorded!==true)throw Error('actual native collection adoption acknowledgement missing');
    await recordSession(args.agent.session,'sic/collection-adoption',ack,{key:args.publication.publication_id});return ack;
  };
  const sdkRuntime=transport==='sdk'?installSdkContext(ctx,{cfg,api,system,mode,store,clientFor,bindingFor,guards,publisher,recordSession,sessionAPI,editorDriver,adoptPublication,imageCompression,imageSource,drainHigher:mode==='edit'?drainHigher:null}):null;

  // The original serializer receives this SDK projection. The projected
  // prefix is the same system + selected CB messages for Worker and Editor.
  async function projectContext(exchange,next){
    if(getSdkAdoption())return next();
    const editor=getEditorInvocation();
    if(editor){editor.verifyNativeOptions(exchange.options);return next();}
    const options=exchange.options,sessionId=options?.sessionId,binding=bindingFor(sessionId);
    if(options?.purpose)return next();
    if(!binding)return next();
    const agent=ctx.agents.get(sessionId);if(!agent)throw Error('context projection requires exact live session');
    const requestId=contextRequestId(sessionId,ctx.sessionProjections.stateOf(agent.session,'turnBoundary'),options);
    const projection=await store.context(sessionId,requestId,async()=>{
      const cut=await clientFor(binding).operate('cut.get',{},options.signal);
      const planned=await projectHarnessContext({options,history:exchange.history,cut,requestId,api,system});
      planned.collection_records=agent.session.snapshotEvents().map(sessionAPI.pluginRecordOf).filter(r=>r?.type==='plugin:sic/jobs/collection').map(r=>r.data);
      return planned;
    });
    const freeze=value=>{if(value&&typeof value==='object'){for(const child of Object.values(value))freeze(child);Object.freeze(value);}return value;};freeze(projection);
    exchange.history=projection.projected_history;Object.defineProperty(exchange,'projection',{value:projection,writable:false,configurable:false});
    const history=await next();
    if(digest(history)!==projection.context_digest)throw Error('downstream plugin changed the approved SDK context projection');
    return history;
  }
  ctx.on('llm/context-project',projectContext);
  ctx.on('llm/native-fetch',async(exchange,next)=>nativeFetch(exchange,next));
  async function pinDispatch(exchange){
    if(typeof exchange.addFinalGuard!=='function')throw Error('monotonic native transport guard source patch required');
    const approved=wireDigest(await exchange.request.clone().text());
    exchange.addFinalGuard(async actual=>{if(wireDigest(await actual.clone().text())!==approved)throw Error('native context changed after host adoption');});
  }
  async function nativeFetch(exchange,next){
    const editor=getEditorInvocation();
    if(editor){editor.verifyNativeOptions(exchange.provenance?.options);await pinDispatch(exchange);exchange.addFinalGuard(()=>editor.verifyNativeOptions(exchange.provenance?.options));return next();}
    const sdk=getSdkAdoption();
    if(sdk){
      sdk.verifyNativeOptions(exchange.provenance.options);await pinDispatch(exchange);
      await sdk.observe({stage:'native-wire-dispatch',format:exchange.format,raw_native_request:await exchange.request.clone().text()});
      try{const response=await next();await sdk.observe({stage:'response-returned',status:response.status,content_type:response.headers.get('content-type'),provider_request_id:response.headers.get('request-id')??response.headers.get('x-request-id')});return response;}
      catch(error){await sdk.observe({stage:'native-dispatch-failed'});throw error;}
    }
    const options=exchange.provenance?.options,sessionId=options?.sessionId;
    if(options?.purpose)return next();
    const binding=bindingFor(sessionId);if(!binding)return next();
    const agent=ctx.agents.get(sessionId);if(!agent||agent.session.id!==sessionId)throw Error('native request has no exact live session');
    const guard=guards.get(agent);if(!guard)throw Error('reviewed prompt not mounted before native dispatch');
    const systemMessage=options.messages.findLast(m=>m.role==='system');
    const runtime=options.messages.findLast(m=>m.source?.kind==='runtime-context');
    guard({renderedSystem:options.system??systemMessage?.content.filter(b=>b.type==='text').map(b=>b.text).join(''),tools:options.tools??[],contexts:runtime?.source.sections??[],scope:agent});
    const projection=exchange.provenance.context_projection;
    if(!projection?.request_id||digest(exchange.provenance.history)!==projection.context_digest)throw Error('trusted pre-serializer context projection required');
    const raw=await exchange.request.clone().text(),native=JSON.parse(raw),origins=messagesOrigins(exchange.provenance.history,native),requestId=projection.request_id;
    const protected_cb_bindings=projection.cb_sdk_bindings.map(b=>{
      const leaf=origins.find(o=>o.message_id===b.message_id&&o.block_index===b.block_index);
      if(!leaf)throw Error('selected CB native leaf missing');return{cb_id:b.cb_id,native_anchors:[leaf.anchor]};
    });
    const masked=new Set(projection.masked_origins.map(o=>JSON.stringify([o.message_id,o.block_index])));
    const protectedText=protectedWorkerText(projection.original_history,cfg.preserveLatestWorkerUserMessage?.(agent)===true);
    const editable=mode==='passthrough'?[]:origins.filter(o=>['user','model','tool'].includes(o.source_kind)&&!protectedText.has(o.message_id)&&!masked.has(JSON.stringify([o.message_id,o.block_index])));
    const mediaPointers=new Set(projection.masked_origins.filter(o=>o.kind==='media-pointer').map(o=>JSON.stringify([o.message_id,o.block_index])));
    const host_opaque_origins=messagesBlockOrigins(exchange.provenance.history,native,projection.media_bindings).filter(o=>o.type==='image').map(o=>{const[message_id,block_index]=JSON.parse(o.identity);return{message_id,block_index,native_anchor:o.anchor};});
    const provenances=nativeCollectionProvenances(projection.collection_records,origins,editable,binding.chain_id);
    let adopted_i;
    if(projection.instant){
      const leaf=origins.find(o=>o.message_id===projection.instant_message.id&&o.block_index===0);
      if(!leaf||leaf.text!==projection.instant.section_text)throw Error('Core I native leaf missing');
      adopted_i={...projection.instant,native_anchor:leaf.anchor};
    }
    const core=clientFor(binding),routes=await core.operate('chain.list',{},exchange.request.signal),cut=(routes.routes??[]).find(r=>r.chain_id===binding.chain_id)?.cut_seq;
    const invocation={actor:cfg.actor,session_id:sessionId,request_id:requestId,role:binding.role??'WORKER',format:exchange.format,
      expected_cut:cut,idempotency_key:'dsh-native:'+requestId,editable_anchors:editable.map(o=>o.anchor),protected_cb_bindings,adopted_i,provenances,
      host_opaque_origins,host_message_origins:origins.filter(o=>!mediaPointers.has(JSON.stringify([o.message_id,o.block_index]))).map(o=>({message_id:o.message_id,block_index:o.block_index,native_anchor:o.anchor})),
      host_binding:{harness_raw_original:raw,origins,context_projection:projection}};
    const fetcher=createTransparentFetch({core,resolveInvocation:async()=>invocation,
      selectEdits:async assigned=>materializeWithEditor(editorDriver,{...assigned,role:'EDITOR',system,provider:options.provider,model:options.model,tools:options.tools,originalOptions:options,
        shared_messages:projection.shared_messages,runtime_sections:projection.runtime_sections,instant_message:projection.instant_message,
        editable_units:assigned.source.units.filter(u=>editable.some(a=>JSON.stringify(a.anchor)===JSON.stringify(u.anchor)))},
        (delivery,signal)=>materializeEditorDelivery({core,source:assigned.source,args:assertImageDeliveryDisabled(delivery),allowedAnchors:invocation.editable_anchors,binding:bindingFor(sessionId),signal})),
      adoptPublished:async({source,candidate_request,candidate_version,raw_candidate,publication,signal})=>{
        if(typeof exchange.provenance.serializeHistory!=='function')throw Error('original native serializer re-layout source patch required');
        const layoutId=digest({request_id:requestId,publication_id:publication.publication_id,kind:'published-context'});
        const effective=await store.context(sessionId,layoutId,async()=>{
          if(mode==='edit')await drainHigher({agent,core,options,requestId,signal,trace_source_id:source.source_id});
          const cut=await core.operate('cut.get',{},signal);let i_projection=null;
          if(projection.instant&&source.binding?.edit_capabilities)i_projection=(await core.refreshI({source_id:source.source_id,expected_version:candidate_version,cut_id:cut.cut_id,idempotency_key:'native-i:'+requestId+':'+cut.cut_id},signal)).active_i_projection;
          const plan=await projectHarnessContext({options,history:projection.original_history,cut,requestId,api,system,instantMessageId:projection.instant_message?.id});plan.active_i_projection=i_projection;return plan;
        });
        const serialized=exchange.provenance.serializeHistory(effective.projected_history);
        messagesOrigins(effective.projected_history,serialized);
        if(!isDeepStrictEqual(serialized.system,native.system)||!isDeepStrictEqual(serialized.tools,native.tools))throw Error('original serializer changed the frozen public header/tools during context adoption');
        const raw_request=repackageNativeContext(raw_candidate,serialized,exchange.provenance.history,effective.projected_history,effective.media_bindings);
        return{request:JSON.parse(raw_request),raw_request,layout_receipt:{kind:'harness-canonical-cut-adoption',layout_id:layoutId,media_bindings:effective.media_bindings,
          source_id:publication.source_id??null,publication_id:publication.publication_id,selected_cut_id:effective.selected_cut_id,
          selected_sequence:effective.selected_sequence,candidate_digest:digest(candidate_request),adopted_wire_digest:wireDigest(raw_request),
          shared_messages:effective.shared_messages,projected_history:effective.projected_history,masked_origins:effective.masked_origins,instant:effective.instant,active_i_projection:effective.active_i_projection}};
      },admit:cfg.admitFinal,publishProjection:async record=>{const ack=await publisher(record,exchange.request.signal);if(record.publication){const decision=await adoptPublication({agent,publication:record.publication,request_id:requestId,adopted_cut_id:record.layout_receipt?.selected_cut_id});if(decision.adoptions?.length||decision.skipped?.length)await store.observe(sessionId,record.id,{stage:'collection-adoption',...decision});}return ack;},
      observeDispatch:async event=>{await store.observe(sessionId,event.projection_id,event);await recordSession(agent.session,'sic/native-response',event);},
      fetchImpl:async request=>{exchange.request=request;await pinDispatch(exchange);return next();}});
    return fetcher(exchange.request);
  }
  return{nativeFetch,projectContext,mountAgent,store,goalRuntime,jobsRuntime,sdkRuntime,hierarchy,compactionRuns,imageCompression,editorDriver};
}
