"""Read-only selection and presentation of frozen Editor jobs from real Core state."""
import copy
import json
from sic_workflow.prefix_forms import safe_settings,safe_meta,safe_aliases

SAFE_MODEL_FIELDS={'model','route_id','tokenizer_model','tokenizer_status','max_input_tokens','max_output_tokens','context_window','expected_returned_models','timeout_seconds'}
TASK_FIELDS=('job_id','status','context_id','level','wb_id','source_id','base_cut','created_event','assignments','background','background_manifest','instant_reference','editor_backgrounds','editor_instants','readonly_reads','readonly_media','responses','attempts','program_rules','program_rule_version','editor_profiles','decision_revision','task','prompt','candidate')


def text(value):return json.dumps(value,ensure_ascii=False,sort_keys=True)


def safe_binding(binding):
    return {key:copy.deepcopy(value) for key,value in (binding or {}).items() if key in SAFE_MODEL_FIELDS}


def directory(chain,offset=0,limit=20):
    jobs=sorted(chain.get('jobs',{}).values(),key=lambda job:(job.get('created_event',0),job['job_id']))
    selected=jobs[offset:offset+limit]
    rows=[{'job_id':job['job_id'],'status':job.get('status'),'context_id':job.get('context_id','main'),'level':job.get('level',1),'source_id':job.get('source_id'),'wb_id':job.get('wb_id'),'base_cut':job.get('base_cut'),'editors':[a['editor'] for a in job.get('assignments',[])],'unit_count':len(job.get('source_snapshot',{}).get('units',[])),'delivered_editors':list(job.get('responses',{}))} for job in selected]
    return rows,{'offset':offset,'limit':limit,'total':len(jobs),'next_offset':offset+len(rows) if offset+len(rows)<len(jobs) else None,'order':'created_event_then_job_id','selection':'Enter an explicit job_id to inspect frozen text, assignments, role backgrounds and original delivery.'}


def task_view(job):
    view={key:copy.deepcopy(job[key]) for key in TASK_FIELDS if key in job}
    # Native protocol/media are retained in source archives, not editable rows.
    source=job.get('source_snapshot',{})
    view['source_snapshot']={key:copy.deepcopy(source[key]) for key in ('source_ids','units','format','open_calls','media','media_source_ids') if key in source}
    view['model_bindings']={editor:safe_binding(binding) for editor,binding in job.get('model_bindings',{}).items()}
    view['prefix_layout_config']=safe_settings(job.get('prefix_layout_config',{}))
    view['prefix_layout']={editor:safe_meta(meta) for editor,meta in job.get('prefix_layout',{}).items()}
    view['editor_alias_maps']={editor:safe_aliases(value) for editor,value in job.get('editor_alias_maps',{}).items()}
    view['attempts']={editor:{**{key:copy.deepcopy(value[key]) for key in ('call_id','status','started_at','ended_at','request_id','response_id','error_type') if key in value},**({'prefix_layout':safe_meta(value['prefix_layout'])} if 'prefix_layout' in value else {})} for editor,value in job.get('attempts',{}).items()}
    view['archive_navigation']={'source_operation':'source.read','source_page_kind':'source','media_page_kind':'media','source_ids':list(source.get('source_ids',[])),'frozen_source_scope':True,'original_protocol_is_readonly':True}
    return view


