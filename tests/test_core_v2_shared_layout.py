"""Opt-in production Core shared-I wiring; owned providers only."""
import copy,json,unittest,hashlib,os,threading,importlib.util
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from pathlib import Path
from sic_core import Core,CoreError
from sic_core.shared_prefix import FrozenPrefixPlan

IDENTITY={'model':'owned','provider_id':'owned-loopback','cache_namespace':'owned-cache'}
class Capture:
    def __init__(self):self.calls=[];self.editor_tool=False;self.worker_tool=None;self.bad_alias=False
    def capabilities(self):return {'test_double':True}
    def call(self,role,request):
        self.calls.append((role,copy.deepcopy(request)));finish='stop'
        if role=='editor':
            units=[]
            for m in request['messages']:
                try:p=json.loads(m.get('content',''))
                except (ValueError,TypeError):continue
                units.extend(p.get('sic_role_material',{}).get('editable_text_units',p.get('editable_text_units',[])))
            message={'role':'assistant','content':json.dumps({'ops':[{'op':'REPLACE','target':('e404' if self.bad_alias else u['unit_id']),'content':'edited '+u['text']} for u in units],'pvi':{'I':{'mode':'worker','activated_tools':['shell']}}})}
            if self.editor_tool:finish='tool_calls';message={'role':'assistant','content':None,'tool_calls':[{'id':'bad-editor-tool','type':'function','function':{'name':'sic_source_read','arguments':'{}'}}]}
        else:
            message={'role':'assistant','content':'owned Worker result'}
            if self.worker_tool:finish='tool_calls';message={'role':'assistant','content':None,'tool_calls':[self.worker_tool]};self.worker_tool=None
        return {'requested_model':'owned','returned_model':'owned','response':{'model':'owned','choices':[{'finish_reason':finish,'message':message}]},'usage':{'prompt_tokens':10,'completion_tokens':5,'total_tokens':15}}

