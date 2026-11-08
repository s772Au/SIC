"""Same-cut request comparison through rollback-only real publication and role assembly."""
import copy
import hashlib
import json
from .adapters import encode


def canonical(request):return json.dumps(request,ensure_ascii=False,sort_keys=True).encode()
def common(a,b):
    for i,(x,y) in enumerate(zip(a,b)):
        if x!=y:return i
    return min(len(a),len(b))


def compare(core,state,actor,p):
    from .service import require
    require(p['expected_cut']==state['cut_seq'],'comparison cut changed')
    job=core._job(state,p);require(job['base_cut']==state['cut_seq'],'comparison job is stale')
    core._combined(state,job) # Validate complete candidate before either simulation.
    role=p.get('role','worker');require(isinstance(role,str) and (role=='worker' or role.startswith('editor:')),'role must be worker or editor:<identity>')
    budget=int(p.get('length_budget',0));position_budget=int(p.get('position_budget',0))
    require(budget>=0 and position_budget>=0,'nonnegative comparison budgets required')
    editor=role.split(':',1)[1] if role!='worker' else None
    if editor:require(bool(editor) and p.get('next_source_id'),'Editor comparison requires identity and next_source_id')
    def request_for(s,previous=False):
        from . import prefix_layout
        if role=='worker':return prefix_layout.worker_request(core,s,actor,encode(core._working_view(s,'worker')),_comparison_ids={'plan':'same-future-prefix-plan'})[0]
        source=core._source(s,p['next_source_id'])
        if previous:
            # Preparing this same declared future scope needs no publication;
            # only the local copy's order is advanced to inspect its old background.
            from . import hierarchy
            row=hierarchy.level(s,hierarchy.source_level(s,p['next_source_id']),hierarchy.source_context(s,p['next_source_id']))
            row['pending'][:]=row['pending'][row['pending'].index(p['next_source_id']):]
        scope=p.get('next_unit_ids',[u['unit_id'] for u in source['units']])
        result=core._dispatch('edit.prepare',s,actor,{'source_id':p['next_source_id'],'assignments':[{'editor':editor,'unit_ids':scope}]},_comparison_ids={'job':'same-future-editor-job','wb':'same-future-wb','plan':'same-future-prefix-plan'})
        return prefix_layout.editor_request(core,s,result['job'],editor,core._editor_request(result['job'],editor))[0]
    def sandbox(kind):
        core.store.db.execute('SAVEPOINT sic_frontier_comparison')
        try:
            simulated=copy.deepcopy(state)
            if kind is not None:
                target=simulated['jobs'][job['job_id']]
                if kind=='necessary':
                    target.pop('candidate',None)
                    target['assignments']=[{'editor':'necessary-keep','unit_ids':[u['unit_id'] for u in target['source_snapshot']['units']]}]
                    target['responses']={'necessary-keep':{'ops':[],'pvi':{},'origin':'comparison-keep'}}
                    target['status']='ready'
                core._publish(simulated,{'job_id':job['job_id'],'expected_cut':state['cut_seq'],'idempotency_key':'frontier-rollback-only'},_comparison_ids={'cb':'same-pending-slot','cut':'same-frontier-cut','source':'same-volatile-source'})
            return request_for(simulated,previous=kind is None)
        finally:
            core.store.db.execute('ROLLBACK TO sic_frontier_comparison')
            core.store.db.execute('RELEASE sic_frontier_comparison')
    old=sandbox(None)
    necessary=sandbox('necessary');additional=sandbox('additional')
    a,b=canonical(necessary),canonical(additional);prefix=common(a,b)
    before=old or necessary;old_bytes=canonical(before);rebuild_prefix=common(old_bytes,a)
    earlier_equal=a[:rebuild_prefix]==b[:rebuild_prefix]
    def first_difference(left,right):
        for i,(x,y) in enumerate(zip(left['messages'],right['messages'])):
            if x!=y:return i
        return min(len(left['messages']),len(right['messages']))
    frontier=first_difference(before,necessary)
    def slot(request):
        for i,message in enumerate(request['messages']):
            try:value=json.loads(message.get('content',''))
            except (TypeError,ValueError):continue
            if isinstance(value,dict) and value.get('sic_cb')=='same-pending-slot':return i
        return first_difference(before,request)
    necessary_position,additional_position=slot(necessary),slot(additional)
    position_delta=abs(necessary_position-additional_position);position_ok=position_delta<=position_budget
    controls=lambda req:{k:v for k,v in req.items() if k!='messages'}
    same_control=controls(necessary)==controls(additional)
    cut=core.store.get(state['current_cut'],'cut') if state['current_cut'] else None
    current_tools=[t['schema'] for t in state['tools'] if t.get('expanded') and all(core._tool_acl(state,t,layer,actor) for layer in ('discover','expand','execute'))]
    previous_tools=[t['schema'] for t in (cut or {}).get('tools',[]) if t.get('expanded')]
    prior_control_change=bool(cut) and current_tools!=previous_tools
    prior_teaching_change=bool(cut) and state['config'].get('worker_prompt')!=cut.get('config',{}).get('worker_prompt')
    length_delta=len(b)-len(a)
    return {'same_cut':state['current_cut'],'job_id':job['job_id'],'role':role,'edited_level':job.get('level',1),'edited_context':job.get('context_id','main'),'necessary_request':necessary,'additional_request':additional,'previous_request':old,'structured_frontier_message_index':frontier,'necessary_rebuild_prefix_bytes':rebuild_prefix,'necessary_slot_message_index':necessary_position,'additional_slot_message_index':additional_position,'position_delta_messages':position_delta,'position_budget_unit':'canonical-message-index','earlier_prefix_equal':earlier_equal,'controls_equal':same_control,'canonical_common_prefix_bytes':prefix,'length_delta_bytes':length_delta,'length_budget_bytes':budget,'position_budget':position_budget,'position_within_budget':position_ok,'control_change_against_previous':prior_control_change,'teaching_change_against_previous':prior_teaching_change,'necessary_rebuild_may_start_before_messages':prior_control_change or prior_teaching_change,'within_local_window':earlier_equal and same_control and position_ok and not prior_control_change and not prior_teaching_change and abs(length_delta)<=budget,'physical_cache_hit':'unknown','provider_internal_layout':'unknown','stable_segment_digest':hashlib.sha256(a[:rebuild_prefix]).hexdigest(),'request_affected_by_candidate':necessary!=additional,'comparison_ids':{'cb':'same-pending-slot','cut':'same-frontier-cut','volatile_source':'same-volatile-source','future_job':'same-future-editor-job','future_wb':'same-future-wb','prefix_plan':'same-future-prefix-plan','meaning':'controlled hypothetical identities, not published UUIDs'},'simulation':'real-publication-and-role-assembly-savepoint-rolled-back'}
