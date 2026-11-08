"""Explicit opt-in real DSH -> isolated SIC -> user-authorized test gateway.

No fixture answers, no automatic model retry, no existing service/profile edits.
Raw synthetic experiment records remain in the caller's private output folder.
"""
import argparse,copy,hashlib,json,os,re,subprocess,sys,threading,time,uuid,zipfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path[:0]=[str(ROOT),str(ROOT/'integrations/tests')]
from sic_core.service import Core
from sic_core.server import make_server
from sic_core.bridge import make_bridge_server
from sic_core.adapters import LiteLLMProvider
from native_peer_faults import Harness,records,background

def save(path,value):
    path.parent.mkdir(parents=True,exist_ok=True)
    path.write_text(json.dumps(value,ensure_ascii=False,indent=2,allow_nan=False)+'\n',encoding='utf8')

def validate_stage_session(events,previous_session=None):
    identities={v['sessionId'] for v in events if v.get('type')=='session' and isinstance(v.get('sessionId'),str)}
    if len(identities)!=1:raise AssertionError('Real DSH stage must identify exactly one native Session; do not continue in a new Session')
    current=identities.pop()
    if previous_session is not None and current!=previous_session:raise AssertionError('Late correction did not resume the same real DSH Session')
    return current

class MeteredProvider:
    def __init__(self,folder,roles,limit,editor_reasoning_effort=None):
        self.folder=folder;self.delegate=LiteLLMProvider(roles);self.limit=limit;self.calls=[];self.lock=threading.Lock();self.editor_reasoning_effort=editor_reasoning_effort
        self._capture_local=threading.local();self._old_httpx_send=None
    def enable_http_capture(self):
        """Capture the actual serialized body to 8341, never HTTP headers."""
        import httpx
        self._old_httpx_send=httpx.Client.send;original=self._old_httpx_send;owner=self
        def audited(client,request,*args,**kwargs):
            prefix=getattr(owner._capture_local,'prefix',None)
            if prefix is not None and request.url.host in ('127.0.0.1','localhost') and request.url.port==8341 and request.url.path.endswith('/chat/completions'):
                raw=request.content;body=json.loads(raw)
                if any(k in body for k in ('api_key','authorization','access_token')):raise ValueError('Provider secret must not be serialized into the request body')
                secret=os.environ.get('SIC_LIVE_PROVIDER_KEY','')
                if secret and secret.encode() in raw:raise ValueError('Provider credential appeared in the request body; request blocked')
                prefix.with_suffix('.http-request.json').write_bytes(raw)
                with owner.lock:
                    row=owner._capture_local.row;row.update(http_request_bytes=len(raw),http_request_sha256=hashlib.sha256(raw).hexdigest(),http_headers_captured=False)
            return original(client,request,*args,**kwargs)
        httpx.Client.send=audited
    def close_http_capture(self):
        if self._old_httpx_send is not None:
            import httpx
            httpx.Client.send=self._old_httpx_send;self._old_httpx_send=None
    def capabilities(self):return self.delegate.capabilities()
    def call(self,role,request):
        native=copy.deepcopy(request);native.update(temperature=0,top_p=1)
        if role=='editor':native['response_format']={'type':'json_object'}
        if role=='editor' and self.editor_reasoning_effort:native['reasoning_effort']=self.editor_reasoning_effort
        with self.lock:
            if len(self.calls)>=self.limit:raise ValueError('Explicit live model call cap reached; no automatic continuation')
            row={'attempt':len(self.calls)+1,'role':role,'started_at':time.time(),'status':'inflight','requested_output_budget':native.get('max_completion_tokens',native.get('max_tokens',4096))};self.calls.append(row)
            prefix=self.folder/'calls'/str(row['attempt']).zfill(4)
            save(prefix.with_suffix('.request.json'),native);save(self.folder/'provider-attempts.json',self.calls)
        try:
            self._capture_local.prefix=prefix;self._capture_local.row=row
            result=self.delegate.call(role,native)
            save(prefix.with_suffix('.response.json'),result)
            row.update(status='completed',returned_model=result.get('returned_model'),usage=result.get('usage'),requested_model=result.get('requested_model'))
            completion=(result.get('usage') or {}).get('completion_tokens')
            row['reported_completion_exceeds_requested_budget']=isinstance(completion,int) and completion>row['requested_output_budget']
            return result
        except Exception as e:
            message=str(e)
            key=os.environ.get('SIC_LIVE_PROVIDER_KEY','')
            if key:message=message.replace(key,'[redacted]')
            row.update(status='failed',error_type=type(e).__name__,message=message[:2500]);raise
        finally:
            self._capture_local.prefix=None
            row.update(ended_at=time.time());row['seconds']=row['ended_at']-row['started_at']
            with self.lock:save(self.folder/'provider-attempts.json',self.calls)

