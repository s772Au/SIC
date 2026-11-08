"""Real official LFX and LangGraph mappings against an owned ephemeral Core HTTP server."""
import asyncio
import copy
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[2]
sys.path[:0]=[str(ROOT),str(ROOT/'integrations'),str(ROOT/'scripts')]
from pydantic import ValidationError
from lfx.custom.utils import build_custom_component_template
from langgraph.checkpoint.memory import InMemorySaver
from components.sic.runtime import SICRuntime
from sic_workflow.domain_graphs import RuntimeForm,runtime_request,make_graph
from sic_workflow.frontier_forms import frontier_rows
from v2_experiment_fixture import OwnedFixture


class FrontierFormTests(unittest.TestCase):
    def test_worker_default_preserves_original_payload_and_ignores_hidden_editor_values(self):
        form=RuntimeForm(action='frontier',job_id='job',expected_cut=7,comparison_role='worker',frontier_editor='',next_source_id='ignored',next_unit_ids=['hidden']).model_dump()
        self.assertEqual(runtime_request(form),('frontier.compare',{'job_id':'job','expected_cut':7,'position_budget':0,'length_budget':4096}))
    def test_editor_full_and_explicit_empty_scope_are_distinct(self):
        for units in (None,[],['u2','u1']):
            form=RuntimeForm(action='frontier',comparison_role='editor',frontier_editor='alpha',next_source_id='next',next_unit_ids=units).model_dump()
            payload=runtime_request(form)[1]
            self.assertEqual(payload['role'],'editor:alpha');self.assertEqual(payload['next_source_id'],'next')
            if units is None:self.assertNotIn('next_unit_ids',payload)
            else:self.assertEqual(payload['next_unit_ids'],units)
    def test_editor_identity_source_and_units_fail_before_dispatch(self):
        for values in ({'frontier_editor':''},{'next_source_id':''},{'next_unit_ids':['u1','u1']},{'next_unit_ids':['']}):
            with self.assertRaises(ValidationError):RuntimeForm(action='frontier',comparison_role='editor',**({'frontier_editor':'alpha','next_source_id':'next'}|values))
        with self.assertRaises(ValidationError):RuntimeForm(action='frontier',comparison_role='unknown')
        with self.assertRaises(ValidationError):RuntimeForm(action='frontier',payload={'operation':'cut.publish'})
    def test_lfx_build_config_reveals_only_explicit_comparison_scope(self):
        component=SICRuntime();template,_=build_custom_component_template(component);config=template['template']
        config=component.update_build_config(config,'比较同次编辑窗口','action');config['action']['value']='比较同次编辑窗口'
        self.assertTrue(config['comparison_role']['show']);self.assertFalse(config['next_source_id']['show'])
        config=component.update_build_config(config,'editor','comparison_role');config['comparison_role']['value']='editor'
        self.assertTrue(config['frontier_editor']['show']);self.assertTrue(config['next_source_id']['show']);self.assertFalse(config['next_unit_ids']['show'])
        config=component.update_build_config(config,False,'next_all_units')
        self.assertTrue(config['next_unit_ids']['show'])
        self.assertFalse(any(key in config for key in ('payload','request_json','operation')))
        component.set(action='比较同次编辑窗口',comparison_role='editor',frontier_editor='',next_source_id='next');component._pre_run_setup()
        result=component.run_runtime().data
        self.assertFalse(result['ok']);self.assertEqual(result['error']['code'],'frontier_scope_invalid')

    def test_generated_runtime_flow_contains_new_typed_fields_and_safe_defaults(self):
        flow=json.loads((ROOT/'integrations/flows/workbench-runtime.json').read_text(encoding='utf-8'))
        nodes=flow['data']['nodes'];self.assertEqual(len(nodes),1)
        fields=nodes[0]['data']['node']['template']
        for key in ('comparison_role','frontier_editor','next_source_id','next_all_units','next_unit_ids'):
            self.assertIn(key,fields)
        self.assertEqual(fields['comparison_role']['value'],'worker')
        self.assertFalse(fields['allow_write']['value']);self.assertFalse(fields['allow_model']['value'])
        self.assertTrue(fields['next_all_units']['value'])


