"""Persistent full-history OpenAI Chat bridge over the new SIC Core.

Delivery is buffered SSE/JSON. Durable receipts prevent duplicate ingestion and
model replays; indeterminate provider attempts require explicit host recovery.
"""
import argparse,copy,hmac,json,os,threading,time,uuid
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from pathlib import Path
from .adapters import decode,digest
from .history import canonical,generated_assistant_equivalent
from .service import Core,CoreError,require
from .store import dumps,uid
from . import hierarchy

def process_alive(pid):
    if os.name=='nt':
        import ctypes
        kernel=ctypes.WinDLL('kernel32',use_last_error=True);kernel.OpenProcess.restype=ctypes.c_void_p;kernel.OpenProcess.argtypes=[ctypes.c_ulong,ctypes.c_int,ctypes.c_ulong];kernel.GetExitCodeProcess.argtypes=[ctypes.c_void_p,ctypes.POINTER(ctypes.c_ulong)];kernel.CloseHandle.argtypes=[ctypes.c_void_p]
        handle=kernel.OpenProcess(0x1000,False,pid)
        if not handle:return False
        try:
            code=ctypes.c_ulong();return bool(kernel.GetExitCodeProcess(handle,ctypes.byref(code))) and code.value==259
        finally:kernel.CloseHandle(handle)
    try:os.kill(pid,0);return True
    except ProcessLookupError:return False
    except PermissionError:return True

INTERNAL_TOOLS={'sic_source_read','sic_source_search','sic_operation_directory'} | set(Core._tool_operations())

