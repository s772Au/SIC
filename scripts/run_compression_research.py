"""New-Core compression comparison; default owned fixture, explicit fixed provider opt-in."""
import os
import argparse,hashlib,importlib.util,json,sys,time,uuid,zipfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];sys.path[:0]=[str(ROOT),str(ROOT/'scripts')]
from sic_core.accounting import summarize_records,compare_workflows
from v2_experiment_fixture import OwnedFixture
from research_provider import ConfiguredCore,public_config,route_disclosure
spec=importlib.util.spec_from_file_location('control_pack',ROOT/'research/compression/pack/controls.py');control=importlib.util.module_from_spec(spec);spec.loader.exec_module(control)
PACK=ROOT/'research/compression/pack'
SCHEMA='sic-compression-research-run-v2'
SECRET_VALUES=[]

def save(p,x):
    p.parent.mkdir(parents=True,exist_ok=True);text=json.dumps(x,ensure_ascii=False,indent=2,allow_nan=False)
    for value in SECRET_VALUES:
        if value:text=text.replace(value,'[redacted]')
    p.write_text(text+'\n',encoding='utf-8')
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def answer(response):
    try:return response['choices'][0]['message']['content']
    except (KeyError,TypeError,IndexError):return None

def media_hashes(request):
    from sic_core.media import image_info
    return [image_info(part)['sha256'] for message in request['messages'] if isinstance(message.get('content'),list) for part in message['content'] if part.get('type')=='image_url']

def task_requests(task,media_root=None):
    if task.get('media_required'):
        from benchmarks.core_lme_carrier import load_prepared_trajectories,trajectory_request
        for row in load_prepared_trajectories(task,media_root):yield trajectory_request(row,media_root)
    else:
        for i,block in enumerate(task['blocks']):yield {'messages':([*task.get('protected',[])] if i==0 else [])+[{'role':'user','content':block}]}

