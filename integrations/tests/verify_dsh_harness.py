"""Read-only actual isolated DSH/Core evidence verifier. Never requests inference."""
import argparse,hashlib,io,json,os,re,sys,urllib.request
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[2]))

def check(condition,message):
    if not condition:raise AssertionError(message)
def sha(value):return hashlib.sha256(json.dumps(value,sort_keys=True,separators=(',',':'),ensure_ascii=False).encode()).hexdigest()
def read_jsonl(path):
    return [json.loads(line) for line in Path(path).read_text(encoding='utf-8-sig').splitlines() if line.strip()]
def session_records(home,session):
    check(re.fullmatch(r'session-[a-zA-Z0-9-]+',session),'unsafe session identifier')
    # Enumerate names only: never open another session body.
    candidates=list((home/'sessions').glob('*/'+session+'/session.v4.jsonl.zstd'))
    check(len(candidates)==1,'expected exactly one explicit session record file')
    check(candidates[0].resolve().is_relative_to(home.resolve()),'session file escapes isolated DSH_HOME')
    import zstandard
    with candidates[0].open('rb') as stream,zstandard.ZstdDecompressor().stream_reader(stream) as reader:
        with io.TextIOWrapper(reader,encoding='utf-8') as text:
            records=[json.loads(line) for line in text if line.strip()]
    return records,candidates[0].relative_to(home).as_posix()
class ReadOnlyAPI:
    def __init__(self,core,bridge,token,actor):
        self.core=core.rstrip('/');self.bridge=bridge.rstrip('/');self.token=token;self.actor=actor
        self.client=urllib.request.build_opener(urllib.request.ProxyHandler({}))
        for url in (self.core,self.bridge):
            parsed=urllib.parse.urlparse(url)
            check(parsed.scheme=='http' and parsed.hostname in ('127.0.0.1','localhost','::1') and parsed.port!=8341 and not parsed.username and not parsed.query,'only dedicated loopback control endpoints allowed')
    def request(self,path,payload=None,bridge=False):
        request=urllib.request.Request((self.bridge if bridge else self.core)+path,data=None if payload is None else json.dumps(payload).encode(),headers={'Authorization':'Bearer '+self.token,'Content-Type':'application/json'})
        with self.client.open(request,timeout=20) as response:return json.load(response)
    def op(self,chain,operation,payload=None):
        check(operation in ('chain.get','ledger.export'),'verifier operation is not read-only')
        return self.request('/api/v2/operations',{'operation':operation,'context':{'actor':self.actor,'chain_id':chain,'allow_write':False,'allow_model':False},'payload':payload or {}})['result']

