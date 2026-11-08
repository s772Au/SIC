"""Execute new ledger/frontier forms on the owned persistent Langflow API.

Creates new inspectable flows and a synthetic program-only Core route. Does not
restart services, invoke models, or claim browser/Studio GUI acceptance.
"""
import argparse,json,sys,uuid
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
sys.path[:0]=[str(ROOT),str(ROOT/'integrations'),str(ROOT/'integrations/tests')]
from replay_runtime_binding import OfficialRuntimes,core_call
from generate_workbench_flows import make_workbench_flow
from sic_workflow.v2_client import CoreClient

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--peer-report',type=Path,required=True)
    p.add_argument('--output',type=Path,default=ROOT/'.local/integration-test-results/continuation-lf-live.json')
    args=p.parse_args();native=json.loads(args.peer_report.read_text(encoding='utf-8'))
    assert native['passed'],'Actual native verification must pass first'
    core=CoreClient();roles=core.capabilities().get('adapters',{}).get('roles',{})
    assert roles and all(r.get('model')=='openai/sic-offline-fixture' for r in roles.values()),'Only the owned fixture Core may receive this replay'
    runtime=OfficialRuntimes();nonce='continuation-'+uuid.uuid4().hex[:10]
    report={'status':'incomplete','gui_verified':False,'langgraph_service_modified':False,'model_calls':0,'flows':[]}
    try:
        root=native['root_chain']
        ledger_flow=make_workbench_flow('SIC task ledger '+nonce,[{'component':'SICAccounting','values':{'chain_id':root,'scope':'task','additional_chain_ids':'[]','prices_by_chain':'{}','task_interval':'[]'}}])
        fid=runtime.import_flow(ledger_flow);ledger=runtime.lf_run(fid,'SICAccounting-1')['result']
        direct=core_call(core,'ledger.export',{'chain_id':root},{'scope':'task'})['result']
        assert ledger['summary']==direct['summary']
        assert set(ledger['scope']['included_chain_ids'])=={native['root_chain'],native['peer_chain']}
        assert len(ledger['timing_by_chain'])==2 and not ledger['summary']['complete_cost']
        report['flows'].append({'kind':'actual native task ledger','flow_id':fid,'url':runtime.lf+'/flow/'+fid,'chain_id':root,'calls':len(ledger['calls']),'cost_complete':False})
        chain=core_call(core,'chain.create',{},write=True)['result']['chain_id']
        def op(name,payload=None):return core_call(core,name,{'chain_id':chain},payload or {},write=True)['result']
        op('config.update',{'memory_chars':65536,'editor_memory_chars':65536})
        def publish(sid):
            job=op('edit.program',{'source_id':sid})
            return op('cut.publish',{'job_id':job['job_id'],'expected_cut':job['job']['base_cut'],'idempotency_key':nonce+'-'+job['job_id']})['cb_id']
        def ingest(context,text):
            payload={'request':{'messages':[{'role':'user','content':text}]}}
            if context!='main':payload['context_id']=context
            return op('source.ingest' if context=='main' else 'hierarchy.context.ingest',payload)['source_id']
        def first_hierarchy(context):
            if context!='main':op('hierarchy.context.create',{'context_id':context,'shared_contexts':[]})
            cb=publish(ingest(context,nonce+' '+context+' material'))
            for level in (2,3):cb=publish(op('hierarchy.form',{'context_id':context,'level':level,'cb_ids':[cb]})['source_id'])
            return cb
        first_hierarchy('main');first_hierarchy('notes')
        op('hierarchy.select',{'level':3,'roles':['worker']})
        op('hierarchy.select',{'context_id':'notes','level':3,'roles':['editor:alpha']})
        lower=publish(ingest('main',nonce+' second main material'))
        second=publish(op('hierarchy.form',{'level':2,'cb_ids':[lower]})['source_id'])
        sid=op('hierarchy.form',{'level':3,'cb_ids':[second]})['source_id']
        job=op('edit.prepare',{'source_id':sid})
        op('edit.submit',{'job_id':job['job_id'],'editor':'editor-1','ops':[{'op':'REPLACE','target':'u1','content':nonce+' candidate higher representation'}]})
        following=ingest('main',nonce+' following WB')
        cut=op('chain.get')['cut_seq']
        for role in ('worker','editor'):
            values={'chain_id':chain,'action':'比较同次编辑窗口','job_id':job['job_id'],'expected_cut':cut,'comparison_role':role,'frontier_editor':'alpha','next_source_id':following,'next_all_units':True,'length_budget':100000}
            fid=runtime.import_flow(make_workbench_flow('SIC frontier '+role+' '+nonce,[{'component':'SICRuntime','values':values}]))
            result=runtime.lf_run(fid,'SICRuntime-1')['result']
            payload={'job_id':job['job_id'],'expected_cut':cut,'length_budget':100000,'position_budget':0}
            if role=='editor':payload.update(role='editor:alpha',next_source_id=following)
            expected=core_call(core,'frontier.compare',{'chain_id':chain},payload)['result']
            assert result==expected and result['edited_level']==3
            assert result['physical_cache_hit']=='unknown'
            report['flows'].append({'kind':'actual selected '+role+' frontier','flow_id':fid,'url':runtime.lf+'/flow/'+fid,'chain_id':chain,'edited_level':3,'role':result['role'],'candidate_affects_request':result['request_affected_by_candidate']})
        assert not op('usage.report')['calls'],'Program-only form replay unexpectedly called a model'
        unchanged=core_call(core,'ledger.export',{'chain_id':root},{'scope':'task'})['result']
        assert [c['call_id'] for c in unchanged['calls']]==[c['call_id'] for c in direct['calls']],'Reading task ledger created model calls'
        report.update(status='passed',new_forms_loaded_in_langflow=True,studio_new_forms_loaded=False,source='official persistent Langflow HTTP execution; not browser GUI')
    finally:
        runtime.close();report['trace']=runtime.trace
        args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({'status':report['status'],'flows':len(report['flows']),'model_calls':0,'gui_verified':False}))

if __name__=='__main__':main()
