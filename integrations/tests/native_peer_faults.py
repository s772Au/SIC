"""Actual isolated DSH profile/Loader/native-tool fault matrix.

Only the nondeterministic model and local transport are fixtures. No existing
Core/UI service, desktop profile, real provider or user session is used.
All evidence (including own synthetic requests) stays in the private output.
"""
import argparse,copy,hashlib,json,os,subprocess,sys,threading,time,uuid
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from pathlib import Path
from urllib.request import Request,ProxyHandler,build_opener
from urllib.error import HTTPError

ROOT=Path(__file__).resolve().parents[2];sys.path[:0]=[str(ROOT),str(ROOT/'integrations/tests')]
from sic_core import Core
from sic_core.server import make_server
from sic_core.bridge import make_bridge_server
from verify_dsh_harness import session_records,sha
SOURCE_FILES=[path.relative_to(ROOT).as_posix() for path in sorted((ROOT/'sic_core').glob('*.py'))]+['sic_core/operations.json','sic_core/dsh_plugin/index.mjs','sic_core/dsh_plugin/peer-host.mjs','integrations/tests/native_peer_faults.py']
SOURCE_SNAPSHOT={name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in SOURCE_FILES}

CASES=['cancel-during-body','revoke-before-dispatch','revoke-during-body','lost-completion-response','unknown-result-recovery','concurrent-peers','lease-expires-during-body','mixed-peer-stop','host-transport-disconnect','pause-editor-and-wake','pause-before-dispatch-and-wake','pause-body-and-wake','stop-editor-terminal','explicit-failed-attempt-retry','message-only-acl-consumption','builtin-read-roundtrip']
def save(path,value):path.write_text(json.dumps(value,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
def check(condition,message):
    if not condition:raise AssertionError(message)
def records(path):
    values=[]
    for line in path.read_text(encoding='utf-8-sig',errors='replace').splitlines():
        try:values.append(json.loads(line))
        except ValueError:pass
    return values
def background(server):
    thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();return thread

class Harness:
    def __init__(self,args,folder):
        self.args=args;self.folder=folder;self.home=folder/'home';self.home.mkdir();(self.home/'.sic-integration-only').write_text('Owned synthetic native fault tests only.')
        self.env={**os.environ,'DSH_HOME':str(self.home),'NO_PROXY':'localhost,127.0.0.1,::1','HTTP_PROXY':'','HTTPS_PROXY':'','CI':'1'}
        self.profile='sic-native-faults';self.processes=[]
    def run(self,arguments,log,timeout=180):
        with log.open('w',encoding='utf-8') as stream:
            result=subprocess.run([str(self.args.dsh_command),*arguments],cwd=ROOT,env=self.env,stdout=stream,stderr=subprocess.STDOUT,timeout=timeout,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
        check(result.returncode==0,'DSH setup command failed; see private '+log.name)
    def setup(self):
        self.run(['--profile',self.profile,'--from-default-profile','headless','--dump-config'],self.folder/'initial-config.yml')
        self.run(['plugin','--profile',self.profile,'add',str(ROOT/'sic_core/dsh_plugin')],self.folder/'plugin-install.log')
        self.run(['--profile',self.profile,'--help'],self.folder/'headless-help.txt')
    def start(self,overlay,prompt,log,resume=None):
        command=[str(self.args.dsh_command),'--profile',self.profile,'--patch',str(overlay)]
        if resume:command.extend(['--session-id',resume])
        command.extend(['--json',prompt]);stream=log.open('w',encoding='utf-8')
        process=subprocess.Popen(command,cwd=ROOT,env=self.env,stdout=stream,stderr=subprocess.STDOUT,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
        self.processes.append((process,stream));return process
    def settle(self,process,timeout=90):
        process.wait(timeout=timeout)
        for child,stream in self.processes:
            if child is process:stream.flush();stream.close()
        return process.returncode
    def close(self):
        for process,stream in self.processes:
            if process.poll() is None:
                # Only the exact process tree started by this fixture is owned.
                subprocess.run(['taskkill','/PID',str(process.pid),'/T','/F'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0),timeout=15)
                process.wait(10)
            if not stream.closed:stream.close()

class OwnedCore:
    def __init__(self,folder,case,harness):
        self.folder=folder;self.case=case;self.harness=harness;self.events=[];self.model_requests=[];self.guard=threading.Lock();self.fault_done=False;self.complete_count=0;self.transport_cut=False;self.editor_waiting=threading.Event();self.editor_release=threading.Event();self.pause_target=None
        self.peer_failed_once=False;self.retry_waiting=threading.Event();self.retry_release=threading.Event()
        self.builtin_file=harness.home/'owned-native-read.txt';self.builtin_marker='SIC_BUILTIN_READ_'+uuid.uuid4().hex
        if case=='builtin-read-roundtrip':self.builtin_file.write_text(self.builtin_marker+'\nsecond owned line\n',encoding='utf-8')
        self.token=uuid.uuid4().hex;os.environ['SIC_NATIVE_FAULT_TOKEN']=self.token;os.environ['SIC_NATIVE_FIXTURE_KEY']='own-loopback-only';os.environ['LITELLM_LOCAL_MODEL_COST_MAP']='true';os.environ['LITELLM_TELEMETRY']='false'
        self.sender_token=uuid.uuid4().hex;os.environ['SIC_NATIVE_SENDER_TOKEN']=self.sender_token
        os.environ['NO_PROXY']='localhost,127.0.0.1,::1'
        for key in ('HTTP_PROXY','HTTPS_PROXY','ALL_PROXY'):os.environ.pop(key,None)
        harness.env['SIC_NATIVE_FAULT_TOKEN']=self.token
        owner=self
        class Model(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_POST(self):
                request=json.loads(self.rfile.read(int(self.headers['Content-Length'])));serialized=json.dumps(request,ensure_ascii=False)
                try:material=json.loads(request['messages'][-1]['content'])
                except (ValueError,TypeError,KeyError):material={}
                calls=None
                if isinstance(material,dict) and 'editable_text_units' in material:
                    if owner.case in ('pause-editor-and-wake','stop-editor-terminal') and 'SIC_PEER_WORK_TOOL_TEST' in serialized and not owner.editor_waiting.is_set():
                        owner.editor_waiting.set();check(owner.editor_release.wait(15),'Owned Editor barrier was not released')
                    time.sleep(.15);role='editor';content=json.dumps({'ops':[],'pvi':{}})
                else:
                    role='worker'
                    if 'SIC_EXPLICIT_RETRY_CONSUME' in serialized:content='SIC_EXPLICIT_RETRY_CONSUMED' if 'SIC_PEER_RECOVERED_KNOWN_FAILURE' in serialized else 'SIC_RETRY_MISSING_FROM_NEXT_CONTEXT'
                    elif 'SIC_EXPLICIT_RETRY_HOLD' in serialized:
                        owner.retry_waiting.set();check(owner.retry_release.wait(20),'Explicit retry operator did not settle');content='SIC_RETRY_OPERATION_SETTLED'
                    elif 'SIC_UNKNOWN_RECOVERY_INSPECTION' in serialized:content='SIC_UNKNOWN_INSPECTED_WITHOUT_REEXECUTION'
                    elif 'SIC_PEER_TOOL_DONE' in serialized:content='SIC_NATIVE_FAULT_SUCCESS_ACK'
                    elif 'Synthetic peer task did not complete' in serialized:content='SIC_NATIVE_FAULT_EXPECTED_FAILURE_ACK'
                    elif owner.case=='builtin-read-roundtrip' and 'SIC_PEER_WORK_TOOL_TEST' in serialized:
                        if owner.builtin_marker in serialized:content='SIC_PEER_BUILTIN_READ_CONSUMED'
                        else:
                            schema=next((tool['function'] for tool in request.get('tools',[]) if tool.get('function',{}).get('name')=='read'),None)
                            check(schema is not None and schema['parameters']['properties']['file_path']['type']=='string' and 'file_path' in schema['parameters']['required'],'Actual native read schema missing or different')
                            check(set(('offset','limit'))<=set(schema['parameters']['properties']),'Actual native read optional args differ')
                            save(owner.folder/'builtin-registration-proof.json',{'registered_tool':'read','source_reference':'DSH packages/fs/tool-fs/src/read.ts applyReadTool','profile_package':'@deepseek-ai/dsh-tool-fs','actual_runtime_schema':schema,'owned_file_sha256':hashlib.sha256(owner.builtin_file.read_bytes()).hexdigest()})
                            content='Read the newly created owned fixture through the built-in read tool';calls=[{'id':'owned_builtin_read','type':'function','function':{'name':'read','arguments':json.dumps({'file_path':str(owner.builtin_file),'offset':1,'limit':2})}}]
                    elif owner.case=='explicit-failed-attempt-retry' and 'SIC_PEER_WORK_TOOL_TEST' in serialized and 'SIC_NATIVE_TOOL_FAILED' in serialized:
                        if not owner.peer_failed_once:
                            owner.peer_failed_once=True
                            with owner.guard:owner.model_requests.append({'role':'worker','request':request,'response':None,'fault':'owned-peer-worker-failure','time':time.time()})
                            body=b'{"error":{"message":"Owned peer Worker unavailable","type":"owned_fixture","code":"owned_fixture"}}';self.send_response(503);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body);return
                        content='SIC_PEER_RECOVERED_KNOWN_FAILURE'
                    elif 'SIC_PEER_WORK_TOOL_TEST' in serialized and 'SIC_SYNTHETIC_TOOL_RESULT' in serialized:content='SIC_PEER_RESPONSE_OK consumed owned native tool result'
                    elif 'SIC_PEER_WORK_TOOL_TEST' in serialized:
                        content='SIC_PEER_WORK_TOOL_TEST actual native work follows';calls=[{'id':'owned_peer_wait','type':'function','function':{'name':'sic_fixture_wait','arguments':'{}'}}]
                    else:
                        content='Owned native fault test requests historical peers';calls=[{'id':'owned_parent_peer','type':'function','function':{'name':'sic_fixture_peer_roundtrip','arguments':'{}'}}]
                message={'role':'assistant','content':content}
                if calls:message['tool_calls']=calls
                response={'id':'own-'+uuid.uuid4().hex,'object':'chat.completion','created':1,'model':request['model'],'choices':[{'index':0,'finish_reason':'tool_calls' if calls else 'stop','message':message}],'usage':{'prompt_tokens':12,'completion_tokens':8,'total_tokens':20}}
                with owner.guard:owner.model_requests.append({'role':role,'request':request,'response':response,'time':time.time()})
                body=json.dumps(response).encode();self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
        self.model=ThreadingHTTPServer(('127.0.0.1',0),Model);background(self.model)
        role={'model':'openai/sic-native-fixture','api_base':f'http://127.0.0.1:{self.model.server_port}/v1','api_key_env':'SIC_NATIVE_FIXTURE_KEY','route_id':'owned-native-loopback','tokenizer_model':'gpt-4o','max_input_tokens':110000,'max_output_tokens':4096,'context_window':128000,'tokenizer_status':'fixture-not-provider-verified'}
        self.config={'tokens':{'SIC_NATIVE_FAULT_TOKEN':'host'},'roles':{'worker':role,'editor':role},'bridge':{'allow_write':True,'allow_model':True,'editor_mode':'model','seal_bytes':1,'public_model':'sic-worker','host_tools_enabled':True,'host_tool_allowlist':['sic_fixture_wait'],'host_tool_wait_seconds':30,'host_tool_lease_seconds':30}}
        self.config['tokens']['SIC_NATIVE_SENDER_TOKEN']='sender'
        if case=='builtin-read-roundtrip':self.config['bridge']['host_tool_allowlist']=['read']
        if case in ('lease-expires-during-body','host-transport-disconnect'):self.config['bridge']['host_tool_lease_seconds']=1.5
        self.start_core();self.http=build_opener(ProxyHandler({}))
        class Proxy(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_GET(self):self.forward()
            def do_POST(self):self.forward()
            def forward(self):
                raw=self.rfile.read(int(self.headers.get('Content-Length',0)));payload=json.loads(raw) if raw else {};action=payload.get('action')
                if self.path=='/api/v2/bridge/host/action' and owner.transport_cut:
                    self.respond(503,b'{"error":{"code":"owned-host-transport-disconnected"}}');return
                if self.path=='/api/v2/bridge/host/action' and action=='dispatch' and owner.case=='revoke-before-dispatch' and not owner.fault_done:
                    owner.fault_done=True;owner.withdraw_tool(payload['task_id']);owner.events.append({'fault':'withdraw-before-dispatch','time':time.time()})
                if self.path=='/api/v2/bridge/host/action' and action=='dispatch' and owner.case=='pause-before-dispatch-and-wake' and not owner.fault_done:
                    owner.fault_done=True;task=next(t for t in owner.tasks() if t['task_id']==payload['task_id']);owner.signal_peer(task,'wait');owner.events.append({'fault':'paused-before-dispatch','time':time.time()})
                if self.path=='/api/v2/bridge/host/action' and action=='complete':
                    owner.complete_count+=1
                    if owner.case=='unknown-result-recovery' and not owner.fault_done:
                        self.respond(503,b'{"error":{"code":"owned-drop-before-forward"}}');return
                headers={'Authorization':self.headers.get('Authorization',''),'Content-Type':'application/json'}
                if self.headers.get('x-sic-bridge-session'):headers['x-sic-bridge-session']=self.headers['x-sic-bridge-session']
                request=Request(f'http://127.0.0.1:{owner.bridge.server_port}'+self.path,data=raw if self.command=='POST' else None,headers=headers,method=self.command)
                try:
                    with owner.http.open(request,timeout=90) as response:status=response.status;body=response.read();content_type=response.headers.get('Content-Type','application/json')
                except HTTPError as error:status=error.code;body=error.read();content_type='application/json'
                if self.path=='/api/v2/bridge/host/action' and action=='complete' and owner.case=='lost-completion-response' and not owner.fault_done:
                    owner.fault_done=True;owner.events.append({'fault':'completion-committed-response-dropped','upstream_status':status,'time':time.time()});status=503;body=b'{"error":{"code":"owned-drop-after-commit"}}'
                self.respond(status,body,content_type)
            def respond(self,status,body,content_type='application/json'):
                self.send_response(status);self.send_header('Content-Type',content_type);self.send_header('Content-Length',str(len(body)));self.end_headers()
                try:self.wfile.write(body)
                except (BrokenPipeError,ConnectionResetError):pass
        self.proxy=ThreadingHTTPServer(('127.0.0.1',0),Proxy);background(self.proxy)
    def start_core(self,core_port=0,bridge_port=0):
        self.core=Core(str(self.folder/'core.db'),self.config);self.server=make_server(self.core,self.config,port=core_port);self.bridge=make_bridge_server(self.core,self.config,port=bridge_port)
        background(self.server);background(self.bridge);self.core.runtime.start()
    def stop_core(self):
        self.bridge.shutdown();self.server.shutdown();self.bridge.server_close();self.server.server_close();self.core.close()
    def restart_core(self):
        a,b=self.server.server_port,self.bridge.server_port;self.stop_core();self.start_core(a,b)
    def op(self,chain,operation,payload,allow_model=False,actor='host'):
        token=self.token if actor=='host' else self.sender_token
        request=Request(f'http://127.0.0.1:{self.server.server_port}/api/v2/operations',data=json.dumps({'operation':operation,'context':{'actor':actor,'chain_id':chain,'allow_write':True,'allow_model':allow_model},'payload':payload}).encode(),headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'})
        with self.http.open(request,timeout=20) as response:answer=json.load(response)
        check(answer['ok'],'owned operator control denied');return answer['result']
    def tasks(self):
        with self.core.store.transaction():return [json.loads(raw) for (raw,) in self.core.store.db.execute('SELECT state FROM host_tool_tasks')]
    def withdraw_tool(self,task_id):
        task=next(t for t in self.tasks() if t['task_id']==task_id)
        tools=self.op(task['root_chain_id'],'chain.get',{})['tools']
        for tool in tools:
            if tool['name']=='sic_fixture_wait':tool['execute_acl']=[]
        self.op(task['root_chain_id'],'tools.configure',{'tools':tools})
    def cancel(self,task):
        state=self.op(task['chain_id'],'chain.get',{});self.op(task['root_chain_id'],'runtime.recover',{'action':'cancel_collaboration','request_id':state['collaboration_task']['request_id'],'confirm':True})
    def stop_peer(self,task):
        self.signal_peer(task,'stop')
    def signal_peer(self,task,signal):
        checkpoint=self.op(task['chain_id'],'chain.checkpoint',{})['checkpoint_id']
        socket=self.op(task['chain_id'],'socket.create',{'checkpoint_id':checkpoint,'signal_acl':['host']})['socket_id']
        self.op(task['root_chain_id'],'socket.signal',{'socket_id':socket,'signal':signal})
        self.pause_target={**task,'signal_socket':socket,'paused_at':time.time()}
    def editor_target(self):
        with self.core.store.transaction():
            for (raw,) in self.core.store.db.execute('SELECT state FROM chains'):
                state=json.loads(raw)
                if state.get('collaboration_task'):return {'chain_id':state['chain_id'],'root_chain_id':state['collaboration_task']['sender']}
    def send_scoped_messages(self,task):
        root=task['root_chain_id'];checkpoint=self.op(root,'chain.checkpoint',{})['checkpoint_id'];socket=self.op(root,'socket.create',{'checkpoint_id':checkpoint,'message_acl':['sender']})['socket_id']
        sender=self.op(None,'chain.create',{},actor='sender')['chain_id'];denied=[]
        board=self.op(root,'board.get',{});self.op(root,'board.update',{'expected_revision':board['revision'],'value':{'owned_notice':'SIC_NATIVE_BOARD_I'}})
        for chain,operation,payload in [(root,'chain.get',{}),(sender,'socket.spawn',{'socket_id':socket}),(sender,'socket.signal',{'socket_id':socket,'signal':'stop'}),(sender,'board.get',{'board_id':board['board_id']}),(sender,'board.update',{'board_id':board['board_id'],'expected_revision':1,'value':{}})]:
            try:self.op(chain,operation,payload,actor='sender')
            except HTTPError as error:check(error.code==403,'Message-only actor denial status differs');denied.append(operation)
            else:raise AssertionError('Message-only actor obtained history or execution control')
        for lifecycle in ('P','V'):self.op(sender,'socket.message',{'socket_id':socket,'message_id':'owned-message-'+lifecycle,'lifecycle':lifecycle,'text':'SIC_NATIVE_MESSAGE_'+lifecycle},actor='sender')
        self.events.append({'fault':'message-only-sender','denied':denied,'delivered_lifecycles':['P','V'],'time':time.time()})
    def wake_if_safe(self):
        if not self.pause_target:return
        target=self.pause_target;state=self.op(target['chain_id'],'chain.get',{})
        with self.core.store.transaction():workers=sum(self.core.store.get(uid,'usage')['role']=='worker' for uid in state['usage'])
        if self.case=='pause-editor-and-wake':
            if not any(job['status']=='ready' for job in state['jobs'].values()) or time.time()-target['paused_at']<.5:return
            check(workers==0,'Paused peer admitted a new Worker')
        elif self.case=='pause-body-and-wake':
            if not any(t['status']=='completed' for t in self.tasks()) or time.time()-target['paused_at']<.5:return
            check(workers==1,'Paused peer continued after its native tool completed')
        elif self.case=='pause-before-dispatch-and-wake':
            if time.time()-target['paused_at']<.5:return
            check(not any(t.get('dispatched_at') for t in self.tasks()),'Paused reservation dispatched a native tool')
        else:return
        self.op(target['root_chain_id'],'socket.signal',{'socket_id':target['signal_socket'],'signal':'wake'});self.events.append({'fault':'wake-after-safe-wait','paused_worker_calls':workers,'time':time.time()});self.pause_target=None
    def overlay(self):
        count=2 if self.case in ('concurrent-peers','mixed-peer-stop') else 1
        value=f'''- id: sic-core-dsh
  config:
    provider: sic-core
    baseURL: http://127.0.0.1:{self.proxy.server_port}/v1
    apiKeyEnv: SIC_NATIVE_FAULT_TOKEN
    model: sic-worker
    contextWindow: 128000
    maxTokens: 4096
    inputPricePerMillion: 0
    outputPricePerMillion: 0
    cacheReadPricePerMillion: 0
    cacheWritePricePerMillion: 0
    timeoutMs: 30000
    toolEvents: true
    fixtureWaitTool: {'false' if self.case=='builtin-read-roundtrip' else 'true'}
    fixtureWaitMs: 3000
    fixtureWaitFailure: {'true' if self.case=='explicit-failed-attempt-retry' else 'false'}
    peerTools: ['{'read' if self.case=='builtin-read-roundtrip' else 'sic_fixture_wait'}']
    peerConcurrency: 2
    fixturePeerTool: true
    fixturePeerCount: {count}
    fixturePeerReturnLifecycle: {'P' if self.case=='message-only-acl-consumption' else 'V'}
    fixtureCorePort: {self.server.server_port}
- id: agent-default-model
  config:
    provider: sic-core
    model: sic-worker
'''
        for name in ['compaction-basic','command-compact','tool-result-pruner','llm-pi-ai','llm-deepseek','llm-deepseek-account','session-title-llm','session-log-deepseek','session-telemetry-otel','web-search-deepseek','web-deepseek-api']:value+=f'- id: {name}\n  disabled: true\n'
        path=self.folder/'fixture.overlay.yml';path.write_text(value,encoding='utf-8');return path
    def close(self):
        self.editor_release.set()
        self.retry_release.set()
        self.proxy.shutdown();self.proxy.server_close();self.stop_core();self.model.shutdown();self.model.server_close();save(self.folder/'model-evidence.json',self.model_requests);save(self.folder/'fault-events.json',self.events)

def wait_native(harness,owned,process):
    deadline=time.monotonic()+65;acted=False
    while process.poll() is None and time.monotonic()<deadline:
        if owned.case in ('pause-editor-and-wake','stop-editor-terminal') and owned.editor_waiting.is_set() and not acted:
            target=owned.editor_target();owned.signal_peer(target,'wait' if owned.case=='pause-editor-and-wake' else 'stop');owned.events.append({'fault':owned.case,'time':time.time()});owned.editor_release.set();acted=True
        if owned.case=='pause-body-and-wake' and not acted:
            running=[t for t in owned.tasks() if t.get('dispatched_at') and t['status']=='running']
            if running and time.time()-running[0]['dispatched_at']>.25:owned.signal_peer(running[0],'wait');owned.events.append({'fault':owned.case,'time':time.time()});acted=True
        owned.wake_if_safe()
        if owned.case in ('cancel-during-body','revoke-during-body','host-transport-disconnect','mixed-peer-stop','message-only-acl-consumption') and not acted:
            running=[t for t in owned.tasks() if t.get('dispatched_at') and t['status']=='running']
            if running and (owned.case!='mixed-peer-stop' or len(running)==2) and time.time()-running[-1]['dispatched_at']>.25:
                if owned.case=='cancel-during-body':owned.cancel(running[0])
                elif owned.case=='revoke-during-body':owned.withdraw_tool(running[0]['task_id'])
                elif owned.case=='mixed-peer-stop':owned.stop_peer(running[0])
                elif owned.case=='message-only-acl-consumption':owned.send_scoped_messages(running[0])
                else:owned.transport_cut=True
                owned.events.append({'fault':owned.case,'time':time.time()});acted=True
        time.sleep(.025)
    check(process.poll() is not None,'native DSH test timed out; do not automatically replay its tools')
    return harness.settle(process)

def verify_case(case,harness,owned,log):
    cli=records(log);parent=next(v['sessionId'] for v in cli if v.get('type')=='session');native,_=session_records(harness.home,parent)
    tasks=[task for task in owned.tasks() if task['session_key']==parent];expected=0 if case=='stop-editor-terminal' else 2 if case in ('concurrent-peers','mixed-peer-stop') else 1
    check(len(tasks)==expected,'native host attempt count differs')
    peer_reports=[]
    for task in tasks:
        check(task.get('peer_session_id') and task['peer_session_id']!=parent,'real independent native peer missing')
        peer,_=session_records(harness.home,task['peer_session_id']);events=lambda kind:[v['data'] for v in peer if v.get('type')=='plugin:sic/'+kind]
        body=events('fixture-body');starts=[e for e in body if e['phase']=='started'];ends=[e for e in body if e['phase']=='ended'];aborts=[e for e in body if e['phase']=='aborted'];dispatch=events('peer-dispatch')
        bindings=events('peer-binding');check(len(bindings)==1,'Native peer binding missing')
        projection=task['provider_projection'];check(bindings[0]['task']['provider_projection']==projection,'Native and Core provider projection differ')
        with owned.core.store.transaction():
            check(owned.core.store.get(projection['request_id'],'provider_request')['request']==projection['request'],'Stored peer request differs')
            check(owned.core.store.get(projection['response_id'],'provider_response')['response']==projection['response'],'Stored peer response differs')
        if case=='builtin-read-roundtrip':
            check(not body and len(dispatch)==1 and task['name']=='read','Built-in test invoked custom wait or skipped actual dispatch')
            expected_file=(harness.home/'owned-native-read.txt').resolve();check(Path(task['arguments']['file_path']).resolve()==expected_file,'Built-in tool read a different path')
            marker=expected_file.read_text(encoding='utf-8').splitlines()[0]
            result=events('peer-result');check(task['status']=='completed' and len(result)==1 and not result[0]['result'].get('isError') and marker in json.dumps(result[0]['result']),'Built-in actual read result missing')
            with owned.core.store.transaction():
                child=owned.core.store.state(task['chain_id']);calls=[owned.core.store.get(uid,'usage') for uid in child['usage'] if owned.core.store.get(uid,'usage')['role']=='worker'];requests=[owned.core.store.get(call['source_refs']['request_id'],'provider_request')['request'] for call in calls]
            check(len(requests)==2 and expected_file.name in json.dumps(requests[0]) and marker not in json.dumps(requests[0]) and marker in json.dumps(requests[1]),'Actual read goal/content not preserved across peer requests')
            proof=json.loads((log.parent/'builtin-registration-proof.json').read_text(encoding='utf-8'));check(proof['owned_file_sha256']==hashlib.sha256(expected_file.read_bytes()).hexdigest(),'Owned fixture content changed')
        elif case=='revoke-before-dispatch':
            check(not starts and not dispatch and not task.get('dispatched_at'),'withdrawn tool actually executed')
        else:check(len(starts)==1 and len(dispatch)==1,'native body duplicated or actual dispatch absent')
        if case in ('cancel-during-body','revoke-during-body'):
            check(len(aborts)==1 and not ends,'native body did not cooperatively abort')
            check(task['status'] in ('cancelled','indeterminate') and 'result' not in task,'cancelled result adopted')
        if case in ('lease-expires-during-body','host-transport-disconnect'):
            check(task['status']=='indeterminate' and 'result' not in task,'expired/disconnected outcome adopted')
            check(len(aborts)==1 and not ends,'expired/disconnected native body did not abort')
        if case=='mixed-peer-stop':
            if task['status']=='completed':check(len(ends)==1 and not aborts,'Unaffected sibling did not complete')
            else:check(len(aborts)==1 and not ends and 'result' not in task,'Stopped peer result adopted')
        if case=='unknown-result-recovery':
            check(task['status']=='indeterminate' and 'result' not in task,'uncertain completion silently adopted')
            check(len(ends)==1 and len(events('peer-result'))==1 and not events('peer-result-delivered'),'unknown result outbox or receipt differs')
        if case in ('lost-completion-response','concurrent-peers','pause-editor-and-wake','pause-before-dispatch-and-wake','pause-body-and-wake','message-only-acl-consumption'):
            check(task['status']=='completed' and len(ends)==1 and len(events('peer-result'))==1 and len(events('peer-result-delivered'))==1,'completion receipt or real body differs')
        if case=='explicit-failed-attempt-retry':check(task['status']=='failed' and len(ends)==1 and len(events('peer-result'))==1 and events('peer-result')[0]['result'].get('isError') is True,'Known native tool failure missing')
        peer_reports.append({'task_id':task['task_id'],'chain_id':task['chain_id'],'peer_session':task['peer_session_id'],'status':task['status'],'native_body_started':len(starts),'native_body_ended':len(ends),'native_body_aborted':len(aborts),'dispatch_count':len(dispatch),'native_record_digest':sha(peer),'interval':[starts[0]['time_ms']/1000,ends[0]['time_ms']/1000] if starts and ends else None})
    check(any(v.get('type')=='final' and v.get('text') in ('SIC_NATIVE_FAULT_SUCCESS_ACK','SIC_NATIVE_FAULT_EXPECTED_FAILURE_ACK') for v in cli),'parent did not consume success/failure through native model turn')
    projections=[v['data'] for v in native if v.get('type')=='plugin:sic/provider-projection']
    marker='SIC_PEER_TOOL_DONE' if case in ('lost-completion-response','concurrent-peers','pause-editor-and-wake','pause-before-dispatch-and-wake','pause-body-and-wake','message-only-acl-consumption','builtin-read-roundtrip') else 'Synthetic peer task did not complete'
    check(any(marker in json.dumps(item.get('request',{})) for value in projections for item in value['projections']),'Actual final parent Worker request did not consume native outcome')
    if case=='lost-completion-response':check(owned.complete_count==2,'lost response must resend receipt once, never native tool')
    if case=='concurrent-peers':
        intervals=[r['interval'] for r in peer_reports];check(min(x[1] for x in intervals)>max(x[0] for x in intervals),'native peer tool bodies did not actually overlap')
        check(len({r['peer_session'] for r in peer_reports})==2 and len({r['chain_id'] for r in peer_reports})==2,'peer identities collapsed')
    if case=='mixed-peer-stop':check(sum(t['status']=='completed' for t in tasks)==1,'Peer stop affected the independent sibling')
    if case=='message-only-acl-consumption':
        check(any(all(marker in json.dumps(item['request']) for marker in ('SIC_NATIVE_MESSAGE_P','SIC_NATIVE_MESSAGE_V','SIC_NATIVE_BOARD_I')) for value in projections for item in value['projections']),'Real requester input did not consume P/V and current shared-board I')
        with owned.core.store.transaction():
            root=owned.core.store.state(tasks[0]['root_chain_id']);blocks=[owned.core.store.get(cid,'cb') for cid in root['cbs']];persistent=next(cb for cb in blocks if 'SIC_NATIVE_MESSAGE_P' in cb['body'])
            request_id=owned.core.store.state(tasks[0]['chain_id'])['collaboration_task']['request_id'];request=root['collaborations'][request_id]
            check(request['return_lifecycle']=='P' and all(branch['return_lifecycle']=='P' for branch in request['branches']),'Native dispatch return policy was not frozen as P')
            result_messages=[message for message in root['inbox'] if message.get('request_id')==request_id]
            check(len(result_messages)==1 and result_messages[0]['lifecycle']=='P','Native automatic collaboration return changed P to V or redelivered')
            check(all(result_messages[0].get(key) is not None for key in ('received_at','received_event_id','receive_receipt_id')),'Native peer return lacks frozen receive-time/receipt provenance')
            check(any('SIC_PEER_TOOL_DONE' in block['body'] for block in blocks),'Native peer P result did not enter a published immutable CB')
            check('SIC_NATIVE_MESSAGE_V' not in persistent['body'],'V was embedded in the same persistent communication CB')
            cuts=[json.loads(raw) for (raw,) in owned.core.store.db.execute("SELECT data FROM objects WHERE kind='cut'")]
            cut=next(c for c in cuts if c['chain_id']==root['chain_id'] and 'owned-message-P' in c.get('inbox_ack',[]))
            volatile=owned.core.store.get(cut['volatile_source'],'source');check(any('SIC_NATIVE_MESSAGE_V' in unit['text'] for unit in volatile['units']),'V did not enter its own next WB')
            check(all('SIC_NATIVE_BOARD_I' not in block['body'] for block in blocks),'Shared-board I leaked into persistent CB body')
            check(all('SIC_NATIVE_BOARD_I' not in unit['text'] for group in root['groups'].values() for sid in group for unit in owned.core.store.get(sid,'source')['units']),'Shared-board I became an editable WB unit')
    if case=='stop-editor-terminal':
        with owned.core.store.transaction():states=[json.loads(raw) for (raw,) in owned.core.store.db.execute('SELECT state FROM chains')]
        child=next(state for state in states if state.get('collaboration_task'));root=next(state for state in states if state['chain_id']==child['collaboration_task']['sender'])
        check(child['run_state']=='stopped' and not any(owned.core.store.get(uid,'usage')['role']=='worker' for uid in child['usage']),'Stopped Editor launched Worker')
        check(root['collaborations'][child['collaboration_task']['request_id']]['status']=='failed','Stopped collaboration remained permanently waiting')
    return {'case':case,'passed':True,'parent_session':parent,'parent_native_digest':sha(native),'peer_attempts':peer_reports,'complete_transport_attempts':owned.complete_count,'real_harness':True,'real_provider_calls':0,'research_quality':'not_evaluated'}

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--dsh-command',type=Path,required=True);parser.add_argument('--output',type=Path,required=True);parser.add_argument('--cases',nargs='+',choices=CASES,default=CASES);args=parser.parse_args()
    check(args.dsh_command.is_file(),'Supported installed DSH CLI required');folder=args.output.resolve();check(not folder.exists(),'Use a new empty output; uncertain native attempts are never automatically replayed');folder.mkdir(parents=True)
    save(folder/'source-snapshot.json',SOURCE_SNAPSHOT)
    harness=Harness(args,folder);reports=[]
    try:
        harness.setup()
        for case in args.cases:
            output=folder/case;output.mkdir();owned=OwnedCore(output,case,harness);log=output/'native.jsonl'
            try:
                prompt='SIC_NATIVE_FAULTS_'+case
                if case=='builtin-read-roundtrip':prompt+=': historical peer must use the DSH built-in read tool on this newly created owned UTF-8 fixture: '+str(owned.builtin_file)
                process=harness.start(owned.overlay(),prompt,log);code=wait_native(harness,owned,process);check(code==0,'Native CLI failed; inspect private log before retrying')
                report=verify_case(case,harness,owned,log)
                if case in ('unknown-result-recovery','host-transport-disconnect'):
                    check('--session-id' in (folder/'headless-help.txt').read_text(encoding='utf-8',errors='replace'),'Installed headless profile has no supported persisted-session option')
                    before=copy.deepcopy(owned.tasks());owned.fault_done=True;owned.transport_cut=False;owned.restart_core();resumed_log=output/'resumed-native.jsonl'
                    resumed=harness.start(owned.overlay(),'SIC_UNKNOWN_RECOVERY_INSPECTION',resumed_log,resume=report['parent_session']);check(harness.settle(resumed)==0,'Explicit native resume failed')
                    check(any(v.get('type')=='final' and v.get('text')=='SIC_UNKNOWN_INSPECTED_WITHOUT_REEXECUTION' for v in records(resumed_log)),'Explicit recovery turn not consumed')
                    after=owned.tasks();check(len(after)==len(before) and all(t['status']=='indeterminate' for t in after),'Unknown attempts were redistributed after native/Core reopen')
                    old_peer,_=session_records(harness.home,report['peer_attempts'][0]['peer_session']);check(sum(v.get('type')=='plugin:sic/fixture-body' and v['data']['phase']=='started' for v in old_peer)==1,'Native tool reexecuted on resume')
                    parent,_=session_records(harness.home,report['parent_session'])
                    if case=='unknown-result-recovery':check(any(v.get('type')=='plugin:sic/peer-recovery-required' for v in parent),'Persisted result outbox did not expose explicit recovery requirement')
                    if case=='unknown-result-recovery':
                        receipt=next(v['data'] for v in old_peer if v.get('type')=='plugin:sic/peer-result')
                        late={'action':'complete','host_id':receipt['host_id'],'task_id':receipt['task_id'],'lease':receipt['lease'],'result':receipt['result'],'is_error':receipt['result'].get('isError') is True}
                        packet=Request(f'http://127.0.0.1:{owned.bridge.server_port}/api/v2/bridge/host/action',data=json.dumps(late).encode(),headers={'Authorization':'Bearer '+owned.token,'Content-Type':'application/json'})
                        try:owned.http.open(packet,timeout=10)
                        except HTTPError as error:check(error.code in (403,409),'Late original native receipt failed for wrong reason')
                        else:raise AssertionError('Unknown original native receipt silently adopted after resume')
                        report['late_original_native_receipt_rejected']=True
                    report.update(native_resume_after_core_restart=True,unknown_attempt_redispatches=0)
                if case=='explicit-failed-attempt-retry':
                    original=owned.tasks()[0];root=original['root_chain_id'];state=owned.op(original['chain_id'],'chain.get',{});request_id=state['collaboration_task']['request_id'];p={'request_id':request_id,'editor_mode':'model'}
                    count=len(owned.model_requests);began=time.time();unchanged=owned.op(root,'collaboration.run',p,allow_model=True)
                    check(unchanged['status']=='failed' and len(owned.model_requests)==count,'Resent failure silently started work')
                    owned.events.append({'fault':'failed-request-resend','started_at':began,'ended_at':time.time(),'model_calls_before':count,'model_calls_after':len(owned.model_requests)})
                    try:owned.op(root,'collaboration.run',{**p,'retry_failed':True},allow_model=True)
                    except HTTPError as error:
                        body=json.load(error);check(error.code==403 and body['error']['code']=='recovery_confirmation','Unacknowledged retry failed for wrong reason')
                    else:raise AssertionError('Retry without explicit duplicate-risk acknowledgement admitted')
                    check(len(owned.model_requests)==count,'Rejected retry invoked a model')
                    held_log=output/'explicit-retry-native.jsonl';held=harness.start(owned.overlay(),'SIC_EXPLICIT_RETRY_HOLD',held_log,resume=report['parent_session'])
                    check(owned.retry_waiting.wait(15),'Resumed native host did not register before explicit retry')
                    try:
                        recovered=owned.op(root,'collaboration.run',{**p,'retry_failed':True,'acknowledge_possible_duplicate':True},allow_model=True)
                        check(recovered['status']=='completed' and recovered['branches'][0]['attempt']==1,'Explicit attempt was not independently identified/completed')
                    finally:owned.retry_release.set()
                    check(harness.settle(held)==0,'Resumed native host failed during explicit retry')
                    consumed_log=output/'retry-consumed-native.jsonl';consumed=harness.start(owned.overlay(),'SIC_EXPLICIT_RETRY_CONSUME',consumed_log,resume=report['parent_session']);check(harness.settle(consumed)==0,'Native requester did not complete after retry')
                    check(any(v.get('type')=='final' and v.get('text')=='SIC_EXPLICIT_RETRY_CONSUMED' for v in records(consumed_log)),'Native requester did not consume retry result')
                    check(len(owned.tasks())==1,'Known original native tool was automatically repeated')
                    report.update(resend_new_model_calls=0,explicit_retry_attempt=1,original_native_tool_executions=1,retry_result_consumed_by_native_requester=True)
                save(output/'verification.json',report);reports.append(report);print(json.dumps({'case':case,'passed':True,'real_provider_calls':0}),flush=True)
            except Exception as error:
                save(output/'failure.json',{'case':case,'passed':False,'failure_type':type(error).__name__,'failure':str(error),'automatic_retry':False});raise
            finally:owned.close()
        save(folder/'matrix.json',{'passed':True,'cases':reports,'scope':'real DSH isolated CLI/Loader/native tools; model/transport owned fixtures','existing_services_touched':False,'real_provider_calls':0,'source_sha256':SOURCE_SNAPSHOT,'source_files_unchanged_on_disk':all(hashlib.sha256((ROOT/name).read_bytes()).hexdigest()==value for name,value in SOURCE_SNAPSHOT.items())})
    finally:harness.close()

if __name__=='__main__':main()
