"""Immutable image sidecars, actual SDK carriers and fail-closed media admission."""
import base64,copy,hashlib,importlib.util,io,json,os,threading,unittest
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from pathlib import Path
from sic_core import Core,CoreError
from sic_core.adapters import decode,encode,LiteLLMProvider
from sic_core import media


def image(color='red',size=(1024,768),detail='high'):
    from PIL import Image
    output=io.BytesIO();Image.new('RGB',size,color).save(output,format='PNG')
    return {'type':'image_url','image_url':{'url':'data:image/png;base64,'+base64.b64encode(output.getvalue()).decode(),'detail':detail}}


class MediaCoreTests(unittest.TestCase):
    def setUp(self):
        self.core=Core(config={'revocation_actors':['host']});self.ctx={'actor':'host','allow_write':True,'allow_model':False};self.ctx['chain_id']=self.op('chain.create')['chain_id'];self.op('config.update',{'memory_chars':65536,'editor_memory_chars':65536})
    def tearDown(self):self.core.close()
    def op(self,name,p=None):return self.core.operate(name,self.ctx,p or {})['result']
    def publish(self,sid):
        job=self.op('edit.program',{'source_id':sid})['job_id'];return self.op('cut.publish',{'job_id':job,'expected_cut':self.op('chain.get')['cut_seq'],'idempotency_key':job})['cb_id']
    def test_source_identity_order_text_scope_recursive_dedup_and_exact_paged_native_read(self):
        red,blue=image('red'),image('blue')
        native={'messages':[{'role':'user','content':[{'type':'text','text':'before'},red,{'type':'text','text':'between'},blue,{'type':'text','text':'after'}]}]}
        sid=self.op('source.ingest',{'request':native})['source_id'];source=self.op('source.read',{'source_id':sid})['source']
        self.assertEqual(source['sidecar'],native);self.assertEqual([x['anchor']['part'] for x in source['media']],[1,3]);self.assertEqual([x['text'] for x in source['units']],['before','between','after'])
        first=self.publish(sid);upper=self.op('hierarchy.form',{'level':2,'cb_ids':[first]})['source_id'];second=self.publish(upper);third=self.publish(self.op('hierarchy.form',{'level':3,'cb_ids':[second]})['source_id'])
        self.op('hierarchy.select',{'level':3})
        with self.core.store.transaction():
            state=self.core.store.state(self.ctx['chain_id']);state['_actor']='host';view=self.core._working_view(state,'worker');request=encode(view)
        carried=[b for m in request['messages'] for b in (m.get('content') if isinstance(m.get('content'),list) else []) if b['type']=='image_url']
        self.assertEqual(carried,[red,blue]);self.assertEqual(self.core.store.get(third,'cb')['media_source_ids'],[sid])
        page=[];offset=0
        while True:
            r=self.op('memory.page',{'kind':'media','object_id':sid,'offset':offset,'limit':521});page.append(r['value'])
            if r['next_offset'] is None:break
            offset=r['next_offset']
        self.assertEqual(json.loads(''.join(page))['native_request'],native)
        self.assertEqual(source,self.op('source.read',{'source_id':sid})['source'])
        self.assertEqual(self.op('memory.replay',{'cb_id':first})['body'],'before\nbetween\nafter')
    def test_image_only_wb_has_no_fake_editable_unit_and_omission_is_explicit(self):
        block=image();sid=self.op('source.ingest',{'request':{'messages':[{'role':'user','content':[block]}]}})['source_id']
        self.assertEqual(self.op('source.read',{'source_id':sid})['source']['units'],[])
        cid=self.publish(sid);projection=self.op('memory.project',{'max_chars':512})
        self.assertEqual(projection['cbs'],[])
        self.assertEqual(projection['manifest']['media']['omitted_source_count'],1)
        self.assertTrue(projection['manifest']['media']['complete_provider_admission_required'])
        self.assertEqual(self.op('source.read',{'source_id':sid})['source']['sidecar']['messages'][0]['content'],[block])
        self.assertEqual(self.core.store.get(cid,'cb')['body'],'')

    def test_closed_tool_group_media_keeps_entire_protocol_across_ingests(self):
        call={'role':'assistant','content':'inspect','tool_calls':[{'id':'actual-group','type':'function','function':{'name':'owned','arguments':'{}'}}]}
        sid=self.op('source.ingest',{'request':{'messages':[call]}})['source_id']
        result={'role':'tool','tool_call_id':'actual-group','content':[{'type':'text','text':'tool observation'},image()]}
        self.op('source.ingest',{'request':{'messages':[result]}})
        cid=self.publish(sid)
        with self.core.store.transaction():
            state=self.core.store.state(self.ctx['chain_id']);state['_actor']='host';request=encode(self.core._working_view(state,'worker'))
        decoded=decode('openai-chat',request);self.assertEqual(decoded['open_calls'],[])
        assistant=next(m for m in request['messages'] if m.get('tool_calls'));tool=next(m for m in request['messages'] if m['role']=='tool')
        self.assertEqual(assistant['tool_calls'],call['tool_calls']);self.assertEqual(tool['tool_call_id'],'actual-group');self.assertEqual(tool['content'][1],result['content'][1])
        self.assertEqual(len(self.core.store.get(cid,'cb')['media_source_ids']),2)
    def test_native_responses_and_anthropic_keep_images_and_reject_lossy_projection(self):
        block=image(detail='auto');uri=block['image_url']['url'];a={'messages':[{'role':'user','content':[{'type':'text','text':'look'},{'type':'image','source':{'type':'base64','media_type':'image/png','data':uri.split(',',1)[1]}}]}]}
        b={'input':[{'role':'user','content':[{'type':'input_text','text':'look'},{'type':'input_image','image_url':uri,'detail':'high'}]}]}
        for fmt,native in [('anthropic-messages',a),('openai-responses',b)]:
            source=decode(fmt,native);view={'protected':[],'cbs':[],'instant':{},'pending':[source],'controls':{}}
            out=encode(view,fmt);self.assertIn(uri.split(',',1)[1],json.dumps(out));self.assertEqual(len(source['media']),1);self.assertEqual(len(source['units']),1)
        with self.assertRaises(ValueError):encode({'protected':[],'cbs':[],'pending':[decode('openai-chat',{'messages':[{'role':'user','content':[image()]}]})]},'anthropic-messages')
    def test_invalid_mutable_unsupported_media_and_editor_image_target_are_rejected(self):
        for block in ({'type':'image_url','image_url':{'url':'https://example.com/live.png'}},{'type':'image_url','image_url':{'url':'data:image/png;base64,YmFk'}},{'type':'input_audio','input_audio':{'data':'aaa'}}):
            with self.assertRaises(ValueError):decode('openai-chat',{'messages':[{'role':'user','content':[block]}]})
        sid=self.op('source.ingest',{'request':{'messages':[{'role':'user','content':[{'type':'text','text':'editable'},image()]}]}})['source_id'];job=self.op('edit.prepare',{'source_id':sid})['job_id']
        with self.assertRaises(CoreError):self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[{'op':'REPLACE','target':'media1','content':'changed image'}]})
        self.op('security.revoke',{'source_ids':[sid],'reason':'owned test','confirm_revoke':True})
        with self.assertRaises(CoreError):self.op('source.read',{'source_id':sid})


