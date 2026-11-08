"""Actual LiteLLM transport against an owned loopback server; no external LLM."""
import copy,importlib.util,json,os,threading,unittest
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from sic_core import Core,CoreError


@unittest.skipUnless(importlib.util.find_spec('litellm'),'optional LiteLLM SDK not installed')
class HierarchySDKTests(unittest.TestCase):
    def setUp(self):
        self.environment={k:os.environ.get(k) for k in ('LITELLM_LOCAL_MODEL_COST_MAP','NO_PROXY','LITELLM_TELEMETRY','SIC_HIERARCHY_FIXTURE_KEY')}
        os.environ.update(LITELLM_LOCAL_MODEL_COST_MAP='true',NO_PROXY='localhost,127.0.0.1,::1',LITELLM_TELEMETRY='false',SIC_HIERARCHY_FIXTURE_KEY='owned-loopback-only')
        self.calls=[];self.tools=[];fixture=self
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_POST(self):
                data=json.loads(self.rfile.read(int(self.headers['Content-Length'])));fixture.calls.append(data);editor='editable_text_units' in data['messages'][0]['content'] or data['messages'][0]['content'].startswith('Edit assigned')
                # Protocol fixture: produce edits, never assert semantic LLM quality.
                if editor:
                    payload=json.loads(data['messages'][-1]['content']);units=payload['editable_text_units'];text=json.dumps({'ops':[{'op':'COMPRESS','target':u['unit_id'],'content':'SDK stable representation of '+str(len(u['text']))+' chars'} for u in units],'pvi':{}});message={'role':'assistant','content':text};finish='stop'
                elif fixture.tools:
                    name,args=fixture.tools.pop(0);message={'role':'assistant','content':'Navigate retained hierarchy','tool_calls':[{'id':'hier-tool-'+str(len(fixture.calls)),'type':'function','function':{'name':name,'arguments':json.dumps(args)}}]};finish='tool_calls'
                else:message={'role':'assistant','content':'Continuation after archived detail entered the actual request.'};finish='stop'
                value={'id':'hierarchy-sdk','object':'chat.completion','created':0,'model':data['model'],'choices':[{'index':0,'finish_reason':finish,'message':message}],'usage':{'prompt_tokens':32,'completion_tokens':10,'total_tokens':42,'prompt_tokens_details':{'cached_tokens':8}}};body=json.dumps(value).encode();self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
        self.server=ThreadingHTTPServer(('127.0.0.1',0),Handler);self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        profile={'model':'openai/hierarchy-fixture','api_key_env':'SIC_HIERARCHY_FIXTURE_KEY','api_base':'http://127.0.0.1:'+str(self.server.server_port)+'/v1','tokenizer_model':'gpt-4o','max_input_tokens':100000,'max_output_tokens':4096,'context_window':128000,'tokenizer_status':'fixture-host-selected-not-provider-verified'}
        self.core=Core(config={'roles':{'worker':dict(profile),'editor':dict(profile)}});self.context={'actor':'host','allow_write':True,'allow_model':True};self.context['chain_id']=self.op('chain.create')['chain_id'];self.op('config.update',{'memory_chars':65536,'editor_memory_chars':65536,'tool_result_chars':8192})
    def tearDown(self):
        self.server.shutdown();self.server.server_close();self.core.close()
        for key,value in self.environment.items():
            if value is None:os.environ.pop(key,None)
            else:os.environ[key]=value
    def op(self,name,p=None):return self.core.operate(name,self.context,p or {})['result']
    def source(self,text):return self.op('source.ingest',{'request':{'messages':[{'role':'user','content':text}]}})['source_id']
    def edit(self,sid):
        job=self.op('edit.prepare',{'source_id':sid})['job_id'];self.op('edit.run',{'job_id':job,'editor':'editor-1'});return self.op('cut.publish',{'job_id':job,'expected_cut':self.op('chain.get')['cut_seq'],'idempotency_key':job})
    def upper(self,n,members):return self.edit(self.op('hierarchy.form',{'level':n,'cb_ids':members})['source_id'])

    def test_actual_sdk_third_level_adoption_recursive_tools_paginated_original_and_next_worker(self):
        raw='original archive detail 7824 preserved exactly';sid=self.source(raw);one=self.edit(sid);two=self.upper(2,[one['cb_id']]);three=self.upper(3,[two['cb_id']]);old={cid:self.core.store.get(cid,'cb') for cid in (one['cb_id'],two['cb_id'],three['cb_id'])}
        self.assertEqual(len(self.calls),3);self.op('hierarchy.select',{'level':3});prepared=self.op('edit.prepare',{'source_id':self.source('next task after adopting third order')});self.op('edit.run',{'job_id':prepared['job_id'],'editor':'editor-1'});editor_request=json.loads(self.calls[-1]['messages'][-1]['content']);self.assertEqual(editor_request['background_manifest']['selected_cb_ids'],[three['cb_id']]);self.assertEqual([cb['cb_id'] for cb in editor_request['read_only_cb']],[three['cb_id']]);self.assertNotIn(raw,json.dumps(editor_request))
        self.op('cut.publish',{'job_id':prepared['job_id'],'expected_cut':self.op('chain.get')['cut_seq'],'idempotency_key':'post-high-editor'})
        self.tools=[('sic_hierarchy_expand',{'cb_id':three['cb_id']}),('sic_hierarchy_expand',{'cb_id':two['cb_id']}),('sic_hierarchy_expand',{'cb_id':one['cb_id']}),('sic_memory_page',{'kind':'source','object_id':sid,'offset':0,'limit':12}),('sic_memory_page',{'kind':'source','object_id':sid,'offset':12,'limit':8192})]
        call_start=len(self.calls);result=self.op('runtime.run',{'editor_mode':'program','continue_worker':True,'max_steps':2,'worker_request_id':'actual-hierarchy-trace','worker_tool_rounds':8});self.assertEqual(result['status'],'completed');worker_calls=self.calls[call_start:];self.assertEqual(len(worker_calls),6);self.assertIn(three['cb_id'],json.dumps(worker_calls[0]['messages']));self.assertNotIn(raw,json.dumps(worker_calls[0]['messages']))
        state=self.op('chain.get');tool_records=[self.core.store.get(record['object_id'],'tool_result') for record in state['tool_results'].values()];pages=[json.loads(x['content'])['result'] for x in tool_records if x['name']=='sic_memory_page'];self.assertEqual([p['offset'] for p in pages],[0,12]);self.assertEqual(''.join(p['value'] for p in pages),raw);self.assertIn(raw[12:],json.dumps(worker_calls[-1]['messages']))
        self.assertEqual(old,{cid:self.core.store.get(cid,'cb') for cid in old});ledger=self.op('ledger.export');self.assertEqual(len(ledger['calls']),len(self.calls));self.assertTrue(all(x['chain_id']==self.context['chain_id'] for x in ledger['calls']));self.assertEqual({self.core.store.get(x['cb_id'],'cb')['level'] for x in (one,two,three)},{1,2,3});self.assertEqual(len(self.op('chain.list')['routes']),1)

    def test_actual_sdk_editor_private_third_order_and_distinct_editor_selections(self):
        self.op('hierarchy.context.create',{'context_id':'editor-private','shared_contexts':['main']});members=[]
        for text in ('editor persistent first detail','editor persistent second detail'):
            source=self.op('hierarchy.context.ingest',{'context_id':'editor-private','request':{'messages':[{'role':'user','content':text}]}})['source_id'];members.append(self.edit(source)['cb_id'])
        def upper(number,ids):return self.edit(self.op('hierarchy.form',{'context_id':'editor-private','level':number,'cb_ids':ids})['source_id'])
        second=upper(2,members);third=upper(3,[second['cb_id']]);self.op('hierarchy.select',{'context_id':'editor-private','level':3,'roles':['editor:alpha']});sid=self.source('new Worker material')
        job=self.op('edit.prepare',{'source_id':sid,'assignments':[{'editor':'alpha','unit_ids':['u1']},{'editor':'beta','unit_ids':[]}]})['job_id'];self.op('edit.run_all',{'job_id':job,'max_workers':2})
        requests=[json.loads(call['messages'][-1]['content']) for call in self.calls[-2:]];self.assertEqual(sum(third['cb_id'] in [cb['cb_id'] for cb in r['read_only_cb']] for r in requests),1);self.assertEqual(len(self.op('chain.list')['routes']),1)