def arm(folder,task,engine,pack,profile,prices,trim_budget,fanout,media_root=None,allow_test_gateway=False):
    start=time.time();cpu=time.process_time();factory=ConfiguredCore(folder/'core.db',profile,allow_test_gateway=allow_test_gateway) if profile else OwnedFixture(folder/'core.db',research_answers=True,media_profile=bool(task.get('media_required')))
    with factory as host:
        client=host.client;client.call('chain.create',{'creators':[]})
        client.call('config.update',{'worker_prompt':pack['prompts']['worker']+'\nTask: '+('Maintain chronological memory for future queries; downstream question is not available during editing.' if task.get('media_required') else task['goal']),'editor_prompt':pack['prompts']['structured_summary' if engine=='structured_summary' else 'sic_editor'],'program_rules':[],'schedule_policy':'wait'})
        client.call('runtime.configure',{'enabled':False,'editor_mode':'model','worker_mode':'host','seal_chars':1,'editor_workers':1})
        cbs=[];steps=[];edits=[];compressor_resources=[];expected_media=[]
        def seal():
            state=client.call('chain.get')
            if not state['pending']:return
            wb=client.call('wb.form',{'source_ids':state['pending'],'reason':'frozen-'+engine})
            root=wb.get('source_id') or client.call('chain.get')['pending'][0]
            if engine in ('sic','batch','structured_summary'):
                job=client.call('edit.prepare',{'source_id':root});client.call('edit.run_all',{'job_id':job['job_id']})
            else:
                job=client.call('edit.prepare',{'source_id':root});units=job['editable_units']
                if engine=='llmlingua':
                    from llmlingua_adapter import compress_units
                    local=compress_units(units,folder/'compressor-evidence');ops=local['ops']
                    compressor_resources.append({'record_id':'local-compressor-'+str(len(compressor_resources)),'resource':'program','quantity':str(local['process_cpu_seconds']),'unit':'process_cpu_second','measurement_method':'official local compressor subprocess CPU including weight load and actual inference; no invented provider-token bill'})
                else:ops=control.unit_ops(units,engine,trim_budget)
                client.call('edit.submit',{'job_id':job['job_id'],'editor':job['job']['assignments'][0]['editor'],'ops':ops,'pvi':{}})
            material=client.call('edit.validate',{'job_id':job['job_id']})
            pub=client.call('cut.publish',{'job_id':job['job_id'],'expected_cut':job['job']['base_cut'],'idempotency_key':uuid.uuid4().hex})
            now=client.call('cut.get')['view']['cbs'];assert now[:len(cbs)]==cbs,'Immutable CB changed';cbs[:]=now
            if fanout:client.call('hierarchy.maintain',{'fanout':fanout,'rules':['deduplicate_exact'],'roles':['worker','editor']})
            edits.append({'source_id':root,'job_id':job['job_id'],'wb_id':job['job']['wb_id'],'level':job['job']['level'],'source_text':'\n'.join(u['text'] for u in job['editable_units']),'candidate':material['material'],'publication':pub})
        for i,native in enumerate(task_requests(task,media_root)):
            if task.get('media_required'):expected_media.extend(media_hashes(native))
            client.call('source.ingest',{'request':native,'idempotency_key':'source-'+str(i)})
            if engine=='sic':seal()
            response=client.call('provider.continue',{'idempotency_key':'worker-'+str(i)})
            steps.append({'block':i,'answer':answer(response['response']),'raw_response':response['response'],'request_id':response['request_id']})
        seal()
        selected=client.call('hierarchy.get')
        active=selected.get('role_views',{}).get('worker',{'context_id':'main','level':selected['active']['worker']})
        level_state=(selected['levels'] if active.get('context_id','main')=='main' else selected['contexts'][active['context_id']]['levels'])[str(active['level'])]
        expected_ids=level_state['cbs']
        if task.get('media_required'):
            from benchmarks.core_lme_carrier import question_request
            final_native=question_request(task,media_root);final_native['messages'][0]['content'][0]['text']=task['final_question'];expected_media.extend(media_hashes(final_native))
        else:final_native={'messages':[{'role':'user','content':task['final_question']}]}
        client.call('source.ingest',{'request':final_native,'idempotency_key':'final-question'})
        final=client.call('provider.continue',{'idempotency_key':'final-answer'})
        request_archive=host.core.store.get(final['request_id'],'provider_request')
        actual_request=request_archive['request'];visible=[]
        for message in actual_request['messages']:
            try:record=json.loads(message.get('content',''))
            except (ValueError,TypeError):continue
            if isinstance(record,dict) and record.get('sic_cb'):visible.append(record['sic_cb'])
        if expected_ids and not set(visible).intersection(expected_ids):raise ValueError('Final Worker did not consume selected edited CB representation')
        actual_media=media_hashes(actual_request) if task.get('media_required') else []
        if not set(expected_media)<=set(actual_media):raise ValueError('Final Worker dropped immutable image bytes; text flattening forbidden')
        state=client.call('chain.get');usage=client.call('usage.report');end=time.time()
        save(folder/'final-request.json',request_archive)
        consumption={'active_before_final_question':active,'expected_active_cb_ids':expected_ids,'actually_visible_cb_ids':visible,'request_id':final['request_id'],'cut_id':final['cut_id'],'complete_selected_ids_visible':set(expected_ids)<=set(visible),'expected_image_sha256':expected_media,'actual_image_sha256':actual_media,'media_preserved':set(expected_media)<=set(actual_media),'bounded_projection_policy':'omitted history remains explicit navigation; no claim all bodies fit'}
        resources=[{'record_id':'program-cpu','resource':'program','quantity':str(time.process_time()-cpu),'unit':'process_cpu_second','measurement_method':'isolated arm process_time; includes SDK and fixture overhead'}, {'record_id':'storage-end','resource':'storage','quantity':str(sum(p.stat().st_size for p in folder.glob('core.db*'))),'unit':'byte_at_snapshot','measurement_method':'SQLite files at end; not byte-hours'}]
        resources.extend(compressor_resources)
        known_jobs={e['job_id'] for e in edits}
        ids=list(state['cbs'])
        for level in state.get('hierarchy',{}).get('levels',{}).values():ids.extend(level['cbs'])
        archived_cbs=[host.core.store.get(i,'cb') for i in dict.fromkeys(ids)]
        for job_id,job in state['jobs'].items():
            if job_id in known_jobs:continue
            cb=next((c for c in archived_cbs if c.get('source_id')==job['source_id']),None)
            if cb:edits.append({'source_id':job['source_id'],'job_id':job_id,'wb_id':job['wb_id'],'level':job['level'],'source_text':'\n'.join(u['text'] for u in job['source_snapshot']['units']),'candidate':{'body':cb['body'],'ops':cb['ops']},'publication':{'cb_id':cb['cb_id']}})
        ledger={'calls':usage['calls'],'resources':resources,'prices':prices,'task_interval':[start,end],'events':[]}
        summary=summarize_records(ledger['calls'],resources,[start,end],prices,[])
        save(folder/'ledger.json',ledger);save(folder/'summary.json',summary);save(folder/'trace.json',client.trace)
        save(folder/'result.json',{'schema':'sic-compression-arm-v2','engine':engine,'worker_steps':steps,'edits':edits,'fixed_cbs':cbs,'quality':{'status':'not_evaluated','score':None},'pending':state['pending'],'endpoint_policy':'retain-final-question-and-final-answer-unedited; no post-answer compression','final_answer':answer(final['response']),'final_response':final['response'],'final_consumption':consumption})
        if not profile:save(folder/'model-http.json',host.model_requests)
        return {'engine':engine,'status':'passed','calls':len(usage['calls']),'editor_calls':sum(c['role']=='editor' for c in usage['calls']),'summary':summary}

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--task',type=Path);ap.add_argument('--media-data-root',type=Path);ap.add_argument('--dataset');ap.add_argument('--chunk-bytes',type=int,default=32768);ap.add_argument('--output',type=Path,required=True);ap.add_argument('--engines',nargs='+',default=['batch','sic','structured_summary','program_exact','langchain_trim','unchanged']);ap.add_argument('--provider-config',type=Path);ap.add_argument('--allow-provider',action='store_true');ap.add_argument('--allow-test-gateway',action='store_true',help='Explicit exploratory 8341 route; no fixed-model/generalization claim');ap.add_argument('--prices',type=Path);ap.add_argument('--trim-budget',type=int,default=4096);ap.add_argument('--hierarchy-fanout',type=int,default=0);args=ap.parse_args()
    if args.hierarchy_fanout and args.hierarchy_fanout<2:raise ValueError('Hierarchy fanout must be at least two')
    if args.provider_config and not args.allow_provider:raise ValueError('Fixed provider requires explicit --allow-provider')
    if args.allow_test_gateway and not (args.allow_provider and args.provider_config):raise ValueError('--allow-test-gateway requires --provider-config and --allow-provider')
    if args.allow_provider and not args.provider_config:raise ValueError('--allow-provider requires an explicit private config')
    pack=json.loads((PACK/'control-pack.json').read_text(encoding='utf-8'))
    if any(e not in pack['engines'] or pack['engines'][e].get('executable') is False for e in args.engines):raise ValueError('Unavailable control; no silent fallback')
    if args.dataset:
        from benchmarks.core_adapters import tasks
        task=next(tasks(args.dataset,limit=1,chunk_bytes=args.chunk_bytes,question_limit=1))
    else:task=json.loads(args.task.read_text(encoding='utf-8-sig')) if args.task else {'schema':'sic-research-task-v2','task_id':'synthetic-mechanical-only','goal':'Preserve evidence and choose a supported next action.','blocks':['Fact: server is on port 7193. Failed: port 7192 refused.','Suggestion, not executed: inspect the configuration.'], 'references':{'answer':'7193'},'scoring':{'kind':'normalized_exact','official_score':False}}
    if task.get('media_required') and not args.media_data_root:raise ValueError('Prepared media task requires explicit --media-data-root; images never flattened')
    if task.get('missing_images'):raise ValueError('Prepared task has missing screenshots; no partial official substitution')
    if not task.get('media_required') and (not task.get('blocks') or any(not isinstance(b,str) for b in task['blocks'])):raise ValueError('Nonempty text blocks required')
    task.setdefault('final_question','SIC_RESEARCH_FINAL_QUESTION\nAnswer now using the retained context and authorized source navigation.\n'+(task['goal'].get('text') if isinstance(task['goal'],dict) else task['goal']))
    out=args.output
    if out.exists() and any(out.iterdir()):raise ValueError('Output must be new/empty')
    profile=json.loads(args.provider_config.read_text(encoding='utf-8-sig')) if args.provider_config else None
    if profile:
        SECRET_VALUES[:]=[os.environ.get(p.get('api_key_env',''),'') for p in profile.get('roles',{}).values()]
    disclosure=route_disclosure(profile,args.allow_test_gateway) if profile else {'route_status':'owned-fixture'}
    prices=json.loads(args.prices.read_text(encoding='utf-8-sig')) if args.prices else []
    save(out/'task.json',task);save(out/'configuration.json',{'schema':'sic-research-configuration-v2',**disclosure,'mode':'fixed_provider' if profile else 'owned_fixture','profiles':public_config(profile,args.allow_test_gateway) if profile else {'model':'openai/gpt-4o' if task.get('media_required') else 'openai/sic-experiment-fixture','route':'owned-loopback-fixture-only-not-real-provider','usage':'synthetic-character-quanta','tokenizer_status':'fixture-not-provider-verified'},'engines':args.engines,'trim_budget':args.trim_budget,'hierarchy_fanout':args.hierarchy_fanout,'hierarchy_policy':'program maintain after each publication; changes workflow, not a pure cache comparison','prices':prices,'native_input_policy':'official ordered raw trajectories with immutable images; downstream question only after final seal' if task.get('media_required') else 'lossless text blocks','source_lock_sha256':sha(ROOT/'benchmarks/sources-lock.json'),'pack_sha256':sha(PACK/'control-pack.json'),'sampling':'per fixed profile; no automatic retries','quality':'not_evaluated','resources_without_prices':'unknown'})
    sources=list((ROOT/'sic_core').rglob('*.py'))+[ROOT/'sic_core/operations.json',Path(__file__),ROOT/'scripts/research_provider.py',ROOT/'scripts/llmlingua_adapter.py',ROOT/'scripts/llmlingua_worker.py',ROOT/'scripts/research_scoring.py',ROOT/'scripts/score_compression_research.py',ROOT/'scripts/dataset_adapters.py',ROOT/'benchmarks/source_paths.py',ROOT/'benchmarks/sources-lock.json',ROOT/'scripts/v2_experiment_fixture.py',ROOT/'benchmarks/core_adapters.py',ROOT/'benchmarks/core_lme_carrier.py']+list(PACK.rglob('*'))
    with zipfile.ZipFile(out/'code-snapshot.zip','w',zipfile.ZIP_DEFLATED) as z:
        for p in sources:
            if p.is_file():z.write(p,p.relative_to(ROOT).as_posix())
    status='passed';results=[]
    try:
        for engine in args.engines:results.append(arm(out/engine,task,engine,pack,profile,prices,args.trim_budget,args.hierarchy_fanout,args.media_data_root,args.allow_test_gateway))
        anchor=next((r for r in results if r['engine']=='batch'),results[0]);comparisons={r['engine']:compare_workflows(anchor['summary'],r['summary']) for r in results if r is not anchor}
        save(out/'comparison.json',{'schema':'sic-compression-comparison-v2','arms':results,'reference_engine':anchor['engine'],'independent_accounting_pairs':comparisons,'quality':{'status':'not_evaluated'},'causal_claim':'No quality/cost superiority inferred; output/caching/resources differ by observed workflow'})
    except Exception as e:status='failed';save(out/'failure.json',{'type':type(e).__name__,'message':str(e),'quality':'not_evaluated'})
    files={p.relative_to(out).as_posix():{'sha256':sha(p),'bytes':p.stat().st_size} for p in sorted(out.rglob('*')) if p.is_file()}
    save(out/'manifest.json',{'schema':SCHEMA,**disclosure,'status':status,'mode':'fixed_provider' if profile else 'owned_fixture','new_results_only':True,'files':files,'research_evaluation':'not_evaluated','real_provider_calls':None if profile else 0})
    print(json.dumps({'status':status,**disclosure,'mode':'fixed_provider' if profile else 'owned_fixture','quality':'not_evaluated','engines':args.engines,'real_provider_calls':None if profile else 0}))
    if status!='passed':raise SystemExit(1)
if __name__=='__main__':main()
