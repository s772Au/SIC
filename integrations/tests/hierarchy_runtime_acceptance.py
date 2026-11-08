"""Replay semantic hierarchy operations through two owned official runtimes.
Creates a new route and owned fixture requests only. Does not restart services.
Reports API execution separately from GUI acceptance.
"""
import json
import sqlite3
import sys
import uuid
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
sys.path[:0]=[str(ROOT),str(ROOT/'integrations'),str(ROOT/'integrations/tests')]
from replay_runtime_binding import OfficialRuntimes,core_call
from generate_workbench_flows import hierarchy_steps,make_workbench_flow
from sic_workflow.v2_client import CoreClient


def main():
    core=CoreClient();caps=core.capabilities()
    roles=caps.get('adapters',{}).get('roles',{})
    assert roles and all(r.get('model')=='openai/sic-offline-fixture' for r in roles.values()),'Only owned loopback fixture permitted'
    names={x['operation'] for x in caps['operations']}
    assert {'hierarchy.legacy','memory.replay','hierarchy.context.create','hierarchy.maintain'}<=names
    runtime=OfficialRuntimes();nonce='hier-runtime-'+uuid.uuid4().hex[:10]
    ctx=core_call(core,'chain.create',{}, {'creators':[]},write=True)['context'];chain=ctx['chain_id']
    def call(op,p=None,write=False,model=False):return core_call(core,op,{'chain_id':chain},p,write,model)['result']
    def graph(name,form):
        thread,value=runtime.lg_run(name,{'chain_id':chain,'form':form})
        if value.get('__interrupt__'):
            _,value=runtime.lg_run(name,None,thread,value['__interrupt__'][0]['value']['form'])
        assert value['result']['ok'],value['result'].get('error')
        return value['result']['result'],thread
    def h(**f):return graph('sic_hierarchy',f)
    def publish(source):
        job=call('edit.program',{'source_id':source,'rules':['deduplicate_exact']},True)
        return call('cut.publish',{'job_id':job['job_id'],'expected_cut':job['job']['base_cut'],'idempotency_key':nonce+'-'+source},True)['cb_id']
    try:
        needle=nonce+' original Unicode detail 甲🙂'
        source=call('source.ingest',{'request':{'messages':[{'role':'user','content':needle}]}},True)['source_id']
        one=publish(source)
        original=call('hierarchy.expand',{'cb_id':one})
        secondsource=call('source.ingest',{'request':{'messages':[{'role':'user','content':nonce+' second ordered member'}]}},True)['source_id']
        second=publish(secondsource)
        flow=make_workbench_flow(nonce+' ordered higher WB edit publish',hierarchy_steps({'chain_id':chain},[one,second],2),connect=True)
        for n in flow['data']['nodes']:
            fields=n['data']['node']['template']
            assert fields.get('allow_write',{}).get('value',False) is False
            assert fields.get('allow_model',{}).get('value',False) is False
            if 'allow_write' in fields:fields['allow_write']['value']=True
            if n['id']=='SICEditorTask-3':fields['allow_model']['value']=True
        flowid=runtime.import_flow(flow);runtime.lf_run(flowid,'SICAccounting-12')
        two=call('hierarchy.timeline',{'level':2,'kind':'cb'})['rows'][0]['cb_id']
        assert [m['cb_id'] for m in call('hierarchy.expand',{'cb_id':two})['members']]==[one,second]
        formed,formthread=h(action='form',level=3,cb_ids=[two],allow_write=True)
        prepared,_=graph('sic_editor',{'action':'prepare','source_id':formed['source_id'],'allow_write':True})
        job=prepared['job_id'];original_response=None
        graph('sic_editor',{'action':'run_all','job_id':job,'allow_write':True,'allow_model':True})
        original_response=call('chain.get')['jobs'][job]['responses']
        candidate={'action':'revise','edit_mode':'pvi_only','persistent':[nonce+' durable higher note'],'volatile':[], 'instant':{'higher_note':nonce},'expected_revision':0,'allow_write':True}
        thread,paused=runtime.lg_run('sic_candidate',{'chain_id':chain,'job_id':job,'form':candidate})
        assert paused.get('__interrupt__')
        _,applied=runtime.lg_run('sic_candidate',None,thread,paused['__interrupt__'][0]['value']['form']);assert applied['result']['ok'],applied['result']
        for action in ['validate','publish']:
            _,paused=runtime.lg_run('sic_candidate',{'chain_id':chain,'job_id':job,'form':{'action':action,'expected_revision':1,'expected_cut':-1,'allow_write':True}},thread)
            assert paused.get('__interrupt__')
            _,applied=runtime.lg_run('sic_candidate',None,thread,paused['__interrupt__'][0]['value']['form']);assert applied['result']['ok'],applied['result']
        three=applied['result']['result']['cb_id']
        h(action='select',level=3,roles=['worker','editor'],allow_write=True)
        levels=[];current=three
        while True:
            expanded,_=h(action='expand_cb',object_id=current);levels.append(expanded['level'])
            if expanded['level']==1:break
            current=expanded['members'][0]['cb_id']
        assert levels==[3,2,1]
        replay,_=h(action='replay',object_id=three);assert replay['body_matches_published_cb'] and replay['external_effects_replayed'] is False
        assert needle in call('memory.page',{'kind':'source','object_id':source,'offset':0,'limit':8192})['value']
        assert call('hierarchy.expand',{'cb_id':one})==original
        assert call('chain.get')['jobs'][job]['responses']==original_response
        # Editor-private actual third-order program products and identity selection.
        h(action='create_context',context_id='editor-notes',allow_write=True)
        for i in range(4):
            private,_=h(action='ingest_context',context_id='editor-notes',working_text=nonce+' private '+str(i),allow_write=True)
            publish(private['source_id'])
        h(action='maintain',context_id='editor-notes',fanout=2,roles=['editor:alpha'],allow_write=True)
        h(action='select',context_id='editor-notes',level=3,roles=['editor:alpha'],allow_write=True)
        h(action='select',context_id='main',level=1,roles=['editor:beta'],allow_write=True)
        private_high=call('hierarchy.timeline',{'context_id':'editor-notes','level':3,'kind':'cb'})['rows'][0]['cb_id']
        nextsource=call('source.ingest',{'request':{'messages':[{'role':'user','content':nonce+' actual role check'}]}},True)['source_id']
        prepared,_=graph('sic_editor',{'action':'prepare','source_id':nextsource,'two_editors':True,'editor_one_name':'alpha','editor_two_name':'beta','editor_one_units':['u1'],'editor_two_units':[],'allow_write':True})
        graph('sic_editor',{'action':'run_all','job_id':prepared['job_id'],'allow_write':True,'allow_model':True})
        call('cut.publish',{'job_id':prepared['job_id'],'expected_cut':prepared['job']['base_cut'],'idempotency_key':nonce+'-role-check'},True)
        worker=call('provider.continue',{'format':'openai-chat','idempotency_key':nonce+'-worker'},True,True)
        state=call('chain.get');jobstate=state['jobs'][prepared['job_id']]
        db=sqlite3.connect('file:'+str(ROOT/'.local/core-ui-demo.db')+'?mode=ro',uri=True)
        def request(rid):
            row=db.execute("SELECT data FROM objects WHERE id=? AND kind='provider_request'",(rid,)).fetchone()
            assert row,'Actual provider request archive missing'
            return json.loads(row[0])
        requests={editor:request(attempt['request_id']) for editor,attempt in jobstate['attempts'].items()}
        assert private_high in json.dumps(requests['alpha']) and private_high not in json.dumps(requests['beta'])
        assert three in json.dumps(request(worker['request_id']))
        db.close()
        config,_=graph('sic_runtime',{'action':'configure','enabled':False,'max_pending_seconds':0.5,'allow_write':True})
        assert '0.5' in json.dumps(config)
        report={'status':'passed','chain_id':chain,'langflow_flow_id':flowid,'langgraph_hierarchy_thread':formthread,'langgraph_candidate_thread':thread,'official_lfx_connected_nodes':12,'real_levels':levels,'same_core':core.base_url,'original_lower_cb_unchanged':True,'candidate_original_response_unchanged':True,'editor_private_third_order_and_alpha_beta_actual_inputs':True,'worker_actual_third_order_input':True,'runtime_max_pending_seconds':0.5,'model_calls':len(call('ledger.export')['calls']),'real_provider_calls':0,'langflow_gui_verified':False,'studio_gui_verified':False,'trace':runtime.trace}
        path=ROOT/'.local/integration-test-results/hierarchy-runtime-acceptance.json';path.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8');print(json.dumps({k:v for k,v in report.items() if k!='trace'}))
    except BaseException:
        evidence={'status':'failed','chain_id':chain,'flows':runtime.flows,'threads':runtime.threads,'trace':runtime.trace,'gui_verified':False}
        (ROOT/'.local/integration-test-results/hierarchy-runtime-last-failure.json').write_text(json.dumps(evidence,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
        raise
    finally:runtime.close()

if __name__=='__main__':main()