class ExperimentCore(Core):
    def _new(self,*args,**kwargs):
        state=super()._new(*args,**kwargs)
        pack=self.experiment_prompts
        state['config'].update(worker_prompt=pack['prompts']['worker'],editor_prompt=pack['prompts']['sic_editor'],task_goal='Complete the current Harness task using retained source evidence. Preserve exact identifiers, late corrections and distinctions between performed tool actions and plans.',program_rules=[] if self.experiment_engine=='unchanged' else ['deduplicate_exact'],memory_chars=24000,editor_memory_chars=16000)
        if self.experiment_prefix_layout=='shared_i':
            state['config'].update(prefix_layout='shared_i',shared_prefix_budget=20000,shared_prefix_identity=self.experiment_shared_identity)
        return state

class OwnedHarness(Harness):
    def start(self,overlay,prompt,log,resume=None):
        command=[str(self.args.dsh_command),'--profile',self.profile,'--patch',str(overlay)]
        if resume:command.extend(['--session-id',resume])
        command.extend(['--json',prompt]);stream=log.open('w',encoding='utf8')
        process=subprocess.Popen(command,cwd=self.task_directory,env=self.env,stdout=stream,stderr=subprocess.STDOUT,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
        self.processes.append((process,stream));return process

def overlay(path,port):
    value=f'''- id: sic-core-dsh
  config:
    provider: sic-core
    baseURL: http://127.0.0.1:{port}/v1
    apiKeyEnv: SIC_LIVE_CORE_TOKEN
    model: sic-worker
    contextWindow: 128000
    maxTokens: 4096
    inputPricePerMillion: 0
    outputPricePerMillion: 0
    cacheReadPricePerMillion: 0
    cacheWritePricePerMillion: 0
    timeoutMs: 240000
    toolEvents: true
    fixtureWaitTool: false
    fixturePeerTool: false
- id: agent-default-model
  config:
    provider: sic-core
    model: sic-worker
- id: sandbox-policy
  config:
    mode: read-only
- id: approval
  config:
    policy: never
- id: permission
  config:
    defaultPreset: read-only
- id: tool-fs
  config:
    read: true
    readImage: false
    write: false
    edit: false
'''
    # Native read remains enabled; unrelated providers, shell, network, skills,
    # subagents and independent context alteration are disabled for this pack.
    disabled=['compaction-basic','command-compact','tool-result-pruner','llm-pi-ai','llm-deepseek','llm-deepseek-account','session-title-llm','session-log-deepseek','session-telemetry-otel','web-search-deepseek','web-deepseek-api','tool-bash','tool-pwsh','tool-jobs','tool-skill','tool-subagent','tool-subagent-fork','tool-subagent-control','tool-subagent-list-agents','tool-workflow','tool-web','tool-plugin-manager','agent-instructions','skill-filesystem','tool-todo','tool-goal','tool-working-directory','tool-fs-search','ptc-runtime','workflow-ptc']
    for name in disabled:value+=f'- id: {name}\n  disabled: true\n'
    path.write_text(value,encoding='utf8');return path

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--dsh-command',required=True,type=Path)
    p.add_argument('--prepared',required=True,type=Path,help='Prepared isolated test plan, never send its references to a model')
    p.add_argument('--output',required=True,type=Path)
    p.add_argument('--model',required=True)
    p.add_argument('--tokenizer-model',default='gpt-4o')
    p.add_argument('--endpoint',default='http://127.0.0.1:8341/v1')
    p.add_argument('--allow-test-gateway',action='store_true')
    p.add_argument('--max-model-calls',type=int,default=60)
    p.add_argument('--engine',choices=['sic','unchanged'],default='sic',help='unchanged uses program KEEP, no LLM Editor, through the same SIC bridge; not raw DSH without SIC')
    p.add_argument('--editor-reasoning-effort',choices=['low','medium','high'],help='Explicit request intent only; gateway enforcement is not attested')
    p.add_argument('--prefix-layout',choices=['separate','shared_i'],default='separate')
    p.add_argument('--case',action='append',dest='case_ids')
    p.add_argument('--stage-timeout',type=int,default=600)
    a=p.parse_args()
    from urllib.parse import urlsplit
    u=urlsplit(a.endpoint)
    if not a.allow_test_gateway or u.hostname not in ('127.0.0.1','localhost') or u.port!=8341 or u.path!='/v1' or u.query or u.fragment or u.username or u.password:p.error('Explicit --allow-test-gateway and local 8341 /v1 endpoint required')
    if a.max_model_calls<1:p.error('Positive explicit call cap required')
    if a.output.exists():p.error('Output must be a new directory; partial attempts are never overwritten or automatically resumed')
    if not os.environ.get('SIC_LIVE_PROVIDER_KEY'):p.error('Private SIC_LIVE_PROVIDER_KEY environment variable required')
    plan=json.loads(a.prepared.read_text(encoding='utf-8-sig'))
    a.output.mkdir(parents=True);folder=a.output.resolve()
    role={'model':'openai/'+a.model,'api_base':a.endpoint,'api_key_env':'SIC_LIVE_PROVIDER_KEY','route_id':'user-authorized-8341-nonfixed-exploratory','expected_returned_models':[a.model],'tokenizer_model':a.tokenizer_model,'tokenizer_status':'host-selected-surrogate-not-provider-attested','max_input_tokens':100000,'max_output_tokens':4096,'context_window':128000,'timeout_seconds':180}
    provider=MeteredProvider(folder,{'worker':role,'editor':role},a.max_model_calls,a.editor_reasoning_effort)
    provider.enable_http_capture()
    os.environ['SIC_LIVE_CORE_TOKEN']=uuid.uuid4().hex
    config={'tokens':{'SIC_LIVE_CORE_TOKEN':'host'},'roles':{},'profiles':{},'bridge':{'allow_write':True,'allow_model':True,'editor_mode':'program' if a.engine=='unchanged' else 'model','editor_workers':1,'seal_bytes':4096,'public_model':'sic-worker','editor_wait_seconds':240,'host_tools_enabled':False}}
    core=ExperimentCore(str(folder/'core.db'),config,provider=provider)
    core.experiment_engine=a.engine
    core.experiment_prompts=json.loads((ROOT/'research/compression/pack/control-pack.json').read_text(encoding='utf8'))
    core.experiment_prefix_layout=a.prefix_layout
    core.experiment_shared_identity={'model':role['model'],'provider_id':'user-authorized-loopback-8341','cache_namespace':'host-declared-test-namespace-physical-identity-unknown'}
    api=make_server(core,config,port=0);bridge=make_bridge_server(core,config,port=0);background(api);background(bridge)
    harness=OwnedHarness(a,folder);harness.profile='sic-live-8341';harness.env['SIC_LIVE_CORE_TOKEN']=os.environ['SIC_LIVE_CORE_TOKEN']
    harness.env.update(DSH_PERMISSION_MODE='read-only',LANGSMITH_TRACING='false',LANGCHAIN_TRACING_V2='false')
    # The Harness authenticates only to Core; it does not inherit provider secrets.
    harness.env.pop('SIC_LIVE_PROVIDER_KEY',None);harness.env.pop('AG_API_KEY',None)
    report={'schema':'sic-dsh-live-run-v1','kind':'nonfixed-gateway-exploratory','engine':a.engine,'status':'running','started_at':time.time(),'harness':'real-installed-DSH-headless','requested_model':a.model,'route':a.endpoint,'physical_model_revision':'unknown','tokenizer_status':role['tokenizer_status'],'sampling':{'temperature':0,'top_p':1},'quality_generalization':False,'prices':'unknown; DSH route display zeros are required placeholders and not actual prices','call_cap':a.max_model_calls,'cases':[],'plan_sha256':hashlib.sha256(a.prepared.read_bytes()).hexdigest()}
    report['editor_output_request']={'response_format':{'type':'json_object'},'provider_enforcement_attested':False,'invalid_response_policy':'preserve-and-fail; no repair or retry'}
    if a.editor_reasoning_effort:report['editor_output_request']['reasoning_effort']=a.editor_reasoning_effort
    report['prefix_architecture']=a.prefix_layout
    report['cross_role_prefix_cache_reuse_verified']=False
    save(folder/'configuration.json',{**report,'role_profile':{k:v for k,v in role.items() if k!='api_key_env'},'bridge':config['bridge'],'services':{'api_port':api.server_port,'bridge_port':bridge.server_port}})
    save(folder/'plan.json',plan)
    with zipfile.ZipFile(folder/'code-snapshot.zip','w',zipfile.ZIP_DEFLATED) as archive:
        paths=sorted((ROOT/'sic_core').glob('*.py'))+sorted((ROOT/'sic_core').glob('*.json'))+[Path(__file__).resolve(),ROOT/'scripts/verify_dsh_live_run.py',ROOT/'research/compression/pack/control-pack.json',ROOT/'research/dsh-live-smoke/tasks.json']
        paths+=sorted(p for p in (ROOT/'sic_core/dsh_plugin').iterdir() if p.is_file() and p.suffix in ('.mjs','.json','.yaml','.yml'))
        paths+=sorted((ROOT/'research/dsh-live-smoke/fixtures').rglob('*.txt'))
        for path in paths:
            archive.write(path,path.relative_to(ROOT).as_posix())
    try:
        harness.setup();patch=overlay(folder/'live.overlay.yml',bridge.server_port)
        for case in plan['cases']:
            case_id=case.get('case_id',case.get('id'))
            if a.case_ids and case_id not in a.case_ids:continue
            harness.task_directory=Path(case['working_directory']).resolve()
            row={'case_id':case_id,'stages':[],'status':'running'};report['cases'].append(row);session=None
            for index,stage in enumerate(case['stages']):
                prompt_path=Path(stage['prompt_file'])
                if hashlib.sha256(prompt_path.read_bytes()).hexdigest()!=stage['prompt_sha256']:raise ValueError('Prepared prompt changed')
                prompt=prompt_path.read_text(encoding='utf8');log=folder/(case_id+'-stage-'+str(index+1)+'.jsonl')
                child=harness.start(patch,prompt,log,resume=session)
                stage_start=time.time()
                while child.poll() is None and time.time()-stage_start<a.stage_timeout:time.sleep(.2)
                if child.poll() is None:raise TimeoutError('Owned DSH stage timed out; no automatic replay')
                code=harness.settle(child);events=records(log)
                session=validate_stage_session(events,session)
                row['stages'].append({'stage':index+1,'log':log.name,'session_id':session,'exit_code':code,'seconds':time.time()-stage_start})
                save(folder/'run.json',report)
                if code:raise RuntimeError('Real DSH stage failed; retained partial evidence in '+log.name)
            row['status']='completed'
        report['status']='completed'
    except Exception as e:
        if report['cases'] and report['cases'][-1]['status']=='running':report['cases'][-1]['status']='failed'
        report.update(status='failed',error_type=type(e).__name__,error=str(e));raise
    finally:
        harness.close()
        # Settle already-admitted work before exporting a terminal snapshot.
        # Stopping the owned schedulers does not authorize any replay/retry.
        bridge.shutdown();bridge.server_close();api.shutdown();api.server_close()
        core.runtime.stop()
        try:
            chains=[]
            with core.store.transaction():bindings=core.store.db.execute('SELECT state FROM bridge_bindings').fetchall()
            for binding in bindings:
                b=json.loads(binding[0]);cid=b['chain_id'];context={'actor':'host','chain_id':cid,'allow_write':False,'allow_model':False}
                state=core.operate('chain.get',context,{})['result'];ledger=core.operate('ledger.export',context,{})['result']
                save(folder/'chains'/(cid+'.state.json'),state);save(folder/'chains'/(cid+'.ledger.json'),ledger)
                chains.append({'chain_id':cid,'cut_seq':state['cut_seq'],'pending_count':len(state['pending']),'cb_count':len(state['cbs']),'jobs':len(state['jobs'])})
            report['chains']=chains
        finally:
            core.close()
            provider.close_http_capture()
            report.update(ended_at=time.time(),provider_attempts=len(provider.calls),real_provider_completed=sum(c['status']=='completed' for c in provider.calls))
            save(folder/'run.json',report)
            artifacts={p.relative_to(folder).as_posix():{'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in folder.rglob('*') if p.is_file() and not p.is_relative_to(harness.home)}
            save(folder/'manifest.json',{'schema':'sic-dsh-live-artifacts-v1','run_kind':report['kind'],'files':artifacts})
            print(json.dumps({'status':report['status'],'directory':str(folder),'provider_attempts':len(provider.calls),'completed':report['real_provider_completed']}))

if __name__=='__main__':main()
