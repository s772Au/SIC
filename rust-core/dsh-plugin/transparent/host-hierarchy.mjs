import {randomUUID} from 'node:crypto';
import {digest,wireDigest} from './transport.mjs';
import {jsonSnapshot} from './session-records.mjs';
import {projectHarnessContext} from './context-project.mjs';
import {createHarnessEditorDriver} from './editor-driver.mjs';
import {ARCHIVE_OPS,pageSourceText,readEditorOperations,sharedOperationIds,SourcePageRequestError,sourcePageToolResult} from './source-page.mjs';
import {assertImageDeliveryDisabled} from './image-compression.mjs';
import {materializeWithEditor} from './edit-capabilities.mjs';
import {normalizeEditorDelivery} from './editor-delivery.mjs';

const OPS=['KEEP','DELETE','FLAG','REORDER','COMPRESS','REPLACE','ADD'];
const CHANNELS=['P','I'];
const attemptOf=(view,editor)=>view.host_editor?.attempts?.[editor];
function capabilitiesFor(view,binding){
  const core=view.edit_capabilities??{ops:OPS,pvi:CHANNELS},host=binding.edit_capabilities;
  const boundV=core.required_host_bindings?.V==='core-same-level-source-group-v1';
  return{ops:core.ops.filter(op=>OPS.includes(op)&&(!host||host.ops?.includes(op))),
    pvi:core.pvi.filter(kind=>(CHANNELS.includes(kind)||kind==='V'&&boundV)&&(!host||host.pvi?.includes(kind))),
    ...(core.constraints?{constraints:core.constraints}:{})};
}

