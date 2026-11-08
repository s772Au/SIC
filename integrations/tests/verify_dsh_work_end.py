"""Read actual successful native turn-end and Core flush evidence; no inference."""
import argparse,json,os,sys
from pathlib import Path
from verify_dsh_harness import ReadOnlyAPI,check,read_jsonl,session_records,sha

def verify(log,home,api):
    cli=read_jsonl(log);sessions={r['sessionId'] for r in cli if r.get('type')=='session'}
    check(len(sessions)==1,'expected one explicit native session')
    session=sessions.pop();records,relative=session_records(home,session)
    native=[r for r in records if r.get('type')=='turn/end' and r.get('data',{}).get('reason',{}).get('kind')=='completed']
    delivered=[r for r in records if r.get('type')=='plugin:sic/tool-event' and r['data'].get('phase')=='delivered' and r['data'].get('event',{}).get('type')=='work_end']
    check(len(native)==len(delivered)==1,'one actual completed turn and one delivered work_end required')
    event=delivered[0]['data']['event']
    check(event.get('observed_by')=='dsh-native-turn-end','model finish substituted for actual native turn end')
    check(event.get('turn')==native[0]['data']['turn'],'native turn differs')
    check(native[0]['time']<=event['host_time_ms']<=delivered[0]['time'],'terminal delivery outside native event chronology')
    projections=[r['data'] for r in records if r.get('type')=='plugin:sic/provider-projection']
    check(bool(projections),'actual provider projections absent')
    chain=projections[-1]['chain_id'];state=api.op(chain,'chain.get');ledger=api.op(chain,'ledger.export')
    ends=[r for r in ledger['events'] if r.get('type')=='work_end' and r.get('observed_by')=='dsh-native-turn-end' and r.get('host_time_ms')==event['host_time_ms']]
    check(len(ends)==1,'terminal fact missing/duplicated in actual Core chain')
    check(not state['pending'],'owned fixture left short material pending after actual work_end')
    check(not state['open_tools'],'fixture left an executing tool open')
    worker_ids={r['call_id'] for r in ledger['calls'] if r.get('role')=='worker'}
    native_worker_ids={p['call_id'] for turn in projections for p in turn['projections']}
    check(worker_ids==native_worker_ids,'terminal flush dispatched an extra hidden Worker')
    return {'passed':True,'session_id':session,'chain_id':chain,'native_record':relative,'native_record_digest':sha(records),'terminal_event_id':event['event_id'],'worker_call_count':len(worker_ids),'pending_after_terminal':0,'published_cb_count':len(state['cbs']),'inference_requested':False}

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--jsonl',required=True,type=Path);p.add_argument('--dsh-home',required=True,type=Path)
    p.add_argument('--token-file',type=Path);p.add_argument('--core-url',default='http://127.0.0.1:18888');p.add_argument('--bridge-url',default='http://127.0.0.1:18890');p.add_argument('--actor',default='host');p.add_argument('--output',required=True,type=Path)
    args=p.parse_args();report={'passed':False,'inference_requested':False}
    try:
        token=args.token_file.read_text().strip() if args.token_file else os.environ.get('SIC_CORE_TOKEN','');check(bool(token),'control credential not configured')
        report=verify(args.jsonl,args.dsh_home,ReadOnlyAPI(args.core_url,args.bridge_url,token,args.actor))
    except Exception as error:
        report['passed']=False;report['failure_type']=type(error).__name__;report['failure']=str(error) if isinstance(error,AssertionError) else 'Read-only evidence acquisition failed'
    args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({'passed':report['passed'],'failure':report.get('failure'),'inference_requested':False}))
    sys.exit(0 if report['passed'] else 1)
