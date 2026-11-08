"""Real Core HTTP, official semantic LFX/Studio graphs; owned model only."""
import asyncio
import copy
import json
import os
from pathlib import Path
import sys
import tempfile

ROOT=Path(__file__).resolve().parents[2]
sys.path[:0]=[str(ROOT),str(ROOT/'integrations'),str(ROOT/'scripts'),str(ROOT/'tests')]
from lfx.load import load_flow_from_json
from lfx.schema import Data
from langgraph.checkpoint.memory import InMemorySaver
from langgraph.types import Command
from components.sic import SICHierarchy,SICEditorTask,SICCandidate,SICMemory
from generate_workbench_flows import make_workbench_flow,hierarchy_steps
from sic_workflow.domain_graphs import make_graph
from sic_workflow.hierarchy_forms import HierarchyForm,hierarchy_request,hierarchy_rows
from v2_experiment_fixture import OwnedFixture
from test_core_v2_hierarchy_legacy import LegacyHierarchyTests


def main():
    saved={k:os.environ.get(k) for k in ('SIC_CORE_URL','SIC_CORE_TOKEN','SIC_CORE_ACTOR')}
    try:
        with tempfile.TemporaryDirectory(prefix='sic-hier-ui-') as directory,OwnedFixture(Path(directory)/'core.db') as fixture:
            os.environ.update(SIC_CORE_URL=fixture.client.origin,SIC_CORE_TOKEN=fixture.client.token,SIC_CORE_ACTOR='host')
            client=fixture.client;client.call('chain.create',{'creators':[]});chain=client.context['chain_id']
            def node(cls,previous=None,**values):
                component=cls();component.set(chain_id=chain,**({'previous':Data(data=previous)} if previous else {}),**values);component._pre_run_setup();return component
            def hier(**values):
                component=node(SICHierarchy,**values);envelope=component.run_hierarchy().data
                assert envelope['ok'],envelope.get('error');assert len(component.records())>0
                return envelope
            def edit(source):
                task=node(SICEditorTask,source_id=source,action='准备单Editor',allow_write=True).run_task().data
                assert task['ok'],task
                run=node(SICEditorTask,previous=task,action='并发执行所有Editor',allow_write=True,allow_model=True).run_task().data
                assert run['ok'],run
                candidate=node(SICCandidate,previous=run,action='读取候选').run_candidate().data
                validate=node(SICCandidate,previous=candidate,action='校验候选',expected_revision=-1).run_candidate().data
                assert validate['ok'],validate
                publication=node(SICCandidate,previous=validate,action='发布此版本',expected_revision=-1,expected_cut=-1,allow_write=True).run_candidate().data
                assert publication['ok'],publication
                return publication['result']['cb_id']
            needle='ARCHIVE-needle-甲🙂-729'
            source=client.call('source.ingest',{'request':{'messages':[{'role':'user','content':'word '*40+needle}]}})['source_id']
            one=edit(source);frozen=copy.deepcopy(fixture.core.store.get(one,'cb'))
            flow=make_workbench_flow('actual higher hierarchy semantic acceptance',hierarchy_steps({'chain_id':chain},[one],2),connect=True)
            for nd in flow['data']['nodes']:
                fields=nd['data']['node']['template']
                assert fields.get('allow_write',{}).get('value',False) is False
                assert fields.get('allow_model',{}).get('value',False) is False
                if 'allow_write' in fields:fields['allow_write']['value']=True
                if nd['id']=='SICEditorTask-3':fields['allow_model']['value']=True
            result=asyncio.run(load_flow_from_json(flow).arun(inputs=[{}],outputs=['SICAccounting-12']))
            value=result[0].outputs[0].outputs['result'];value=value.get('message',value.get('data',value)) if isinstance(value,dict) else value.data
            assert value['ok'],value
            level2=hier(action='cb_timeline',level=2)['result']['rows'][0]['cb_id']
            formed=hier(action='form',level=3,cb_ids=level2,allow_write=True)
            three=edit(formed['result']['source_id'])
            hier(action='select',level=3,roles='worker\neditor',allow_write=True)
            for level in (1,2,3):
                wb=hier(action='wb_timeline',level=level)['result'];cb=hier(action='cb_timeline',level=level)['result']
                assert wb['rows'][0]['cb_id']==cb['rows'][0]['id']
                assert cb['rows'][0]['wb_id']==wb['rows'][0]['id']
            current=three;expanded=[]
            while True:
                body=hier(action='expand_cb',object_id=current)['result'];expanded.append(body['level'])
                if not body['members']:break
                current=body['members'][0]['cb_id']
            assert expanded==[3,2,1],expanded
            read=client.call('memory.page',{'kind':'source','object_id':source,'offset':0,'limit':8192})
            assert needle in read['value']
            initial=hier(action='replay',object_id=one,step=0)['result']
            complete=hier(action='replay',object_id=one,step=-1)['result']
            assert needle in initial['body'] and complete['body_matches_published_cb'] is True
            assert complete['external_effects_replayed'] is False and complete['cut_instant_reconstructed'] is False
            next_source=client.call('source.ingest',{'request':{'messages':[{'role':'user','content':'New task after third order selection'}]}})['source_id']
            edit(next_source)
            editor_request=json.loads(fixture.model_requests[-1]['request']['messages'][-1]['content'])
            assert three in [cb['cb_id'] for cb in editor_request['read_only_cb']]
            client.call('provider.continue',{'format':'openai-chat','idempotency_key':'hier-ui-worker-adoption'})
            assert three in json.dumps(fixture.model_requests[-1]['request'])
            assert needle not in json.dumps(fixture.model_requests[-1]['request'])
            hier(action='create_context',context_id='editor-notes',shared_contexts='main',allow_write=True)
            members=[]
            for text in ('Editor private first fact','Editor private second fact'):
                own=hier(action='ingest_context',context_id='editor-notes',working_text=text,allow_write=True)
                members.append(edit(own['result']['source_id']))
            two_private=edit(hier(action='form',context_id='editor-notes',level=2,cb_ids='\n'.join(members),allow_write=True)['result']['source_id'])
            three_private=edit(hier(action='form',context_id='editor-notes',level=3,cb_ids=two_private,allow_write=True)['result']['source_id'])
            hier(action='select',context_id='editor-notes',level=3,roles='editor:alpha',allow_write=True)
            hier(action='select',context_id='main',level=1,roles='editor:beta',allow_write=True)
            pending=client.call('chain.get')['pending'][0]
            task=node(SICEditorTask,source_id=pending,action='准备双Editor分工',editor_one_name='alpha',editor_two_name='beta',editor_one_units='u1',editor_two_units='',allow_write=True).run_task().data
            assert task['ok'],task
            both=node(SICEditorTask,previous=task,action='并发执行所有Editor',allow_write=True,allow_model=True)
            assert both.run_task().data['ok'];assert len(both.background())>=5
            last=[json.loads(c['request']['messages'][-1]['content']) for c in fixture.model_requests[-2:]]
            assert sum(three_private in [cb['cb_id'] for cb in req['read_only_cb']] for req in last)==1
            graph=make_graph('hierarchy-test',HierarchyForm,'hierarchy.get',hierarchy_request,InMemorySaver(),hierarchy_rows)
            cfg={'configurable':{'thread_id':'hierarchy-real-core'}}
            state=graph.invoke({'chain_id':chain,'form':{'action':'select','level':3,'roles':['worker'],'allow_write':True}},cfg)
            assert state.get('__interrupt__')
            state=graph.invoke(Command(resume=state['__interrupt__'][0].value['form']),cfg)
            assert state['result']['ok'] and state['result']['result']['sequence']>=1
            legacy_rows=hierarchy_rows({'chain_id':chain,'level':None,'wb_id':'legacy-wb','cb_id':'legacy-cb','hierarchy_index_status':'legacy-unindexed','wb_formation_order':'unknown','known_cb_order':{'membership_index':0},'members':[]})
            assert legacy_rows[0]['阶次'] is None and legacy_rows[0]['WB形成顺序']=='unknown'
            assert fixture.core.store.get(one,'cb')==frozen
            assert len(client.call('chain.list')['routes'])==1
            # Construct an actual old persisted format only in this owned test
            # database, then reach it through HTTP and official components.
            client.call('chain.create',{'creators':[]});chain=client.context['chain_id']
            old=LegacyHierarchyTests();old.core=fixture.core;old.context={**client.context,'allow_write':True,'allow_model':False}
            legacy=old.old('Legacy original detail retained 甲🙂')
            model_count=len(fixture.model_requests)
            directory=hier(action='legacy')['result']
            assert directory['legacy_unindexed_count']==1 and directory['wb_formation_order']=='unknown'
            expanded_legacy=hier(action='expand_cb',object_id=legacy['cb_id'])['result']
            assert expanded_legacy['level'] is None and expanded_legacy['wb_formation_order']=='unknown'
            legacy_replay=hier(action='replay',object_id=legacy['cb_id'])['result']
            assert legacy_replay['body_matches_published_cb'] is True
            assert legacy['wb_id'] not in [r['wb_id'] for r in hier(action='wb_timeline',level=1)['result']['rows']]
            projection=node(SICMemory,action='有限背景投影',max_chars=1024)
            assert projection.navigate().data['result']['manifest']['legacy_unindexed_count']==1
            assert any(r.get('WB形成顺序')=='unknown' for r in projection.records().to_dict('records'))
            assert len(fixture.model_requests)==model_count
            report={'status':'passed','official_lfx_connected_nodes':12,'real_levels':[1,2,3],'independent_wb_cb_timelines':True,
                    'recursive_original_read':True,'content_replay_matches':True,'worker_and_editor_actual_adoption':True,
                    'editor_private_third_order_and_alpha_beta_distinct_actual_inputs':True,'studio_sdk_interrupt_resume':True,
                    'legacy_real_persisted_http_ui_and_bounded_manifest':True,'published_lower_cb_unchanged':True,'main_agent_routes':1,'separate_owned_legacy_fixture_routes':1,
                    'owned_fixture_model_calls':len(fixture.model_requests),'real_provider_calls':0,'gui_verified':False}
            path=ROOT/'.local/integration-test-results/hierarchy-workbench-acceptance.json';path.write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))
    finally:
        for key,value in saved.items():
            if value is None:os.environ.pop(key,None)
            else:os.environ[key]=value


if __name__=='__main__':main()