class Bridge:
    def __init__(self,core,config):
        self.core=core;self.store=core.store;self.config=config;self.policy=config.get('bridge',{});self.instance=uuid.uuid4().hex
        from .host_tools import HostToolBroker
        if getattr(core,'host_tools',None) is None:core.host_tools=HostToolBroker(core,self.policy)
        self.host_tools=core.host_tools
        self.executor=ThreadPoolExecutor(max_workers=int(self.policy.get('editor_workers',2)),thread_name_prefix='sic-editor');self.drive_locks={};self.futures={};self.guard=threading.Lock();self.closed=False
        with self.store.transaction():
            schema='''CREATE TABLE IF NOT EXISTS bridge_bindings(actor TEXT NOT NULL,session_key TEXT NOT NULL,state TEXT NOT NULL,lease TEXT,lease_until REAL NOT NULL DEFAULT 0,PRIMARY KEY(actor,session_key));
            CREATE TABLE IF NOT EXISTS bridge_turns(actor TEXT NOT NULL,session_key TEXT NOT NULL,fingerprint TEXT NOT NULL,state TEXT NOT NULL,PRIMARY KEY(actor,session_key,fingerprint));'''
            for statement in schema.split(';'):
                if statement.strip():self.store.db.execute(statement)
            if 'lease_pid' not in [row[1] for row in self.store.db.execute('PRAGMA table_info(bridge_bindings)')]:self.store.db.execute('ALTER TABLE bridge_bindings ADD COLUMN lease_pid INTEGER')
    def close(self):self.closed=True;self.executor.shutdown(wait=True)
    def _binding(self,actor,key):
        row=self.store.db.execute('SELECT state FROM bridge_bindings WHERE actor=? AND session_key=?',(actor,key)).fetchone();return json.loads(row[0]) if row else None
    def _save(self,actor,key,state):self.store.db.execute('UPDATE bridge_bindings SET state=? WHERE actor=? AND session_key=?',(dumps(state),actor,key))
    def _turn(self,actor,key,fp):
        row=self.store.db.execute('SELECT state FROM bridge_turns WHERE actor=? AND session_key=? AND fingerprint=?',(actor,key,fp)).fetchone();return json.loads(row[0]) if row else None
    def _save_turn(self,actor,key,fp,value):self.store.db.execute('INSERT INTO bridge_turns VALUES(?,?,?,?) ON CONFLICT(actor,session_key,fingerprint) DO UPDATE SET state=excluded.state',(actor,key,fp,dumps(value)))
    def _state(self,binding,actor):
        state=self.store.state(binding['chain_id']);state['_actor']=actor;require(actor in state['owners'],'chain permission denied','forbidden',403)
        require(not any(self.store.revoked(x) for group in state['groups'].values() for x in group),'revoked chain dependency','revoked_dependency',403);return state
    def _op(self,actor,binding,operation,payload=None,model=False):
        return self.core.operate(operation,{'actor':actor,'chain_id':binding['chain_id'],'allow_write':True,'allow_model':model},payload or {})['result']
    def _allowed(self,model=False):
        require(self.policy.get('allow_write') is True,'Bridge host write authorization disabled','write_denied',403)
        require(not model or self.policy.get('allow_model') is True,'Bridge host model authorization disabled','model_denied',403)
    @staticmethod
    def normalize(request):
        require(isinstance(request,dict),'Chat request must be an object','invalid_request',400)
        request=copy.deepcopy(request);require(request.get('n',1)==1,'Only one Chat choice supported','unsupported',422);request.pop('n',None)
        require(isinstance(request.get('stream',False),bool),'stream must be boolean','invalid_request',400);request.pop('stream',None)
        options=request.pop('stream_options',{})
        require(isinstance(options,dict) and not set(options)-{'include_usage'},'unsupported stream_options','unsupported',422)
        require(request.get('store',False) is False,'Provider store=true is unsupported','unsupported',422)
        request.pop('store',None)
        source=decode('openai-chat',request);require(not source['open_calls'],'Full-history request ends with unresolved native calls','tool_group_open',409)
        require(bool(source['units']) or any(m.get('role')=='tool' for m in request['messages']),'Working input required','invalid_request',400)
        return request
    def sessions(self,actor):
        with self.store.transaction():
            rows=self.store.db.execute('SELECT session_key,state FROM bridge_bindings WHERE actor=?',(actor,)).fetchall()
            return [{'session_key':key,**{k:json.loads(raw).get(k) for k in ('chain_id','phase','current_fingerprint','editor_error')}} for key,raw in rows]
    def evidence(self,actor,key):
        """Return the exact model views for the last admitted turn, without credentials."""
        with self.store.transaction():
            binding=self._binding(actor,key);require(binding is not None,'Session binding not found','not_found',404)
            state=self._state(binding,actor);turn=self._turn(actor,key,binding['current_fingerprint'])
            require(turn is not None and turn['phase']=='delivered','No delivered turn evidence','not_ready',409)
            projections=[]
            for item in turn.get('projections',[]):
                value=self.store.get(item['request_id'],'provider_request')
                require(value.get('chain_id')==binding['chain_id'],'Projection is not on the authorized chain','forbidden',403)
                projections.append({**item,'request':value['request'],'cut_id':value.get('cut_id'),
                    'cache_frontier':value.get('cache_frontier'),'request_digest':digest(value['request'])})
            return {'chain_id':binding['chain_id'],'turn_fingerprint':binding['current_fingerprint'],
                    'published_cut':state['current_cut'],'projections':projections,
                    'usage':copy.deepcopy(turn.get('usage',[])),
                    'history_contract':'full-original-history plus these exact fixed-provider request views'}
    def _claim(self,actor,key):
        require(isinstance(key,str) and 0<len(key)<=256,'Explicit session identity required','session_required',400)
        lease=uid('lease')
        with self.store.transaction():
            binding=self._binding(actor,key)
            if binding is None:
                state=self.core._new(actor);state['runtime']={'enabled':True,'editor_mode':self.policy.get('editor_mode','model'),'worker_mode':'host','seal_chars':int(self.policy.get('seal_chars',self.policy.get('seal_bytes',65536))),'editor_workers':int(self.policy.get('editor_workers',2))};state['runtime_model_authorized']=self.policy.get('allow_model') is True;self.store.save(state);binding={'chain_id':state['chain_id'],'history':[],'generated_index':None,'phase':'idle','current_fingerprint':None,'editor_jobs':{},'editor_error':None,'tools_digest':None,'proposed_tools':[]}
                self.store.db.execute('INSERT INTO bridge_bindings(actor,session_key,state,lease,lease_until) VALUES(?,?,?,NULL,0)',(actor,key,dumps(binding)))
            row=self.store.db.execute('SELECT lease,lease_until,lease_pid FROM bridge_bindings WHERE actor=? AND session_key=?',(actor,key)).fetchone()
            require(not row[0] or row[1]<time.time() or row[2] is not None and not process_alive(row[2]),'Session has a concurrent request','session_busy',409)
            self._state(binding,actor);self.store.db.execute('UPDATE bridge_bindings SET lease=?,lease_until=?,lease_pid=? WHERE actor=? AND session_key=?',(lease,time.time()+float(self.policy.get('lease_seconds',900)),os.getpid(),actor,key))
        return binding,lease
    def _release(self,actor,key,lease):
        with self.store.transaction():self.store.db.execute('UPDATE bridge_bindings SET lease=NULL,lease_until=0 WHERE actor=? AND session_key=? AND lease=?',(actor,key,lease))
    def _check_history(self,binding,messages):
        expected=binding['history'];require(len(messages)>=len(expected),'History was truncated or rolled back; fork explicitly','history_mismatch',409)
        generated=binding.get('generated_index')
        for i,old in enumerate(expected):
            matches=generated_assistant_equivalent(old,messages[i]) if i==generated else canonical(old)==canonical(messages[i])
            require(matches,'Accepted history/protected prefix differs; use an explicit new session or fork','history_mismatch',409)
    def chat(self,actor,key,request):
        self._allowed(model=True);require(request.get('model')==self.policy.get('public_model','sic-worker'),'Unknown bridge model alias','unknown_model',404);normalized=self.normalize(request);fp=digest(normalized);binding,lease=self._claim(actor,key)
        try:
            with self.store.transaction():
                binding=self._binding(actor,key);self._state(binding,actor);turn=self._turn(actor,key,fp)
                if turn and turn['phase']=='delivered':return copy.deepcopy(turn['reply']),binding['chain_id']
                if turn and turn['phase']=='failed':raise CoreError('turn_failed','Previous attempt failed or is indeterminate; explicit authorized recovery required',409)
                if binding['phase']=='generating':require(binding['current_fingerprint']==fp,'A different turn is still unresolved','turn_pending',409)
                if turn is None:
                    self._check_history(binding,normalized['messages']);offset=len(binding['history']);new=normalized['messages'][offset:]
                    require(bool(new),'No new client input after last delivered assistant','no_new_input',409)
                    # Save ingestion and cursor together; no model call inside the transaction.
                    state=self._state(binding,actor)
                    schemas=normalized.get('tools',[]);require(isinstance(schemas,list),'tools must be an array','invalid_request',422);names=set();registered=[t for t in state['tools'] if t['name'] in INTERNAL_TOOLS]
                    for schema in schemas:
                        name=schema.get('function',{}).get('name');require(schema.get('type')=='function' and isinstance(name,str) and name not in names and name not in INTERNAL_TOOLS,'Invalid, duplicate or reserved tool declaration','invalid_request',422);names.add(name);registered.append({'name':name,'descriptor':schema['function'].get('description',name),'schema':copy.deepcopy(schema),'discover_acl':[actor],'expand_acl':[actor],'execute_acl':[actor],'expanded':True})
                    tools_digest=digest(schemas);schema_changed=binding.get('tools_digest') is not None and binding['tools_digest']!=tools_digest
                    self.core._dispatch('tools.configure',state,actor,{'tools':registered});binding['tools_digest']=tools_digest
                    state=self._state(binding,actor);payload={k:v for k,v in normalized.items() if k!='messages'};payload['messages']=copy.deepcopy(new)
                    if offset==0:
                        payload['messages']=copy.deepcopy(normalized['messages'])
                    result=self.core._dispatch('source.ingest',state,actor,{'format':'openai-chat','request':payload})
                    # Returned external results end only the corresponding observed tool executions.
                    state=self._state(binding,actor)
                    for message in new:
                        if message['role']=='tool' and message.get('tool_call_id') in state['open_tools']:
                            self.core._dispatch('harness.event',state,actor,{'type':'tool_end','call_id':message['tool_call_id'],'observed_by':'bridge-result'})
                    binding['history']=copy.deepcopy(normalized['messages']);binding['generated_index']=None;binding['phase']='generating';binding['current_fingerprint']=fp
                    turn={'phase':'ingested','attempt':0,'round':0,'source_id':result['source_id'],'schema_changed':schema_changed,'history':copy.deepcopy(normalized['messages']),'usage':[]};self._save_turn(actor,key,fp,turn);self._save(actor,key,binding)
            # The shared runtime owns editing; the Harness owns this route's Worker.
            self.core.runtime.notify(binding['chain_id'],actor,{'type':'bridge-source','session_key':key})
            should_wait=self.policy.get('schedule_policy','wait')=='wait' or turn.get('schema_changed',False)
            self._drive(actor,key,force=turn.get('schema_changed',False),wait=should_wait,blocking_worker=should_wait)
            with self.store.transaction():
                binding=self._binding(actor,key);state=self._state(binding,actor)
                require(state['run_state']=='running','Agent is paused; resume it before retrying this admitted turn','agent_waiting',409)
            reply=self._worker(actor,key,fp)
            with self.store.transaction():
                binding=self._binding(actor,key);self._state(binding,actor);turn=self._turn(actor,key,fp);turn.update(phase='delivered',reply=reply);self._save_turn(actor,key,fp,turn)
                binding['history']=copy.deepcopy(turn['history'])+[copy.deepcopy(reply['choices'][0]['message'])];binding['generated_index']=len(binding['history'])-1;binding['phase']='idle';self._save(actor,key,binding)
            self.kick(actor,key,force=bool(reply['choices'][0]['message'].get('tool_calls')))
            return copy.deepcopy(reply),binding['chain_id']
        except Exception as error:
            with self.store.transaction():
                turn=self._turn(actor,key,fp)
                if turn and turn['phase']!='delivered':
                    resumable=isinstance(error,CoreError) and error.code in ('agent_waiting','editor_wait_timeout')
                    turn['phase']='ingested' if resumable else 'failed';self._save_turn(actor,key,fp,turn)
                    binding=self._binding(actor,key);binding['phase']='awaiting_resume' if resumable else 'failed';self._save(actor,key,binding)
            raise
        finally:self._release(actor,key,lease)
    def _drive_lock(self,actor,key):
        with self.guard:return self.drive_locks.setdefault((actor,key),threading.Lock())
    def kick(self,actor,key,force=False):
        if self.closed:return
        with self.guard:
            prior=self.futures.get((actor,key))
            if prior and not prior.done():return
            self.futures[(actor,key)]=self.executor.submit(self._background,actor,key,force)
    def _background(self,actor,key,force):
        try:self._drive(actor,key,force,True)
        except Exception as error:
            with self.store.transaction():
                binding=self._binding(actor,key)
                if binding:binding['editor_error']={'type':type(error).__name__,'code':getattr(error,'code','editor_failed')};self._save(actor,key,binding)
    def _drive(self,actor,key,force=False,wait=True,blocking_worker=False):
        held_since=self.core.now() if blocking_worker else None
        lock=self._drive_lock(actor,key)
        if not lock.acquire(blocking=wait):return
        deadline=time.monotonic()+float(self.policy.get('editor_wait_seconds',120))
        try:
            with self.store.transaction():
                binding=self._binding(actor,key);state=self._state(binding,actor);eligible=set(state['pending'])
                require(not binding.get('editor_error'),'Editor failed; inspect candidate and recover explicitly','editor_failed',409)
            publications=0
            while publications<int(self.policy.get('max_publications_per_pass',8)):
                with self.store.transaction():
                    binding=self._binding(actor,key);state=self._state(binding,actor)
                    if not state['pending'] or state['pending'][0] not in eligible:return
                    if state['run_state']!='running':return
                    root=state['pending'][0]
                    jobs=[j for j in state['jobs'].values() if j['source_id']==root and j['status'] not in ('published','superseded','abandoned','failed')]
                    if not jobs:
                        candidates=[];total=0;threshold=int(self.policy.get('seal_bytes',65536))
                        frozen={self.store.get(wid,'wb')['source_id'] for wid in hierarchy.level(state,1)['wbs']}
                        for candidate in state['pending']:
                            if candidate not in eligible:break
                            # A formed WB remains a boundary even if its Editor
                            # failed or no job has been prepared yet. Let the
                            # Runtime publish/recover it without regrouping.
                            if candidate in frozen:
                                if not candidates:
                                    candidates.append(candidate);total=sum(len(u['text'].encode()) for u in self.core._source(state,candidate)['units'])
                                break
                            if any(j['source_id']==candidate and j['status'] not in ('published','superseded','abandoned','failed') for j in state['jobs'].values()):break
                            candidates.append(candidate);total+=sum(len(u['text'].encode()) for u in self.core._source(state,candidate)['units'])
                            if total>=threshold:break
                        if not force and total<threshold and root not in frozen:return
                        if len(candidates)>1:
                            self.core._dispatch('wb.form',state,actor,{'source_ids':candidates,'reason':'bridge-accumulated-boundary'})
                try:
                    outcome=self.core.runtime.run(binding['chain_id'],actor,{
                        'max_steps':1,'editor_mode':self.policy.get('editor_mode','model'),'continue_worker':False,
                        # The bridge measures the whole admission wait below,
                        # including waiting for another runtime's editor batch.
                        'blocking_worker':False})
                except CoreError as error:
                    if error.code not in ('runtime_busy','publication_deferred'):raise
                    if not wait:return
                    require(time.monotonic()<deadline,'Editor wait deadline reached; candidate retained','editor_wait_timeout',409)
                    with self.core.runtime.condition:self.core.runtime.condition.wait(timeout=0.05)
                    continue
                published=any(step.get('action')=='published' for step in outcome['steps'])
                if any(step.get('action')=='publication-deferred' for step in outcome['steps']):
                    if not wait:return
                    require(time.monotonic()<deadline,'Editor wait deadline reached; candidate retained','editor_wait_timeout',409)
                    with self.core.runtime.condition:self.core.runtime.condition.wait(timeout=0.05)
                    continue
                if not published:return
                publications+=1
        finally:
            lock.release()
            if held_since is not None:self._record_host_wait(actor,key,held_since,self.core.now())
    def _record_host_wait(self,actor,key,start,end):
        """Measure held Worker time, excluding observed tools/declared waits."""
        with self.store.transaction():
            binding=self._binding(actor,key);state=self._state(binding,actor)
            tools={};waits={};natural=[]
            for event in state['events']:
                if event.get('duplicate_observation'):continue
                when=event.get('time',end)
                if when>end:continue
                kind=event.get('type')
                if kind=='tool_start':tools[event['call_id']]=when
                elif kind=='tool_end' and event['call_id'] in tools:
                    natural.append((tools.pop(event['call_id']),when))
                elif kind=='wait':waits[event['wait_id']]=(when,when+event['seconds'])
                elif kind in ('wait_cancel','wait_complete') and event['wait_id'] in waits:
                    a,b=waits.pop(event['wait_id']);natural.append((a,min(b,when)))
            natural.extend((a,end) for a in tools.values());natural.extend(waits.values())
            cursor=start;segments=[]
            for a,b in sorted(natural):
                a=max(start,a);b=min(end,b)
                if b<=cursor or b<=a:continue
                if a>cursor:segments.append((cursor,a))
                cursor=max(cursor,b)
            if cursor<end:segments.append((cursor,end))
            wait_id=uid('host_wait')
            for a,b in segments:
                self.core._dispatch('harness.event',state,actor,{
                    'type':'interval','kind':'serial_wait','start':a,'end':b,
                    'clock':'core-epoch-perf-counter','wait_id':wait_id,
                    'reason':'bridge Worker admission held, excluding observed natural windows'})
    @staticmethod
    def _reply(raw,external=None):
        reply=copy.deepcopy(raw);require(len(reply.get('choices',[]))==1,'Unsupported completion choices','provider_response',422);choice=reply['choices'][0];message=choice['message'];require(message.get('role')=='assistant','Non-assistant completion','provider_response',422)
        require(choice.get('finish_reason') in ('stop','tool_calls','length'),'Unsupported completion termination','provider_response',422)
        allowed={'role','content','tool_calls','refusal','annotations','reasoning_content','reasoning','reasoning_text','reasoning_details'}
        # SDKs may wrap even null refusal/reasoning fields in this transport
        # container. Unwrap only fields the replay contract already supports.
        extras=message.pop('provider_specific_fields',None)
        if extras is not None:
            require(isinstance(extras,dict),'Invalid SDK completion metadata','provider_response',422)
            for name,value in extras.items():
                if value in (None,[],{}):continue
                require(name in allowed and name!='role','Non-replayable SDK completion field','provider_response',422)
                require(message.get(name) in (None,[],{}) or message[name]==value,'Conflicting SDK completion field','provider_response',422)
                message[name]=value
        require(not any(k not in allowed and v not in (None,[],{}) for k,v in message.items()),'Non-replayable completion field','provider_response',422)
        message={k:v for k,v in message.items() if k in allowed and (v is not None or k=='content')}
        if external is not None:message['tool_calls']=external
        if not message.get('tool_calls'):message.pop('tool_calls',None)
        else:require(choice['finish_reason']=='tool_calls','Truncated/native tool completion cannot be delivered safely','provider_response',422)
        choice['message']=message;reply['object']='chat.completion';return reply
    def _worker(self,actor,key,fp):
        admission_deadline=time.monotonic()+float(self.policy.get('editor_wait_seconds',120))
        for _ in range(int(self.policy.get('memory_tool_rounds',8))):
            with self.store.transaction():binding=self._binding(actor,key);self._state(binding,actor);turn=self._turn(actor,key,fp)
            payload={'idempotency_key':'bridge:'+digest([actor,key,fp,turn['attempt'],turn['round']]),
                     'policy':self.policy.get('schedule_policy','wait')}
            while True:
                try:
                    result=self._op(actor,binding,'provider.continue',payload,model=True)
                    break
                except CoreError as error:
                    # A background timer may start an Editor between _drive and
                    # provider admission. This rejection occurs before a model
                    # attempt; only that admission may be revisited. Provider,
                    # tool and indeterminate-call failures are never retried.
                    if error.code!='worker_wait':raise
                    require(time.monotonic()<admission_deadline,'Editor admission wait deadline reached; work retained','editor_wait_timeout',409)
                    self._drive(actor,key,force=True,wait=True,blocking_worker=True)
                    with self.core.runtime.condition:self.core.runtime.condition.wait(timeout=0.01)
            require('response' in result,'Worker outcome is failed or indeterminate; explicit recovery required','worker_indeterminate',409)
            raw=result['response'];reply=self._reply(raw);calls=reply['choices'][0]['message'].get('tool_calls',[]);internal=[c for c in calls if c['function']['name'] in INTERNAL_TOOLS];external=[c for c in calls if c['function']['name'] not in INTERNAL_TOOLS]
            with self.store.transaction():
                turn=self._turn(actor,key,fp)
                if result['call_id'] not in [u['call_id'] for u in turn['usage']]:turn['usage'].append({'call_id':result['call_id'],'usage':raw.get('usage')})
                if not any(x['call_id']==result['call_id'] for x in turn.setdefault('projections',[])):
                    turn['projections'].append({'call_id':result['call_id'],'request_id':result['request_id'],
                                               'response_id':result['response_id']})
                self._save_turn(actor,key,fp,turn)
            if internal:
                # Pure SIC reads are replay-safe; core authorization still applies.
                tool_messages=[]
                for call in internal:
                    args=json.loads(call['function']['arguments']);fingerprint=digest(call)
                    with self.store.transaction():
                        binding=self._binding(actor,key);state=self._state(binding,actor);turn=self._turn(actor,key,fp)
                        receipt=turn.setdefault('tool_receipts',{}).get(call['id'])
                        if receipt:
                            require(receipt['fingerprint']==fingerprint,'Internal tool identity reused with other arguments','tool_conflict',409)
                            value=receipt['result']
                        else:
                            value=self.core._dispatch('tools.invoke',state,actor,{'name':call['function']['name'],'arguments':args})
                            turn['tool_receipts'][call['id']]={'fingerprint':fingerprint,'result':value}
                            self._save_turn(actor,key,fp,turn)
                    if call['function']['name']=='sic_socket_dispatch':
                        self.core.runtime.notify(binding['chain_id'],actor,{'type':'collaboration.dispatch','request_id':args['request_id']})
                    tool_messages.append({'role':'tool','tool_call_id':call['id'],'content':dumps(value)})
                with self.store.transaction():
                    binding=self._binding(actor,key);state=self._state(binding,actor);turn=self._turn(actor,key,fp)
                    marker=str(turn['round'])
                    if marker not in turn.get('internal_results',[]):
                        self.core._dispatch('source.ingest',state,actor,{'request':{'messages':tool_messages}});turn.setdefault('internal_results',[]).append(marker);self._save_turn(actor,key,fp,turn)
            if external or not internal:
                reply=self._reply(raw,external if internal else None)
                with self.store.transaction():
                    binding=self._binding(actor,key);state=self._state(binding,actor)
                    for call in external:
                        self.core._dispatch('tools.authorize',state,actor,{'name':call['function']['name']})
                        if call['id'] not in binding.setdefault('proposed_tools',[]):binding['proposed_tools'].append(call['id'])
                    self._save(actor,key,binding)
                    turn=self._turn(actor,key,fp);usages=[u['usage'] for u in turn['usage']]
                    if usages and all(isinstance(u,dict) and all(isinstance(u.get(k),int) for k in ('prompt_tokens','completion_tokens','total_tokens')) for u in usages):
                        reply['usage']={k:sum(u[k] for u in usages) for k in ('prompt_tokens','completion_tokens','total_tokens')}
                        if all(isinstance((u.get('prompt_tokens_details') or {}).get('cached_tokens'),int) for u in usages):reply['usage']['prompt_tokens_details']={'cached_tokens':sum(u['prompt_tokens_details']['cached_tokens'] for u in usages)}
                    elif len(usages)>1:reply.pop('usage',None)
                return reply
            with self.store.transaction():turn=self._turn(actor,key,fp);turn['round']+=1;self._save_turn(actor,key,fp,turn)
        raise CoreError('memory_tool_budget','SIC memory tool round budget exhausted; no hidden continuation',409)
    def events(self,actor,key,event):
        self._allowed()
        with self.store.transaction():
            binding=self._binding(actor,key);require(binding is not None,'Session binding not found','not_found',404);state=self._state(binding,actor)
            receipt_key='bridge-event:'+digest([actor,key,event.get('event_id')]) if event.get('event_id') else None
            if receipt_key:
                prior=self.store.receipt(receipt_key,digest(event))
                if prior:return prior
            if event.get('type') in ('tool_start','tool_end'):
                require(event.get('call_id') in binding.get('proposed_tools',[]),'Tool event does not belong to a delivered bridge proposal','event_scope',403)
                duplicate=(event['type']=='tool_start' and event['call_id'] in state['open_tools']) or (event['type']=='tool_end' and event['call_id'] not in state['open_tools'])
                if duplicate:
                    observed={**event,'event_id':uid('event'),'time':time.time(),'duplicate_observation':True};state['events'].append(observed);self.store.save(state);result={'chain_id':binding['chain_id'],'event_recorded':True,'duplicate_observation':True}
                    if receipt_key:self.store.remember(receipt_key,digest(event),result)
                    return result
            require(event.get('type') in ('tool_start','tool_end','wait','work_end'),'Unsupported bridge harness event','invalid',422)
            if event.get('type')=='work_end':
                unresolved=any(self.core._source(state,root)['open_calls'] for root in state['pending'])
                require(binding['phase']=='idle' and not state['open_tools'] and not unresolved,'Work end requires a delivered idle turn with no unresolved tools','event_scope',409)
            result=self.core._dispatch('harness.event',state,actor,event)
            if event.get('event_id'):self.store.remember('bridge-event:'+digest([actor,key,event['event_id']]),digest(event),{'chain_id':binding['chain_id'],**result})
        self.core.runtime.notify(binding['chain_id'],actor,{'type':'harness.event','harness_type':'work_end'} if event['type']=='work_end' else {'type':'bridge-harness','event':event})
        if event['type']=='work_end':self._drive(actor,key,force=True,wait=True)
        if event['type'] in ('tool_start','wait'):self.kick(actor,key,True)
        return {'chain_id':binding['chain_id'],**result}
    def recover(self,actor,key,payload):
        self._allowed();require(payload.get('confirm') is True,'Explicit recovery confirmation required','confirmation',403)
        runtime=self.core.runtime
        lock=self._drive_lock(actor,key)
        require(lock.acquire(blocking=False),'Editor still executing; wait before recovery','session_busy',409)
        try:
            with self.store.transaction():
                binding=self._binding(actor,key);require(binding is not None,'Session not found','not_found',404);state=self._state(binding,actor)
                row=self.store.db.execute('SELECT lease,lease_until,lease_pid FROM bridge_bindings WHERE actor=? AND session_key=?',(actor,key)).fetchone();require(not row[0] or row[1]<time.time() or row[2] is not None and not process_alive(row[2]),'Session request still has a valid lease','session_busy',409)
                action=payload['action']
                if action=='retry_editor':
                    require(not runtime._lock(binding['chain_id']).locked(),'Live runtime work must finish or pause before recovery','runtime_busy',409)
                    runtime_policy=copy.deepcopy(state.get('runtime',{}))
                    jobs=[j['job_id'] for j in state['jobs'].values() if j['source_id'] in state['pending'] and j['status'] not in ('published','superseded','abandoned')]
                    for job_id in jobs:
                        self.core._dispatch('runtime.recover',state,actor,{'action':'abandon_editor','job_id':job_id,'confirm':True})
                    if not jobs:self.core._dispatch('runtime.recover',state,actor,{'action':'resume_queue','confirm':True})
                    # An explicit bridge recovery retains this route's existing
                    # host authorization, never grants a new model permission.
                    state['runtime']=runtime_policy
                    binding['editor_jobs']={}
                    binding['editor_error']=None
                    turn=self._turn(actor,key,binding['current_fingerprint'])
                    if turn and turn['phase']=='failed':
                        turn['phase']='ingested';self._save_turn(actor,key,binding['current_fingerprint'],turn)
                        binding['phase']='generating'
                    self.store.save(state)
                elif action=='retry_worker':
                    require(payload.get('acknowledge_possible_duplicate') is True,'Indeterminate remote attempts may have completed; explicit duplicate acknowledgement required','confirmation',403)
                    fp=binding['current_fingerprint'];turn=self._turn(actor,key,fp);require(turn and turn['phase']=='failed','No failed Worker turn to recover')
                    inflight=state.get('provider_inflight')
                    if inflight:
                        failure={'call_id':inflight['call_id'],'status':'host-marked-indeterminate','automatic_retry':False};self.store.remember(inflight['receipt_key'],inflight['fingerprint'],failure);state['provider_inflight']=None
                        self.core._usage(state,{'call_id':inflight['call_id'],'role':'worker','model':'configured-role','categories':[],'usage_status':'unknown','attempt_status':'host-recovered-indeterminate'})
                    turn.update(phase='ingested',attempt=turn['attempt']+1);self._save_turn(actor,key,fp,turn);binding['phase']='generating';self.store.save(state)
                else:raise CoreError('invalid_recovery','Use retry_editor or retry_worker',422)
                self.store.put('bridge_recovery',{'actor':actor,'chain_id':binding['chain_id'],'action':action,'time':time.time()});self._save(actor,key,binding);return {'chain_id':binding['chain_id'],'action':action,'model_called':False}
        finally:lock.release()

