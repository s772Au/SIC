"""Read-only correlation of a real DSH peer tool roundtrip. No inference calls."""
import argparse,json,sqlite3,sys
from pathlib import Path
from verify_dsh_harness import check,read_jsonl,session_records,sha

def verify(log,home,db):
    cli=read_jsonl(log)
    sessions={v['sessionId'] for v in cli if v.get('type')=='session'}
    check(len(sessions)==1,'one explicit parent session required')
    parent=sessions.pop()
    check(any(v.get('type')=='final' and v.get('text')=='SIC_DSH_PEER_ROUNDTRIP_OK' for v in cli),'parent final marker missing')
    native,_=session_records(home,parent)
    tasks=[json.loads(r[0]) for r in db.execute('SELECT state FROM host_tool_tasks')]
    tasks=[t for t in tasks if t['session_key']==parent]
    check(len(tasks)==1,'expected exactly one host execution attempt')
    task=tasks[0];peer=task['peer_session_id']
    check(peer!=parent and task['chain_id']!=task['root_chain_id'],'peer identity collapsed into parent')
    check(task['status']=='completed' and task.get('dispatched_at'),'actual host task not completed/dispatched')
    records,_=session_records(home,peer)
    events=lambda name:[v['data'] for v in records if v.get('type')=='plugin:sic/'+name]
    binding=events('peer-binding');check(len(binding)==1,'peer native binding missing')
    check(binding[0]['root_session']==parent and binding[0]['task']['task_id']==task['task_id'],'native peer route differs')
    dispatch=events('peer-dispatch');body=events('fixture-body')
    check(len(dispatch)==1 and len(body)==2,'native dispatch/body absent or duplicated')
    start=next(v['time_ms']/1000 for v in body if v['phase']=='started')
    end=next(v['time_ms']/1000 for v in body if v['phase']=='ended')
    check(task['started_at']<=task['dispatched_at']<=dispatch[0]['host_time_ms']/1000<=start<end<=task['ended_at'],'body outside actual dispatch interval')
    result=events('peer-result');check(len(result)==1 and not result[0]['result'].get('isError'),'native peer result failed')
    check('SIC_SYNTHETIC_TOOL_RESULT' in json.dumps(result[0]),'actual fixture result missing')
    check(len(events('peer-result-delivered'))==1,'peer receipt absent/duplicated')
    check(sum(v.get('type')=='plugin:sic/peer-result-delivered' and v['data']['task_id']==task['task_id'] for v in native)==1,'parent durable delivery receipt missing')
    state=lambda chain:json.loads(db.execute('SELECT state FROM chains WHERE id=?',(chain,)).fetchone()[0])
    child=state(task['chain_id']);root=state(task['root_chain_id'])
    check(child['collaboration_task']['sender']==root['chain_id'],'SIC historical peer lineage differs')
    # The public ledger is also persisted in each chain; use its actual object refs.
    from verify_dsh_harness import ReadOnlyAPI
    api=ReadOnlyAPI(ARGS.core_url,ARGS.bridge_url,ARGS.token_file.read_text().strip(),ARGS.actor)
    ledger=api.op(task['chain_id'],'ledger.export')
    overlap=[{'call_id':v['call_id'],'seconds':min(v['ended_at'],end)-max(v['started_at'],start)} for v in ledger['calls'] if v.get('role')=='editor' and v.get('ended_at') is not None and min(v['ended_at'],end)>max(v['started_at'],start)]
    check(overlap,'no actual Editor interval overlaps the actual native tool body')
    check(any(v.get('type')=='tool_start' and v.get('host_task_id')==task['task_id'] for v in ledger['events']),'peer tool_start missing from own chain ledger')
    # Correlate request objects through chain-specific model ledger refs.
    refs={v.get('source_refs',{}).get('request_id') for v in ledger['calls'] if v.get('role')=='worker'}
    peer_requests=[json.loads(db.execute('SELECT data FROM objects WHERE id=?',(key,)).fetchone()[0]) for key in refs if key]
    check(any('SIC_SYNTHETIC_TOOL_RESULT' in json.dumps(v) for v in peer_requests),'actual next peer Worker request did not consume native result')
    projections=[v['data'] for v in native if v.get('type')=='plugin:sic/provider-projection']
    check(any('SIC_PEER_TOOL_DONE' in json.dumps(p.get('request',{})) for turn in projections for p in turn['projections']),'parent Worker did not consume returned collaboration result')
    projection=task['provider_projection'];check(projection['call_id'] in {v['call_id'] for v in ledger['calls']},'host request not linked to actual peer Worker call')
    check(binding[0]['task']['provider_projection']==projection,'native peer request evidence differs from Core')
    call=next(v for v in ledger['calls'] if v['call_id']==projection['call_id'])
    check(call.get('source_refs',{}).get('request_id')==projection['request_id'],'peer ledger request reference differs')
    saved_request=json.loads(db.execute('SELECT data FROM objects WHERE id=? AND kind=?',(projection['request_id'],'provider_request')).fetchone()[0])
    saved_response=json.loads(db.execute('SELECT data FROM objects WHERE id=? AND kind=?',(projection['response_id'],'provider_response')).fetchone()[0])
    check(saved_request['request']==projection['request'] and saved_response['response']==projection['response'],'native provider projection differs from stored request/response bodies')
    return {'parent_session':parent,'peer_session':peer,'root_chain':root['chain_id'],'peer_chain':child['chain_id'],'task_id':task['task_id'],'tool_call_id':task['call_id'],'native_body_interval':[start,end],'editor_overlaps':overlap,'native_parent_digest':sha(native),'native_peer_digest':sha(records),'provider_request_id':projection['request_id'],'native_dispatch_count':len(dispatch),'native_result_receipt_count':len(events('peer-result-delivered')),'passed':True}

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--jsonl',type=Path,required=True);p.add_argument('--dsh-home',type=Path,required=True)
    p.add_argument('--db',type=Path,required=True);p.add_argument('--token-file',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--core-url',default='http://127.0.0.1:18888');p.add_argument('--bridge-url',default='http://127.0.0.1:18890');p.add_argument('--actor',default='host')
    p.add_argument('--baseline',type=Path,help='Compare persisted native identity, receipts and request refs after coordinated restart')
    ARGS=p.parse_args()
    report={'passed':False,'inference_requested':False}
    try:
        db=sqlite3.connect(ARGS.db.resolve().as_uri()+'?mode=ro',uri=True)
        report.update(verify(ARGS.jsonl,ARGS.dsh_home,db))
        if ARGS.baseline:
            old=json.loads(ARGS.baseline.read_text(encoding='utf-8'))
            check(old.get('passed'),'baseline did not pass')
            for key in ('parent_session','peer_session','root_chain','peer_chain','task_id','native_parent_digest','native_peer_digest','provider_request_id','native_dispatch_count','native_result_receipt_count'):
                check(report[key]==old[key],'after-restart identity or receipt differs: '+key)
            report['after_restart_verified']=True
    except Exception as e:
        report['passed']=False
        report['failure_type']=type(e).__name__;report['failure']=str(e) if isinstance(e,AssertionError) else 'Evidence acquisition failed'
    ARGS.output.parent.mkdir(parents=True,exist_ok=True)
    ARGS.output.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({'passed':report['passed'],'failure':report.get('failure'),'inference_requested':False}))
    sys.exit(0 if report['passed'] else 1)