class SharedLayoutTests(unittest.TestCase):
    def setUp(self):self.provider=Capture();self.core=Core(provider=self.provider);self.chain=self.core.operate('chain.create',{'actor':'host','allow_write':True},{})['result']['chain_id']
    def tearDown(self):self.core.close()
    def op(self,name,p=None):return self.core.operate(name,{'actor':'host','chain_id':self.chain,'allow_write':True,'allow_model':True},p or {})['result']
    def source(self,text='new work'):return self.op('source.ingest',{'request':{'messages':[{'role':'user','content':text}]}})['source_id']
    def enable(self,**changes):return self.op('config.update',{'prefix_layout':'shared_i','shared_prefix_identity':IDENTITY,'shared_prefix_budget':4096,'schedule_policy':'interleave',**changes})
    def seed(self):
        source=self.source('frozen prior CB');job=self.op('edit.program',{'source_id':source})['job_id'];return self.op('cut.publish',{'job_id':job,'expected_cut':self.op('chain.get')['cut_seq'],'idempotency_key':'seed'+job})['cb_id']
    def request(self,request_id):
        with self.core.store.transaction():return self.core.store.get(request_id,'provider_request')
    def test_default_layout_unchanged_and_invalid_config_atomic(self):
        self.assertEqual(self.op('config.get')['configuration']['prefix_layout'],'separate')
        with self.assertRaises(CoreError):self.op('config.update',{'prefix_layout':'invented'})
        self.assertEqual(self.op('config.get')['configuration']['prefix_layout'],'separate')
    def test_switch_back_to_separate_clears_active_gate_preserves_old_call_binding(self):
        self.enable();self.source();self.provider.worker_tool={'id':'old-shared-call','type':'function','function':{'name':'sic_source_read','arguments':'{}'}};self.op('provider.continue',{'idempotency_key':'old-shared'})
        old=self.op('chain.get')['shared_tool_call_modes']['old-shared-call'];self.op('harness.event',{'type':'tool_start','call_id':'old-shared-call'});self.op('harness.event',{'type':'tool_end','call_id':'old-shared-call'});self.op('source.ingest',{'request':{'messages':[{'role':'tool','tool_call_id':'old-shared-call','content':'closed original native result'}]}})
        self.op('config.update',{'prefix_layout':'separate'});self.source('new separate invocation');self.provider.worker_tool={'id':'new-separate-call','type':'function','function':{'name':'sic_source_read','arguments':'{}'}};self.op('provider.continue',{'idempotency_key':'new-separate'})
        state=self.op('chain.get');self.assertNotIn('shared_worker_capability',state);self.assertEqual(state['shared_tool_call_modes']['old-shared-call'],old);self.assertEqual(state['last_prefix_layout']['applied_layout'],'separate');self.op('tools.authorize',{'name':'sic_source_read'});self.op('harness.event',{'type':'tool_start','call_id':'new-separate-call'});self.assertIn('new-separate-call',self.op('chain.get')['open_tools'])
    def test_actual_worker_editor_same_frozen_prefix_same_tools_alias_publication_replay(self):
        old=self.seed();self.enable();source=self.source();job=self.op('edit.prepare',{'source_id':source})['job_id'];worker=self.op('provider.continue',{'idempotency_key':'worker'});editor=self.op('edit.run',{'job_id':job,'editor':'editor-1'})
        w=self.request(worker['request_id']);s=self.op('chain.get');e=self.request(s['jobs'][job]['attempts']['editor-1']['request_id']);self.assertEqual(w['prefix_layout']['plan_id'],e['prefix_layout']['plan_id']);self.assertEqual(w['prefix_layout']['prefix_bytes_sha256'],e['prefix_layout']['prefix_bytes_sha256']);self.assertEqual(w['request']['tools'],e['request']['tools'])
        with self.core.store.transaction():plan=FrozenPrefixPlan._freeze(self.core.store.get(w['prefix_layout']['plan_id'],'shared_prefix_plan')['payload']);old_value=copy.deepcopy(self.core.store.get(old,'cb'))
        count=len(plan.prefix_messages);self.assertEqual(w['request']['messages'][:count],e['request']['messages'][:count]);self.assertIn('e1',json.dumps(e['request']));self.assertEqual(s['jobs'][job]['responses']['editor-1']['ops'][0]['target'],'u1');self.assertEqual(s['jobs'][job]['responses']['editor-1']['pvi']['I']['mode'],'worker')
        published=self.op('cut.publish',{'job_id':job,'expected_cut':1,'idempotency_key':'shared-publish'});replayed=self.op('memory.replay',{'cb_id':published['cb_id']});self.assertTrue(replayed['body_matches_published_cb'])
        with self.core.store.transaction():self.assertEqual(self.core.store.get(old,'cb'),old_value)
        page=self.op('memory.page',{'kind':'prefix_plan','object_id':w['prefix_layout']['plan_id'],'limit':8192});self.assertIn('prefix',page['unit']);self.assertTrue(self.op('config.get')['shared_prefix_plan_ids']);self.assertEqual(len(self.op('ledger.export')['calls']),2)
    def test_editor_worker_tool_calls_denied_before_any_body_and_raw_response_kept(self):
        self.enable();source=self.source();job=self.op('edit.prepare',{'source_id':source})['job_id'];self.provider.editor_tool=True
        with self.assertRaises(CoreError) as denied:self.op('edit.run',{'job_id':job,'editor':'editor-1'})
        self.assertEqual(denied.exception.status,403);self.assertEqual(self.op('chain.get')['jobs'][job]['status'],'failed');self.assertEqual(len(self.provider.calls),1)
        with self.core.store.transaction():self.assertEqual(self.core.store.db.execute("select count(*) from objects where kind='provider_response'").fetchone()[0],1)
    def test_worker_virtual_editor_tool_is_early_403_and_no_source_adoption(self):
        self.enable();self.source();before=self.op('chain.get');self.provider.worker_tool={'id':'forged','type':'function','function':{'name':'sic_edit_blocks','arguments':'{}'}}
        with self.assertRaises(CoreError) as denied:self.op('provider.continue',{'idempotency_key':'denied'})
        self.assertEqual(denied.exception.status,403);self.assertEqual(self.op('chain.get')['pending'],before['pending']);self.assertEqual(len(self.provider.calls),1)
    def test_delivery_then_revoke_denied_at_native_tool_start_before_body(self):
        self.enable();self.source();self.provider.worker_tool={'id':'read-call','type':'function','function':{'name':'sic_source_read','arguments':'{}'}};self.op('provider.continue',{'idempotency_key':'proposal'})
        tools=self.op('chain.get')['tools'];tools[0]['execute_acl']=[];self.op('tools.configure',{'tools':tools})
        for operation,payload in [('tools.authorize',{'name':'sic_source_read'}),('tools.invoke',{'name':'sic_source_read','arguments':{}}),('harness.event',{'type':'tool_start','call_id':'read-call'})]:
            with self.subTest(operation=operation),self.assertRaises(CoreError) as denied:self.op(operation,payload)
            self.assertEqual(denied.exception.status,403)
        self.assertFalse(self.op('chain.get')['open_tools'])
    def test_frozen_job_prompt_layout_and_scope_survive_config_change(self):
        self.enable(editor_prompt='owned original Editor protocol');source=self.source();job=self.op('edit.prepare',{'source_id':source})['job_id'];plan=self.op('chain.get')['jobs'][job]['prefix_layout']['editor-1']['plan_id'];self.op('config.update',{'prefix_layout':'separate','editor_prompt':'changed after freeze'})
        self.op('edit.run',{'job_id':job,'editor':'editor-1'});raw=json.dumps(self.provider.calls[-1][1]);self.assertIn('owned original Editor protocol',raw);self.assertNotIn('changed after freeze',raw);self.assertEqual(self.op('chain.get')['last_prefix_layout']['plan_id'],plan)
    def test_context_budget_identity_mismatch_have_complete_separate_fallback(self):
        self.enable(shared_prefix_identity={'worker':IDENTITY,'editor':{**IDENTITY,'model':'different'}});source=self.source();job=self.op('edit.prepare',{'source_id':source})['job_id'];self.op('edit.run',{'job_id':job,'editor':'editor-1'});meta=self.op('chain.get')['last_prefix_layout'];self.assertEqual(meta['applied_layout'],'separate');self.assertIn('mismatch',meta['reason']);self.assertIn('new work',json.dumps(self.provider.calls[-1][1]))
    def test_precise_alias_unknown_rejected_raw_not_repaired(self):
        self.enable();source=self.source();job=self.op('edit.prepare',{'source_id':source})['job_id'];self.provider.bad_alias=True
        with self.assertRaises(CoreError):self.op('edit.run',{'job_id':job,'editor':'editor-1'})
        self.assertEqual(self.op('chain.get')['jobs'][job]['status'],'failed');self.assertFalse(self.op('chain.get')['jobs'][job]['responses'])

    def test_nth_level_private_role_matching_uses_same_plan_without_cross_scope_leak(self):
        lower=self.seed();source=self.op('hierarchy.form',{'level':2,'cb_ids':[lower]})['source_id'];job=self.op('edit.program',{'source_id':source})['job_id'];second=self.op('cut.publish',{'job_id':job,'expected_cut':1,'idempotency_key':'level2'})['cb_id']
        self.op('hierarchy.select',{'level':2,'roles':['worker','editor']});self.enable();source=self.source('primary work while same high background');job=self.op('edit.prepare',{'source_id':source})['job_id'];self.op('provider.continue',{'idempotency_key':'higher-worker'});self.op('edit.run',{'job_id':job,'editor':'editor-1'});s=self.op('chain.get');self.assertEqual(s['last_prefix_layout']['applied_layout'],'shared_i')
        ref=s['last_prefix_layout']['plan_id']
        with self.core.store.transaction():p=self.core.store.get(ref,'shared_prefix_plan')['payload']
        self.assertEqual(p['level'],2);self.assertIn(second,p['cb_ids']);self.assertNotIn(lower,p['cb_ids'])
        self.op('hierarchy.context.create',{'context_id':'editor-private'});src=self.op('hierarchy.context.ingest',{'context_id':'editor-private','request':{'messages':[{'role':'user','content':'PRIVATE_EDITOR_ONLY'}]}})['source_id']
        privjob=self.op('edit.program',{'source_id':src})['job_id'];self.op('cut.publish',{'job_id':privjob,'expected_cut':s['cut_seq'],'idempotency_key':'private'})
        self.op('hierarchy.select',{'context_id':'editor-private','level':1,'roles':['editor']})
        # A new preparation preserves its real private background instead of
        # pulling it into the Worker's shared prefix to manufacture a match.
        source=self.op('chain.get')['pending'][0];new=self.op('edit.prepare',{'source_id':source})['job_id'];self.op('edit.run',{'job_id':new,'editor':'editor-1'});self.assertEqual(self.op('chain.get')['last_prefix_layout']['applied_layout'],'separate');self.assertIn('PRIVATE_EDITOR_ONLY',json.dumps(self.provider.calls[-1][1]))
    def test_media_full_original_carrier_and_readonly_descriptors_survive_explicit_fallback(self):
        import base64,io
        from PIL import Image
        stream=io.BytesIO();Image.new('RGB',(2,3),(1,2,3)).save(stream,format='PNG');uri='data:image/png;base64,'+base64.b64encode(stream.getvalue()).decode()
        self.enable();native={'messages':[{'role':'user','content':[{'type':'text','text':'before'},{'type':'image_url','image_url':{'url':uri,'detail':'high'}},{'type':'text','text':'after'}]}]};source=self.op('source.ingest',{'request':native})['source_id'];job=self.op('edit.prepare',{'source_id':source})['job_id'];self.op('provider.continue',{'idempotency_key':'media'});worker=self.provider.calls[-1][1];self.assertIn(uri,json.dumps(worker));self.assertEqual(worker['messages'][-1],native['messages'][0]);self.assertEqual(self.op('chain.get')['last_prefix_layout']['applied_layout'],'separate')
        self.op('edit.run',{'job_id':job,'editor':'editor-1'});editor=self.provider.calls[-1][1];self.assertNotIn(uri,json.dumps(editor));self.assertIn('readonly_media',json.dumps(editor));self.assertIn('media',self.op('chain.get')['last_prefix_layout']['reason'])
    def test_readonly_protocol_freeze_has_proposal_result_and_actual_host_observation(self):
        self.enable();self.source();self.provider.worker_tool={'id':'native-read','type':'function','function':{'name':'sic_source_read','arguments':'{}'}};self.op('provider.continue',{'idempotency_key':'native'});self.op('harness.event',{'type':'tool_start','call_id':'native-read','observed_by':'owned-around-dispatch'});self.op('harness.event',{'type':'tool_end','call_id':'native-read','observed_by':'owned-around-dispatch','outcome':'completed'})
        source=self.op('source.ingest',{'request':{'messages':[{'role':'tool','tool_call_id':'native-read','content':'actual retained native result'}]}})['source_id'];first=self.op('chain.get')['pending'][0];formed=self.op('wb.form',{'source_ids':self.op('chain.get')['pending']})['source_id'];job=self.op('edit.prepare',{'source_id':formed})['job_id'];self.op('edit.run',{'job_id':job,'editor':'editor-1'});request=self.provider.calls[-1][1];payload=json.loads(request['messages'][-1]['content']);provenance=payload['readonly_protocol_messages'];self.assertTrue(any(p['assistant_tool_proposals'] for p in provenance));self.assertTrue(any(p['tool_result_for']=='native-read' for p in provenance))
        control=next(json.loads(m['content'])['sic_host_activation'] for m in request['messages'] if m.get('role')=='system' and 'sic_host_activation' in m.get('content',''));facts=control['observed_host_tool_events'][0]['events'];self.assertTrue(any(e['type']=='tool_end' and e['outcome']=='completed' for e in facts));self.assertEqual(control['activated_tools'],[])

    def test_matching_private_contexts_share_without_unselected_main_CB(self):
        self.seed();self.op('hierarchy.context.create',{'context_id':'private'});source=self.op('hierarchy.context.ingest',{'context_id':'private','request':{'messages':[{'role':'user','content':'private common context'}]}})['source_id'];job=self.op('edit.program',{'source_id':source})['job_id'];self.op('cut.publish',{'job_id':job,'expected_cut':1,'idempotency_key':'private-common'});self.op('hierarchy.select',{'context_id':'private','level':1,'roles':['worker','editor']});self.enable();source=self.source('new main WB');job=self.op('edit.prepare',{'source_id':source})['job_id'];self.op('provider.continue',{'idempotency_key':'private-worker'});self.op('edit.run',{'job_id':job,'editor':'editor-1'});state=self.op('chain.get');self.assertEqual(state['last_prefix_layout']['applied_layout'],'shared_i')
        with self.core.store.transaction():plan=self.core.store.get(state['last_prefix_layout']['plan_id'],'shared_prefix_plan')['payload']
        self.assertEqual(plan['context_id'],'private');self.assertIn('private common context',json.dumps(plan['rendered_cb_messages']));self.assertNotIn('frozen prior CB',json.dumps(plan['rendered_cb_messages']))
    def test_projection_budget_mismatch_falls_back_without_truncating_units(self):
        self.op('config.update',{'prefix_layout':'shared_i','shared_prefix_identity':IDENTITY,'memory_chars':8192,'editor_memory_chars':4096});source=self.source('complete original text');job=self.op('edit.prepare',{'source_id':source})['job_id'];self.op('edit.run',{'job_id':job,'editor':'editor-1'});metadata=self.op('chain.get')['last_prefix_layout'];self.assertEqual(metadata['applied_layout'],'separate');self.assertIn('budget mismatch',metadata['reason']);self.assertIn('complete original text',json.dumps(self.provider.calls[-1][1]))
    def test_actual_runtime_executor_allowed_and_revoked_gate_precedes_body(self):
        self.enable();tools=self.op('chain.get')['tools'];tools.append({'name':'owned_native','descriptor':'owned body','schema':{'type':'function','function':{'name':'owned_native','parameters':{'type':'object','properties':{}}}},'discover_acl':['host'],'expand_acl':['host'],'execute_acl':['host'],'expanded':True});self.op('tools.configure',{'tools':tools});bodies=[];self.core.runtime.register_tool('owned_native',lambda context,args:bodies.append('executed') or {'owned':'result'});self.provider.worker_tool={'id':'runtime-owned','type':'function','function':{'name':'owned_native','arguments':'{}'}};self.source('exercise real runtime body')
        self.op('runtime.run',{'max_steps':1,'editor_mode':'program','continue_worker':True,'worker_request_id':'owned-body'});self.assertEqual(bodies,['executed']);self.assertFalse(self.op('chain.get')['open_tools'])
        # The same real Executor remains registered, but the pre-body native
        # start hook must reject a delivered proposal after ACL revocation.
        self.provider.worker_tool={'id':'runtime-revoked','type':'function','function':{'name':'owned_native','arguments':'{}'}};self.op('provider.continue',{'idempotency_key':'pending-revoked'});tools=self.op('chain.get')['tools'];next(t for t in tools if t['name']=='owned_native')['execute_acl']=[];self.op('tools.configure',{'tools':tools})
        with self.assertRaises(CoreError):self.op('harness.event',{'type':'tool_start','call_id':'runtime-revoked'})
        self.assertEqual(bodies,['executed'])

    def test_shared_provider_preview_uses_real_assembler_without_persistent_side_effects(self):
        self.enable();self.source()
        with self.core.store.transaction():before='\n'.join(self.core.store.db.iterdump())
        preview=self.op('provider.preview');self.assertEqual(preview['prefix_layout']['applied_layout'],'shared_i');self.assertTrue(preview['prefix_layout']['preview_only'])
        with self.core.store.transaction():self.assertEqual('\n'.join(self.core.store.db.iterdump()),before)
        self.op('provider.continue',{'idempotency_key':'preview-actual'});self.assertEqual(preview['request']['messages'][0],self.provider.calls[-1][1]['messages'][0]);self.assertEqual(preview['request']['tools'],self.provider.calls[-1][1]['tools'])
    def test_shared_frontier_higher_unselected_candidate_does_not_fake_request_effect(self):
        one=self.seed();two=self.seed();higher=self.op('hierarchy.form',{'level':2,'cb_ids':[one,two]})['source_id'];self.enable();job=self.op('edit.prepare',{'source_id':higher})['job_id'];self.op('edit.run',{'job_id':job,'editor':'editor-1'});future=self.source('next primary scope')
        with self.core.store.transaction():before='\n'.join(self.core.store.db.iterdump())
        for role,extras in [('worker',{}),('editor:future',{'next_source_id':future})]:
            compared=self.op('frontier.compare',{'job_id':job,'expected_cut':2,'role':role,**extras})
            self.assertFalse(compared['request_affected_by_candidate']);self.assertIn('sic_host_activation',json.dumps(compared['necessary_request']));self.assertEqual(compared['necessary_request']['tools'],compared['additional_request']['tools'])
            with self.core.store.transaction():self.assertEqual('\n'.join(self.core.store.db.iterdump()),before)
        self.assertEqual(len(self.provider.calls),1)

