"""Opt-in shared-I Core request wiring; legacy requests are complete fallbacks."""
import copy,json,hashlib
from . import hierarchy,memory,media
from .adapters import encode,digest
from .shared_prefix import (FrozenPrefixPlan,HostModeSnapshot,InvocationCapability,CurrentAuthorization,compose,authorize_tool,SIC_ADAPTIVE_TEACHING,OPS)
from .editor_aliases import freeze_aliases,FrozenAliasMap,alias_materials,canonical_response

SCHEMA_VERSION='sic-shared-i-content-json-v1'
LAYOUT_KEYS=('prefix_layout','shared_prefix_identity','shared_prefix_budget')

def validate_config(p):
    if 'prefix_layout' in p and p['prefix_layout'] not in ('separate','shared_i'):raise ValueError('prefix_layout must be separate/shared_i')
    if 'shared_prefix_budget' in p and (type(p['shared_prefix_budget']) is not int or p['shared_prefix_budget']<512):raise ValueError('shared prefix budget must be >=512')
    if 'shared_prefix_identity' in p:
        value=p['shared_prefix_identity']
        entries=list(value.values()) if isinstance(value,dict) and set(value)<= {'worker','editor'} else [value]
        if not entries or any(not isinstance(x,dict) or set(x)!={'model','provider_id','cache_namespace'} or any(not isinstance(v,str) or not v for v in x.values()) for x in entries):raise ValueError('host-declared shared identity needs model/provider_id/cache_namespace')

def _cfg(core,s,role,editor=None,job=None):
    if role=='editor' and job is not None:return job.get('model_bindings',{}).get(editor) or getattr(core.provider,'roles',{}).get(role) or core.config.get('roles',{}).get(role)
    profile=s['config'].get('worker_profile') if role=='worker' else s['config'].get('editor_profiles',{}).get(editor)
    return core.config.get('profiles',{}).get(profile) if profile else core.config.get('roles',{}).get(role) or getattr(core.provider,'roles',{}).get(role)

def _identity(core,s,settings,role,editor=None,job=None):
    cfg=_cfg(core,s,role,editor,job) or {};declaration=settings.get('shared_prefix_identity',{})
    declared=declaration.get(role,{}) if set(declaration)<= {'worker','editor'} else declaration
    if declared:
        if cfg.get('model') and cfg['model']!=declared['model']:raise ValueError('declared '+role+' model differs from configured binding')
        actual=core.provider.capabilities().get('roles',{}).get(role,{}).get('model') if not cfg else None
        if actual and actual!=declared['model']:raise ValueError('declared '+role+' model differs from injected provider binding')
        return copy.deepcopy(declared),'host-declared-logical-identity; physical cache identity unknown'
    if not cfg.get('model'):raise ValueError('shared identity unavailable; host declaration required for injected Provider')
    namespace=cfg.get('cache_namespace') or str(cfg.get('api_base',cfg.get('route_id','default-provider-route')))+'|'+cfg['model']
    return {'model':cfg['model'],'provider_id':str(cfg.get('api_base',cfg.get('route_id','default-provider-route'))),'cache_namespace':namespace},'fixed-role-config; physical cache identity unknown'

def _epoch(core,s,actor):
    rows=[{'name':t['name'],'discover':core._tool_acl(s,t,'discover',actor),'expand':core._tool_acl(s,t,'expand',actor),'execute':core._tool_acl(s,t,'execute',actor),'schema':t['schema'],'expanded':t.get('expanded',False)} for t in s['tools']]
    return int(digest(rows)[:12],16)

def _load_plan(core,ref):
    record=core.store.get(ref,'shared_prefix_plan');plan=FrozenPrefixPlan._freeze(record['payload'])
    if plan.digest!=record['plan_digest']:raise ValueError('persisted prefix plan digest mismatch')
    return plan,record

def _auth(core,s,actor,mode,plan,*,job=None,editor=None,aliases=None,scope=None):
    p=plan.data();visible=[t['name'] for t in s['tools'] if core._tool_acl(s,t,'discover',actor) and core._tool_acl(s,t,'expand',actor) and t.get('expanded')]
    allowed=[t['name'] for t in s['tools'] if t['name'] in visible and core._tool_acl(s,t,'execute',actor)] if mode=='worker' else []
    units=[row['alias'] for row in aliases.as_dict()['mapping']] if aliases is not None else []
    assignment_id=digest({'editor':editor,'unit_ids':list(aliases.unit_ids)}) if aliases is not None else None
    return CurrentAuthorization.from_host(actor=actor,chain_id=s['chain_id'],context_id=p['context_id'],level=p['level'],mode=mode,job_id=job['job_id'] if job else None,scope_id=job['wb_id'] if job else scope or 'worker:'+p['cut_id'],assigned_unit_ids=units,allowed_tools=allowed,visible_tools=visible,acl_epoch=_epoch(core,s,actor),assignment_id=assignment_id)