class FrontierOfficialSDKTests(unittest.TestCase):
    def test_real_lfx_build_and_studio_graph_preserve_same_core_comparison(self):
        saved={key:os.environ.get(key) for key in ('SIC_CORE_URL','SIC_CORE_TOKEN','SIC_CORE_ACTOR')}
        captures=[]
        try:
            with tempfile.TemporaryDirectory(prefix='sic-frontier-ui-') as directory,OwnedFixture(Path(directory)/'core.db') as fixture:
                os.environ.update(SIC_CORE_URL=fixture.client.origin,SIC_CORE_TOKEN=fixture.client.token,SIC_CORE_ACTOR='host')
                client=fixture.client;chain=client.call('chain.create')['chain_id']
                client.call('config.update',{'memory_chars':65536,'editor_memory_chars':65536})
                def publish(sid):
                    job=client.call('edit.program',{'source_id':sid})['job_id']
                    return client.call('cut.publish',{'job_id':job,'expected_cut':client.call('chain.get')['cut_seq'],'idempotency_key':job})['cb_id']
                def hierarchy(context='main'):
                    if context!='main':client.call('hierarchy.context.create',{'context_id':context,'shared_contexts':[]})
                    sid=client.call('source.ingest' if context=='main' else 'hierarchy.context.ingest',{'context_id':context,'request':{'messages':[{'role':'user','content':'owned '+context+' retained material'}]}})['source_id']
                    member=publish(sid)
                    for level in (2,3):member=publish(client.call('hierarchy.form',{'level':level,'context_id':context,'cb_ids':[member]})['source_id'])
                    return member
                main=hierarchy();private=hierarchy('private')
                client.call('hierarchy.select',{'level':3,'roles':['worker']})
                client.call('hierarchy.select',{'level':3,'context_id':'private','roles':['editor:alpha']})
                sid=client.call('source.ingest',{'request':{'messages':[{'role':'user','content':'candidate source'}]}})['source_id']
                job=client.call('edit.prepare',{'source_id':sid})['job_id']
                client.call('edit.submit',{'job_id':job,'editor':'editor-1','ops':[{'op':'REPLACE','target':'u1','content':'candidate edited'}]})
                next_sid=client.call('source.ingest',{'request':{'messages':[{'role':'user','content':'next first'},{'role':'user','content':'next second'}]}})['source_id']
                cut=client.call('chain.get')['cut_seq']
                for selected,units in [('worker',None),('editor',['u2']),('editor',[])]:
                    component=SICRuntime();component.set(chain_id=chain,action='比较同次编辑窗口',job_id=job,expected_cut=cut,
                         comparison_role=selected,frontier_editor='alpha',next_source_id=next_sid,next_all_units=units is None,next_unit_ids='\n'.join(units or []),length_budget=100000)
                    # Execute the official component output builder, including all grouped outputs.
                    results,artifacts=asyncio.run(component.build_results())
                    lfx=results['result'].data;self.assertTrue(lfx['ok'],lfx)
                    window=results['window'];self.assertGreater(len(window),20)
                    form={'action':'frontier','job_id':job,'expected_cut':cut,'comparison_role':selected,'frontier_editor':'alpha','next_source_id':next_sid,'next_unit_ids':units,'length_budget':100000}
                    graph=make_graph('frontier_sdk',RuntimeForm,'runtime.get',runtime_request,InMemorySaver(),presenter=frontier_rows)
                    studio=graph.invoke({'chain_id':chain,'form':form},{'configurable':{'thread_id':'frontier-'+selected+'-'+str(units)}})
                    self.assertEqual(studio['status'],'applied');self.assertTrue(studio['result']['ok'],studio)
                    self.assertEqual(lfx['result'],studio['result']['result'])
                    self.assertEqual(studio['records'],frontier_rows(lfx['result']))
                    self.assertEqual(lfx['result']['physical_cache_hit'],'unknown')
                    self.assertIn('comparison_ids',lfx['result']);self.assertEqual(lfx['result']['edited_level'],1)
                    if selected=='editor':
                        payload=json.loads(lfx['result']['additional_request']['messages'][-1]['content'])
                        self.assertEqual([cb['cb_id'] for cb in payload['read_only_cb']],[private])
                        self.assertEqual([unit['unit_id'] for unit in payload['editable_text_units']],units)
                    else:self.assertIn(main,json.dumps(lfx['result']['additional_request']))
                    captures.append({'form':form,'lfx':lfx,'studio':studio,'lfx_artifacts':artifacts})
                self.assertEqual(client.call('chain.get')['cut_seq'],cut)
                # A valid form with a nonexistent target fails in Core, preserving current state.
                bad=RuntimeForm(action='frontier',job_id=job,expected_cut=cut,comparison_role='editor',frontier_editor='alpha',next_source_id='not-owned').model_dump()
                graph=make_graph('bad_frontier',RuntimeForm,'runtime.get',runtime_request,InMemorySaver(),presenter=frontier_rows)
                failure=graph.invoke({'chain_id':chain,'form':bad},{'configurable':{'thread_id':'bad'}})
                self.assertFalse(failure['result']['ok']);self.assertEqual(failure['status'],'failed');self.assertEqual(failure['records'],[])
                captures.append({'negative':failure})
                report={'status':'passed','official_lfx_output_builder':True,'official_langgraph_sdk':True,'browser_gui_loaded':False,'existing_services_restarted':False,'external_model_calls':0,'owned_fixture_model_requests':len(fixture.model_requests),'captures':captures,'core_http_trace':client.trace}
                dest=ROOT/'.local/contract-rebuild/frontier-forms-sdk.json';dest.write_text(json.dumps(report,ensure_ascii=False,indent=2,default=str),encoding='utf-8')
        finally:
            for key,value in saved.items():
                if value is None:os.environ.pop(key,None)
                else:os.environ[key]=value

if __name__=='__main__':unittest.main()