def rows(view):
    assignments=view.get('assignments',[]);assigned={uid:a['editor'] for a in assignments for uid in a['unit_ids']}
    scope=[{'unit_id':u['unit_id'],'editable_text':u['text'],'assigned_editor':assigned.get(u['unit_id'],'implicit KEEP'),'source_anchor_readonly':copy.deepcopy(u.get('anchor',{})),'job_status':view.get('status')} for u in view.get('source_snapshot',{}).get('units',[])]
    division=[{'editor':a['editor'],'unit_ids':list(a['unit_ids']),'unit_count':len(a['unit_ids']),'single_round_status':view.get('attempts',{}).get(a['editor'],{}).get('status','not_dispatched'),'delivered':a['editor'] in view.get('responses',{})} for a in assignments]
    backgrounds=[];models=[]
    for a in assignments:
        editor=a['editor'];background=view.get('editor_backgrounds',{}).get(editor,{'cbs':view.get('background',[]),'manifest':view.get('background_manifest',{})})
        instant=view.get('editor_instants',{}).get(editor,view.get('instant_reference',{}))
        backgrounds.extend([{'editor':editor,'source':'frozen read-only CB projection','material':text(background.get('cbs',[])),'manifest':copy.deepcopy(background.get('manifest',{}))},
                            {'editor':editor,'source':'frozen read-only instantaneous view','material':text(instant)}])
        models.append({'editor':editor,'profile_name':view.get('editor_profiles',{}).get(editor),'binding_frozen':editor in view.get('model_bindings',{}),'binding':copy.deepcopy(view.get('model_bindings',{}).get(editor,{})),'binding_status':'configured' if view.get('model_bindings',{}).get(editor) else 'not_configured_in_snapshot'})
    backgrounds.extend([{'source':'frozen Editor teaching','material':view.get('prompt','')},
                        {'source':'program-prefetched frozen archive pages','material':text(view.get('readonly_reads',[]))},
                        {'source':'read-only immutable image identities','material':text(view.get('readonly_media',[]))}])
    responses=[];prefixes=[];aliases=[]
    for editor,response in view.get('responses',{}).items():
        translated=bool(response.get('canonical_response_id'))
        responses.append({'editor':editor,'source':'canonical translated delivery; raw model response is a separate immutable object' if translated else 'original single-round delivery, not current candidate','origin':response.get('origin'),'response_id':response.get('response_id'),'raw_model_response_id':response.get('response_id') if response.get('origin')=='model' else None,'canonical_response_id':response.get('canonical_response_id'),'alias_map_id':response.get('alias_map_id'),'ops':copy.deepcopy(response.get('ops',[])),'pvi':copy.deepcopy(response.get('pvi',{})),'original_response':{'object_id':response.get('response_id'),'kind':'provider_response','content_not_embedded_in_task_row':True,'published_read_kind':'proposal'} if translated else copy.deepcopy(response)})
    for editor,meta in view.get('prefix_layout',{}).items():
        attempt=view.get('attempts',{}).get(editor,{})
        prefixes.append({'editor':editor,'phase':'frozen prepared plan; not proof of invocation','metadata':copy.deepcopy(meta),'host_mode_readonly':meta.get('mode'),'prefix_plan_read':{'kind':'prefix_plan','object_id':meta.get('plan_id')},'actual_request_id':attempt.get('request_id'),'actual_layout_metadata':copy.deepcopy(attempt.get('prefix_layout',{})),'actual_request_read':{'kind':'provider_request','object_id':attempt.get('request_id')} if attempt.get('request_id') else None,'capability_editable':False,'physical_cache_hit':'unknown'})
    for editor,value in view.get('editor_alias_maps',{}).items():
        aliases.extend({'editor':editor,'map_id':value.get('map_id'),'alias':row['alias'],'canonical_unit_id':row['unit_id'],'readonly':True,'translation':'exact target/ADD.anchor.target only; raw response unchanged'} for row in value.get('mapping',[]))
    return {'assignment_rows':division,'scope_rows':scope,'background_rows':backgrounds,'response_rows':responses,'model_rows':models,'prefix_rows':prefixes,'alias_rows':aliases}


def editor_postprocess(state,form,result,after):
    """Named make_graph postprocessor; it performs no Core or model operations."""
    chain=(result.get('result',{}) if result.get('operation')=='chain.get' and result.get('ok') else after.get('result',{}) if after.get('ok') else {})
    offset,limit=form.get('directory_offset',0),form.get('directory_limit',20)
    catalog,navigation=directory(chain,offset,limit)
    selected=(form.get('job_id','').strip() if form['action']=='read' else result.get('context',{}).get('job_id') or form.get('job_id','').strip())
    output={'task_mode':'selected' if selected else 'directory','selected_job_id':selected,'task_view':{},'task_directory':catalog,'task_navigation':navigation,'assignment_rows':[],'scope_rows':[],'background_rows':[],'response_rows':[],'model_rows':[],'prefix_rows':[],'alias_rows':[]}
    safe_after={**after,'result':{'task_mode':output['task_mode'],'selected_job_id':selected,'task_directory':catalog,'task_navigation':navigation}} if after.get('ok') else after
    if not result.get('ok'):
        return {**output,'after':safe_after,'records':[{'status':'failed','code':result.get('error',{}).get('code'),'message':result.get('error',{}).get('message')}],'status':'failed'}
    if not selected:
        body={'task_directory':catalog,'task_navigation':navigation,'task_mode':'directory'}
        return {**output,'after':{**safe_after,'result':body},'result':{**result,'result':body},'records':catalog or [{'status':'no_editor_jobs','total':navigation['total']}],'status':'applied'}
    job=chain.get('jobs',{}).get(selected)
    if job is None:
        failure={**result,'ok':False,'result':{'requested_job_id':selected,'task_directory':catalog,'task_navigation':navigation},'error':{'code':'editor_job_not_found','message':'The selected job does not exist on this route. Choose a job from the bounded directory.'},'evidence':{**result.get('evidence',{}),'core_read_succeeded':True,'selection_failed':True}}
        return {**output,'after':safe_after,'result':failure,'records':[{'status':'failed','code':'editor_job_not_found','job_id':selected,'message':failure['error']['message']}],'status':'failed'}
    view=task_view(job);tables=rows(view)
    body=copy.deepcopy(result.get('result',{})) if result.get('operation')!='chain.get' else {}
    if 'job' in body:body['job']=view
    body.update(job_id=selected,task_view=view,task_directory=catalog,task_navigation=navigation)
    return {**output,**tables,'after':{**safe_after,'result':body},'task_view':view,'result':{**result,'result':body,'context':{**result.get('context',{}),'job_id':selected,'reviewed_cut':job.get('base_cut')}},'records':[{'job_id':selected,'status':job.get('status'),'context_id':job.get('context_id','main'),'level':job.get('level',1),'wb_id':job.get('wb_id'),'source_id':job.get('source_id'),'base_cut':job.get('base_cut'),'frozen_units':len(tables['scope_rows']),'editors':len(tables['assignment_rows']),'original_deliveries':len(tables['response_rows'])}],'status':'applied'}