def _plan(core,s,actor,settings,editor=None,job=None,*,_comparison_plan_id=None):
    worker=hierarchy.selected_view(s,'worker');editor_view=hierarchy.selected_view(s,'editor:'+editor if editor else 'editor')
    if job is not None:
        manifest=job.get('editor_backgrounds',{}).get(editor,{}).get('manifest',job.get('background_manifest',{}));editor_view={'context_id':manifest.get('context_id','main'),'level':manifest.get('active_level',1)}
    if worker!=editor_view:raise ValueError('worker/editor context or level mismatch')
    identity,source=_identity(core,s,settings,'worker');other,_=_identity(core,s,settings,'editor',editor,job)
    if identity!=other:raise ValueError('worker/editor model/provider/cache namespace mismatch')
    wc=_cfg(core,s,'worker') or {};ec=_cfg(core,s,'editor',editor,job) or {}
    for field in ('tokenizer_model','max_input_tokens','max_output_tokens','context_window'):
        if wc.get(field)!=ec.get(field):raise ValueError('worker/editor model admission budget mismatch: '+field)
    budget=settings.get('shared_prefix_budget')
    if budget is None:
        wb=settings.get('memory_chars',16384);eb=settings.get('editor_memory_chars',16384)
        if wb!=eb:raise ValueError('worker/editor projection budget mismatch; explicit shared budget required')
        budget=wb
    view=core._view(s);ids=hierarchy.selected_ids(core,s,'worker')
    if job and job.get('readonly_media'):raise ValueError('Editor media scope incompatible; original carrier retained in separate layout')
    if any(core.store.get(cid,'cb').get('media_source_ids') for cid in ids) or any(media.source_ids(core,core._source(s,r)) for r in s['pending']):raise ValueError('media scope incompatible; preserve native carriers with separate layout')
    teaching=SIC_ADAPTIVE_TEACHING+'\nWorker task/context guidance:\n'+view['protected'][0]['content']+'\nPrioritize newer native WB/tool-result and observed host tool_end evidence over stale CB/I suggestions; a proposed tool call is not a performed action, and CB text containing an answer candidate is not evidence that this invocation delivered a final reply to the Harness. Do not repeat an action solely because an older I view still marks it pending.'
    # One renderer invocation per frozen key, shared by both actual roles.
    tools=view['tool_schemas'];key=digest({'identity':identity,'chain':s['chain_id'],'view':worker,'cut':s['current_cut'],'ids':ids,'tools':tools,'budget':budget,'protected':view['protected'][1:],'teaching':teaching,'query':settings.get('task_goal',''),'schema':SCHEMA_VERSION})
    cached=s.setdefault('shared_prefix_plans',{}).get(key)
    if cached:return cached,_load_plan(core,cached)[0],source
    projection=memory.project(core,s,'worker',budget,settings.get('task_goal',''))
    rendered=encode({'protected':[],'cbs':projection['cbs'],'instant':{},'pending':[]})['messages'] if projection['cbs'] else []
    plan=FrozenPrefixPlan.freeze(**identity,chain_id=s['chain_id'],context_id=worker['context_id'],level=worker['level'],cut_id=s['current_cut'] or 'unpublished-cut-0',cb_ids=[c['cb_id'] for c in projection['cbs']],rendered_cb_messages=copy.deepcopy(view['protected'][1:])+rendered,common_tools=tools,projection_budget=budget,schema_version=SCHEMA_VERSION,tool_version=digest(tools),projection_scope=digest({'view':worker,'ids':ids}),worker_teaching=teaching)
    ref=core.store.put('shared_prefix_plan',{'chain_id':s['chain_id'],'payload':plan.data(),'plan_digest':plan.digest,'prefix_bytes_sha256':hashlib.sha256(plan.prefix_bytes).hexdigest(),'projection_manifest':projection['manifest'],'identity_source':source,'physical_cache_hit':'unknown'},_comparison_plan_id)
    s['shared_prefix_plans'][key]=ref;return ref,plan,source

