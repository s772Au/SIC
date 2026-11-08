"""Independent read-only native fault evidence correlation. No services/models."""
import argparse,copy,hashlib,json,sqlite3,sys,tempfile
from pathlib import Path
from types import SimpleNamespace
ROOT=Path(__file__).resolve().parents[2];sys.path[:0]=[str(ROOT),str(ROOT/'integrations/tests')]
from sic_core import Core
from native_peer_faults import verify_case,check,records
from verify_dsh_harness import session_records

def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def verify_batch(folder):
    source=json.loads((folder/'matrix.json').read_text(encoding='utf-8'));check(source['passed'],'Producer batch did not pass');verified=[]
    for old in source['cases']:
        case=old['case'];directory=folder/case
        with tempfile.TemporaryDirectory(prefix='sic-native-readback-') as temp:
            original=sqlite3.connect((directory/'core.db').resolve().as_uri()+'?mode=ro',uri=True);path=Path(temp)/'copy.db';target=sqlite3.connect(path)
            try:original.backup(target)
            finally:original.close();target.close()
            core=Core(str(path))
            try:
                def tasks():
                    with core.store.transaction():return [json.loads(raw) for (raw,) in core.store.db.execute('SELECT state FROM host_tool_tasks')]
                owned=SimpleNamespace(core=core,tasks=tasks,complete_count=old['complete_transport_attempts'])
                current=verify_case(case,SimpleNamespace(home=folder/'home'),owned,directory/'native.jsonl')
                check(current['passed'] and len(current['peer_attempts'])==len(old['peer_attempts']),'Independent current task/native state differs')
                for row in current['peer_attempts']:
                    prior=next(item for item in old['peer_attempts'] if item['task_id']==row['task_id'])
                    check(row==prior,'Original peer native identity/body/receipt changed')
                if case=='explicit-failed-attempt-retry':
                    task=tasks()[0];child=core.store.state(task['chain_id']);request_id=child['collaboration_task']['request_id'];root=core.store.state(task['root_chain_id']);request=root['collaborations'][request_id]
                    check(request['status']=='completed' and request['branches'][0]['attempt']==1,'Explicit retry state differs')
                    check(len(tasks())==1 and task['status']=='failed','Original known failed native tool was rerun/rewritten')
                    check(any(v.get('type')=='final' and v.get('text')=='SIC_EXPLICIT_RETRY_CONSUMED' for v in records(directory/'retry-consumed-native.jsonl')),'Requester final consumption missing')
                    native,_=session_records(folder/'home',old['parent_session']);projections=[v['data'] for v in native if v.get('type')=='plugin:sic/provider-projection']
                    check(any('SIC_PEER_RECOVERED_KNOWN_FAILURE' in json.dumps(p['request']) for event in projections for p in event['projections']),'Retry result not present in actual requester input')
                verified.append({'case':case,'passed':True,'native_tools':len(current['peer_attempts']),'core_request_response_matches_native_binding':True,'parent_next_worker_consumes_actual_outcome':True,'actual_native_body_and_cancellation_checked':True,'producer_verification_sha256':digest(directory/'verification.json'),'native_cli_sha256':digest(directory/'native.jsonl'),'native_peer_digests':[row['native_record_digest'] for row in current['peer_attempts']],'unknown_resume_after_core_restart':old.get('native_resume_after_core_restart',False),'unknown_redispatches':old.get('unknown_attempt_redispatches',0)})
            finally:core.close()
    return {'batch_reference':folder.name,'matrix_sha256':digest(folder/'matrix.json'),'cases':verified}

def main():
    p=argparse.ArgumentParser();p.add_argument('--batches',type=Path,nargs='+',required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    reports=[verify_batch(folder.resolve()) for folder in a.batches];cases=[row for batch in reports for row in batch['cases']]
    check(len({row['case'] for row in cases})==len(cases),'Duplicate scenarios must be reviewed explicitly, not added to a pass count')
    report={'schema':'sic-native-fault-independent-correlation-v1','passed':True,'real_harness_scenarios':len(cases),'real_native_peer_attempts':sum(row['native_tools'] for row in cases),'batches':reports,'database_access':'read-only original SQLite backup to disposable copy','original_reports_modified':False,'inference_calls':0,'real_provider_calls':0,'existing_services_touched':False,'scope':'Only listed native fault paths, not every SIC requirement or all native tools','verifier_source_sha256':digest(Path(__file__))}
    a.output.parent.mkdir(parents=True,exist_ok=True);a.output.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8');print(json.dumps({'passed':True,'scenarios':len(cases),'native_peer_attempts':report['real_native_peer_attempts'],'inference_calls':0}))

if __name__=='__main__':main()
