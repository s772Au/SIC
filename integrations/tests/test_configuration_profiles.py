"""Named/N Editor profile edits through real Core HTTP and official LFX flow."""
import asyncio,copy,json,os,sys,tempfile,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2];sys.path[:0]=[str(ROOT),str(ROOT/'integrations'),str(ROOT/'scripts')]
from lfx.load import load_flow_from_json
from components.sic.configuration import SICConfiguration
from generate_workbench_flows import make_workbench_flow
from v2_experiment_fixture import OwnedFixture

class EditorProfileConfiguration(unittest.TestCase):
 def test_unicode_identity_has_same_core_and_studio_navigation_contract(self):
  from sic_core import Core
  from sic_workflow.workbench_graph import MemoryInput
  core=Core(config={'profiles':{'p':{'model':'openai/owned-delta','route_id':'delta'}}});context={'actor':'host','allow_write':True,'allow_model':False}
  def op(name,p=None):return core.operate(name,context,p or {})['result']
  try:
   context['chain_id']=op('chain.create')['chain_id'];op('config.update',{'editor_profiles':{'δ':'p'}})
   source=op('source.ingest',{'request':{'messages':[{'role':'user','content':'retained history'}]}})['source_id'];job=op('edit.program',{'source_id':source});op('cut.publish',{'job_id':job['job_id'],'expected_cut':0,'idempotency_key':'first'})
   op('hierarchy.select',{'level':1,'roles':['editor:δ']});source=op('source.ingest',{'request':{'messages':[{'role':'user','content':'new WB'}]}})['source_id'];job=op('edit.prepare',{'source_id':source,'assignments':[{'editor':'δ','unit_ids':['u1']}]})
   self.assertEqual(job['job']['model_bindings']['δ']['route_id'],'delta');self.assertTrue(core._editor_request(job['job'],'δ')['messages'])
   for role in ('worker','editor','editor:δ','editor:分析者','editor:alpha-beta','editor:🛠'):
    self.assertEqual(MemoryInput(chain_id=context['chain_id'],role=role).role,role)
   for role in ('editor:','editor: ','editor:alpha beta','editor:alpha\n','editor:\u202e','editor:editor:alpha','unknown'):
    with self.assertRaises(ValueError):MemoryInput(chain_id=context['chain_id'],role=role)
  finally:core.close()

 def test_arbitrary_names_merge_replace_clear_frozen_binding_and_negative_cases(self):
  saved={k:os.environ.get(k) for k in ['SIC_CORE_URL','SIC_CORE_TOKEN','SIC_CORE_ACTOR']}
  try:
   with tempfile.TemporaryDirectory() as d,OwnedFixture(Path(d)/'core.db') as own:
    os.environ.update(SIC_CORE_URL=own.client.origin,SIC_CORE_TOKEN=own.client.token,SIC_CORE_ACTOR='host')
    c=own.client;c.call('chain.create',{'creators':[]});chain=c.context['chain_id']
    for name in ['old','a','b','c','keep']:
     own.core.config['profiles'][name]={**copy.deepcopy(own.profile),'model':'openai/'+name+'-owned-profile','route_id':'route-'+name}
    c.call('config.update',{'editor_profiles':{'alpha':'old','untouched':'keep'}})
    source=c.call('source.ingest',{'request':{'messages':[{'role':'user','content':'first'},{'role':'user','content':'second'}]}})['source_id']
    old=c.call('edit.prepare',{'source_id':source,'assignments':[{'editor':'alpha','unit_ids':['u1','u2']}]})
    frozen=copy.deepcopy(old['job']['model_bindings'])
    flow=make_workbench_flow('owned arbitrary Editor mappings',[{'component':'SICConfiguration','values':{'chain_id':chain,'action':'保存授权配置','editor_one_name':'alpha','editor_one':'a','editor_two_name':'beta','editor_two':'b','editor_profiles_json':'{"gamma":"c"}'}}])
    template=flow['data']['nodes'][0]['data']['node']['template'];self.assertFalse(template['allow_write']['value']);self.assertTrue(template['editor_one_name']['show']);template['allow_write']['value']=True
    result=asyncio.run(load_flow_from_json(flow).arun(inputs=[{}],outputs=['SICConfiguration-1']))
    value=result[0].outputs[0].outputs['result'];value=value.get('message',value.get('data',value)) if isinstance(value,dict) else value.data
    self.assertTrue(value['ok'],value)
    state=c.call('chain.get');self.assertEqual(state['config']['editor_profiles'],{'alpha':'a','beta':'b','gamma':'c','untouched':'keep'});self.assertEqual(state['jobs'][old['job_id']]['model_bindings'],frozen)
    new=c.call('edit.prepare',{'source_id':source,'assignments':[{'editor':'alpha','unit_ids':['u1']},{'editor':'beta','unit_ids':['u2']}]})
    self.assertEqual(new['job']['model_bindings']['alpha']['route_id'],'route-a');self.assertEqual(new['job']['model_bindings']['beta']['route_id'],'route-b')
    def apply(**values):
     node=SICConfiguration();node.set(chain_id=chain,action='保存授权配置',allow_write=True,**values);node._pre_run_setup();return node.run_configuration().data
    for fields in [{'editor_one_name':'editor:alpha','editor_one':'a'},{'editor_one_name':' ','editor_one':'a'},{'editor_profiles_json':'{"alpha":"not-allowed"}'},{'editor_profiles_json':'{"alpha":"a","alpha":"b"}'},{'editor_profiles_json':'[]'}]:
     before=copy.deepcopy(c.call('chain.get')['config']);self.assertFalse(apply(**fields)['ok']);self.assertEqual(c.call('chain.get')['config'],before)
    self.assertTrue(apply(editor_mapping_mode='完整替换映射',editor_profiles_json='{"δ":"c"}',editor_one_name='ignored',editor_one='a')['ok']);self.assertEqual(c.call('chain.get')['config']['editor_profiles'],{'δ':'c'})
    self.assertTrue(apply(editor_mapping_mode='清空映射',editor_one_name='alpha',editor_one='a')['ok']);after=c.call('chain.get');self.assertEqual(after['config']['editor_profiles'],{});self.assertEqual(after['jobs'][old['job_id']]['model_bindings'],frozen)
    fresh=c.call('edit.prepare',{'source_id':source,'assignments':[{'editor':'alpha','unit_ids':['u1']}]});self.assertEqual(fresh['job']['model_bindings']['alpha'],own.profile)
    self.assertEqual(own.model_requests,[])
  finally:
   for key,value in saved.items():
    if value is None:os.environ.pop(key,None)
    else:os.environ[key]=value

if __name__=='__main__':unittest.main()
