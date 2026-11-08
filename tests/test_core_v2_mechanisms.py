"""New-contract behavioral acceptance. All providers here are explicit local doubles."""
import concurrent.futures,copy,json,threading,time,unittest
from sic_core import Core,CoreError
from sic_core.decision import VIEWS

class MechanismProvider:
    def __init__(self,barrier=None):self.calls=[];self.barrier=barrier;self.lock=threading.Lock()
    def capabilities(self):return {'test_double':True,'live_provider_verified':False}
    def call(self,role,request):
        with self.lock:self.calls.append((role,copy.deepcopy(request)))
        if self.barrier and role=='editor':self.barrier.wait(3)
        if role=='editor':
            value=json.loads(request['messages'][-1]['content']);content=json.dumps({'ops':[{'op':'REPLACE','target':u['unit_id'],'content':u['text']} for u in value['editable_text_units']],'pvi':{}})
        else:content='Local fixture branch result'
        return {'requested_model':'local-mechanism','returned_model':'local-mechanism','response':{'model':'local-mechanism','choices':[{'finish_reason':'stop','message':{'role':'assistant','content':content}}]},'usage':{'prompt_tokens':10,'completion_tokens':5,'total_tokens':15,'prompt_tokens_details':{'cached_tokens':0}}}

class MechanismTests(unittest.TestCase):
    def setUp(self):self.provider=MechanismProvider();self.core=Core(provider=self.provider);self.chain=self.op('chain.create')['chain_id']
    def tearDown(self):self.core.runtime.stop();self.core.store.db.close()
    def op(self,name,p=None,chain=None,**flags):return self.core.operate(name,{'actor':'host','chain_id':chain or getattr(self,'chain',None),'allow_write':True,'allow_model':True,**flags},p or {})['result']
    def source(self,text='input',chain=None):return self.op('source.ingest',{'request':{'messages':[{'role':'user','content':text}]}},chain)['source_id']
    def publish(self,job,chain=None):
        state=self.op('chain.get',chain=chain);return self.op('cut.publish',{'job_id':job,'expected_cut':state['cut_seq'],'idempotency_key':job},chain)
    def test_model_candidate_cas_and_composable_ops_original_response_immutable(self):
        source=self.source();job=self.op('edit.prepare',{'source_id':source})['job_id'];response=self.op('edit.run',{'job_id':job,'editor':'editor-1'});raw=self.core.store.get(response['response_id'],'provider_response');candidate=self.op('edit.candidate.get',{'job_id':job});self.assertEqual(candidate['revision'],0)
        ops=[{'op':'REPLACE','target':'u1','content':'human revision','risk':'low'},{'op':'FLAG','target':'u1','content':'verify evidence'},{'op':'REORDER','target':'u1','order':-1}];changed=self.op('edit.candidate.update',{'job_id':job,'expected_revision':0,'ops':ops,'pvi':{'P':['manual P'],'V':['manual V'],'I':{'workbench':'manual I'}}});self.assertEqual(changed['revision'],1)
        with self.assertRaises(CoreError):self.op('edit.candidate.update',{'job_id':job,'expected_revision':0,'ops':[]})
        with self.assertRaises(CoreError):self.op('cut.publish',{'job_id':job,'expected_cut':0,'expected_candidate_revision':0,'idempotency_key':'stale'})
        self.publish(job);cb=self.op('cut.get')['view']['cbs'][0];self.assertIn('human revision',cb['body']);self.assertIn('FLAG',cb['body']);self.assertEqual(cb['candidate_revision_id'],changed['revision_id']);self.assertEqual(self.core.store.get(response['response_id'],'provider_response'),raw)
    def test_generated_content_lifecycle_independent_from_persistent_ops(self):
        source=self.source();job=self.op('edit.prepare',{'source_id':source})['job_id'];ops=[{'op':'ADD','content':'stable','lifecycle':'P'},{'op':'ADD','content':'volatile','lifecycle':'V'},{'op':'ADD','content':'current','lifecycle':'I','view_key':'current-note'}];self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':ops});self.publish(job);view=self.op('cut.get')['view'];cb=view['cbs'][0];self.assertIn('stable',cb['body']);self.assertNotIn('volatile',cb['body']);self.assertNotIn('current',cb['body']);self.assertEqual(view['instant']['current-note'],'current');self.assertEqual(view['pending'][0]['units'][0]['text'],'volatile');self.assertEqual([o['lifecycle'] for o in cb['ops'] if o['op']=='ADD'],['P','V','I'])
    def test_real_parallel_editors_and_active_program_rule_combine(self):
        self.core.provider=MechanismProvider(threading.Barrier(2));source=self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'a'},{'role':'user','content':'b'},{'role':'user','content':'a'}]}})['source_id'];job=self.op('edit.prepare',{'source_id':source,'assignments':[{'editor':'one','unit_ids':['u1']},{'editor':'two','unit_ids':['u2']}],'program_rules':['deduplicate_exact']})['job_id'];result=self.op('edit.run_all',{'job_id':job,'max_workers':2});self.assertEqual(result['parallel_workers'],2);self.publish(job);cb=self.op('cut.get')['view']['cbs'][0];self.assertEqual(cb['body'],'a\nb');self.assertTrue(any(o.get('editor')=='program-rules' for o in cb['ops']))
    def test_wb_form_preserves_all_source_links_and_frozen_scope_refuses_regroup(self):
        a=self.source('a');b=self.source('b');formed=self.op('wb.form',{'source_ids':[a,b],'reason':'task complete'});job=self.op('edit.program',{'source_id':formed['source_id']})['job_id']
        with self.assertRaises(CoreError):self.op('wb.form',{'source_ids':[a]})
        published=self.publish(job);located=self.op('memory.locate',{'source_id':b});self.assertEqual(located['rows'][0]['cb_id'],published['cb_id']);self.assertEqual(located['rows'][0]['source_ids'],[a,b]);self.assertTrue(located['rows'][0]['wb_id'])
    def test_four_view_decision_updates_with_sourced_events_and_editor(self):
        self.op('decision.update',{'expected_revision':0,'events':[{'kind':kind,'text':kind+' item','evidence':[{'source_id':'declared-host-event'}]} for kind in ('work','failure','candidate','overlooked')]});view=self.op('decision.get');self.assertEqual(set(view['views']),set(VIEWS));self.assertTrue(all(view['views'][key] for key in VIEWS));self.assertEqual(view['discovery_prompt'],'What have I missed?')
        source=self.source();job=self.op('edit.program',{'source_id':source})['job_id'];self.publish(job);self.assertEqual(self.op('cut.get')['view']['instant']['decision_tree']['revision'],1)
        with self.assertRaises(CoreError):self.op('decision.update',{'expected_revision':0,'events':[]})
    def test_bounded_editor_and_worker_context_archive_pages_and_navigation(self):
        self.op('config.update',{'memory_chars':2048,'editor_memory_chars':2048})
        first=None
        for i in range(12):
            source=self.source(('early needle '+str(i)+' ')+'word '*600);first=first or source;job=self.op('edit.program',{'source_id':source})['job_id'];self.publish(job)
        count=len(self.op('chain.get')['cbs']);projection=self.op('memory.project',{'role':'worker','max_chars':2048});self.assertEqual(projection['manifest']['archive_count'],count);self.assertGreater(projection['manifest']['omitted_count'],0)
        source=self.source('next block');prepared=self.op('edit.prepare',{'source_id':source});self.assertGreater(prepared['job']['background_manifest']['omitted_count'],0);self.assertLess(len(json.dumps(prepared['job']['background'])),5000)
        output='';offset=0
        while True:
            page=self.op('memory.page',{'kind':'source','object_id':first,'offset':offset,'limit':128});output+=page['value']
            if page['next_offset'] is None:break
            offset=page['next_offset']
        self.assertIn('early needle 0',output);self.assertTrue(output.endswith('word '*600));self.assertEqual(self.op('memory.locate',{'source_id':first})['rows'][0]['membership_index'],0)
    def test_long_single_text_split_keeps_exact_code_characters(self):
        text='{"task":"'+('姹夊瓧abc'*1500)+'"}';source=self.source(text);job=self.op('edit.program',{'source_id':source,'rules':[]})['job_id'];self.publish(job);self.assertEqual(self.op('cut.get')['view']['cbs'][0]['body'],text)
    def test_same_cut_two_projection_window_and_model_unknown(self):
        source=self.source();job=self.op('edit.prepare',{'source_id':source})['job_id'];self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[{'op':'REPLACE','target':'u1','content':'changed'}]});comparison=self.op('frontier.compare',{'job_id':job,'expected_cut':0,'length_budget':4096});self.assertTrue(comparison['earlier_prefix_equal']);self.assertTrue(comparison['controls_equal']);self.assertTrue(comparison['within_local_window']);self.assertEqual(comparison['physical_cache_hit'],'unknown')
    def test_wait_statement_changes_actual_schedule_input_and_cancellation(self):
        source=self.source();self.op('edit.prepare',{'source_id':source});ordinary=self.op('harness.schedule',{'policy':'adaptive'});self.assertTrue(ordinary['worker_may_continue']);self.op('harness.event',{'type':'wait','wait_id':'host-wait','seconds':30});waiting=self.op('harness.schedule',{'policy':'adaptive'});self.assertEqual(waiting['decision'],'overlap-declared-wait');self.op('harness.event',{'type':'wait_cancel','wait_id':'host-wait'});done=self.op('harness.schedule',{'policy':'adaptive'});self.assertFalse(done['wait_input']['active_wait_ids'])
    def test_worker_inflight_publish_defers_without_discarding_valid_reply(self):
        started=threading.Event();finish=threading.Event();provider=self.provider
        class Slow(MechanismProvider):
            def call(self,role,request):
                if role=='worker':started.set();finish.wait(3)
                return super().call(role,request)
        self.core.provider=Slow();source=self.source();job=self.op('edit.prepare',{'source_id':source})['job_id'];results=[]
        def run():results.append(self.op('provider.continue',{'idempotency_key':'inflight','policy':'interleave'}))
        thread=threading.Thread(target=run);thread.start();self.assertTrue(started.wait(2));self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[]})
        with self.assertRaises(CoreError) as error:self.publish(job)
        self.assertEqual(error.exception.code,'publication_deferred');finish.set();thread.join(3);self.assertIn('response',results[0]);self.publish(job);self.assertEqual(self.op('cut.get')['sequence'],1)
    def test_event_driven_program_runtime_progresses_and_does_not_idle_publish(self):
        self.op('runtime.configure',{'enabled':True,'editor_mode':'program','worker_mode':'host','seal_chars':1});self.source('event work');deadline=time.monotonic()+3
        while time.monotonic()<deadline:
            state=self.op('chain.get')
            if state['cbs']:break
            time.sleep(.01)
        self.assertTrue(state['cbs']);before=len(state['cbs']);self.core.runtime.drain();self.assertEqual(len(self.op('chain.get')['cbs']),before);self.assertEqual(self.provider.calls,[])
    def test_model_runtime_standing_authorization_and_actual_batch(self):
        with self.assertRaises(CoreError):self.op('runtime.configure',{'enabled':True,'editor_mode':'model'},allow_model=False)
        self.source('first');self.source('second');run=self.op('runtime.run',{'form_all':True,'editor_mode':'model','continue_worker':True,'max_steps':3});self.assertTrue(any(step['action']=='worker' for step in run['steps']));self.assertEqual([role for role,_ in self.provider.calls],['editor','worker']);self.assertEqual(len(self.op('cut.get')['view']['cbs'][0]['source_ids']),2)
    def test_historical_peer_executes_edits_and_returns_result_without_blocking_sender(self):
        source=self.source('shared experience');job=self.op('edit.program',{'source_id':source})['job_id'];self.publish(job);cp=self.op('chain.checkpoint')['checkpoint_id'];socket=self.op('socket.create',{'checkpoint_id':cp,'spawn_acl':['host'],'message_acl':['host'],'signal_acl':['host']})['socket_id'];request=self.op('collaboration.dispatch',{'socket_ids':[socket,socket],'request_id':'explore','question':'Find alternatives','lifecycle':'V'});self.assertEqual(len(request['branches']),2)
        self.source('sender independently continues');ran=self.op('collaboration.run',{'request_id':'explore','editor_mode':'model','max_steps':8});self.assertEqual(ran['status'],'completed');self.assertEqual(len(ran['branches']),2)
        for branch in ran['branches']:
            state=self.op('chain.get',chain=branch['chain_id']);self.assertGreater(len(state['cbs']),1);self.assertEqual(state['creators'],[])
        sender=self.op('chain.get');self.assertEqual(len(sender['inbox']),2);self.assertTrue(sender['pending']);self.op('runtime.run',{'editor_mode':'program','continue_worker':False});self.assertIn('Local fixture branch result',json.dumps(self.op('provider.preview')['request']))
    def test_worker_socket_tool_write_gate_and_board_claim_conflict(self):
        self.op('board.claim',{'task_id':'work','expected_revision':0,'description':'investigate'});branch=self.op('chain.create')['chain_id']
        with self.assertRaises(CoreError):self.op('board.claim',{'board_id':self.op('board.get')['board_id'],'task_id':'work','expected_revision':1},chain=branch)
        with self.assertRaises(CoreError):self.op('tools.invoke',{'name':'sic_decision_update','arguments':{'expected_revision':0,'events':[]}},allow_write=False)
        tools=self.op('tools.resolve')['tools'];self.assertIn('sic_socket_dispatch',[tool['name'] for tool in tools])
    def test_signal_pauses_inflight_editor_preserving_candidate_for_cas_and_resume(self):
        source=self.source('prefix');job=self.op('edit.program',{'source_id':source})['job_id'];self.publish(job);cp=self.op('chain.checkpoint')['checkpoint_id'];socket=self.op('socket.create',{'checkpoint_id':cp,'signal_acl':['host']})['socket_id'];entered=threading.Event();finish=threading.Event()
        class SlowEditor(MechanismProvider):
            def call(self,role,request):
                if role=='editor':entered.set();finish.wait(3)
                return super().call(role,request)
        self.core.provider=SlowEditor();self.source('inflight editor input');results=[]
        thread=threading.Thread(target=lambda:results.append(self.op('runtime.run',{'editor_mode':'model','continue_worker':True})));thread.start();self.assertTrue(entered.wait(2));self.op('socket.signal',{'socket_id':socket,'signal':'wait'});finish.set();thread.join(3);state=self.op('chain.get');self.assertEqual(len(state['cbs']),1);ready=next(j for j in state['jobs'].values() if j['status']=='ready');self.op('edit.candidate.update',{'job_id':ready['job_id'],'expected_revision':0,'ops':[{'op':'REPLACE','target':'u1','content':'reviewed while paused'}],'pvi':{}})
        with self.assertRaises(CoreError):self.publish(ready['job_id'])
        self.op('socket.signal',{'socket_id':socket,'signal':'wake'});self.op('runtime.run',{'editor_mode':'model','continue_worker':True});self.assertIn('reviewed while paused',json.dumps(self.core.provider.calls[-1][1]))
    def test_automatic_runtime_two_routes_dispatch_models_concurrently(self):
        self.core.provider=MechanismProvider(threading.Barrier(2));other=self.op('chain.create')['chain_id']
        for chain in (self.chain,other):self.op('runtime.configure',{'enabled':True,'editor_mode':'model','worker_mode':'host','seal_chars':1},chain=chain)
        self.source('route one');self.source('route two',other);deadline=time.monotonic()+4
        while time.monotonic()<deadline:
            if all(self.op('chain.get',chain=chain)['cbs'] for chain in (self.chain,other)):break
            time.sleep(.01)
        self.assertTrue(all(self.op('chain.get',chain=chain)['cbs'] for chain in (self.chain,other)));self.core.runtime.drain()
    def test_host_tool_execution_overlaps_real_editor_dispatch_and_legal_continuation(self):
        editor_during_tool=threading.Event();tool_running=threading.Event()
        class ToolProvider(MechanismProvider):
            def __init__(self):super().__init__();self.worker_calls=0
            def call(self,role,request):
                if role=='editor' and tool_running.is_set():editor_during_tool.set()
                result=super().call(role,request)
                if role=='worker':
                    self.worker_calls+=1
                    if self.worker_calls==1:result['response']['choices'][0]={'finish_reason':'tool_calls','message':{'role':'assistant','content':'Execute source lookup','tool_calls':[{'id':'host-call','type':'function','function':{'name':'fixture_lookup','arguments':'{"key":"value"}'}}]}}
                return result
        self.core.provider=ToolProvider();tools=self.op('chain.get')['tools'];tools.append({'name':'fixture_lookup','descriptor':'Owned local fixture','schema':{'type':'function','function':{'name':'fixture_lookup','parameters':{'type':'object'}}},'discover_acl':['host'],'expand_acl':['host'],'execute_acl':['host'],'expanded':True});self.op('tools.configure',{'tools':tools})
        def execute(context,arguments):
            tool_running.set();self.assertTrue(editor_during_tool.wait(3));tool_running.clear();return {'actual_result':arguments['key']}
        self.core.runtime.register_tool('fixture_lookup',execute);self.source('do the local tool task');result=self.op('runtime.run',{'editor_mode':'model','continue_worker':True});worker=result['steps'][-1]['result'];self.assertIn('response',worker);self.assertEqual(self.core.provider.worker_calls,2);self.assertTrue(editor_during_tool.is_set());self.assertEqual(self.op('chain.get')['open_tools'],{});self.assertGreater(self.op('ledger.export')['summary']['timing']['editor_tool_overlap_seconds'],0)
    def test_worker_reads_original_model_ops_and_manual_revision_via_bounded_proposal_tool(self):
        source=self.source('original');job=self.op('edit.prepare',{'source_id':source})['job_id'];self.op('edit.run',{'job_id':job,'editor':'editor-1'});self.op('edit.candidate.update',{'job_id':job,'expected_revision':0,'ops':[{'op':'REPLACE','target':'u1','content':'manual replacement'}],'pvi':{}});cut=self.publish(job);text='';offset=0
        while True:
            returned=self.op('tools.invoke',{'name':'sic_memory_page','arguments':{'kind':'proposal','object_id':cut['cb_id'],'offset':offset,'limit':128}})['result'];self.assertNotIn('deferred_result_id',returned);text+=returned['value']
            if returned['next_offset'] is None:break
            offset=returned['next_offset']
        proposal=json.loads(text);self.assertEqual(proposal['original_responses'][0]['ops'][0]['content'],'original');self.assertEqual(proposal['candidate_revisions'][0]['ops'][0]['content'],'manual replacement');self.assertIn('proposal',json.dumps(self.op('provider.preview')['request']))
    def test_large_model_tool_results_are_bounded_and_full_ops_page_reconstructs(self):
        source=self.source('large-source-'+'x'*18000);job=self.op('edit.program',{'source_id':source})['job_id'];cut=self.publish(job);read=self.op('tools.invoke',{'name':'sic_source_read','arguments':{'source_id':source,'limit':100000}})['result'];self.assertLess(len(json.dumps(read)),2048);self.assertTrue(read['next_offset']);directory=self.op('tools.invoke',{'name':'sic_operation_directory'})['result'];self.assertLess(len(json.dumps(directory)),2048)
        collected='';offset=0
        while True:
            value=self.op('tools.invoke',{'name':'sic_memory_page','arguments':{'kind':'ops','object_id':cut['cb_id'],'offset':offset,'limit':200}})['result'];collected+=value['value']
            if value['next_offset'] is None:break
            offset=value['next_offset']
        self.assertEqual(json.loads(collected),self.core.store.get(cut['cb_id'],'cb')['ops'])
    def test_failed_collaboration_is_terminal_until_explicit_new_attempt_and_cancel_is_visible(self):
        source=self.source('history');job=self.op('edit.program',{'source_id':source})['job_id'];self.publish(job);cp=self.op('chain.checkpoint')['checkpoint_id'];socket=self.op('socket.create',{'checkpoint_id':cp,'spawn_acl':['host']})['socket_id'];self.op('collaboration.dispatch',{'socket_ids':[socket],'request_id':'failure-case','question':'explore'})
        class Failed(MechanismProvider):
            def call(self,role,request):self.calls.append((role,request));raise RuntimeError('owned failure')
        failed=Failed();self.core.provider=failed;result=self.op('collaboration.run',{'request_id':'failure-case'});self.assertEqual(result['status'],'failed');count=len(failed.calls);self.op('collaboration.run',{'request_id':'failure-case'});self.assertEqual(len(failed.calls),count)
        self.core.provider=self.provider;recovered=self.op('collaboration.run',{'request_id':'failure-case','retry_failed':True,'acknowledge_possible_duplicate':True});self.assertEqual(recovered['status'],'completed')
        self.op('collaboration.dispatch',{'socket_ids':[socket],'request_id':'cancel-case','question':'cancel before work'});self.op('runtime.recover',{'action':'cancel_collaboration','request_id':'cancel-case','confirm':True});cancelled=self.op('collaboration.run',{'request_id':'cancel-case'});self.assertEqual(cancelled['status'],'cancelled')
    def test_inherited_large_instant_manifest_points_to_authorized_branch_cut(self):
        source=self.source();job=self.op('edit.program',{'source_id':source,'pvi':{'I':{'large-view':'state '*3000}}})['job_id'];published=self.publish(job);cp=self.op('chain.checkpoint')['checkpoint_id'];branch=self.op('chain.fork',{'checkpoint_id':cp})['chain_id'];self.op('config.update',{'memory_chars':1024},chain=branch);projection=self.op('provider.preview',chain=branch);current=self.op('cut.get',chain=branch);self.assertNotEqual(current['cut_id'],published['cut_id']);self.assertEqual(current['cut']['chain_id'],branch)
        view=self.op('memory.page',{'kind':'view','object_id':current['cut_id'],'limit':128},chain=branch);self.assertTrue(view['value']);self.assertTrue(view['next_offset']);self.assertIn(current['cut_id'],json.dumps(projection['request']))
    def test_source_idempotent_receipt_does_not_duplicate_material_or_event(self):
        payload={'idempotency_key':'host-input','request':{'messages':[{'role':'user','content':'one accepted input'}]}};first=self.op('source.ingest',payload);before=self.op('chain.get');again=self.op('source.ingest',payload);self.assertEqual(first,again);after=self.op('chain.get');self.assertEqual(before['pending'],after['pending']);self.assertEqual(before['events'],after['events'])
        with self.assertRaises(CoreError):self.op('source.ingest',{**payload,'request':{'messages':[{'role':'user','content':'changed input'}]}})
    def test_editor_receives_requested_full_operation_page_before_single_round(self):
        source=self.source('historical evidence');old_job=self.op('edit.program',{'source_id':source})['job_id'];old=self.publish(old_job);new_source=self.source('new work');job=self.op('edit.prepare',{'source_id':new_source,'readonly_reads':[{'kind':'ops','object_id':old['cb_id'],'offset':0,'limit':2048}]})['job_id'];self.op('edit.run',{'job_id':job,'editor':'editor-1'});request=json.loads(self.provider.calls[-1][1]['messages'][-1]['content']);read=request['readonly_archive_reads'][0];self.assertEqual(json.loads(read['value']),self.core.store.get(old['cb_id'],'cb')['ops']);self.assertEqual(len(self.provider.calls),1)
    def test_measured_serial_wait_and_explicit_interleave_intervals_are_separate(self):
        self.source('work requiring serialized edit');self.op('runtime.run',{'editor_mode':'model','continue_worker':True});ledger=self.op('ledger.export');self.assertGreater(ledger['summary']['timing']['serial_wait_seconds'],0);now=self.core.now();self.op('harness.event',{'type':'interval','kind':'interleave','start':now-0.01,'end':now,'clock':'core-epoch-perf-counter'});timing=self.op('ledger.export')['summary']['timing'];self.assertGreater(timing['interleave_interval_seconds'],0)
    def test_runtime_replayed_worker_reports_no_new_model_call(self):
        self.source('one work item');first=self.op('runtime.run',{'editor_mode':'program','continue_worker':True,'worker_request_id':'one-worker'});self.assertTrue(first['provider_called']);count=len(self.provider.calls);again=self.op('runtime.run',{'editor_mode':'program','continue_worker':True,'worker_request_id':'one-worker'});self.assertFalse(again['provider_called']);self.assertEqual(again['provider_call_count'],0);self.assertEqual(len(self.provider.calls),count)
    def test_requeued_failed_editor_does_not_silently_start_a_new_model_task(self):
        class Failing(MechanismProvider):
            def call(self,role,request):self.calls.append((role,request));raise RuntimeError('owned editor failure')
        self.core.provider=Failing();self.source('must not silently retry this scope')
        with self.assertRaises(CoreError):self.op('runtime.run',{'editor_mode':'model'})
        self.assertEqual(len(self.core.provider.calls),1)
        with self.assertRaises(CoreError) as error:self.op('runtime.run',{'editor_mode':'model','automatic_event':'duplicate-queued-event'})
        self.assertEqual(error.exception.code,'editor_recovery_required');self.assertEqual(len(self.core.provider.calls),1);failed=next(j for j in self.op('chain.get')['jobs'].values() if j['status']=='failed');self.op('runtime.recover',{'action':'abandon_editor','job_id':failed['job_id'],'confirm':True});self.core.provider=self.provider;self.op('runtime.run',{'editor_mode':'model'});self.assertTrue(self.op('chain.get')['cbs'])
    def test_autonomous_worker_and_branch_budgets_have_explicit_recovery(self):
        self.op('runtime.configure',{'enabled':False,'editor_mode':'program','worker_mode':'host','worker_turns':1,'branch_budget':1});self.source('bounded task');self.op('runtime.run',{'editor_mode':'program','continue_worker':True,'worker_request_id':'first'});count=len(self.provider.calls);stopped=self.op('runtime.run',{'editor_mode':'program','continue_worker':True,'worker_request_id':'second'});self.assertEqual(stopped['steps'][-1]['result']['status'],'budget-exhausted');self.assertEqual(len(self.provider.calls),count);self.source('explicit new user task');resumed=self.op('runtime.run',{'editor_mode':'program','continue_worker':True,'worker_request_id':'third'});self.assertIn('response',resumed['steps'][-1]['result'])
        cp=self.op('chain.checkpoint')['checkpoint_id'];socket=self.op('socket.create',{'checkpoint_id':cp,'spawn_acl':['host']})['socket_id']
        with self.assertRaises(CoreError):self.op('collaboration.dispatch',{'socket_ids':[socket,socket],'request_id':'too-many','question':'bounded fork count'})
    def test_worker_socket_dispatch_autonomously_executes_peer_and_consumes_return(self):
        source=self.source('old shared history');job=self.op('edit.program',{'source_id':source})['job_id'];self.publish(job);cp=self.op('chain.checkpoint')['checkpoint_id'];socket=self.op('socket.create',{'checkpoint_id':cp,'spawn_acl':['host']})['socket_id']
        class DelegatingProvider(MechanismProvider):
            def __init__(self):super().__init__();self.delegated=False
            def call(self,role,request):
                result=super().call(role,request);text=json.dumps(request)
                if role=='worker':
                    if 'delegate-task-marker' in text and not self.delegated:
                        self.delegated=True;result['response']['choices'][0]={'finish_reason':'tool_calls','message':{'role':'assistant','content':'Delegate historical exploration','tool_calls':[{'id':'delegate-call','type':'function','function':{'name':'sic_socket_dispatch','arguments':json.dumps({'socket_ids':[socket],'request_id':'auto-peer','question':'explore beta','lifecycle':'V'})}}]}}
                    else:result['response']['choices'][0]['message']['content']='BRANCH_RETURN_MARKER' if 'explore beta' in text and 'delegate-task-marker' not in text else 'PARENT_ADVANCE_MARKER'
                return result
        self.core.provider=DelegatingProvider();self.source('delegate-task-marker');self.op('runtime.configure',{'enabled':True,'editor_mode':'model','worker_mode':'automatic','seal_chars':1,'worker_turns':16});deadline=time.monotonic()+5;done=False
        while time.monotonic()<deadline:
            state=self.op('chain.get');request=state.get('collaborations',{}).get('auto-peer',{})
            if request.get('status')=='completed' and any(role=='worker' and 'delegate-task-marker' in json.dumps(body) and 'BRANCH_RETURN_MARKER' in json.dumps(body) for role,body in self.core.provider.calls):done=True;break
            time.sleep(.01)
        self.assertTrue(done);self.assertTrue(request['branches'][0].get('consumed_cut_id'))
    def test_ledger_resource_unknown_and_explicit_measurements(self):
        self.source();self.op('provider.continue',{'idempotency_key':'ledger'});self.op('resource.record',{'record_id':'cpu','resource':'program','quantity':.01,'unit':'cpu-second','measurement_method':'thread_time'});export=self.op('ledger.export');self.assertEqual(len(export['calls']),1);self.assertEqual(export['calls'][0]['source_refs']['chain_id'],self.chain);self.assertFalse(export['summary']['complete_cost']);self.assertTrue(export['resource_records'])

if __name__=='__main__':unittest.main()
