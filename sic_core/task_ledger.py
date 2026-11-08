"""Authorized, transaction-consistent accounting over a declared task graph.

No provider traffic. All raw records are preserved; per-chain rates are frozen
onto derived records before aggregation so prices never bleed between routes.
"""
import copy
from . import accounting


def export_task(core, root_state, actor, payload):
    from .service import require
    require(core.store.db.in_transaction,'Task ledger requires one Store transaction')
    require(not set(payload)-{'scope','additional_chain_ids','prices_by_chain','task_interval'},'Unknown task ledger option')
    extra=payload.get('additional_chain_ids',[])
    require(isinstance(extra,list) and all(isinstance(x,str) and x for x in extra) and len(extra)==len(set(extra)),'additional_chain_ids must be unique route IDs')
    require('prices' not in payload,'Task prices must use per-chain prices_by_chain, not one global override')
    seeds=list(dict.fromkeys([root_state['chain_id']]+extra));states={};edges=[];queue=list(seeds)
    while queue:
        chain=queue.pop(0)
        if chain in states:continue
        state=core.store.state(chain)
        require(actor in state['owners'],'Task scope contains an unauthorized route','forbidden',403)
        try:
            for group in state['groups'].values():
                for source_id in group:core.store.get(source_id,'source')
        except ValueError as error:
            raise type_error('Task route depends on withdrawn or unreadable material') from error
        states[chain]=state
        for request_id,request in state.get('collaborations',{}).items():
            for branch in request['branches']:
                child_id=branch['chain_id'];child=core.store.state(child_id)
                require(actor in child['owners'],'Task scope contains an unauthorized route','forbidden',403)
                task=child.get('collaboration_task',{})
                require(task.get('sender')==chain and task.get('request_id')==request_id,'Collaboration branch has inconsistent task provenance','task_scope_conflict',409)
                edges.append({'parent_chain_id':chain,'request_id':request_id,'chain_id':child_id,'request_status':request['status'],'branch_status':branch['status']})
                if child_id not in states:queue.append(child_id)
    overrides=payload.get('prices_by_chain',{})
    require(isinstance(overrides,dict) and set(overrides)<=set(states) and all(isinstance(v,list) for v in overrides.values()),'prices_by_chain must bind only included routes to price arrays')
    calls={};resources={};priced_calls={};priced_resources={};locations={'calls':{},'resources':{}};price_bindings={};contexts={};events=[];intervals=[];pending=[];chains=[]
    for chain,state in states.items():
        prices=copy.deepcopy(overrides.get(chain,state['config'].get('prices',[])))
        chain_calls=[];chain_resources=[]
        for object_id in state['usage']:
            record=core.store.get(object_id,'usage');identity=record['call_id'];derived=bind_prices(record,prices,False)
            insert_unique(calls,priced_calls,locations['calls'],identity,record,derived,chain,require)
            chain_calls.append(identity)
            refs=record.get('source_refs',{});job=state.get('jobs',{}).get(refs.get('job_id'),{})
            context={'chain_id':chain,'role':record.get('role'),'model':record.get('model'),'route':record.get('route'),'job_id':refs.get('job_id'),'level':record.get('level',refs.get('level',job.get('level'))),'context_id':record.get('context_id',refs.get('context_id',job.get('context_id')))}
            if job:
                background=job.get('editor_backgrounds',{}).get(refs.get('editor'),{}).get('manifest',{})
                context.update(editing_level=job.get('level'),background_level=background.get('active_level'),background_context_id=background.get('context_id'))
            if record.get('role')=='worker' and refs.get('request_id'):
                request=core.store.get(refs['request_id'],'provider_request')
                # A caller-supplied source reference never grants access to an
                # unrelated route's request or historical representation.
                if request.get('chain_id') in states and request.get('cut_id'):
                    cut=core.store.get(request['cut_id'],'cut')
                    if cut['chain_id']==request['chain_id']:
                        from .hierarchy import selected_view
                        view=selected_view(copy.deepcopy(cut),'worker')
                        context.update(level=view['level'],context_id=view['context_id'],representation_evidence='immutable-request-cut')
            contexts.setdefault(identity,[]).append(context)
        for object_id in state.get('resources',[]):
            record=core.store.get(object_id,'resource');identity=record['record_id'];declared=record.get('scope_chain_ids')
            require(declared is None or isinstance(declared,list) and bool(declared) and set(declared)<=set(states),'Resource scope_chain_ids must stay within declared task scope','task_scope_conflict',409)
            insert_unique(resources,priced_resources,locations['resources'],identity,record,bind_prices(record,prices,True),chain,require);chain_resources.append(identity)
        events.extend({'chain_id':chain,'event':copy.deepcopy(event)} for event in state['events'])
        intervals.extend({**copy.deepcopy(event),'chain_id':chain} for event in core._account_events(state))
        inflight=state.get('provider_inflight')
        if inflight and inflight['call_id'] not in calls:pending.append({'chain_id':chain,'role':'worker',**copy.deepcopy(inflight)})
        for job in state['jobs'].values():
            for editor,attempt in job.get('attempts',{}).items():
                if attempt.get('call_id') and attempt['call_id'] not in calls and attempt.get('status') in ('running','indeterminate'):
                    pending.append({'chain_id':chain,'role':'editor','editor':editor,'job_id':job['job_id'],'level':job.get('level'),'context_id':job.get('context_id'),**copy.deepcopy(attempt)})
        price_bindings[chain]={'source':'explicit-override' if chain in overrides else 'chain-config-snapshot','prices':prices}
        chains.append({'chain_id':chain,'cut_id':state['current_cut'],'cut_sequence':state['cut_seq'],'run_state':state['run_state'],'call_ids':chain_calls,'resource_ids':chain_resources})
    pending_by_id={}
    for attempt in pending:
        if attempt['call_id'] in calls:continue
        require(attempt['call_id'] not in pending_by_id or pending_by_id[attempt['call_id']]==attempt,'Conflicting pending call identity','ledger_identity_conflict',409)
        pending_by_id[attempt['call_id']]=attempt
    pending=list(pending_by_id.values())
    summary=accounting.summarize_records(list(priced_calls.values()),list(priced_resources.values()),payload.get('task_interval'),[],intervals,required_resources=())
    timing_by_chain={chain:accounting.timing_summary([calls[identity] for identity in dict.fromkeys(next(row['call_ids'] for row in chains if row['chain_id']==chain))],core._account_events(state),payload.get('task_interval')) for chain,state in states.items()}
    summary['timing']['intersection_scope']='task-wide intersections may cross routes; not evidence of one Agent natural-window reuse'
    coverage={resource:set() for resource in ('program','storage')}
    for identity,record in resources.items():
        if record.get('resource') in coverage and record.get('quantity') is not None:
            coverage[record['resource']].update(record.get('scope_chain_ids') or locations['resources'][identity])
    for resource,covered in coverage.items():
        for chain in states:
            if chain not in covered:summary['unknown'].append({'chain_id':chain,'resource':resource,'reason':'resource_measurement_missing_for_route'})
    # A call that is billed later must not disappear or become a free snapshot.
    for attempt in pending:summary['unknown'].append({'chain_id':attempt['chain_id'],'call_id':attempt['call_id'],'reason':'attempt_usage_not_yet_recorded'})
    for chain,state in states.items():
        for call_id,observation in state.get('open_tools',{}).items():
            summary['timing']['unknown'].append({'chain_id':chain,'call_id':call_id,'reason':'tool_interval_not_complete','started_at':observation.get('started_at')})
    unfinished=[edge for edge in edges if edge['request_status'] not in ('completed','failed','cancelled') or edge['branch_status'] not in ('completed','failed','cancelled')]
    summary['complete_cost']=not summary['unknown']
    if not summary['complete_cost']:summary['total_cost']=None
    return {'schema':'sic-task-ledger-v1','scope':{'kind':'task','root_chain_id':root_state['chain_id'],'additional_chain_ids':extra,'included_chain_ids':list(states),'traversal':'recursive recorded collaboration branches, all statuses; additional routes and their branches','excluded_relations':['unreferenced forks','blank-created routes','creator relations','Socket endpoints without a recorded dispatched branch'],'completeness':'all recorded calls/resources in declared graph at one Store snapshot; no claim about unreferenced routes or future calls','cost_horizon':'observed snapshot, not a prediction of final task cost','unfinished_branches':unfinished},'snapshot':{'transaction_consistent':True,'exported_at':core.now(),'chains':chains},'calls':list(calls.values()),'resource_records':list(resources.values()),'priced_calls':list(priced_calls.values()),'priced_resource_records':list(priced_resources.values()),'record_locations':locations,'call_contexts':contexts,'prices_by_chain':price_bindings,'pending_attempts':pending,'events':events,'accounting_intervals':intervals,'timing_by_chain':timing_by_chain,'summary':summary}


def type_error(message):
    from .service import CoreError
    return CoreError('revoked_dependency',message,403)


def bind_prices(record,prices,resource):
    derived=copy.deepcopy(record)
    items=[derived] if resource else derived.get('categories',[])
    for item in items:
        lookup={**item,'category':item.get('category',item.get('resource'))}
        rate=accounting._price_for(lookup,derived,prices)
        if rate:
            for key in ('unit_price','per_quantity','currency','price_version','price_basis'):
                if key in rate:item[key]=copy.deepcopy(rate[key])
    return derived


def insert_unique(raws,priced,locations,identity,record,derived,chain,require):
    require(isinstance(identity,str) and bool(identity),'Accounting record requires a nonempty identity')
    if identity in raws:
        require(raws[identity]==record and priced[identity]==derived,'Duplicate accounting identity has conflicting record or price binding','ledger_identity_conflict',409)
    else:raws[identity]=copy.deepcopy(record);priced[identity]=derived
    locations.setdefault(identity,[])
    if chain not in locations[identity]:locations[identity].append(chain)
