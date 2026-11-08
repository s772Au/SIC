"""Prepare synthetic files/prompts and independently score recorded DSH runs.

No service connection, provider call, Core mutation or inference is performed.
"""
import argparse,hashlib,json,shutil,sqlite3,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
DEFAULT_TASKS=ROOT/'research/dsh-live-smoke/tasks.json'
def digest(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()
def object_digest(value):return hashlib.sha256(json.dumps(value,sort_keys=True,ensure_ascii=False,separators=(',',':'),allow_nan=False).encode()).hexdigest()
def read_json(path):return json.loads(Path(path).read_text(encoding='utf-8-sig'))
def save(path,value):
    path=Path(path);path.parent.mkdir(parents=True,exist_ok=True);path.write_text(json.dumps(value,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
def check(value,message):
    if not value:raise ValueError(message)
def find_case(tasks,case_id):
    rows=[row for row in tasks['cases'] if row['id']==case_id];check(len(rows)==1,'unknown or duplicate case');return rows[0]
def strings(value):
    if isinstance(value,str):yield value
    elif isinstance(value,dict):
        for child in value.values():yield from strings(child)
    elif isinstance(value,list):
        for child in value:yield from strings(child)
def prepare(tasks_path,output):
    tasks_path=Path(tasks_path).resolve();tasks=read_json(tasks_path);output=Path(output).resolve();check(not output.exists(),'prepare requires a new output directory')
    plan={'schema':'sic-dsh-live-smoke-plan-v1','tasks_sha256':digest(tasks_path),'verifier_sha256':digest(__file__),'cases':[],'contains_ground_truth':False}
    for case in tasks['cases']:
        base=output/case['id'];materials=base/'materials';materials.mkdir(parents=True)
        files={}
        for name in case['fixtures']:
            source=(tasks_path.parent/'fixtures'/case['id']/name).resolve();check(source.is_relative_to((tasks_path.parent/'fixtures'/case['id']).resolve()),'fixture source escape')
            target=(materials/name).resolve();check(target.is_relative_to(materials),'fixture target escape');target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(source,target);files[name]=digest(target)
        stages=[]
        for stage in case['stages']:
            prompt=stage['prompt'].replace('{{MATERIALS}}',materials.as_posix());target=base/(stage['id']+'.prompt.txt');target.write_text(prompt+'\n',encoding='utf-8');stages.append({'id':stage['id'],'prompt_file':str(target),'prompt_sha256':digest(target),'required_reads':stage['required_reads']})
        plan['cases'].append({'id':case['id'],'working_directory':str(materials),'fixture_sha256':files,'stages':stages,'same_native_session_within_case':True,'new_session_between_cases':True})
    save(output/'controller-plan.json',plan);return {'prepared':True,'cases':len(plan['cases']),'controller_plan':str(output/'controller-plan.json'),'provider_calls':0}
def cli_records(path):
    rows=[]
    for line in Path(path).read_text(encoding='utf-8-sig').splitlines():
        try:value=json.loads(line)
        except json.JSONDecodeError:continue
        if isinstance(value,dict):rows.append(value)
    check(rows,'empty or non-JSON CLI evidence');return rows
def strict_object(text):
    def pairs(items):
        result={}
        for key,value in items:
            check(key not in result,'duplicate answer key');result[key]=value
        return result
    def constant(value):raise ValueError('non-JSON numeric constant: '+value)
    result=json.loads(text,object_pairs_hook=pairs,parse_constant=constant);check(isinstance(result,dict),'answer must be an object');return result
def answer_score(case,text):
    try:answer=strict_object(text)
    except (ValueError,json.JSONDecodeError) as error:return {'format_valid':False,'exact_match':False,'field_accuracy':0,'error':str(error)}
    expected=case['answer'];matches={}
    for key,value in expected.items():
        actual=answer.get(key)
        if key in case['unordered_fields']:
            matches[key]=isinstance(actual,list) and len(actual)==len(value) and all(isinstance(x,str) for x in actual) and set(actual)==set(value)
        elif isinstance(value,bool):matches[key]=type(actual) is bool and actual==value
        elif isinstance(value,(int,float)):matches[key]=type(actual) in (int,float) and actual==value
        else:matches[key]=type(actual) is type(value) and actual==value
    forbidden=[value for value in strings(answer) if value in case['forbidden_values']]
    return {'format_valid':True,'exact_match':all(matches.values()) and set(answer)==set(expected) and not forbidden,'field_accuracy':sum(matches.values())/len(matches),'field_matches':matches,'unexpected_fields':sorted(set(answer)-set(expected)),'forbidden_value_present':bool(forbidden)}
def native_requests(home,session):
    check(session.startswith('session-') and all(c.isalnum() or c=='-' for c in session),'invalid explicit session identifier')
    home=Path(home).resolve();paths=list((home/'sessions').glob('*/'+session+'/session.v4.jsonl.zstd'));check(len(paths)==1,'explicit isolated session record not found uniquely');check(paths[0].resolve().is_relative_to(home),'session path escape')
    import io,zstandard
    with paths[0].open('rb') as raw,zstandard.ZstdDecompressor().stream_reader(raw) as stream:
        with io.TextIOWrapper(stream,encoding='utf-8') as decoded:records=[json.loads(line) for line in decoded if line.strip()]
    projections=[projection for row in records if row.get('type')=='plugin:sic/provider-projection' for projection in row.get('data',{}).get('projections',[]) if 'request' in projection]
    return projections,digest(paths[0])

class CoreConsumptionProof:
    """One read-only snapshot, scoped to explicit native Worker references."""
    def __init__(self,path):
        self.path=Path(path).resolve();check(self.path.is_file(),'Core database missing')
        self.db=sqlite3.connect(self.path.as_uri()+'?mode=ro',uri=True);self.db.execute('BEGIN');self.cache={}
    def close(self):self.db.rollback();self.db.close()
    def get(self,identity,kind):
        if identity in self.cache:
            actual,value=self.cache[identity];check(actual==kind,'immutable object kind differs');return value
        check(self.db.execute('SELECT 1 FROM revocations WHERE id=?',(identity,)).fetchone() is None,'proof dependency was revoked')
        row=self.db.execute('SELECT kind,data,digest FROM objects WHERE id=?',(identity,)).fetchone();check(row is not None and row[0]==kind,'referenced immutable object missing or wrong kind');value=json.loads(row[1]);check(object_digest(value)==row[2],'immutable object digest mismatch');self.cache[identity]=(kind,value);return value
    def selected(self,cut):
        hierarchy=cut.get('hierarchy',{});view=hierarchy.get('role_views',{}).get('worker',{'context_id':'main','level':hierarchy.get('active',{}).get('worker',1)})
        def context_ids(context_id,target,seen):
            check(context_id not in seen,'context sharing cycle');ctx=hierarchy if context_id=='main' else hierarchy.get('contexts',{}).get(context_id);check(ctx is not None,'selected context missing')
            levels=ctx.get('levels',{});ids=list(levels.get('1',{}).get('cbs',cut.get('cbs',[]) if context_id=='main' else []))
            for number in range(2,target+1):
                for cid in levels.get(str(number),{}).get('cbs',[]):
                    members=self.get(cid,'cb').get('members',[])
                    if not members:ids.append(cid);continue
                    if members[0] in ids:
                        start=ids.index(members[0])
                        if ids[start:start+len(members)]==members:ids[start:start+len(members)]=[cid]
            shared=[]
            for other in ctx.get('shared_contexts',[]):
                parent=hierarchy if other=='main' else hierarchy.get('contexts',{}).get(other);check(parent is not None,'shared context missing');shared.extend(context_ids(other,parent.get('active',{}).get('worker',parent.get('active',{}).get('editor',1)),seen|{context_id}))
            return list(dict.fromkeys(shared+ids))
        return set(context_ids(view['context_id'],view['level'],set()))
    def cb_path(self,cid,source_id,seen=None):
        seen=set() if seen is None else seen;check(cid not in seen,'CB member cycle');cb=self.get(cid,'cb');wb=self.get(cb['wb_id'],'wb')
        check(wb['source_id']==cb['source_id'] and wb.get('members',[])==cb.get('members',[]),'CB/WB relation differs');check(set(wb['source_ids'])<=set(cb['source_ids']),'CB lost its WB source relation')
        for sid in cb['source_ids']:self.get(sid,'source')
        if source_id in cb['source_ids']:return [cid]
        for member in cb.get('members',[]):
            path=self.cb_path(member,source_id,seen|{cid})
            if path:return [cid]+path
        return None
    def prove(self,projections,read_evidence):
        requests=[];chains=set();seen=set()
        for projection in projections:
            rid=projection.get('request_id');check(rid,'native projection request reference missing')
            value=self.get(rid,'provider_request');check(value.get('role')=='worker','projection did not reference a Worker invocation');check(value['request']==projection['request'] and object_digest(value['request'])==projection.get('request_digest'),'native projection differs from immutable actual request')
            if projection.get('cut_id') is not None:check(projection['cut_id']==value.get('cut_id'),'native projection cut differs')
            response=self.get(projection['response_id'],'provider_response');check(response.get('sic_context',{}).get('request_id')==rid and response['sic_context'].get('chain_id')==value['chain_id'],'response does not belong to projected request')
            chains.add(value['chain_id'])
            if rid not in seen:requests.append((rid,value));seen.add(rid)
        check(len(chains)==1,'one case native requests span multiple Core routes');chain_id=next(iter(chains));row=self.db.execute('SELECT state FROM chains WHERE id=?',(chain_id,)).fetchone();check(row is not None,'request route state missing');state=json.loads(row[0]);scope={sid for members in state['groups'].values() for sid in members}
        for _,value in requests:
            if value.get('cut_id'):
                cut=self.get(value['cut_id'],'cut');check(cut['chain_id']==chain_id,'request cut on a different route');scope.update(sid for members in cut.get('groups',{}).values() for sid in members)
        rows=[]
        for evidence in read_evidence:
            sources=[]
            for sid in scope:
                source=self.get(sid,'source')
                for message in source.get('sidecar',{}).get('messages',[]):
                    if message.get('role')=='tool' and message.get('tool_call_id')==evidence['call_id'] and all(line in '\n'.join(strings(message.get('content'))) for line in evidence['lines']):sources.append(sid);break
            check(sources,'completed read result is not fully retained in a route source/WB')
            adopted=None
            for rid,value in requests:
                request=value['request'];cut=self.get(value['cut_id'],'cut') if value.get('cut_id') else None;allowed=self.selected(cut) if cut else set()
                for message in request.get('messages',[]):
                    if message.get('role')=='tool' and message.get('tool_call_id')==evidence['call_id'] and all(line in '\n'.join(strings(message.get('content'))) for line in evidence['lines']):
                        adopted={'mode':'original-pending-WB','source_id':sources[0],'request_id':rid,'cut_id':value.get('cut_id')};break
                    if not isinstance(message.get('content'),str):continue
                    try:header=json.loads(message['content'])
                    except (ValueError,TypeError):continue
                    if not isinstance(header,dict) or header.get('sic_cb') not in allowed:continue
                    cid=header['sic_cb'];cb=self.get(cid,'cb');body=header.get('body');ops=header.get('operations')
                    check(header.get('wb')==cb['source_id'] and isinstance(body,str) and cb['body'].startswith(body) and isinstance(ops,list) and ops==cb['ops'][:len(ops)],'actual projected CB fragment differs from immutable CB')
                    for sid in sources:
                        path=self.cb_path(cid,sid)
                        if path:
                            # Membership alone is insufficient: each CB must have
                            # been published in the immutable request cut.
                            all_published={x for ctx in [cut.get('hierarchy',{}),*cut.get('hierarchy',{}).get('contexts',{}).values()] for level in ctx.get('levels',{}).values() for x in level.get('cbs',[])}|set(cut.get('cbs',[]))
                            check(set(path)<=all_published,'CB ancestry was not published in the adopted cut')
                            adopted={'mode':'published-CB' if len(path)==1 else 'published-higher-CB-ancestor','source_id':sid,'request_id':rid,'cut_id':value['cut_id'],'adopted_cb_id':cid,'cb_path':path,'projected_body_chars':len(body),'full_cb_body_chars':len(cb['body'])};break
                    if adopted:break
                if adopted:break
            check(adopted,'retained read source has no subsequent actual Worker adoption as WB/CB/higher ancestor');rows.append({'file':evidence['file'],'tool_call_id':evidence['call_id'],**adopted})
        return {'status':'verified-source-WB-CB-actual-request-path','chain_id':chain_id,'reads':rows,'immutable_objects_checked':len(self.cache),'projected_request_count':len(requests),'database_access':'SQLite URI mode=ro; one consistent read transaction; no service connection'}

def verify(tasks_path,case_id,prepared_root,stage_logs,run_kind,dsh_home=None,core_db=None):
    tasks_path=Path(tasks_path).resolve();tasks=read_json(tasks_path);case=find_case(tasks,case_id);prepared_root=Path(prepared_root).resolve();plan=read_json(prepared_root/'controller-plan.json')
    check(plan['tasks_sha256']==digest(tasks_path),'task manifest differs from prepared version');prepared=find_case(plan,case_id);materials=Path(prepared['working_directory']).resolve();check(materials.is_relative_to(prepared_root),'prepared material path escape')
    check(list(stage_logs)==[stage['id'] for stage in case['stages']],'provide each stage log exactly once in task order')
    check(all(digest(materials/name)==sha for name,sha in prepared['fixture_sha256'].items()),'prepared fixture file changed')
    sessions=set();observed=[];stage_reports=[];all_calls=set();final_text=None;read_evidence=[]
    for stage,control in zip(case['stages'],prepared['stages']):
        check(digest(control['prompt_file'])==control['prompt_sha256'],'prepared prompt changed')
        rows=cli_records(stage_logs[stage['id']]);session_ids={r['sessionId'] for r in rows if r.get('type')=='session'};check(len(session_ids)==1,'each stage needs one actual CLI session');sessions.update(session_ids)
        check(not any(r.get('type')=='status' and r.get('reason',{}).get('kind')=='error' for r in rows),'DSH stage ended in an error')
        finals=[r['text'] for r in rows if r.get('type')=='final'];check(len(finals)==1 and bool(finals[0].strip()),'stage final output missing or ambiguous');final_text=finals[0]
        reads=[]
        for index,row in enumerate(rows):
            if row.get('type')!='tool_call':continue
            check(row.get('tool')=='read','unexpected tool; only built-in read is permitted');call=row['callId'];check(call not in all_calls,'tool call identity repeated across stages');all_calls.add(call)
            args=row.get('input',{});args=json.loads(args) if isinstance(args,str) else args;check(isinstance(args,dict) and isinstance(args.get('file_path'),str),'read argument lacks actual file_path')
            target=Path(args['file_path']);target=(materials/target).resolve() if not target.is_absolute() else target.resolve();check(target.is_relative_to(materials),'read escaped isolated case materials');name=target.relative_to(materials).as_posix();check(name in case['fixtures'],'read targeted an unlisted fixture')
            results=[r for r in rows[index+1:] if r.get('type')=='tool_result' and r.get('callId')==call];check(len(results)==1 and results[0].get('status')=='completed','read lacks one successful native tool result')
            check(not (isinstance(results[0].get('result'),dict) and results[0]['result'].get('isError')),'read returned an actual tool error')
            body='\n'.join(strings(results[0].get('result')));lines=[line for line in target.read_text(encoding='utf-8').splitlines() if line.strip()]
            check(all(line in body for line in lines),'tool result does not contain the complete actual fixture');reads.append(name);observed.append(name)
            read_evidence.append({'file':name,'call_id':call,'lines':lines})
        check(all(name in reads for name in stage['required_reads']),'required stage read was not executed')
        stage_reports.append({'stage':stage['id'],'log_sha256':digest(stage_logs[stage['id']]),'completed_reads':reads,'final_sha256':hashlib.sha256(final_text.encode()).hexdigest()})
    check(len(sessions)==1,'multi-stage case must resume the same native session')
    cursor=-1
    for name in case['required_read_order']:
        cursor=next((i for i in range(cursor+1,len(observed)) if observed[i]==name),-1);check(cursor>=0,'linked read order or staged correction sequence missing')
    consumption={'status':'not_verified','native_session_sha256':None}
    if dsh_home:
        projections,native_sha=native_requests(dsh_home,next(iter(sessions)));check(projections,'native SIC projection evidence missing')
        if core_db:
            proof=CoreConsumptionProof(core_db)
            try:consumption={**proof.prove(projections,read_evidence),'native_session_sha256':native_sha}
            finally:proof.close()
        else:consumption={'status':'native-references-present-core-db-required','native_session_sha256':native_sha,'projected_request_count':len(projections)}
    if run_kind=='nonfixed-gateway-exploratory':check(dsh_home is not None,'exploratory scoring requires isolated native session evidence')
    if run_kind=='nonfixed-gateway-exploratory':check(core_db is not None,'exploratory scoring requires read-only Core database source/adoption evidence')
    quality=answer_score(case,final_text) if run_kind=='nonfixed-gateway-exploratory' else {'status':'not_evaluated','reason':'fixture/mechanical mode does not produce model quality scores'}
    if run_kind=='nonfixed-gateway-exploratory':quality.update(status='exploratory-scored',score=int(quality['exact_match']),denominator=1)
    return {'schema':'sic-dsh-live-run-verification-v1','case_id':case_id,'mechanical_passed':True,'run_kind':run_kind,'model_identity':'nonfixed gateway; verifier does not establish a fixed returned model','tasks_sha256':digest(tasks_path),'verifier_sha256':digest(__file__),'stages':stage_reports,'worker_consumption':consumption,'quality':quality,'provider_calls_by_verifier':0,'limits':['Small new synthetic task only; no baseline effect, cost parity, Pareto or physical-cache conclusion.','CLI/session records are trusted local evidence, not cryptographic proof against a fabricated run.']}
def main():
    parser=argparse.ArgumentParser(description=__doc__);sub=parser.add_subparsers(dest='command',required=True)
    prep=sub.add_parser('prepare');prep.add_argument('--tasks',type=Path,default=DEFAULT_TASKS);prep.add_argument('--output',type=Path,required=True)
    run=sub.add_parser('verify');run.add_argument('--tasks',type=Path,default=DEFAULT_TASKS);run.add_argument('--case',required=True);run.add_argument('--prepared-root',type=Path,required=True);run.add_argument('--stage-log',action='append',required=True,help='stage_id=recorded CLI JSONL path, in task order');run.add_argument('--run-kind',choices=['mechanical-fixture','nonfixed-gateway-exploratory'],default='mechanical-fixture');run.add_argument('--dsh-home',type=Path);run.add_argument('--core-db',type=Path,help='run Core SQLite database/export, opened URI mode=ro');run.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    if args.command=='prepare':print(json.dumps(prepare(args.tasks,args.output)));return 0
    try:
        pairs=[entry.split('=',1) for entry in args.stage_log];check(all(len(pair)==2 for pair in pairs),'stage-log must be stage_id=path');check(len({pair[0] for pair in pairs})==len(pairs),'duplicate stage log');report=verify(args.tasks,args.case,args.prepared_root,dict(pairs),args.run_kind,args.dsh_home,args.core_db)
    except Exception as error:report={'schema':'sic-dsh-live-run-verification-v1','case_id':args.case,'mechanical_passed':False,'quality':{'status':'not_evaluated','reason':'incomplete or invalid execution evidence'},'error':str(error),'provider_calls_by_verifier':0}
    save(args.output,report);print(json.dumps({'case_id':args.case,'mechanical_passed':report['mechanical_passed'],'quality':report['quality']}));return 0 if report['mechanical_passed'] else 1
if __name__=='__main__':raise SystemExit(main())