// This is an executor for Rust plans, not a second hierarchy planner. Only
// runtime.schedule groups lower CBs, forms WBs, validates and publishes cuts.
export function createHostHierarchyRuntime({ctx,cfg,api,system,store,clientFor,bindingFor,recordSession,publisher,imageCompression}){
  const pending=new Map(),serial=new Map();
  const selected=agent=>{
    const binding=bindingFor(agent.session.id,agent);
    if(ctx.agents.get(agent.session.id)!==agent||!binding?.chain_id||!binding.scope_id||binding.role==='EDITOR')throw Error('exact selected Worker hierarchy owner required');
    return binding;
  };
  async function normalizeOptions(agent,options,signal){
    if(!options?.provider||!options.model)throw Error('original Harness request configuration required for hierarchy Editor');
    const history=[...(options.messages??[])];
    if(options.system!==undefined&&options.system!==system)throw Error('unreviewed one-shot hierarchy system prompt');
    if(options.system===undefined&&!history.some(message=>message.role==='system'))history.unshift(api.freezeMessage({id:randomUUID(),role:'system',source:{kind:'system-prompt'},content:[{type:'text',text:system}]}));
    if(!history.some(message=>message.source?.kind==='runtime-context'))history.push(api.createUserMessage({source:{kind:'runtime-context',form:'snapshot',sections:[]},content:[{type:'text',text:'Current Harness context.'}]}));
    return {...options,sessionId:agent.session.id,messages:history,tools:options.tools??(await ctx.systemPrompt.assemble({scope:agent})).tools,signal};
  }
  function policyFor(input,intent){
    const configured=cfg.hierarchyPolicy??{},max=configured.max_level??8;
    if(!Number.isSafeInteger(max)||max<2||max>128)throw Error('hierarchy max_level must be between 2 and 128');
    const flush=['manual','context-overflow','projected-pressure'].includes(input.trigger);
    const max_level=flush?Math.min(max,Math.max(2,intent.active_level+1)):max;
    const level_batch_sizes={};
    for(let level=2;level<=max_level;level++)level_batch_sizes[level]=configured.level_batch_sizes?.[level]??8;
    return {context_id:'main',min_level:2,min_sources_per_wb:1,max_sources_per_wb:8,max_units_per_wb:512,
      level_batch_sizes,max_level,protected_prefix_blocks:configured.protected_prefix_blocks??0,flush,
      max_actions:configured.max_actions??128,max_model_calls:0,editor_mode:'host',program_rules:[]};
  }
  async function execute(input){
    const {agent,requestId,signal}=input,binding=selected(agent),sessionId=agent.session.id,core=input.core??clientFor(binding);
    signal?.throwIfAborted();
    const key=digest(['host-hierarchy',sessionId,binding.chain_id,requestId]),traceSource=input.trace_source_id??'hierarchy-'+key;
    const intent=await store.context(sessionId,digest(['hierarchy-intent',key]),async()=>{
      const cut=await core.operate('cut.get',{},signal),chain=await core.operate('chain.get',{},signal);
      return {base_cut:cut.sequence,active_level:chain.role_views.worker.level,board_id:chain.board_id??null,
        cb_ids:cut.view.cbs.map(cb=>cb.cb_id),request_id:requestId};
    });
    const policy=policyFor(input,intent),maximum=cfg.hierarchyPolicy?.max_host_editor_calls??32;
    if(!Number.isSafeInteger(maximum)||maximum<1||maximum>128)throw Error('invalid hierarchy Host Editor work budget');
    let resolved;
    const optionsFor=async()=>resolved??=(await normalizeOptions(agent,input.options??(await input.resolveCall?.())?.options,signal));
    const receiptId=digest(['hierarchy-receipt',key]);
    const result=await store.context(sessionId,digest(['hierarchy-result',key]),async()=>{
      const reports=[],jobs=[];let dispatched=0;
      for(let pass=0;pass<=maximum*2+2;pass++){
        signal?.throwIfAborted();selected(agent);
        const report=await core.operate('runtime.schedule',policy,signal);reports.push(report);
        if(report.failure)throw Error('Rust hierarchy scheduler: '+report.failure.code+': '+report.failure.message);
        if(report.wait?.code==='higher_volatile_binding')throw Error('higher-level volatile material requires its supported next-WB binding; no Worker dispatch is allowed to bypass it');
        const planned=report.host_jobs??[];
        if(!planned.length){
          const cut=await core.operate('cut.get',{},signal),cbIds=cut.view.cbs.map(cb=>cb.cb_id);
          return {recorded:true,receipt_id:receiptId,request_id:requestId,progressed:JSON.stringify(cbIds)!==JSON.stringify(intent.cb_ids),
            publication_ids:reports.flatMap(row=>row.published??[]).map(row=>row.cut_id),reports,jobs,trace_source_id:traceSource,
            base_cut:intent.base_cut,selected_cut_id:cut.cut_id,selected_sequence:cut.sequence,board_id:cut.instant_scope?.board_id??null};
        }
        for(const job of planned){
          for(const assignment of job.assignments??[]){
            const editor=assignment.editor,view=await core.operate('edit.host.get',{job_id:job.job_id,editor},signal),prior=attemptOf(view,editor);
            if(prior?.status==='completed')continue;
            if(dispatched>=maximum)throw Error('hierarchy Host Editor work budget exhausted; durable plan retained');
            const options=await optionsFor(),cut=await core.operate('cut.get',{},signal);
            const layout=await projectHarnessContext({options,history:options.messages,cut,requestId,api,system,imageCompression});
            const readOnly=sharedOperationIds(options.tools,layout.shared_messages);
            const claimParts=['hierarchy-editor',binding.chain_id,requestId,job.job_id,editor];
            const recovered=view.host_editor?.recovered_attempts?.[editor];
            if(recovered)claimParts.push(recovered.claim_id);
            const claim_id=digest(claimParts);
            await store.recordTrace(sessionId,traceSource,{stage:'hierarchy-plan',job_id:job.job_id,editor,claim_id,source_id:view.source_id,level:view.level,base_cut:view.base_cut});
            const claimed=await core.operate('edit.host.claim',{job_id:job.job_id,editor,claim_id},signal);
            if(claimed.execute!==true){
              if(attemptOf(claimed,editor)?.status==='completed')continue;
              throw Error('hierarchy Editor outcome requires explicit recovery; dispatch permit was already consumed');
            }
            dispatched++;
            const source={source_id:claimed.source_id,source_digest:claimed.source_digest,version:claimed.version,units:claimed.units,
              binding:{editable_unit_ids:claimed.assigned_unit_ids,edit_capabilities:capabilitiesFor(claimed,selected(agent))}};
            const authorize=async()=>{
              const currentBinding=selected(agent);
              if(currentBinding.chain_id!==binding.chain_id||currentBinding.scope_id!==binding.scope_id)throw Error('hierarchy owner binding changed');
              const current=await core.operate('edit.host.get',{job_id:job.job_id,editor},signal),attempt=attemptOf(current,editor);
              if(!current.fresh||attempt?.claim_id!==claim_id||attempt.status!=='claimed'||current.source_digest!==source.source_digest||current.version!==source.version)throw Error('hierarchy Editor scope or claim is stale');
              return {role:'EDITOR',scope_id:job.job_id+':'+editor,source_id:source.source_id,source_digest:source.source_digest,
                expected_version:source.version,assigned_unit_ids:current.assigned_unit_ids,
                allowed_tool_names:['sic_edit_blocks','sic_source_text_page',...(readOnly.length?[ARCHIVE_OPS]:[])],
                ...(readOnly.length?{read_only_cb_ids:readOnly}:{}),
                edit_capabilities:capabilitiesFor(current,currentBinding),job_id:job.job_id,editor,level:current.level,claim_id,
                editor_instructions:Object.fromEntries(Object.entries(current.editor_instructions??{}).filter(([_key,value])=>value!==null&&value!==undefined))};
            };
            try{
              const driver=await createHarnessEditorDriver({llm:ctx.llm,llmAPI:api,limits:cfg.editorLimits,authorizeEditor:authorize,
                record:async event=>{
                  await store.recordTrace(sessionId,traceSource,{...jsonSnapshot(event),job_id:job.job_id,editor,level:claimed.level});
                  if(event.stage!=='editor-chunk')await recordSession(agent.session,'sic/hierarchy-editor',{job_id:job.job_id,editor,source_id:source.source_id,trace_source_id:traceSource,stage:event.stage,round:event.round});
                },executeNativeTool:async execution=>{
                  const authority=await authorize(),args=execution.arguments;
                  return sourcePageToolResult(async()=>{
                    if(execution.name===ARCHIVE_OPS)return readEditorOperations({core,authority,args,signal:execution.signal});
                    if(execution.name!=='sic_source_text_page'||args?.source_id!==source.source_id||!claimed.assigned_unit_ids.includes(args?.unit_id))throw new SourcePageRequestError('The requested hierarchy page is outside the assigned frozen WB.');
                    const offset=args.offset??0,limit=args.limit??4096;
                    if(!Number.isSafeInteger(offset)||offset<0||!Number.isSafeInteger(limit)||limit<1||limit>8192)throw new SourcePageRequestError('Invalid hierarchy source page arguments.');
                    const unit=source.units.find(unit=>unit.unit_id===args.unit_id);
                    return {source_id:source.source_id,unit_id:args.unit_id,offset,...pageSourceText(unit.text,offset,limit)};
                  });
                }});
              const materialize=(args,signal)=>imageCompression?imageCompression.prepare({args,history:options.messages,sessionId,signal}):assertImageDeliveryDisabled(args);
              const archived=input.recoveredDelivery;
              let delivery;
              if(archived?.job_id===job.job_id&&archived.editor===editor){
                const authority=await authorize();
                if(archived.source_id!==authority.source_id||archived.source_digest!==authority.source_digest
                  ||JSON.stringify(archived.assigned_unit_ids)!==JSON.stringify(authority.assigned_unit_ids))throw Error('Archived hierarchy delivery does not match the reclaimed Editor scope');
                delivery=await materialize(normalizeEditorDelivery(archived.arguments),signal);
                await store.recordTrace(sessionId,traceSource,{stage:'editor-delivery-recovered',job_id:job.job_id,editor,source_id:source.source_id,claim_id});
              }else delivery=await materializeWithEditor(driver,{source,invocation:{actor:cfg.actor,session_id:sessionId,request_id:requestId,role:'EDITOR'},signal,
                originalOptions:options,shared_messages:layout.shared_messages,runtime_sections:layout.runtime_sections,instant_message:layout.instant_message,
                image_compression:imageCompression?.offer(options.messages),reference_shared:true,capacity_pressure:input.trigger==='projected-pressure'},materialize);
              await authorize();
              try{await core.operate('edit.host.submit',{job_id:job.job_id,editor,claim_id,ops:delivery.ops,pvi:delivery.pvi},signal);}
              catch(error){const current=await core.operate('edit.host.get',{job_id:job.job_id,editor});if(attemptOf(current,editor)?.status!=='completed')throw error;}
              jobs.push({job_id:job.job_id,editor,source_id:source.source_id,claim_id,level:claimed.level});
            }catch(error){
              try{
                const current=await core.operate('edit.host.get',{job_id:job.job_id,editor});
                if(attemptOf(current,editor)?.status==='claimed'){
                  await core.operate('edit.host.fail',{job_id:job.job_id,editor,claim_id,reason:'original Harness Editor did not deliver an acknowledged valid submission'});
                  // This DSH adapter outcome is produced during local request
                  // preparation. The next native attempt uses offloaded images.
                  const failure=error instanceof api.LlmError?error.failure:null;
                  if(failure&&failure.code===api.IMAGE_OFFLOAD_REQUIRED_CODE&&Number.isSafeInteger(failure.offloadImages)&&failure.offloadImages>0){
                    await core.operate('edit.host.reopen',{job_id:job.job_id,editor,claim_id,confirm:true,reason:'Native adapter requested image offload before Provider dispatch'});
                  }
                }
              }catch{}
              throw error;
            }
          }
        }
      }
      throw Error('Rust hierarchy scheduler did not converge within the Host work budget');
    });
    // Recover the mandatory native receipt after a crash between Core commit
    // and local acknowledgement without invoking another Editor.
    await recordSession(agent.session,'sic/hierarchy',result,{key:receiptId});
    if(!input.trace_source_id&&result.progressed){
      const options=await optionsFor(),projection=await store.context(sessionId,digest(['hierarchy-layout',key]),async()=>{
        const cut=await core.operate('cut.get',{},signal);
        return projectHarnessContext({options,history:options.messages,cut,requestId,api,system,imageCompression});
      });
      const original=jsonSnapshot(options),adopted={...original,messages:projection.projected_history},raw=JSON.stringify(adopted);
      await publisher({id:digest(['hierarchy-view',key]),session_id:sessionId,request_id:requestId,actor:cfg.actor,role:'WORKER',format:'dsh-messages',
        source_id:traceSource,source_kind:'host-hierarchy',original,adopted,raw_original:JSON.stringify(original),raw_adopted:raw,adopted_digest:wireDigest(raw),
        stage:'hierarchy-published-awaiting-dispatch',hierarchy:result,board_bindings:[intent.board_id,result.board_id].filter(Boolean),
        layout_receipt:{kind:'harness-hierarchy-context-selection',selected_cut_id:projection.selected_cut_id,projected_history:projection.projected_history,
          ...projection.image_view?{image_view:projection.image_view}:{}},
        publication:{kind:'core-hierarchy',published:result.reports.flatMap(report=>report.published??[])}});
    }
    return result;
  }
  function drainHigher(input){
    const binding=selected(input.agent),requestId=input.requestId??input.request_id;
    if(typeof requestId!=='string'||!requestId)throw Error('stable hierarchy request identity required');
    const identity=digest([input.agent.session.id,binding.chain_id,requestId]);
    if(pending.has(identity))return pending.get(identity);
    const owner=binding.chain_id,work=(serial.get(owner)??Promise.resolve()).catch(()=>{}).then(()=>execute({...input,requestId}));
    pending.set(identity,work);serial.set(owner,work);
    return work.finally(()=>{if(pending.get(identity)===work)pending.delete(identity);if(serial.get(owner)===work)serial.delete(owner);});
  }
  async function authorizeRecord(record,core,signal){
    const chain=await core.operate('chain.get',{},signal);
    if((record.board_bindings??[]).some(board=>board!==chain.board_id))throw Error('archived hierarchy Board view is no longer bound');
    await core.operate('cut.get',{},signal);
    for(const publication of record.publication?.published??[])await core.operate('hierarchy.expand',{cb_id:publication.cb_id},signal);
  }
  return {drainHigher,scheduleCompaction:input=>drainHigher({...input,requestId:input.request_id}),authorizeRecord,
    drain:()=>Promise.all([...pending.values()])};
}
