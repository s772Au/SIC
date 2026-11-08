import concurrent.futures,copy,json,os,sqlite3,tempfile,threading,unittest,urllib.request,urllib.error
from pathlib import Path
from sic_core import Core,CoreError
from sic_core.server import make_server
from sic_core.adapters import decode,encode

class FixedProvider:
    def __init__(self,content=None,fail=False):self.calls=[];self.content=content or json.dumps({'ops':[],'pvi':{}});self.fail=fail
    def capabilities(self):return {'test_double':True,'live_provider_verified':False}
    def call(self,role,request):
        self.calls.append((role,copy.deepcopy(request)))
        if self.fail:raise RuntimeError('private provider detail must not escape')
        return {'response':{'model':'fixed-return','choices':[{'finish_reason':'stop','message':{'role':'assistant','content':self.content if role=='editor' else 'Worker continuation'}}]},'requested_model':'fixed-request','returned_model':'fixed-return','expected_returned_models':['fixed-return'],'route':'test-route','usage':{'prompt_tokens':12,'completion_tokens':5,'prompt_tokens_details':{'cached_tokens':4},'completion_tokens_details':{'reasoning_tokens':2}}}

class CoreTests(unittest.TestCase):
    def setUp(self):self.core=Core();self.actor='host';self.chain=self.call('chain.create',{})['chain_id']
    def tearDown(self):self.core.store.db.close()
    def call(self,op,p=None,chain=None,**flags):
        return self.core.operate(op,{'actor':self.actor,'chain_id':chain or getattr(self,'chain',None),'allow_write':True,'allow_model':False,**flags},p or {})['result']
    def ingest(self,text='source',**kw):return self.call('source.ingest',{'format':'openai-chat','request':{'messages':[{'role':'user','content':text}],**kw}})['source_id']
    def ready(self,text='source',pvi=None):
        source=self.ingest(text);job=self.call('edit.program',{'source_id':source,'pvi':pvi or {}})['job_id'];return source,job
    def publish(self,job,key='publish',cut=0):return self.call('cut.publish',{'job_id':job,'expected_cut':cut,'idempotency_key':key})
    def test_decode_separates_text_json_from_control(self):
        request={'model':'not-editor-target','temperature':0.3,'messages':[{'role':'system','content':'protected'},{'role':'assistant','content':None,'tool_calls':[{'id':'t1','type':'function','function':{'name':'lookup','arguments':'{"q":1}'}}]},{'role':'tool','tool_call_id':'t1','content':'{"task_data":true}'}]}
        source=decode('openai-chat',request);self.assertEqual([x['text'] for x in source['units']],['{"task_data":true}']);self.assertEqual(source['sidecar'],request);self.assertEqual(source['open_calls'],[])
        with self.assertRaises(ValueError):decode('openai-chat',{'messages':request['messages'][2:]})
        with self.assertRaises(ValueError):decode('openai-chat',{'messages':[{'role':'user','content':[{'type':'image_url','image_url':{}}]}]})
    def test_protected_mismatch_and_unsupported_controls(self):
        self.call('source.ingest',{'request':{'messages':[{'role':'system','content':'fixed'},{'role':'user','content':'x'}]}})
        with self.assertRaises(CoreError):self.call('source.ingest',{'request':{'messages':[{'role':'system','content':'changed'}]}})
        with self.assertRaises(CoreError):self.call('source.ingest',{'request':{'messages':[],'headers':{'Authorization':'placeholder'}}})
    def test_program_active_rules_keep_source_immutable(self):
        source=self.call('source.ingest',{'request':{'messages':[{'role':'user','content':'same'},{'role':'user','content':'same'},{'role':'user','content':''}]}})['source_id']
        job=self.call('edit.program',{'source_id':source,'rules':['deduplicate_exact','drop_empty']})['job_id'];cut=self.publish(job);cb=self.call('cut.get')['view']['cbs'][0]
        self.assertEqual(cb['body'],'same');self.assertEqual([x['op'] for x in cb['ops']],['DELETE','DELETE','KEEP']);self.assertEqual(len(self.call('source.read',{'source_id':source})['source']['units']),3)
        with self.assertRaises(sqlite3.IntegrityError):self.core.store.db.execute("UPDATE objects SET data='{}' WHERE id=?",(cut['cb_id'],))
        with self.assertRaises(sqlite3.IntegrityError):self.core.store.db.execute('DELETE FROM objects WHERE id=?',(source,))
    def test_multi_editor_scope_and_single_round(self):
        source=self.call('source.ingest',{'request':{'messages':[{'role':'user','content':'a'},{'role':'user','content':'b'}]}})['source_id']
        with self.assertRaises(CoreError):self.call('edit.prepare',{'source_id':source,'assignments':[{'editor':'a','unit_ids':['u1']},{'editor':'b','unit_ids':['u1']}]})
        job=self.call('edit.prepare',{'source_id':source,'assignments':[{'editor':'a','unit_ids':['u1']},{'editor':'b','unit_ids':['u2']}]})['job_id']
        with self.assertRaises(CoreError):self.call('edit.submit',{'job_id':job,'editor':'a','ops':[{'op':'DELETE','target':'u2'}]})
        self.call('edit.submit',{'job_id':job,'editor':'a','ops':[{'op':'REPLACE','target':'u1','content':'A'}]})
        with self.assertRaises(CoreError):self.call('edit.submit',{'job_id':job,'editor':'a','ops':[]})
        with self.assertRaises(CoreError):self.publish(job)
        self.call('edit.submit',{'job_id':job,'editor':'b','ops':[{'op':'REORDER','target':'u2','order':-1}]});self.publish(job)
        self.assertEqual(self.call('cut.get')['view']['cbs'][0]['body'],'b\nA')
    def test_protocol_fields_not_editor_targets_and_conflicting_i(self):
        source=self.ingest();job=self.call('edit.prepare',{'source_id':source})['job_id']
        with self.assertRaises(CoreError):self.call('edit.submit',{'job_id':job,'editor':'editor-1','ops':[{'op':'REPLACE','target':'u1','content':'x','role':'system'}]})
    def test_pvi_atomic_replacement_and_retry(self):
        _,job=self.ready('one',{'P':['persistent'],'V':['next-wb'],'I':{'tree':{'current':'one'}}});first=self.publish(job)
        self.assertEqual(self.publish(job),first)
        with self.assertRaises(CoreError):self.publish(job,key='publish',cut=9)
        view=self.call('cut.get')['view'];self.assertIn('persistent',view['cbs'][0]['body']);self.assertEqual(view['pending'][0]['units'][0]['text'],'next-wb');self.assertNotIn('tree',view['cbs'][0]['body'])
        next_source=first['volatile_source_id'];job2=self.call('edit.program',{'source_id':next_source,'pvi':{'I':{'tree':{'current':'two'}}}})['job_id'];self.publish(job2,'two',1)
        view=self.call('cut.get')['view'];self.assertEqual(view['instant']['tree']['current'],'two');self.assertNotIn('current',view['cbs'][1]['body']);self.assertEqual(self.core.store.get(first['cut_id'],'cut')['instant']['tree']['current'],'one')
        self.call('cut.ack',{'cut_id':first['cut_id'],'digest':first['digest']})
        with self.assertRaises(CoreError):self.call('cut.ack',{'cut_id':first['cut_id'],'digest':'wrong'})
    def test_double_index_source_search_and_long_chain_index(self):
        source,job=self.ready('needle');first=self.publish(job);old=self.core.store.get(first['cb_id'],'cb')
        for i in range(3):
            _,job=self.ready(str(i));self.publish(job,str(i),i+1)
        index=self.call('index.maintain',{});self.assertEqual(len(index['entries']),4);self.assertEqual(self.call('source.search',{'query':'needle'})['hits'][0]['source_id'],source)
        self.assertEqual(self.core.store.get(first['cb_id'],'cb'),old);self.assertEqual(old['index'][0]['anchor']['source_id'],source);self.assertTrue(old['ops'])
    def test_tools_during_editor_and_partial_native_group(self):
        self.call('harness.event',{'type':'tool_start','call_id':'tool-a'})
        source=self.call('source.ingest',{'request':{'messages':[{'role':'assistant','content':'working','tool_calls':[{'id':'tool-a','type':'function','function':{'name':'lookup','arguments':'{}'}}]}]}})['source_id']
        job=self.call('edit.program',{'source_id':source})['job_id'];self.assertEqual(self.call('harness.schedule',{'job_id':job})['decision'],'overlap')
        with self.assertRaises(CoreError):self.publish(job)
        with self.assertRaises(CoreError):self.call('provider.preview')
        part=self.call('source.ingest',{'request':{'messages':[{'role':'tool','tool_call_id':'tool-a','content':'result'}]}});self.assertEqual(part['source_id'],source)
        self.call('harness.event',{'type':'tool_end','call_id':'tool-a'});self.publish(job)
        cb=self.call('cut.get')['view']['cbs'][0];self.assertIn('result',cb['body']);self.assertEqual(len(cb['source_ids']),2);self.assertEqual(len(cb['protocol_events']),2)
        request=self.call('provider.preview')['request'];self.assertEqual(request['messages'][1]['role'],'user');self.assertIn('tool-a',request['messages'][1]['content'])
    def test_interleave_budget_not_bypassed_by_extra_jobs(self):
        source=self.ingest();job=self.call('edit.prepare',{'source_id':source})['job_id'];self.call('edit.prepare',{'source_id':source})
        one=self.call('harness.schedule',{'policy':'interleave','interleave_budget':1});two=self.call('harness.schedule',{'policy':'interleave','interleave_budget':1})
        self.assertTrue(one['worker_may_continue']);self.assertFalse(two['worker_may_continue']);self.assertEqual(two['interleaves'][source],1)
        with self.assertRaises(CoreError):self.call('harness.event',{'type':'worker_continue'})
    def test_fork_peers_and_multi_creator(self):
        _,job=self.ready();first=self.publish(job);cp=self.call('chain.checkpoint')['checkpoint_id'];a=self.call('chain.fork',{'checkpoint_id':cp});b=self.call('chain.fork',{'checkpoint_id':cp})
        self.assertNotEqual(a['chain_id'],b['chain_id']);self.assertEqual(a['relationship'],'peer-fork');self.assertEqual(self.call('chain.get',chain=a['chain_id'])['creators'],[])
        self.assertEqual(self.call('cut.get',chain=a['chain_id'])['view']['cbs'][0]['cb_id'],first['cb_id'])
        created=self.call('chain.create',{'creators':[a['chain_id'],b['chain_id']]});self.assertEqual(len(created['creators']),2);self.assertEqual(self.call('chain.get',chain=created['chain_id'])['cbs'],[])
    def test_socket_acl_signal_and_message_cut(self):
        _,job=self.ready();self.publish(job);cp=self.call('chain.checkpoint')['checkpoint_id'];sock=self.call('socket.create',{'checkpoint_id':cp,'message_acl':['peer'],'signal_acl':['host']})['socket_id']
        sender=self.call('chain.create',{})['chain_id']
        with self.assertRaises(CoreError):self.call('socket.message',{'socket_id':sock,'text':'x','lifecycle':'V','message_id':'a'},chain=sender)
        self.actor='peer';sender=self.call('chain.create',{})['chain_id'];self.call('socket.message',{'socket_id':sock,'text':'hint','lifecycle':'V','message_id':'a'},chain=sender)
        with self.assertRaises(CoreError):self.call('socket.signal',{'socket_id':sock,'signal':'stop'},chain=sender)
        self.actor='host';self.call('socket.signal',{'socket_id':sock,'signal':'wait'});self.assertEqual(self.call('chain.get')['run_state'],'waiting');self.call('socket.signal',{'socket_id':sock,'signal':'wake'})
        _,job=self.ready('next');cut=self.publish(job,'next',1);self.assertIsNotNone(cut['volatile_source_id']);self.assertIn('hint',self.call('provider.preview')['request']['messages'][-1]['content'])
    def test_board_cas_tools_three_permissions(self):
        self.call('board.update',{'expected_revision':0,'value':{'task':'now'}})
        with self.assertRaises(CoreError):self.call('board.update',{'expected_revision':0,'value':{}})
        tool={'name':'lookup','descriptor':'Find retained sources','schema':{'type':'function','function':{'name':'lookup','parameters':{'type':'object'}}},'discover_acl':['host'],'expand_acl':[],'execute_acl':[],'expanded':True}
        self.call('tools.configure',{'tools':[tool]});self.assertNotIn('schema',self.call('tools.resolve')['tools'][0])
        with self.assertRaises(CoreError):self.call('tools.authorize',{'name':'lookup'})
        tool['expand_acl']=['host'];tool['execute_acl']=['host'];self.call('tools.configure',{'tools':[tool]});self.assertTrue(self.call('tools.authorize',{'name':'lookup'})['authorized']);self.assertIn('schema',self.call('tools.resolve')['tools'][0])
    def test_controls_roundtrip_and_fixed_config(self):
        self.ingest(temperature=0.2,max_tokens=128);preview=self.call('provider.preview')['request'];self.assertEqual(preview['temperature'],0.2);self.assertEqual(preview['max_tokens'],128)
        self.call('config.update',{'editor_prompt':'specific','program_rules':['drop_empty'],'interleave_budget':2});self.assertEqual(self.call('config.get')['configuration']['interleave_budget'],2)
        with self.assertRaises(CoreError):self.call('config.update',{'api_key':'not-accepted'})
        with self.assertRaises(CoreError):self.call('config.update',{'worker_profile':'unknown'})
    def test_permission_dual_gate_and_actor_chain_isolation(self):
        with self.assertRaises(CoreError):self.call('source.ingest',{'request':{'messages':[]}},allow_write=False)
        with self.assertRaises(CoreError):self.call('provider.continue',{'idempotency_key':'x'})
        self.actor='other'
        with self.assertRaises(CoreError):self.call('chain.get')
    def test_model_single_call_and_usage_disjoint_categories(self):
        provider=FixedProvider();self.core.provider=provider;source=self.ingest();job=self.call('edit.prepare',{'source_id':source})['job_id']
        self.call('edit.run',{'job_id':job,'editor':'editor-1'},allow_model=True)
        self.assertEqual(len(provider.calls),1);text=provider.calls[0][1]['messages'][1]['content'];self.assertNotIn('sidecar',text);self.assertIn('editable_text_units',text)
        with self.assertRaises(CoreError):self.call('edit.run',{'job_id':job,'editor':'editor-1'},allow_model=True)
        self.publish(job);first=self.call('provider.continue',{'idempotency_key':'fixed'},allow_model=True);self.assertEqual(first,self.call('provider.continue',{'idempotency_key':'fixed'},allow_model=True));self.assertEqual(len(provider.calls),2)
        report=self.call('usage.report');self.assertEqual(report['category_totals'],{'input_uncached':16,'cache_read':8,'output_nonreasoning':6,'output_reasoning':4});self.assertFalse(report['complete_cost'])
    def test_malformed_editor_retains_billing_and_cannot_retry(self):
        self.core.provider=FixedProvider(content='not json');source=self.ingest();job=self.call('edit.prepare',{'source_id':source})['job_id']
        with self.assertRaises(CoreError):self.call('edit.run',{'job_id':job,'editor':'editor-1'},allow_model=True)
        self.assertEqual(len(self.call('usage.report')['calls']),1)
        with self.assertRaises(CoreError):self.call('edit.run',{'job_id':job,'editor':'editor-1'},allow_model=True)
    def test_provider_failure_no_automatic_duplicate(self):
        self.core.provider=FixedProvider(fail=True);self.ingest()
        with self.assertRaises(CoreError):self.call('provider.continue',{'idempotency_key':'once'},allow_model=True)
        receipt=self.call('provider.continue',{'idempotency_key':'once'},allow_model=True);self.assertFalse(receipt['automatic_retry']);self.assertEqual(len(self.core.provider.calls),1)
    def test_two_connections_competing_publish_exactly_one(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=str(Path(tmp)/'db');self.core.store.db.close();self.core=Core(path);self.chain=self.call('chain.create')['chain_id'];_,job=self.ready();other=Core(path)
            def attempt(core,key):
                try:return core.operate('cut.publish',{'actor':'host','chain_id':self.chain,'allow_write':True},{'job_id':job,'expected_cut':0,'idempotency_key':key})['ok']
                except CoreError:return False
            with concurrent.futures.ThreadPoolExecutor() as pool:results=list(pool.map(lambda pair:attempt(*pair),[(self.core,'a'),(other,'b')]))
            self.assertEqual(sum(results),1);self.assertEqual(self.call('cut.get')['sequence'],1);self.core.store.db.close();other.store.db.close()

    def test_truncated_editor_rejected_even_valid_json(self):
        class Truncated(FixedProvider):
            def call(self,*args):
                result=super().call(*args);result['response']['choices'][0]['finish_reason']='length';return result
        self.core.provider=Truncated();source=self.ingest();job=self.call('edit.prepare',{'source_id':source})['job_id']
        with self.assertRaises(CoreError):self.call('edit.run',{'job_id':job,'editor':'editor-1'},allow_model=True)
        with self.assertRaises(CoreError):self.publish(job)
        self.assertEqual(len(self.call('usage.report')['calls']),1)
    def test_unknown_failed_usage_never_zero_complete(self):
        self.core.provider=FixedProvider(fail=True);self.ingest()
        with self.assertRaises(CoreError):self.call('provider.continue',{'idempotency_key':'failed'},allow_model=True)
        report=self.call('usage.report');self.assertFalse(report['complete_cost']);self.assertEqual(report['calls'][0]['usage_status'],'unknown')
    def test_model_identity_mismatch_preserves_usage_not_content(self):
        class Wrong(FixedProvider):
            def call(self,*args):
                result=super().call(*args);result['returned_model']='unexpected-model';return result
        self.core.provider=Wrong();source=self.ingest();job=self.call('edit.prepare',{'source_id':source})['job_id']
        with self.assertRaises(CoreError) as error:self.call('edit.run',{'job_id':job,'editor':'editor-1'},allow_model=True)
        self.assertEqual(error.exception.code,'model_identity');self.assertEqual(self.call('cut.get')['sequence'],0);self.assertEqual(len(self.call('usage.report')['calls']),1)
    def test_failed_job_recovery_and_slow_response_terminal_race(self):
        started=threading.Event();finish=threading.Event()
        class Slow(FixedProvider):
            def call(self,*args):started.set();finish.wait(5);return super().call(*args)
        self.core.provider=Slow();source=self.ingest();job=self.call('edit.prepare',{'source_id':source})['job_id'];errors=[]
        def run():
            try:self.call('edit.run',{'job_id':job,'editor':'editor-1'},allow_model=True)
            except CoreError as error:errors.append(error)
        thread=threading.Thread(target=run);thread.start();self.assertTrue(started.wait(3));replacement=self.call('edit.program',{'source_id':source})['job_id'];self.publish(replacement);finish.set();thread.join(5)
        self.assertEqual(self.call('chain.get')['jobs'][job]['status'],'superseded');self.assertTrue(self.call('harness.schedule')['worker_may_continue']);self.assertEqual(len(errors),1)
    def test_checkpoint_freezes_v_tools_config_not_latest_head(self):
        self.call('config.update',{'editor_prompt':'old prompt','interleave_budget':3});_,job=self.ready('cb',{'V':['checkpoint WB']});self.publish(job);cp=self.call('chain.checkpoint')['checkpoint_id'];self.call('config.update',{'editor_prompt':'new prompt'});self.call('tools.configure',{'tools':[]});fork=self.call('chain.fork',{'checkpoint_id':cp})['chain_id'];state=self.call('chain.get',chain=fork)
        self.assertEqual(state['config']['editor_prompt'],'old prompt');self.assertTrue(state['tools']);self.assertIn('checkpoint WB',json.dumps(self.call('provider.preview',chain=fork)['request']))
    def test_socket_spawn_separate_history_acl_and_first_message_both_lifetimes(self):
        _,job=self.ready('history',{'V':['old V']});self.publish(job);cp=self.call('chain.checkpoint')['checkpoint_id'];sock=self.call('socket.create',{'checkpoint_id':cp,'message_acl':['peer'],'spawn_acl':['host']})['socket_id']
        for lifecycle in ('P','V'):
            branch=self.call('socket.spawn',{'socket_id':sock,'text':'FIRST QUESTION '+lifecycle,'lifecycle':lifecycle})['chain_id'];view=self.call('cut.get',chain=branch)['view'];self.assertIn('FIRST QUESTION '+lifecycle,json.dumps(self.call('provider.preview',chain=branch)['request']))
            if lifecycle=='P':self.assertIn('FIRST QUESTION P',view['cbs'][-1]['body'])
            else:self.assertNotIn('FIRST QUESTION V',view['cbs'][-1]['body'])
        self.actor='peer';peer=self.call('chain.create')['chain_id']
        with self.assertRaises(CoreError):self.call('socket.spawn',{'socket_id':sock},chain=peer)
        self.actor='host'
        with self.assertRaises(CoreError):self.call('socket.create',{'checkpoint_id':cp,'spawn_acl':'host'})
    def test_revocation_blocks_old_snapshots_search_fork_tools_and_projection(self):
        source,job=self.ready('private withdrawn');self.publish(job);cp=self.call('chain.checkpoint')['checkpoint_id'];sock=self.call('socket.create',{'checkpoint_id':cp,'spawn_acl':['host']})['socket_id'];unaffected=self.call('chain.create')['chain_id']
        with self.assertRaises(CoreError):self.call('security.revoke',{'source_ids':[source],'confirm_revoke':True})
        self.core.config['revocation_actors']=['host'];self.call('security.revoke',{'source_ids':[source],'confirm_revoke':True})
        for op,payload in [('source.read',{'source_id':source}),('source.search',{'query':'private'}),('chain.get',{}),('cut.get',{}),('provider.preview',{}),('chain.fork',{'checkpoint_id':cp}),('edit.validate',{'job_id':job}),('tools.invoke',{'name':'sic_operation_directory'})]:
            with self.subTest(op=op),self.assertRaises(CoreError):self.call(op,payload)
        with self.assertRaises(CoreError):self.call('socket.spawn',{'socket_id':sock},chain=unaffected)
        self.assertTrue(self.core.store.revoked(source))
    def test_post_snapshot_program_keep_precise_index(self):
        source=self.call('source.ingest',{'request':{'messages':[{'role':'assistant','content':'thought','tool_calls':[{'id':'t','type':'function','function':{'name':'f','arguments':'{}'}}]}]}})['source_id'];job=self.call('edit.program',{'source_id':source})['job_id'];self.call('source.ingest',{'request':{'messages':[{'role':'tool','tool_call_id':'t','content':'late'}]}});self.publish(job);cb=self.call('cut.get')['view']['cbs'][0]
        self.assertTrue(cb['program_kept_after_snapshot'])
        for entry in cb['index']:
            for index in entry['operation_indexes']:
                operation=cb['ops'][index];self.assertTrue(operation.get('target')==entry['unit_id'] or entry['unit_id'] in operation.get('targets',[]))
    def test_named_add_anchor_and_unknown_anchor_rejection(self):
        source=self.ingest('original');job=self.call('edit.prepare',{'source_id':source})['job_id'];self.call('edit.submit',{'job_id':job,'editor':'editor-1','ops':[{'op':'ADD','content':'before','anchor':{'target':'u1','position':'before'}}]});self.publish(job);self.assertEqual(self.call('cut.get')['view']['cbs'][0]['body'],'before\noriginal')
    def test_native_text_tool_roundtrips_three_formats(self):
        fixtures=[('openai-responses',{'instructions':'protected','max_output_tokens':100,'input':[{'type':'function_call','call_id':'t','name':'read','arguments':'{}'},{'type':'function_call_output','call_id':'t','output':'result'}]}),('anthropic-messages',{'system':'protected','max_tokens':100,'messages':[{'role':'assistant','content':[{'type':'tool_use','id':'t','name':'read','input':{}}]},{'role':'user','content':[{'type':'tool_result','tool_use_id':'t','content':'result'}]}]})]
        for fmt,request in fixtures:
            with self.subTest(fmt=fmt):
                chain=self.call('chain.create')['chain_id'];source=self.call('source.ingest',{'format':fmt,'request':request},chain=chain)['source_id'];preview=self.call('provider.preview',{'format':fmt},chain=chain)['request'];self.assertIn('result',json.dumps(preview));self.assertEqual(self.call('source.read',{'source_id':source},chain=chain)['source']['sidecar']['native_request'],request)
    def test_memory_tool_execution_and_cache_frontier_evidence(self):
        source,job=self.ready('retrievable');self.publish(job);result=self.call('tools.invoke',{'name':'sic_source_read','arguments':{'source_id':source}});self.assertEqual(result['executed'],'SIC-program');frontier=self.call('provider.preview')['cache_frontier'];self.assertEqual(frontier['stable_cb_count'],1);self.assertEqual(frontier['physical_cache_hit'],'unknown')

    def test_responses_textblocks_and_parallel_calls_roundtrip(self):
        requests=[{'input':[{'role':'user','content':[{'type':'input_text','text':'task JSON {"x":1}'}]}]}, {'input':[{'type':'function_call','call_id':'a','name':'read','arguments':'{}'},{'type':'function_call','call_id':'b','name':'search','arguments':'{}'},{'type':'function_call_output','call_id':'a','output':'A'},{'type':'function_call_output','call_id':'b','output':'B'}]}]
        for request in requests:
            chain=self.call('chain.create')['chain_id'];self.call('source.ingest',{'format':'openai-responses','request':request},chain=chain);native=self.call('provider.preview',{'format':'openai-responses'},chain=chain)['request'];self.assertEqual(decode('openai-responses',native)['open_calls'],[])

    def test_shared_board_latest_at_branch_cut_and_cas(self):
        self.call('board.update',{'expected_revision':0,'value':{'task':'checkpoint'}});_,job=self.ready();self.publish(job);cp=self.call('chain.checkpoint')['checkpoint_id'];branch=self.call('chain.fork',{'checkpoint_id':cp})['chain_id'];original_board=self.call('board.get')['board_id'];self.assertEqual(self.call('board.get',chain=branch)['board_id'],original_board)
        self.call('board.update',{'expected_revision':1,'value':{'task':'latest from peer'}});source=self.call('source.ingest',{'request':{'messages':[{'role':'user','content':'branch work'}]}},chain=branch)['source_id'];job=self.call('edit.program',{'source_id':source},chain=branch)['job_id'];self.call('cut.publish',{'job_id':job,'expected_cut':1,'idempotency_key':'branch-board'},chain=branch);self.assertEqual(self.call('cut.get',chain=branch)['view']['instant']['board']['value']['task'],'latest from peer')
    def test_interleave_concurrency_one_budget(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.core.store.db.close();self.core=Core(str(Path(tmp)/'db'));self.chain=self.call('chain.create')['chain_id'];source=self.ingest();self.call('edit.prepare',{'source_id':source});other=Core(str(Path(tmp)/'db'))
            def schedule(core):return core.operate('harness.schedule',{'actor':'host','chain_id':self.chain,'allow_write':True},{'policy':'interleave','interleave_budget':100})['result']['worker_may_continue']
            with concurrent.futures.ThreadPoolExecutor() as pool:results=list(pool.map(schedule,[self.core,other]))
            self.assertEqual(sum(results),1);other.store.db.close();self.core.store.db.close()

    def test_schedule_receipt_and_grant_not_double_budget(self):
        self.core.provider=FixedProvider();source=self.ingest();self.call('edit.prepare',{'source_id':source});payload={'policy':'interleave','interleave_budget':1,'idempotency_key':'stable-step'};grant=self.call('harness.schedule',payload);self.assertEqual(grant,self.call('harness.schedule',payload));self.assertEqual(grant['interleaves'][source],1)
        self.call('provider.continue',{'idempotency_key':'model-step','grant_id':grant['grant_id']},allow_model=True);self.assertEqual(len(self.core.provider.calls),1)
        with self.assertRaises(CoreError):self.call('harness.event',{'type':'worker_continue','grant_id':grant['grant_id']})
    def test_safe_profile_read_does_not_expose_endpoint_or_secret_reference(self):
        self.core.config['profiles']={'known':{'model':'provider/fixed','api_base':'http://private-endpoint.invalid','api_key_env':'PRIVATE_ENV_REFERENCE','tokenizer_model':'gpt-4o','max_input_tokens':100,'max_output_tokens':10,'context_window':200}};result=self.call('config.get');self.assertEqual(result['safe_profiles']['known']['max_output_tokens'],10);self.assertNotIn('private-endpoint',json.dumps(result));self.assertNotIn('PRIVATE_ENV_REFERENCE',json.dumps(result))

class HTTPTests(unittest.TestCase):
    def test_authenticated_full_http_cycle_and_failures(self):
        core=Core();os.environ['SIC_CORE_TEST_TOKEN']='local-test-token';server=make_server(core,{'tokens':{'SIC_CORE_TEST_TOKEN':'host'}},port=0);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();base='http://127.0.0.1:'+str(server.server_port)
        def request(path,data=None,token=True):
            headers={'Content-Type':'application/json'}
            if token:headers['Authorization']='Bearer local-test-token'
            req=urllib.request.Request(base+path,data=None if data is None else json.dumps(data).encode(),headers=headers)
            with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req) as response:return json.load(response)
        chain=None
        def op(name,p=None,**context):return request('/api/v2/operations',{'operation':name,'context':{'actor':'host','chain_id':chain,'allow_write':True,'allow_model':False,**context},'payload':p or {}})['result']
        try:
            with self.assertRaises(urllib.error.HTTPError) as error:request('/api/v2/capabilities',token=False)
            self.assertEqual(error.exception.code,401);self.assertIn('chain.create',{x['operation'] for x in request('/api/v2/capabilities')['operations']})
            chain=op('chain.create')['chain_id'];source=op('source.ingest',{'request':{'messages':[{'role':'user','content':'actual HTTP source'}]},'format':'openai-chat'})['source_id'];job=op('edit.program',{'source_id':source})['job_id'];cut=op('cut.publish',{'job_id':job,'expected_cut':0,'idempotency_key':'http'})
            self.assertEqual(op('cut.ack',{'cut_id':cut['cut_id'],'digest':cut['digest']})['acknowledged'],True);self.assertIn('actual HTTP source',op('provider.preview')['request']['messages'][1]['content'])
            with self.assertRaises(urllib.error.HTTPError) as error:op('chain.get',actor='forged')
            self.assertEqual(error.exception.code,403)
        finally:server.shutdown();server.server_close();core.store.db.close();os.environ.pop('SIC_CORE_TEST_TOKEN',None)

if __name__=='__main__':unittest.main()