def sse_payload(reply,include_usage=True):
    """Complete buffered Chat reply as spec-compatible SSE, including tool indexes."""
    message=copy.deepcopy(reply['choices'][0]['message']);delta={k:v for k,v in message.items() if k in ('role','content','tool_calls','refusal','reasoning_content','reasoning','reasoning_text','reasoning_details')}
    if delta.get('tool_calls'):delta['tool_calls']=[dict(call,index=i) for i,call in enumerate(delta['tool_calls'])]
    base={k:reply[k] for k in ('id','created','model','system_fingerprint') if k in reply};base.setdefault('id','sic-'+uuid.uuid4().hex);base.setdefault('created',int(time.time()));base['object']='chat.completion.chunk'
    chunks=[{**base,'choices':[{'index':0,'delta':delta,'finish_reason':None}]},{**base,'choices':[{'index':0,'delta':{},'finish_reason':reply['choices'][0]['finish_reason']}]}]
    if include_usage and reply.get('usage') is not None:chunks.append({**base,'choices':[],'usage':reply['usage']})
    return (''.join('data: '+json.dumps(chunk,ensure_ascii=False,allow_nan=False)+'\n\n' for chunk in chunks)+'data: [DONE]\n\n').encode()

def make_bridge_server(core,config,host='127.0.0.1',port=18890):
    identities={os.environ[name]:actor for name,actor in config.get('tokens',{}).items() if os.environ.get(name)}
    if not identities:raise ValueError('Bridge requires environment-bound authentication tokens')
    bridge=Bridge(core,config)
    class Handler(BaseHTTPRequestHandler):
        def log_message(self,*args):pass
        def principal(self):
            header=self.headers.get('Authorization','');token=header[7:] if header.startswith('Bearer ') else '';actor=next((a for k,a in identities.items() if hmac.compare_digest(k,token)),None)
            require(actor is not None,'Valid Bearer authentication required','unauthorized',401);return actor
        def respond(self,status,value,content_type='application/json',chain_id=None):
            body=value if isinstance(value,bytes) else json.dumps(value,ensure_ascii=False,allow_nan=False).encode();self.send_response(status);self.send_header('Content-Type',content_type);self.send_header('Content-Length',str(len(body)));self.send_header('Cache-Control','no-store')
            if chain_id:self.send_header('x-sic-chain-id',chain_id)
            self.end_headers()
            try:self.wfile.write(body)
            except (BrokenPipeError,ConnectionResetError):pass # Durable delivery receipt remains replayable.
        def error(self,error):self.respond(error.status,{'error':{'type':'sic_bridge_error','code':error.code,'message':str(error)}})
        def session_key(self,payload=None):
            values=[self.headers.get(name) for name in ('x-sic-bridge-session','session_id','x-session-id','x-session-affinity') if self.headers.get(name)]
            if payload and payload.get('session_key'):values.append(payload['session_key'])
            require(bool(values) and all(x==values[0] for x in values),'One explicit, consistent session identity required','session_required',400);return values[0]
        def do_GET(self):
            try:
                actor=self.principal()
                if self.path=='/api/v2/bridge/sessions':return self.respond(200,{'sessions':bridge.sessions(actor)})
                if self.path=='/v1/models':return self.respond(200,{'object':'list','data':[{'id':bridge.policy.get('public_model','sic-worker'),'object':'model','owned_by':'sic-host-configured'}]})
                if self.path=='/api/v2/bridge/capabilities':return self.respond(200,{'implementation':'sic-core-v2-fullhistory','history':'persistent-strict-prefix','response':'openai-chat-json-or-buffered-sse','automatic_dsh_session_header':'requires-SIC-dsh-plugin','public_model':bridge.policy.get('public_model','sic-worker'),'write_enabled':bridge.policy.get('allow_write',False),'model_enabled':bridge.policy.get('allow_model',False),'editor_mode':bridge.policy.get('editor_mode','model'),'physical_cache_hit':'unknown','live_provider_verified':False})
                raise CoreError('not_found','Route not found',404)
            except CoreError as error:self.error(error)
        def do_POST(self):
            try:
                actor=self.principal();length=int(self.headers.get('Content-Length','0'));require(0<length<=16*1024*1024,'JSON body must be within 16MiB','request_size',413);data=json.loads(self.rfile.read(length))
                if self.path=='/api/v2/bridge/host/register':
                    bridge._allowed();key=self.session_key(data);binding,lease=bridge._claim(actor,key)
                    try:return self.respond(200,bridge.host_tools.register(actor,key,binding['chain_id'],data))
                    finally:bridge._release(actor,key,lease)
                if self.path=='/api/v2/bridge/host/action':
                    bridge._allowed();return self.respond(200,bridge.host_tools.action(actor,data))
                if self.path=='/v1/chat/completions':
                    reply,chain=bridge.chat(actor,self.session_key(),data)
                    if data.get('stream'):return self.respond(200,sse_payload(reply,(data.get('stream_options') or {}).get('include_usage',False)),'text/event-stream; charset=utf-8',chain)
                    return self.respond(200,reply,chain_id=chain)
                if self.path=='/api/v2/bridge/events':return self.respond(200,bridge.events(actor,self.session_key(data),data['event']))
                if self.path=='/api/v2/bridge/evidence':return self.respond(200,bridge.evidence(actor,self.session_key(data)))
                if self.path=='/api/v2/bridge/recover':return self.respond(200,bridge.recover(actor,self.session_key(data),data))
                raise CoreError('not_found','Route not found',404)
            except CoreError as error:self.error(error)
            except (ValueError,KeyError,TypeError):self.error(CoreError('invalid_request','Malformed or unsupported bridge request',422))
            except Exception:self.error(CoreError('bridge_failed','SIC bridge failed; inspect authenticated session and Core evidence before recovery',502))
    class Server(ThreadingHTTPServer):
        def server_close(self):
            super().server_close();bridge.close()
    server=Server((host,port),Handler);server.bridge=bridge;return server

def main():
    p=argparse.ArgumentParser();p.add_argument('--db',default='.local/core-v2.db');p.add_argument('--config',required=True);p.add_argument('--host',default='127.0.0.1');p.add_argument('--port',type=int,default=18890);args=p.parse_args();config=json.loads(Path(args.config).read_text(encoding='utf-8-sig'));Path(args.db).parent.mkdir(parents=True,exist_ok=True);core=Core(args.db,config);server=make_bridge_server(core,config,args.host,args.port)
    print(json.dumps({'service':'sic-core-v2-bridge','host':args.host,'port':server.server_port,'authentication':'required','streaming':'buffered-sse'}),flush=True)
    try:server.serve_forever()
    except KeyboardInterrupt:pass
    finally:server.server_close();core.store.db.close()
if __name__=='__main__':main()