@unittest.skipUnless(importlib.util.find_spec('litellm'),'optional LiteLLM SDK not installed')
class MediaSDKTests(unittest.TestCase):
    captures=[]
    def setUp(self):
        self.saved={k:os.environ.get(k) for k in ('SIC_MEDIA_FIXTURE_KEY','NO_PROXY','LITELLM_LOCAL_MODEL_COST_MAP','LITELLM_TELEMETRY')};os.environ.update(SIC_MEDIA_FIXTURE_KEY='owned',NO_PROXY='localhost,127.0.0.1',LITELLM_LOCAL_MODEL_COST_MAP='true',LITELLM_TELEMETRY='false')
        self.calls=[];fixture=self
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_POST(self):
                value=json.loads(self.rfile.read(int(self.headers['Content-Length'])));fixture.calls.append(value)
                editor=value['messages'][0]['content'].startswith('Edit assigned')
                if editor:
                    payload=json.loads(value['messages'][-1]['content']);text=json.dumps({'ops':[{'op':'COMPRESS','target':u['unit_id'],'content':'edited text'} for u in payload['editable_text_units']],'pvi':{}})
                else:text='multimodal continuation'
                response={'id':'owned-media','object':'chat.completion','created':0,'model':value['model'],'choices':[{'index':0,'finish_reason':'stop','message':{'role':'assistant','content':text}}],'usage':{'prompt_tokens':12,'completion_tokens':5,'total_tokens':17}}
                body=json.dumps(response).encode();self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
        self.server=ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=self.server.serve_forever,daemon=True).start()
        self.profile={'model':'openai/gpt-4o','tokenizer_model':'gpt-4o','api_base':'http://127.0.0.1:'+str(self.server.server_port)+'/v1','api_key_env':'SIC_MEDIA_FIXTURE_KEY','max_input_tokens':100000,'max_output_tokens':4096,'context_window':128000}
        self.core=Core(config={'roles':{'worker':dict(self.profile),'editor':dict(self.profile)}});self.ctx={'actor':'host','allow_write':True,'allow_model':True};self.ctx['chain_id']=self.op('chain.create')['chain_id'];self.op('config.update',{'memory_chars':65536,'editor_memory_chars':65536})
    def tearDown(self):
        self.captures.append({'test':self.id(),'calls':copy.deepcopy(self.calls),'ledger':self.op('ledger.export'),'state':self.op('chain.get')})
        self.core.close();self.server.shutdown();self.server.server_close()
        for key,value in self.saved.items():
            if value is None:os.environ.pop(key,None)
            else:os.environ[key]=value
    @classmethod
    def tearDownClass(cls):
        path=os.environ.get('SIC_MEDIA_EVIDENCE')
        if path:Path(path).write_text(json.dumps({'scope':'owned SDK mechanical image carrier only; research quality not_evaluated; physical KV unknown','runs':cls.captures},ensure_ascii=False,indent=2),encoding='utf-8')
    def op(self,name,p=None):return self.core.operate(name,self.ctx,p or {})['result']
    def test_sdk_editor_readonly_media_high_order_worker_and_real_library_admission(self):
        red,blue=image('red'),image('blue',detail='low')
        sid=self.op('source.ingest',{'request':{'messages':[{'role':'user','content':[{'type':'text','text':'before'},red,{'type':'text','text':'between'},blue]}]}})['source_id']
        ids=[]
        for level in (1,2,3):
            if level>1:sid=self.op('hierarchy.form',{'level':level,'cb_ids':[ids[-1]]})['source_id']
            job=self.op('edit.prepare',{'source_id':sid})['job_id'];self.op('edit.run',{'job_id':job,'editor':'editor-1'})
            payload=json.loads(self.calls[-1]['messages'][-1]['content']);self.assertEqual(len(payload['readonly_media']),2);self.assertNotIn('base64',json.dumps(payload))
            ids.append(self.op('cut.publish',{'job_id':job,'expected_cut':self.op('chain.get')['cut_seq'],'idempotency_key':job})['cb_id'])
        self.op('hierarchy.select',{'level':3});result=self.op('provider.continue',{'idempotency_key':'media-sdk'})
        request=self.calls[-1];blocks=[b for m in request['messages'] for b in (m.get('content') if isinstance(m.get('content'),list) else []) if b['type']=='image_url'];self.assertEqual(blocks,[red,blue])
        self.assertIn(ids[-1],json.dumps(request));self.assertNotIn(ids[0],json.dumps(request))
        usage=self.op('ledger.export')['calls'][-1];admission=usage['admission'];self.assertEqual(admission['media_count'],2)
        import litellm
        actual=litellm.token_counter(model='gpt-4o',messages=request['messages'],tools=request.get('tools'),use_default_image_token_count=False)
        self.assertEqual(admission['input_tokens'],actual)
        self.assertIn('LiteLLM',admission['media_counting'])
    def test_sdk_unknown_media_admission_refuses_without_http_and_images_count_against_budget(self):
        request={'messages':[{'role':'user','content':[image()]}]}
        for change in ({'model':'openai/unknown-vision-model'},{'max_input_tokens':1}):
            before=len(self.calls)
            with self.assertRaises(ValueError):LiteLLMProvider({'worker':self.profile|change}).call('worker',request)
            self.assertEqual(len(self.calls),before)
        automatic={'messages':[{'role':'user','content':[image(detail='auto')]}]}
        with self.assertRaises(ValueError):LiteLLMProvider({'worker':self.profile}).call('worker',automatic)
        self.assertEqual(self.calls,[])

if __name__=='__main__':unittest.main()