def _meta(applied,**fields):return {'requested_layout':'shared_i','applied_layout':'shared_i' if applied else 'separate','shared':applied,'physical_cache_hit':'unknown','editor_delivery':'content-json; no virtual response tool registered','production_wired':True,'actual_request_assembled':False,'late_material_policy':'keep_this_cb_program_supplement',**fields}

def _schema(primary):
    instant={'type':'object'}
    if primary:instant['properties']={'decision_tree':{'type':'object','description':'Four named views, never a string','properties':{name:{'type':'array','items':{'type':'object','required':['text'],'properties':{'text':{'type':'string'},'evidence':{'type':'array'}}}} for name in ('Working on','Failed Before','Not Tried Yet','Overlooked')}}}
    return {'type':'object','required':['ops','pvi'],'additionalProperties':False,'properties':{'ops':{'type':'array','items':{'type':'object','required':['op'],'properties':{'op':{'enum':list(OPS)},'target':{'type':'string'},'content':{'type':'string'},'reason':{'type':'string'},'lifecycle':{'enum':['P','V','I']},'view_key':{'type':'string'},'risk':{'enum':['none','low','medium','high','unknown']},'order':{'type':'integer'},'anchor':{'type':'object','required':['target','position'],'properties':{'target':{'type':'string'},'position':{'enum':['before','after']}}}}}},'pvi':{'type':'object','additionalProperties':False,'properties':{'P':{'type':'array','items':{'type':'string'}},'V':{'type':'array','items':{'type':'string'}},'I':instant}}}}

def freeze_job(core,s,job,*,_comparison_ids=None):
    job['prefix_layout_config']=copy.deepcopy(s['config'])
    if s['config'].get('prefix_layout','separate')!='shared_i':return
    job['prefix_layout']={};job['editor_alias_maps']={}
    for assignment in job['assignments']:
        editor=assignment['editor']
        try:
            ref,plan,identity_source=_plan(core,s,s['_actor'],job['prefix_layout_config'],editor,job,_comparison_plan_id=(_comparison_ids or {}).get('plan'))
            aliases=freeze_aliases(job['job_id'],editor,assignment['unit_ids']);job['editor_alias_maps'][editor]=aliases.as_dict()
            legacy=core._editor_request(job,editor);payload=json.loads(legacy['messages'][-1]['content']);parts=alias_materials(aliases,payload['editable_text_units'],payload['readonly_provenance'])
            payload['legacy_background_manifest']=payload['background_manifest'];payload['background_manifest']=copy.deepcopy(_load_plan(core,ref)[1]['projection_manifest']);payload['background_manifest']['shared_layout_projection']=True;payload['readonly_provenance']=parts['readonly_provenance'];payload.pop('editable_text_units');payload['editable_text_units_ref']='previous sic_role_material; exact frozen short aliases only';payload['read_only_cb']=[];payload['common_cb_prefix_plan']=ref
            auth=_auth(core,s,s['_actor'],'editor',plan,job=job,editor=editor,aliases=aliases);cap=InvocationCapability.from_host(plan,auth,allowed_tools=[])
            scope={'wb_id':job['wb_id'],'source_id':job['source_id'],'unit_ids':[u['unit_id'] for u in parts['editable_units']],'edit_context_id':job.get('context_id','main'),'edit_level':job.get('level',1),'canonical_scope_digest':digest(assignment['unit_ids']),'alias_map_id':aliases.map_id}
            protocol={'operations':list(OPS),'pvi':['P','V','I'],'candidate_only':True,'delivery':'one content JSON object; tool calls forbidden','editor_prompt':job['prompt'],'response_schema':_schema(job.get('level',1)==1 and job.get('context_id','main')=='main'),'scope_rule':'Only aliases from current new WB; unknown aliases are rejected, not repaired; unmentioned units KEEP. ADD has no target. FLAG/ADD/REPLACE/COMPRESS require content.'}
            snapshot=HostModeSnapshot.from_host(plan,cap,new_wb_scope=scope,editor_units=parts['editable_units'],readonly_background=[{'manifest':payload.get('background_manifest'),'archive_reads':payload.get('readonly_archive_reads',[])}],instant_views=payload.get('read_only_instant',{}),output_protocol=protocol,host_observations=observations(core,s))
            meta=_meta(True,plan_id=ref,plan_digest=plan.digest,identity_source=identity_source,mode='editor',allowed_tools=[],alias_map_id=aliases.map_id)
            job['prefix_layout'][editor]={**meta,'snapshot_payload':snapshot.data(),'suffix':[{'role':'user','content':json.dumps(payload,ensure_ascii=False)}]}
        except (ValueError,TypeError,PermissionError) as error:job['prefix_layout'][editor]=_meta(False,reason=str(error),mode='editor')