def verify(log,home,api):
    cli=read_jsonl(log);sessions={v['sessionId'] for v in cli if v.get('type')=='session'}
    check(len(sessions)==1,'CLI log must identify exactly one session');session=sessions.pop()
    calls=[v for v in cli if v.get('type')=='tool_call' and v.get('tool')=='sic_fixture_wait']
    check(len(calls)==1,'expected one actual CLI fixture tool call');call=calls[0]['callId']
    check(any(v.get('type')=='tool_result' and v.get('callId')==call and v.get('status')=='completed' and 'SIC_SYNTHETIC_TOOL_RESULT' in json.dumps(v) for v in cli),'CLI successful tool result missing')
    check(any(v.get('type')=='final' and v.get('text')=='SIC_DSH_ROUNDTRIP_OK' for v in cli),'CLI final success marker missing')
    indices=[next(i for i,v in enumerate(cli) if v.get('type')==kind and (kind=='final' or v.get('callId')==call)) for kind in ('tool_call','tool_result','final')]
    check(indices==sorted(indices),'CLI tool/result/final order differs')
    records,relative=session_records(home,session)
    projections=[v['data'] for v in records if v.get('type')=='plugin:sic/provider-projection']
    check(len(projections)>=2,'native session lacks both provider projection records')
    chains={v['chain_id'] for v in projections};check(len(chains)==1,'one DSH session mapped to multiple chains');chain=chains.pop()
    mapped=[v for v in api.request('/api/v2/bridge/sessions',bridge=True)['sessions'] if v['session_key']==session]
    check(len(mapped)==1 and mapped[0]['chain_id']==chain,'persistent bridge binding differs from native session')
    live=api.request('/api/v2/bridge/evidence',{'session_key':session},bridge=True)
    check(live['chain_id']==chain,'bridge chain differs')
    check(live['turn_fingerprint']==projections[-1]['turn_fingerprint'],'live final turn fingerprint differs')
    check(live['projections']==projections[-1]['projections'],'native original projections differ from actual Core request/response references')
    check(any('SIC_SYNTHETIC_TOOL_RESULT' in json.dumps(p['request']) for p in live['projections']),'actual final Worker request did not consume the tool result')
    ledger=api.op(chain,'ledger.export');state=api.op(chain,'chain.get');usage={v['call_id']:v for v in ledger['calls']}
    refs=[]
    for turn in projections:
        for p in turn['projections']:
            check(p.get('request_id') and p.get('response_id'),'request/response references absent')
            check(p['call_id'] in usage,'projection call missing from actual Core ledger')
            check(usage[p['call_id']].get('source_refs',{}).get('request_id')==p['request_id'],'Core ledger request reference mismatch')
            # Recompute using Core canonical digest implementation, not a guessed SDK serialization.
            from sic_core.adapters import digest
            check(digest(p['request'])==p['request_digest'],'projection body digest mismatch')
            refs.append({k:p[k] for k in ('call_id','request_id','response_id','request_digest')})
    delivered=[v['data']['event'] for v in records if v.get('type')=='plugin:sic/tool-event' and v['data'].get('phase')=='delivered']
    starts=[v for v in delivered if v.get('type')=='tool_start' and v.get('call_id')==call]
    ends=[v for v in delivered if v.get('type')=='tool_end' and v.get('call_id')==call]
    check(len(starts)==1 and len(ends)==1,'native durable around-dispatch start/end missing or duplicated')
    check(all(v.get('observed_by')=='dsh-around-dispatch' for v in starts+ends),'tool proposal was substituted for actual host execution')
    body=[v['data'] for v in records if v.get('type')=='plugin:sic/fixture-body']
    body_start=[v['time_ms']/1000 for v in body if v.get('phase')=='started'];body_end=[v['time_ms']/1000 for v in body if v.get('phase')=='ended']
    check(len(body_start)==len(body_end)==1 and body_start[0]<body_end[0],'actual tool body time interval missing')
    check(starts[0]['host_time_ms']/1000<=body_start[0]<=body_end[0]<=ends[0]['host_time_ms']/1000,'tool body outside host around-dispatch interval')
    # Core assigns an event ID; correlate preserved host timestamps and call IDs.
    for host in starts+ends:
        check(any(v.get('type')==host['type'] and v.get('call_id')==call and v.get('host_time_ms')==host['host_time_ms'] for v in ledger['events']),'host event not recorded in same Core chain')
    overlaps=[]
    for v in ledger['calls']:
        if v.get('role')!='editor':continue
        a,b=v.get('started_at'),v.get('ended_at')
        if a is None or b is None:continue
        duration=max(0,min(b,body_end[0])-max(a,body_start[0]))
        if duration>0:overlaps.append({'call_id':v['call_id'],'seconds':duration,'request_id':v.get('source_refs',{}).get('request_id')})
    check(overlaps,'FAIL: no actual Editor model interval overlaps actual DSH tool body')
    timing=ledger['summary']['timing'];check(timing.get('editor_tool_overlap_seconds',0)>0,'Core ledger has no actual Editor/tool overlap')
    return {'session_id':session,'chain_id':chain,'session_record':relative,'session_record_digest':sha(records),'record_count':len(records),'tool_call_id':call,'actual_body_interval':[body_start[0],body_end[0]],'editor_body_overlaps':overlaps,'provider_refs':refs,'cb_ids':state['cbs'],'published_cut':state['current_cut'],'ledger_call_ids':sorted(usage),'core_editor_tool_overlap_seconds':timing['editor_tool_overlap_seconds'],'final_ok':True}

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--jsonl',action='append',required=True,type=Path);p.add_argument('--dsh-home',required=True,type=Path);p.add_argument('--core-url',default='http://127.0.0.1:18888');p.add_argument('--bridge-url',default='http://127.0.0.1:18890');p.add_argument('--actor',default='host');p.add_argument('--token-env',default='SIC_CORE_TOKEN');p.add_argument('--token-file',type=Path);p.add_argument('--output',required=True,type=Path);p.add_argument('--baseline',type=Path,help='read-only after-restart comparison against earlier successful report');args=p.parse_args()
    report={'schema':'sic-real-dsh-verification-v1','passed':False,'inference_requested':False,'sessions':[]}
    try:
        token=args.token_file.read_text().strip() if args.token_file else os.environ.get(args.token_env,'');check(bool(token),'local control token not configured')
        api=ReadOnlyAPI(args.core_url,args.bridge_url,token,args.actor)
        for log in args.jsonl:report['sessions'].append(verify(log,args.dsh_home,api))
        rows=report['sessions'];check(len({r['session_id'] for r in rows})==len(rows),'duplicate input session');check(len({r['chain_id'] for r in rows})==len(rows),'independent DSH sessions share a Core chain')
        for i,a in enumerate(rows):
            for b in rows[i+1:]:
                check(not set(a['ledger_call_ids'])&set(b['ledger_call_ids']),'independent chains share model calls')
                check(not set(a['cb_ids'])&set(b['cb_ids']),'independent sessions share newly created CBs')
        if args.baseline:
            old=json.loads(args.baseline.read_text(encoding='utf-8'));check(old.get('passed'),'baseline was not successful');prior={r['session_id']:r for r in old['sessions']}
            for row in rows:
                check(row['session_id'] in prior,'session absent from restart baseline')
                for key in ('chain_id','provider_refs','cb_ids','published_cut','ledger_call_ids','session_record_digest'):
                    check(row[key]==prior[row['session_id']][key],'after-restart evidence changed: '+key)
            report['after_restart_verified']=True
        report['passed']=True
    except Exception as error:
        report['failure_type']=type(error).__name__
        report['failure']=str(error) if isinstance(error,AssertionError) else 'Read-only evidence acquisition failed; inspect local inputs/dependencies/control service.'
    args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({'passed':report['passed'],'verified_sessions':len(report['sessions']),'failure':report.get('failure'),'inference_requested':False},ensure_ascii=False))
    return 0 if report['passed'] else 1
if __name__=='__main__':sys.exit(main())
