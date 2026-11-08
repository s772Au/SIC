"""Isolated official LFX/Studio task accounting; no provider or running UI use."""
import asyncio,json,os,sys,tempfile,hashlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2];sys.path[:0]=[str(ROOT),str(ROOT/'integrations'),str(ROOT/'scripts')]
from lfx.load import load_flow_from_json
from components.sic.accounting import SICAccounting
from generate_workbench_flows import make_workbench_flow
from sic_workflow.accounting_graph import graph,AccountingInput,builder
from langgraph.checkpoint.memory import InMemorySaver
from langgraph.types import Command
from sic_workflow.v2_client import CoreClient
from v2_experiment_fixture import OwnedFixture

def main():
    # Studio requests both schemas before rendering; invoke-only checks miss a
    # mixed root/named state that executes but cannot expose a valid schema.
    ingress=graph.get_input_jsonschema()
    assert ingress['properties']['__root__']['$ref'].endswith('/AccountingInput')
    assert {'scope','additional_chain_ids','prices_by_chain','task_interval'} <= set(ingress['$defs']['AccountingInput']['properties'])
    assert graph.get_output_jsonschema()['type']=='object'
    graph.get_output_schema().model_json_schema()
    saved={key:os.environ.get(key) for key in ('SIC_CORE_URL','SIC_CORE_TOKEN','SIC_CORE_ACTOR')}
    try:
        with tempfile.TemporaryDirectory(prefix='sic-ledger-ui-') as directory,OwnedFixture(Path(directory)/'core.db') as fixture:
            os.environ.update(SIC_CORE_URL=fixture.client.origin,SIC_CORE_TOKEN=fixture.client.token,SIC_CORE_ACTOR='host')
            client=fixture.client;client.call('chain.create');root=client.context['chain_id']
            source=client.call('source.ingest',{'request':{'messages':[{'role':'user','content':'owned ledger fixture'}]}})['source_id']
            job=client.call('edit.program',{'source_id':source})['job_id'];client.call('cut.publish',{'job_id':job,'expected_cut':0,'idempotency_key':'first'})
            cp=client.call('chain.checkpoint')['checkpoint_id'];socket=client.call('socket.create',{'checkpoint_id':cp,'spawn_acl':['host']})['socket_id']
            peer=client.call('collaboration.dispatch',{'socket_ids':[socket],'request_id':'owned-ledger-peer','question':'owned pending task'})['branches'][0]['chain_id']
            api=CoreClient()
            def op(chain,name,p):
                envelope=api.execute(name,{'chain_id':chain},p,allow_write=True);assert envelope['ok'],envelope.get('error');return envelope['result']
            for chain,role in [(root,'editor'),(peer,'worker')]:
                op(chain,'usage.record',{'call_id':'owned-'+role,'role':role,'level':3 if role=='editor' else 1,'model':'owned-model','route':'owned-route','categories':[{'category':'input_uncached','quantity':10,'unit':'token'}],'started_at':10,'ended_at':12,'raw_usage':{'owned_fixture':True}})
            # Actual recorded events on peer B overlap Editor A, not A's own tool.
            with fixture.core.store.transaction():
                state=fixture.core.store.state(peer);state['events'].extend([{'type':'tool_start','call_id':'owned-B','time':10},{'type':'tool_end','call_id':'owned-B','time':12}]);fixture.core.store.save(state)
            rates={chain:[{'category':'input_uncached','unit':'token','unit_price':'0.1','currency':'TEST','price_version':'owned-v1'}] for chain in (root,peer)}
            component=SICAccounting();component.set(chain_id=root,scope='task',additional_chain_ids='[]',prices_by_chain=json.dumps(rates),task_interval='[0,13]');component._pre_run_setup()
            answer=component.run_accounting().data;assert answer['ok'],answer.get('error');body=answer['result']
            assert set(body['scope']['included_chain_ids'])=={root,peer};assert body['summary']['total_cost'] is None
            assert len(component.charges())==2 and len(component.scope_details())>2 and len(component.unknowns())>0 and len(component.timing_details())>0
            studio=graph.invoke({'chain_id':root,'scope':'task','prices_by_chain':rates,'task_interval':[0,13]})
            assert studio['result']['ok'];assert studio['result']['result']['summary']==body['summary']
            assert studio['scope_rows'] and studio['price_rows'] and studio['context_rows'] and studio['timing_rows']
            assert body['summary']['timing']['editor_tool_overlap_seconds']==2
            assert all(item['editor_tool_overlap_seconds']==0 for item in body['timing_by_chain'].values())
            assert any(row['timing_scope']=='same-chain' and row['metric']=='editor_tool_overlap_seconds' and row['value']==0 for row in studio['timing_rows'])
            for alias,value in [('additional_chains',[peer]),('price_by_chain',rates),('task_window',[0,13])]:
                try:graph.invoke({'chain_id':root,'scope':'task',alias:value})
                except ValueError:pass
                else:raise AssertionError('Official graph silently dropped alias '+alias)
            try:SICAccounting().set(additional_chains='[]')
            except (KeyError,ValueError):pass
            else:raise AssertionError('Official component accepted an unknown scope alias')
            try:graph.invoke({'chain_id':root,'scope':'task','prices':[rates[root][0]]})
            except ValueError:pass
            else:raise AssertionError('Global task price override accepted')
            steps=[{'component':'SICAccounting','values':{'chain_id':root,'scope':'task','additional_chain_ids':'[]','prices_by_chain':json.dumps(rates),'task_interval':'[0,13]'}}]
            exported=make_workbench_flow('Owned task ledger SDK acceptance',steps)
            template=exported['data']['nodes'][0]['data']['node']['template']
            assert not template['allow_write']['value'];assert template['scope']['value']=='task'
            loaded=load_flow_from_json(exported);outputs=asyncio.run(loaded.arun(inputs=[{}],outputs=['SICAccounting-1']))
            raw=outputs[0].outputs[0].outputs['result'];value=raw.get('message',raw.get('data',raw)) if isinstance(raw,dict) else raw.data
            assert value['ok'] and value['result']['scope']['kind']=='task'
            chain_component=SICAccounting();chain_component.set(chain_id=root,scope='chain');chain_component._pre_run_setup()
            assert chain_component.run_accounting().data['result']['scope']['kind']=='chain'
            # Keep the existing native Studio confirmation boundary for writes.
            writable=builder.compile(checkpointer=InMemorySaver());config={'configurable':{'thread_id':'owned-ledger-resource'}}
            paused=writable.invoke({'chain_id':root,'action':'record_resource','record_id':'owned-cpu','resource':'program','quantity':'1','unit':'second','measurement_method':'owned fixture input'},config)
            assert paused.get('__interrupt__')
            resumed=writable.invoke(Command(resume={'confirm':True,'allow_write':True}),config)
            assert resumed['result']['ok']
            assert len(fixture.model_requests)==0,'No model traffic in ledger workbench'
            report={'status':'passed','official_lfx_component_and_loaded_flow':True,'official_studio_compiled_graph':True,'task_scope_recursive_peers':True,'strict_alias_rejection':True,'cross_route_overlap_not_same_agent_window':True,'missing_measurements_unknown':True,'model_calls':0,'real_provider_calls':0,'gui_visual_verified':False,'running_workbenches_restarted':False}
            files=['integrations/components/sic/accounting.py','integrations/sic_workflow/accounting_graph.py','integrations/sic_workflow/accounting_scope.py','integrations/flows/workbench-accounting.json','sic_core/task_ledger.py']
            report['source_sha256']={name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in files}
            path=ROOT/'.local/integration-test-results/task-ledger-workbench-acceptance.json';path.parent.mkdir(parents=True,exist_ok=True);path.write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))
    finally:
        for key,value in saved.items():
            if value is None:os.environ.pop(key,None)
            else:os.environ[key]=value

if __name__=='__main__':main()