def editor_request(core,s,job,editor,legacy):
    binding=job.get('prefix_layout',{}).get(editor)
    if not binding:return legacy,{'requested_layout':job.get('prefix_layout_config',{}).get('prefix_layout','separate'),'applied_layout':'separate','production_wired':True,'actual_request_assembled':True}
    if not binding['shared']:return legacy,{**copy.deepcopy(binding),'actual_request_assembled':True}
    try:
        plan,_=_load_plan(core,binding['plan_id']);aliases=FrozenAliasMap.from_dict(job['editor_alias_maps'][editor]);auth=_auth(core,s,s['_actor'],'editor',plan,job=job,editor=editor,aliases=aliases)
        snapshot=HostModeSnapshot._freeze(binding['snapshot_payload']);result=compose(plan,snapshot,binding['suffix'],current_authorization=auth)
        meta={k:copy.deepcopy(v) for k,v in binding.items() if k not in ('snapshot_payload','suffix')};meta['prefix_bytes_sha256']=result.shared_prefix_digest;meta['actual_request_assembled']=True
        meta['capability_id']=core.store.put('invocation_capability',{'chain_id':s['chain_id'],'payload':snapshot.data()['capability'],'plan_id':binding['plan_id']})
        return result.request,meta
    except (ValueError,TypeError,PermissionError) as error:return legacy,_meta(False,mode='editor',reason='frozen plan no longer applicable: '+str(error),actual_request_assembled=True)

def worker_request(core,s,actor,legacy,*,_comparison_ids=None):
    if s['config'].get('prefix_layout','separate')!='shared_i':
        # A new separate-layout invocation must not inherit the previous
        # shared invocation's active gate. Historical per-call bindings remain.
        s.pop('shared_worker_capability',None)
        return legacy,{'requested_layout':'separate','applied_layout':'separate','production_wired':True,'actual_request_assembled':True}
    try:
        ref,plan,identity_source=_plan(core,s,actor,s['config'],_comparison_plan_id=(_comparison_ids or {}).get('plan'));auth=_auth(core,s,actor,'worker',plan);cap=InvocationCapability.from_host(plan,auth)
        view=core._working_view(s,'worker');view['instant']['memory_projection']=copy.deepcopy(_load_plan(core,ref)[1]['projection_manifest']);suffix=encode({'protected':[],'cbs':[],'instant':view['instant'],'pending':view['pending'],'controls':{}})['messages']
        envelope={k:copy.deepcopy(v) for k,v in legacy.items() if k not in ('model','messages','tools')}
        snapshot=HostModeSnapshot.from_host(plan,cap,instant_views=view['instant'],provider_envelope=envelope,host_observations=observations(core,s))
        result=compose(plan,snapshot,suffix,current_authorization=auth);capref=core.store.put('invocation_capability',{'chain_id':s['chain_id'],'payload':cap.data(),'plan_id':ref});s['shared_worker_capability']=capref
        return result.request,_meta(True,plan_id=ref,plan_digest=plan.digest,prefix_bytes_sha256=result.shared_prefix_digest,capability_id=capref,mode='worker',allowed_tools=cap.data()['allowed_tools'],identity_source=identity_source,actual_request_assembled=True)
    except (ValueError,TypeError,PermissionError) as error:
        s.pop('shared_worker_capability',None);return legacy,_meta(False,mode='worker',reason=str(error),actual_request_assembled=True)

def authorize(core,s,actor,name,*,meta=None):
    from .service import require
    ref=(meta or {}).get('capability_id') if meta is not None else s.get('shared_worker_capability')
    if not ref:return
    record=core.store.get(ref,'invocation_capability');require(record['chain_id']==s['chain_id'],'capability route mismatch','forbidden',403)
    plan,_=_load_plan(core,record['plan_id']);cap=InvocationCapability._freeze(record['payload']);c=cap.data()
    if c['mode']=='editor':
        job=s['jobs'].get(c['job_id']);require(job is not None,'Editor job no longer exists','forbidden',403)
        editor=next((e for e,m in job.get('prefix_layout',{}).items() if m.get('plan_id')==record['plan_id'] and job['wb_id']==c['scope_id'] and digest({'editor':e,'unit_ids':list(FrozenAliasMap.from_dict(job['editor_alias_maps'][e]).unit_ids)})==c.get('assignment_id')),None);require(editor is not None,'Editor invocation scope mismatch','forbidden',403)
        aliases=FrozenAliasMap.from_dict(job['editor_alias_maps'][editor]);auth=_auth(core,s,actor,'editor',plan,job=job,editor=editor,aliases=aliases)
    else:auth=_auth(core,s,actor,'worker',plan,scope=c['scope_id'])
    result=authorize_tool(cap,name,current_authorization=auth);require(result.allowed,'Host mode capability denied tool: '+'; '.join(result.reasons),'forbidden',403)

