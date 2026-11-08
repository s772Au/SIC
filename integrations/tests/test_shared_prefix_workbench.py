"""Official workbench source/SDK + real Core HTTP; zero model invocations."""
import asyncio,copy,json,os,sys,tempfile,unittest
from pathlib import Path
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[2];sys.path[:0]=[str(ROOT),str(ROOT/'integrations'),str(ROOT/'scripts')]
from lfx.load import load_flow_from_json
from components.sic.configuration import SICConfiguration
from components.sic.memory import SICMemory
from components.sic.editor_task import SICEditorTask
from generate_workbench_flows import make_workbench_flow
from sic_workflow.domain_graphs import ConfigurationForm,configuration_request,make_graph
from langgraph.checkpoint.memory import InMemorySaver
from langgraph.types import Command
from sic_workflow.workbench_graph import memory_graph,MemoryInput
from sic_workflow.editor_views import task_view,rows
from sic_workflow.prefix_forms import prefix_patch
from v2_experiment_fixture import OwnedFixture

class SharedPrefixWorkbench(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.fixture=OwnedFixture(Path(self.tmp.name)/'core.db').__enter__();self.c=self.fixture.client;self.c.call('chain.create');self.chain=self.c.context['chain_id'];self.env=patch.dict(os.environ,{'SIC_CORE_URL':self.c.origin,'SIC_CORE_TOKEN':self.c.token,'SIC_CORE_ACTOR':'host'});self.env.start()
 def tearDown(self):self.env.stop();self.fixture.__exit__(None,None,None);self.tmp.cleanup()
 def component(self,cls,**values):
  node=cls();node.set(chain_id=self.chain,**values);node._pre_run_setup();return node
 def prepare(self):
  source=self.c.call('source.ingest',{'request':{'messages':[{'role':'user','content':'real HTTP new editable text'}]}})['source_id'];return self.c.call('edit.prepare',{'source_id':source})
 def test_lfx_configuration_real_api_new_job_and_safe_frozen_view(self):
  flow=make_workbench_flow('B opt-in source test',[{'component':'SICConfiguration','values':{'chain_id':self.chain,'action':'保存授权配置','replace_prefix_layout':True,'prefix_layout':'shared_i','shared_prefix_budget':4096}}]);template=flow['data']['nodes'][0]['data']['node']['template'];self.assertFalse(template['allow_write']['value']);template['allow_write']['value']=True
  result=asyncio.run(load_flow_from_json(flow).arun(inputs=[{}],outputs=['SICConfiguration-1']));value=result[0].outputs[0].outputs['result'];value=value.get('message',value.get('data',value)) if isinstance(value,dict) else value.data;self.assertTrue(value['ok'],value)
  current=self.c.call('config.get');self.assertEqual(current['configuration']['prefix_layout'],'shared_i');self.assertEqual(current['configuration']['shared_prefix_budget'],4096);job=self.prepare()['job'];meta=job['prefix_layout']['editor-1'];self.assertEqual(meta['applied_layout'],'shared_i');self.assertTrue(meta['plan_id']);self.assertFalse(job['attempts'])
  safe=task_view(job);table=rows(safe);self.assertTrue(table['prefix_rows']);self.assertIsNone(table['prefix_rows'][0]['actual_request_id']);self.assertFalse(table['prefix_rows'][0]['capability_editable']);self.assertTrue(table['alias_rows']);self.assertNotIn('snapshot_payload',json.dumps(safe));self.assertNotIn('api_key_env',json.dumps(safe));self.assertNotIn('api_base',json.dumps(safe));self.assertEqual(self.fixture.model_requests,[])
 def test_studio_safe_patch_real_config_and_no_hidden_capability_fields(self):
  form=ConfigurationForm(action='update',allow_write=True,replace_prefix_layout=True,prefix_layout='shared_i',shared_prefix_budget=2048,prefix_identity_mode='shared',shared_model=self.fixture.profile['model'],shared_provider_id='owned-test-logical-route',shared_cache_namespace='owned-test-namespace');operation,payload=configuration_request(form.model_dump());self.assertEqual(operation,'config.update');self.c.call(operation,payload);read=self.c.call('config.get');self.assertEqual(read['configuration']['shared_prefix_identity']['cache_namespace'],'owned-test-namespace')
  per_role={'worker':{'model':self.fixture.profile['model'],'provider_id':'logical-worker','cache_namespace':'worker-namespace'},'editor':{'model':self.fixture.profile['model'],'provider_id':'logical-editor','cache_namespace':'editor-namespace'}}
  op,per_payload=configuration_request(ConfigurationForm(action='update',replace_prefix_layout=True,prefix_layout='shared_i',prefix_identity_mode='per_role',shared_prefix_identity=per_role).model_dump());self.c.call(op,per_payload);self.assertEqual(self.c.call('config.get')['configuration']['shared_prefix_identity'],per_role)
  _,unrelated=configuration_request(ConfigurationForm(action='update').model_dump());self.assertNotIn('prefix_layout',unrelated);self.assertNotIn('shared_prefix_identity',unrelated)
  before=copy.deepcopy(self.c.call('config.get')['configuration'])
  with self.assertRaises(ValueError):ConfigurationForm(action='update',capability_id='cannot-set')
  with self.assertRaises(ValueError):prefix_patch({'replace_prefix_layout':True,'prefix_layout':'shared_i','prefix_identity_mode':'per_role','shared_prefix_identity':{'worker':{'model':'x','provider_id':'p','cache_namespace':'n','api_key_env':'not-allowed'}}})
  with self.assertRaises(ValueError):prefix_patch({'replace_prefix_layout':True,'prefix_layout':'shared_i','prefix_identity_mode':'shared','shared_model':'x','shared_provider_id':'http://user:password@example.test/','shared_cache_namespace':'n'})
  self.assertEqual(self.c.call('config.get')['configuration'],before);self.assertEqual(self.fixture.model_requests,[])
 def test_studio_configuration_sdk_interrupt_resume_uses_bound_real_route(self):
  graph=make_graph('B configuration SDK',ConfigurationForm,'config.get',configuration_request,InMemorySaver());config={'configurable':{'thread_id':'owned-B-config'}};form={'action':'update','replace_prefix_layout':True,'prefix_layout':'shared_i','shared_prefix_budget':3072}
  paused=graph.invoke({'chain_id':self.chain,'form':form},config);self.assertTrue(paused.get('__interrupt__'));self.assertEqual(self.c.call('config.get')['configuration']['prefix_layout'],'separate');reviewed=paused['__interrupt__'][0].value['form'];reviewed['allow_write']=True;done=graph.invoke(Command(resume=reviewed),config);self.assertEqual(done['status'],'applied');self.assertEqual(done['result']['operation'],'config.update');self.assertEqual(self.c.call('config.get')['configuration']['shared_prefix_budget'],3072);self.assertEqual(done['result']['context']['chain_id'],self.chain);self.assertEqual(self.fixture.model_requests,[])
 def test_actual_core_fallback_is_visible_and_never_claimed_shared_or_called(self):
  self.c.call('config.update',{'prefix_layout':'shared_i','shared_prefix_budget':4096});self.c.call('hierarchy.context.create',{'context_id':'private-notes'});source=self.c.call('hierarchy.context.ingest',{'context_id':'private-notes','request':{'messages':[{'role':'user','content':'private Editor reference'}]}})['source_id'];job=self.c.call('edit.program',{'source_id':source})['job_id'];self.c.call('cut.publish',{'job_id':job,'expected_cut':self.c.call('chain.get')['cut_seq'],'idempotency_key':'private-prefix-test'});self.c.call('hierarchy.select',{'context_id':'private-notes','level':1,'roles':['editor:editor-1']});prepared=self.prepare()['job'];view=task_view(prepared);row=rows(view)['prefix_rows'][0];self.assertEqual(row['metadata']['applied_layout'],'separate');self.assertFalse(row['metadata']['shared']);self.assertIn('mismatch',row['metadata']['reason']);self.assertIsNone(row['actual_request_id']);self.assertEqual(self.fixture.model_requests,[])
 def test_plan_and_request_paging_both_components_unknown_and_foreign_rejected(self):
  self.c.call('config.update',{'prefix_layout':'shared_i','shared_prefix_budget':4096});job=self.prepare()['job'];plan_id=job['prefix_layout']['editor-1']['plan_id']
  with self.fixture.core.store.transaction():request_id=self.fixture.core.store.put('provider_request',{'chain_id':self.chain,'role':'editor','request':{'messages':[{'role':'user','content':'synthetic immutable archive; not actual invocation'}]}})
  for kind,object_id,action,field in [('prefix_plan',plan_id,'读取冻结前缀计划','prefix_plan_id'),('provider_request',request_id,'读取实际模型请求','provider_request_id')]:
   whole='';offset=0
   while True:
    page=self.component(SICMemory,action=action,page_size=97,offset=offset,**{field:object_id}).navigate().data;self.assertTrue(page['ok'],page);whole+=page['result']['value'];offset=page['result']['next_offset']
    if offset is None:break
   self.assertIsInstance(json.loads(whole),dict)
   bad_lf=self.component(SICMemory,action=action,**{field:'unknown-object'}).navigate().data;self.assertFalse(bad_lf['ok'])
   flow=make_workbench_flow('B page typed source',[{'component':'SICMemory','values':{'chain_id':self.chain,'action':action,field:object_id,'page_size':512}}]);load_flow_from_json(flow)
   studio=memory_graph.invoke({'chain_id':self.chain,'action':kind,'object_id':object_id,'limit':97});self.assertEqual(studio['status'],'readonly');self.assertTrue(studio['page_text'])
   bad=memory_graph.invoke({'chain_id':self.chain,'action':kind,'object_id':'unknown-object','limit':97});self.assertEqual(bad['status'],'failed')
  foreign=self.c.call('chain.create')['chain_id'];self.c.context['chain_id']=foreign
  for kind,object_id in [('prefix_plan',plan_id),('provider_request',request_id)]:
   result=memory_graph.invoke({'chain_id':foreign,'action':kind,'object_id':object_id,'limit':97});self.assertEqual(result['status'],'failed');self.assertFalse(result['result']['ok']);self.assertNotIn('synthetic immutable archive',json.dumps(result))
  self.assertEqual(self.fixture.model_requests,[])
 def test_canonical_rows_never_label_translated_ops_as_raw_or_expose_host_snapshot(self):
  self.c.call('config.update',{'prefix_layout':'shared_i','shared_prefix_budget':4096});job=self.prepare()['job'];job['prefix_layout']['editor-1']['snapshot_payload']={'api_key_env':'FAKE_PRIVATE_ENV','capability':{'token':'FAKE_PRIVATE_TOKEN'}};job['responses']['editor-1']={'origin':'model','response_id':'raw-response-ref','canonical_response_id':'canonical-ref','alias_map_id':'map-ref','ops':[{'op':'KEEP','target':'u1'}],'pvi':{}};job['attempts']['editor-1']={'status':'completed','request_id':'actual-request-ref','prefix_layout':{'applied_layout':'shared_i','mode':'editor','prefix_bytes_sha256':'actual-prefix-sha','capability_payload':{'secret':'FAKE_PRIVATE_TOKEN'}}}
  view=task_view(job);r=rows(view);self.assertEqual(r['response_rows'][0]['raw_model_response_id'],'raw-response-ref');self.assertEqual(r['response_rows'][0]['canonical_response_id'],'canonical-ref');self.assertTrue(r['response_rows'][0]['original_response']['content_not_embedded_in_task_row']);self.assertEqual(r['prefix_rows'][0]['actual_request_id'],'actual-request-ref');self.assertNotIn('FAKE_PRIVATE',json.dumps(view));self.assertNotIn('snapshot_payload',json.dumps(view));self.assertNotIn('suffix',json.dumps(view))

if __name__=='__main__':unittest.main()
