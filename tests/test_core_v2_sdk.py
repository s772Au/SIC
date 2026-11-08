"""Actual in-process LiteLLM SDK against an owned, ephemeral loopback HTTP server.
No provider credentials or external model endpoint are used.
"""
import importlib.util,json,os,threading,unittest
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from sic_core import Core,CoreError

@unittest.skipUnless(importlib.util.find_spec('litellm'),'optional LiteLLM SDK not installed')
class SDKTests(unittest.TestCase):
    def setUp(self):
        os.environ['LITELLM_LOCAL_MODEL_COST_MAP']='true';os.environ['NO_PROXY']='localhost,127.0.0.1,::1';os.environ['LITELLM_TELEMETRY']='false';os.environ['SIC_CORE_FIXTURE_KEY']='not-a-real-provider-key';self.calls=[]
        calls=self.calls;fixture=self;self.next_tool=None
        class Mock(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_POST(self):
                data=json.loads(self.rfile.read(int(self.headers['Content-Length'])));calls.append(data);is_editor='editable_text_units' in json.dumps(data['messages']);units=json.loads(data['messages'][-1]['content']).get('editable_text_units',[]) if is_editor else [];content=json.dumps({'ops':[{'op':'COMPRESS','target':u['unit_id'],'content':'edited through actual SDK'} for u in units],'pvi':{}}) if is_editor else 'Worker used the edited prefix'
                value={'id':'local-fixture','object':'chat.completion','created':0,'model':data['model'],'choices':[{'index':0,'finish_reason':'stop','message':{'role':'assistant','content':content}}],'usage':{'prompt_tokens':30,'completion_tokens':12,'total_tokens':42,'prompt_tokens_details':{'cached_tokens':10}}}
                if not is_editor and fixture.next_tool is not None:
                    call=fixture.next_tool;fixture.next_tool=None;value['choices'][0]={'index':0,'finish_reason':'tool_calls','message':{'role':'assistant','content':'Read archived source through the bounded SIC tool','tool_calls':[call]}}
                body=json.dumps(value).encode();self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
        self.server=ThreadingHTTPServer(('127.0.0.1',0),Mock);self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.profile={'model':'openai/local-fixture','api_key_env':'SIC_CORE_FIXTURE_KEY','api_base':'http://127.0.0.1:'+str(self.server.server_port)+'/v1','tokenizer_model':'gpt-4o','max_input_tokens':100000,'max_output_tokens':4096,'context_window':128000,'tokenizer_status':'fixture-host-selected-not-provider-verified'}
        self.core=Core(config={'roles':{'editor':dict(self.profile),'worker':dict(self.profile)},'profiles':{'fixed':dict(self.profile)}});self.chain=self.op('chain.create')['chain_id']
    def tearDown(self):self.server.shutdown();self.server.server_close();self.core.close();os.environ.pop('SIC_CORE_FIXTURE_KEY',None)
    def op(self,name,p=None):return self.core.operate(name,{'actor':'host','chain_id':getattr(self,'chain',None),'allow_write':True,'allow_model':True},p or {})['result']
    def source(self):return self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'Complete input, exact source.'}],'temperature':0.2}})['source_id']
    def test_actual_sdk_editor_publish_worker_and_admission(self):
        source=self.source();self.op('harness.event',{'type':'tool_start','call_id':'running-owned-tool'});job=self.op('edit.prepare',{'source_id':source})['job_id'];self.op('edit.run',{'job_id':job,'editor':'editor-1'});self.assertIn('running-owned-tool',self.op('chain.get')['open_tools']);self.op('harness.event',{'type':'tool_end','call_id':'running-owned-tool'});self.op('cut.publish',{'job_id':job,'expected_cut':0,'idempotency_key':'sdk'});worker=self.op('provider.continue',{'idempotency_key':'sdk-worker'});self.assertEqual(len(self.calls),2);self.assertIn('edited through actual SDK',json.dumps(self.calls[1]['messages']));self.assertIn('temperature',self.calls[1]);self.assertEqual(worker['response']['model'],'local-fixture');self.assertEqual(self.op('usage.report')['category_totals']['cache_read'],20)
    def test_frozen_profile_and_budget_rejection_before_network(self):
        self.op('config.update',{'editor_profiles':{'editor-1':'fixed'}});source=self.source();job=self.op('edit.prepare',{'source_id':source})['job_id'];self.core.config['profiles']['fixed']['model']='openai/mutated-after-prepare';self.op('edit.run',{'job_id':job,'editor':'editor-1'});self.assertEqual(self.calls[0]['model'],'local-fixture')
        self.core.config['profiles']['tiny']={**self.profile,'max_input_tokens':1};self.op('config.update',{'editor_profiles':{'editor-1':'tiny'}});job2=self.op('edit.prepare',{'source_id':source})['job_id']
        with self.assertRaises(CoreError):self.op('edit.run',{'job_id':job2,'editor':'editor-1'})
        self.assertEqual(len(self.calls),1)
    def test_actual_sdk_parallel_candidate_revision_runtime_and_bounded_projection(self):
        source=self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'a'},{'role':'user','content':'b'}]}})['source_id'];job=self.op('edit.prepare',{'source_id':source,'assignments':[{'editor':'one','unit_ids':['u1']},{'editor':'two','unit_ids':['u2']}]})['job_id'];self.op('edit.run_all',{'job_id':job,'max_workers':2});self.assertEqual(len(self.calls),2);self.op('edit.candidate.update',{'job_id':job,'expected_revision':0,'ops':[{'op':'REPLACE','target':'u1','content':'UI corrected SDK result'}],'pvi':{'V':['next work']}});self.op('cut.publish',{'job_id':job,'expected_cut':0,'expected_candidate_revision':1,'idempotency_key':'revised-sdk'});self.op('runtime.run',{'editor_mode':'model','continue_worker':True,'max_steps':2});self.assertIn('UI corrected SDK result',json.dumps(self.calls[-1]['messages']));self.assertEqual(len(self.calls),4)
    def test_actual_sdk_large_archive_tool_result_and_next_worker_fit_bounded_window(self):
        self.op('config.update',{'memory_chars':2048,'editor_memory_chars':2048,'tool_result_chars':2048});first=None
        for i in range(12):
            source=self.op('source.ingest',{'request':{'messages':[{'role':'user','content':str(i)+' '+('word '*1000)}]}})['source_id'];first=first or source;job=self.op('edit.program',{'source_id':source,'rules':[]})['job_id'];self.op('cut.publish',{'job_id':job,'expected_cut':i,'idempotency_key':'archive-'+str(i)})
        self.core.config['roles']['worker'].update(max_input_tokens=4000,context_window=8192);self.next_tool={'id':'bounded-memory-call','type':'function','function':{'name':'sic_source_read','arguments':json.dumps({'source_id':first,'limit':100000})}};self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'Retrieve the earliest source detail.'}]}});result=self.op('runtime.run',{'editor_mode':'program','continue_worker':True,'worker_request_id':'bounded-sdk','max_steps':4});self.assertEqual(len(self.calls),2);worker=result['steps'][-1]['result'];self.assertIn('response',worker)
        ledger=self.op('ledger.export');self.assertEqual(len(ledger['calls']),2);self.assertTrue(all(call['admission']['input_tokens']<=4000 for call in ledger['calls']));self.assertGreater(len(self.op('chain.get')['cbs']),12)
        source=self.op('source.read',{'source_id':first})['source'];self.assertGreater(sum(len(u['text']) for u in source['units']),4000)
    def test_actual_sdk_different_fixed_worker_editor_models(self):
        self.core.config['roles']['editor']['model']='openai/editor-fixed-fixture';self.core.config['roles']['worker']['model']='openai/worker-fixed-fixture';source=self.source();job=self.op('edit.prepare',{'source_id':source})['job_id'];self.op('edit.run',{'job_id':job,'editor':'editor-1'});self.op('cut.publish',{'job_id':job,'expected_cut':0,'idempotency_key':'paired-models'});self.op('provider.continue',{'idempotency_key':'different-worker'});self.assertEqual([request['model'] for request in self.calls],['editor-fixed-fixture','worker-fixed-fixture']);self.assertEqual({call['role'] for call in self.op('ledger.export')['calls']},{'editor','worker'})
    def test_unknown_tokenizer_rejected_without_network(self):
        self.core.config['profiles']['unknown']={**self.profile,'tokenizer_model':'unknown-tokenizer-no-fallback'};self.op('config.update',{'editor_profiles':{'editor-1':'unknown'}});source=self.source();job=self.op('edit.prepare',{'source_id':source})['job_id']
        with self.assertRaises(CoreError):self.op('edit.run',{'job_id':job,'editor':'editor-1'})
        self.assertEqual(self.calls,[])

if __name__=='__main__':unittest.main()