@unittest.skipUnless(importlib.util.find_spec('litellm'),'optional LiteLLM SDK not installed')
class SharedLayoutSDKTests(unittest.TestCase):
    def test_real_sdk_loopback_shared_requests_and_editor_tool_denied(self):
        saved={k:os.environ.get(k) for k in ['SIC_SHARED_OWNED_KEY','NO_PROXY','LITELLM_TELEMETRY','LITELLM_LOCAL_MODEL_COST_MAP']};os.environ.update(SIC_SHARED_OWNED_KEY='owned-loopback-only',NO_PROXY='localhost,127.0.0.1,::1',LITELLM_TELEMETRY='false',LITELLM_LOCAL_MODEL_COST_MAP='true')
        wires=[];fixture=Capture()
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*a):pass
            def do_POST(self):
                raw=self.rfile.read(int(self.headers['Content-Length']));request=json.loads(raw);wires.append((raw,request));mode=None
                for message in request['messages']:
                    try:mode=json.loads(message.get('content','')).get('sic_host_activation',{}).get('mode') or mode
                    except (ValueError,TypeError):pass
                result=fixture.call(mode,request);body=json.dumps({'id':'owned-sdk-shared','object':'chat.completion','created':0,'model':request['model'],'choices':[{'index':0,**result['response']['choices'][0]}],'usage':result['usage']}).encode();self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start();core=None
        try:
            profile={'model':'openai/gpt-4o','api_base':'http://127.0.0.1:'+str(server.server_port)+'/v1','api_key_env':'SIC_SHARED_OWNED_KEY','tokenizer_model':'gpt-4o','max_input_tokens':100000,'max_output_tokens':4096,'context_window':128000};core=Core(config={'roles':{'worker':dict(profile),'editor':dict(profile)}});ctx={'actor':'host','allow_write':True,'allow_model':True};ctx['chain_id']=core.operate('chain.create',ctx,{})['result']['chain_id']
            def op(name,p=None):return core.operate(name,ctx,p or {})['result']
            source=op('source.ingest',{'request':{'messages':[{'role':'user','content':'owned fixed prior CB'}]}})['source_id'];job=op('edit.program',{'source_id':source})['job_id'];op('cut.publish',{'job_id':job,'expected_cut':0,'idempotency_key':'seed'})
            op('config.update',{'prefix_layout':'shared_i','shared_prefix_budget':4096,'schedule_policy':'interleave'});source=op('source.ingest',{'request':{'messages':[{'role':'user','content':'owned SDK editable work'}]}})['source_id'];job=op('edit.prepare',{'source_id':source})['job_id'];worker=op('provider.continue',{'idempotency_key':'shared-sdk'});op('edit.run',{'job_id':job,'editor':'editor-1'})
            self.assertEqual(len(wires),2);state=op('chain.get');metadata=state['last_prefix_layout'];self.assertEqual(metadata['applied_layout'],'shared_i')
            with core.store.transaction():
                stored=core.store.get(metadata['plan_id'],'shared_prefix_plan');plan=FrozenPrefixPlan._freeze(stored['payload']);worker_record=core.store.get(worker['request_id'],'provider_request')
            prefix_count=len(plan.prefix_messages);self.assertEqual(wires[0][1]['messages'][:prefix_count],wires[1][1]['messages'][:prefix_count]);self.assertEqual(wires[0][1]['tools'],wires[1][1]['tools']);self.assertEqual(worker_record['prefix_layout']['plan_id'],metadata['plan_id'])
            raw_common=next((i for i,(a,b) in enumerate(zip(wires[0][0],wires[1][0])) if a!=b),min(len(wires[0][0]),len(wires[1][0])));self.assertGreater(raw_common,len(plan.prefix_messages[0]['content'].encode('utf8')))
            published=op('cut.publish',{'job_id':job,'expected_cut':1,'idempotency_key':'shared-sdk-pub'});self.assertTrue(op('memory.replay',{'cb_id':published['cb_id']})['body_matches_published_cb'])
            source=op('chain.get')['pending'][0];job2=op('edit.prepare',{'source_id':source})['job_id'];fixture.editor_tool=True
            with self.assertRaises(CoreError) as denied:op('edit.run',{'job_id':job2,'editor':'editor-1'})
            self.assertEqual(denied.exception.status,403);self.assertEqual(len(wires),3)
            output=os.environ.get('SIC_SHARED_LAYOUT_EVIDENCE')
            if output:
                folder=Path(output).resolve();self.assertTrue(folder.is_relative_to(Path(__file__).resolve().parents[1]/'.local'));folder.mkdir(parents=True,exist_ok=True)
                for i,(raw,request) in enumerate(wires,1):(folder/('sdk-'+str(i)+'.request.json')).write_bytes(raw)
                (folder/'verification.json').write_text(json.dumps({'sdk':'actual LiteLLM/OpenAI client to owned ephemeral loopback','real_provider_calls':0,'sdk_requests':3,'actual_raw_common_prefix_bytes':raw_common,'actual_messages_prefix_equal':True,'actual_tools_equal':True,'plan_id_equal':True,'prefix_plan_digest':plan.digest,'prefix_bytes_sha256':metadata['prefix_bytes_sha256'],'editor_tool_early403':True,'published_replay_matches':True,'physical_cache_hit':'unknown','identity':'owned fixture only; gpt-4o is model counting profile, not an external model run'},indent=2))
        finally:
            if core:core.close()
            server.shutdown();server.server_close()
            for key,value in saved.items():
                if value is None:os.environ.pop(key,None)
                else:os.environ[key]=value

if __name__=='__main__':unittest.main()