def adopt_editor(job,editor,parsed,meta):
    if meta.get('applied_layout')!='shared_i':return parsed
    aliases=FrozenAliasMap.from_dict(job['editor_alias_maps'][editor]);return canonical_response(aliases,parsed,job_id=job['job_id'],editor=editor)

def provenance(core,job,assignment):
    rows=[]
    for unit in job['source_snapshot']['units']:
        if unit['unit_id'] not in assignment['unit_ids']:continue
        sid=unit['anchor']['source_id'];message=core.store.get(sid,'source')['sidecar']['messages'][unit['anchor']['message']]
        row={'unit_id':unit['unit_id'],'source_id':sid,'message_index':unit['anchor']['message'],'role':message['role'],'readonly_protocol':{'assistant_tool_proposals':[{'id':c['id'],'name':c['function']['name']} for c in message.get('tool_calls') or []],'proposal_is_execution':False,'tool_result_for':message.get('tool_call_id'),'source_receipt_events':[e for e in job.get('source_snapshot_events',[]) if e.get('part_source_id')==sid],'open_obligations_at_freeze':job['source_snapshot'].get('open_calls',[]),'as_of_chain_event_index':job.get('created_event'),'actual_execution_status':'consult observed host tool events; absent evidence is unknown'}}
        if unit['anchor'].get('communication'):row['communication']=copy.deepcopy(unit['anchor']['communication'])
        rows.append(row)
    return rows

def observations(core,s):
    names={}
    for group in s['groups'].values():
        for sid in group:
            for message in core.store.get(sid,'source')['sidecar']['messages']:
                for call in message.get('tool_calls') or []:names[call['id']]=call['function']['name']
    facts=[{'type':e['type'],'call_id':e['call_id'],'name':names.get(e['call_id']),'outcome':e.get('outcome',e.get('status','unknown')),'observed_by':e.get('observed_by','host-event'),'event_id':e.get('event_id'),'core_observed_at':e.get('time'),'host_time_ms':e.get('host_time_ms')} for e in s['events'] if e.get('type') in ('tool_start','tool_end') and not e.get('duplicate_observation')]
    return [{'events':facts[-64:],'total':len(facts),'omitted':max(0,len(facts)-64),'full_digest':digest(facts),'complete_read_operation':'ledger.export','as_of_chain_event_index':len(s['events']),'interpretation':'Only observed events; a proposal or CB answer candidate is not a completed tool execution or a reply delivered to the Harness.'}]

def protocol_skeleton(core,job):
    result=[]
    for sid in job['source_snapshot']['source_ids']:
        source=core.store.get(sid,'source')
        for index,message in enumerate(source['sidecar']['messages']):
            result.append({'source_id':sid,'message_index':index,'role':message['role'],'assistant_tool_proposals':[{'id':c['id'],'name':c['function']['name']} for c in message.get('tool_calls') or []],'proposal_is_execution':False,'tool_result_for':message.get('tool_call_id'),'text_unit_ids':[u['unit_id'] for u in job['source_snapshot']['units'] if u['anchor']['source_id']==sid and u['anchor']['message']==index],'open_obligations_at_freeze':job['source_snapshot'].get('open_calls',[]),'source_receipt_events':[e for e in job.get('source_snapshot_events',[]) if e.get('part_source_id')==sid],'as_of_chain_event_index':job.get('created_event')})
    return result


def preview_worker(core,s,actor,legacy):
    core.store.db.execute('SAVEPOINT sic_shared_request_preview')
    try:
        request,meta=worker_request(core,copy.deepcopy(s),actor,legacy)
        meta.pop('capability_id',None);meta.pop('plan_id',None);meta['preview_only']=True;meta['simulation']='real assembler, all writes rolled back'
        return request,meta
    finally:
        core.store.db.execute('ROLLBACK TO sic_shared_request_preview');core.store.db.execute('RELEASE sic_shared_request_preview')
