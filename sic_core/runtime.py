"""Shared event-driven Core runtime. No provider traffic without standing or per-run authorization."""
import copy,json,threading,time
from concurrent.futures import ThreadPoolExecutor
from .editing import program_ops
from .store import uid
from . import hierarchy

class Runtime:
    def __init__(self,core):
        self.core=core;self.store=core.store;self.condition=threading.Condition();self.wake=threading.Event();self.thread=None;self.stopping=False;self.local=threading.local();self.locks={};self.guard=threading.Lock();self.executors={};self.pool=ThreadPoolExecutor(max_workers=int(core.config.get('runtime_workers',4)),thread_name_prefix='sic-route')
        with self.store.transaction():self.store.db.execute('CREATE TABLE IF NOT EXISTS runtime_queue(id TEXT PRIMARY KEY,chain_id TEXT,actor TEXT,event TEXT,status TEXT)')
    def notify(self,chain_id,actor,event):
        if not chain_id or self.core._closed:return
        if getattr(self.local,'running',False) and event.get('type') not in ('collaboration.dispatch','collaboration-result','socket.message','socket.signal','work_end'):return
        with self.store.transaction():
            s=self.store.state(chain_id);controller=actor if actor in s['owners'] else s['owners'][0];event_id=uid('runtime_event');self.store.db.execute('INSERT INTO runtime_queue VALUES(?,?,?,?,?)',(event_id,chain_id,controller,json.dumps({**event,'sender_actor':actor,'time':time.time()}),'pending'));enabled=s.get('runtime',{}).get('enabled',False)
        if enabled and not self.stopping:self.start()
        self.wake.set()
        with self.condition:self.condition.notify_all()
        return event_id
    def start(self):
        with self.guard:
            if self.thread and self.thread.is_alive():return
            if self.stopping:self.pool=ThreadPoolExecutor(max_workers=int(self.core.config.get('runtime_workers',4)),thread_name_prefix='sic-route')
            self.stopping=False;self.thread=threading.Thread(target=self._loop,daemon=True,name='sic-runtime');self.thread.start()
    def stop(self):
        self.stopping=True;self.wake.set()
        with self.condition:self.condition.notify_all()
        if self.thread and self.thread is not threading.current_thread():self.thread.join(10)
        self.pool.shutdown(wait=True)
    def drain(self,timeout=10):
        deadline=time.monotonic()+timeout
        while time.monotonic()<deadline:
            with self.store.transaction():rows=self.store.db.execute("SELECT chain_id,status FROM runtime_queue WHERE status IN ('running','pending')").fetchall();pending=sum(status=='running' or (self.store.state(chain).get('runtime',{}).get('enabled',False) and self.store.state(chain)['run_state']=='running') for chain,status in rows)
            if pending==0:return True
            with self.condition:self.condition.wait(min(.05,max(0,deadline-time.monotonic())))
        return False
    def _loop(self):
        while not self.stopping:
            with self.store.transaction():
                next_deadline=self._refresh_deadlines()
                rows=self.store.db.execute("SELECT id,chain_id,actor,event FROM runtime_queue WHERE status='pending' ORDER BY rowid").fetchall();active={row[0] for row in self.store.db.execute("SELECT chain_id FROM runtime_queue WHERE status='running'")};selected=None
                for row in rows:
                    state=self.store.state(row[1])
                    if row[1] not in active and state.get('runtime',{}).get('enabled') and state['run_state']=='running':selected=row;break
                if selected:self.store.db.execute("UPDATE runtime_queue SET status='running' WHERE id=?",(selected[0],))
            if selected:self.pool.submit(self._handle_event,selected)
            else:self.wake.wait(None if next_deadline is None else max(0,next_deadline-self.core.now()));self.wake.clear()
    def _refresh_deadlines(self):
        """Persist one deadline/flush attempt for the oldest real pending WB.

        Called with the Store transaction held. A failed/running attempt is not
        regenerated after restart; explicit recovery owns ambiguous outcomes.
        """
        now=self.core.now();nearest=None
        for (raw,) in self.store.db.execute('SELECT state FROM chains').fetchall():
            s=json.loads(raw);policy=s.get('runtime',{})
            if not policy.get('enabled') or s['run_state']!='running' or not s['pending']:continue
            root=s['pending'][0];record=s.get('runtime_pending_deadline');max_age=float(policy.get('max_pending_seconds',1.0))
            if not record or record['source_id']!=root:
                parts=set(s['groups'][root]);times=[e['time'] for e in s['events'] if e.get('type')=='source_ingested' and (e.get('part_source_id') in parts or e.get('source_id')==root)]
                first=min(times) if times else now
                record={'source_id':root,'first_pending_at':first,'due_at':first+max_age,'fired':False};s['runtime_pending_deadline']=record;self.store.save(s)
            if record['fired']:continue
            if record['due_at']!=record['first_pending_at']+max_age:
                record['due_at']=record['first_pending_at']+max_age;self.store.save(s)
            if record['due_at']<=now:
                event_id=uid('runtime_event');event={'type':'pending_timeout','source_id':root,'time':now,'due_at':record['due_at']}
                self.store.db.execute('INSERT INTO runtime_queue VALUES(?,?,?,?,?)',(event_id,s['chain_id'],s['owners'][0],json.dumps(event),'pending'))
                record.update(fired=True,event_id=event_id);s['events'].append({'event_id':event_id,**event});self.store.save(s)
            else:nearest=record['due_at'] if nearest is None else min(nearest,record['due_at'])
        return nearest
    def _handle_event(self,selected):
        self.local.running=True
        try:
            with self.store.transaction():s=self.store.state(selected[1]);policy=s['runtime'];allowed=s.get('runtime_model_authorized',False)
            if policy.get('editor_mode')=='model' or policy.get('worker_mode')=='automatic':
                from .service import require
                require(allowed,'No standing runtime model grant','model_denied',403)
            event=json.loads(selected[3])
            if s.get('collaboration_task'):
                task=s['collaboration_task'];self.collaborate(task['sender'],selected[2],{'request_id':task['request_id'],'max_steps':policy.get('max_steps',8),'editor_mode':policy.get('editor_mode','model')})
            elif event.get('type')=='collaboration.dispatch':self.collaborate(selected[1],selected[2],{'request_id':event['request_id'],'max_steps':policy.get('max_steps',8),'editor_mode':policy.get('editor_mode','model')})
            else:
                flush=event.get('type') in ('pending_timeout','work_end') or event.get('type')=='harness.event' and event.get('harness_type')=='work_end'
                self.run(selected[1],selected[2],{'max_steps':policy.get('max_steps',8),'continue_worker':not flush and policy.get('worker_mode')=='automatic','editor_mode':policy.get('editor_mode','program'),'automatic_event':selected[0],'flush_pending':flush})
            status='completed'
        except Exception as error:
            with self.store.transaction():
                state=self.store.state(selected[1]);state['runtime_last_error']={'type':type(error).__name__,'code':getattr(error,'code','failed'),'event_id':selected[0]};self.store.save(state)
            status='failed'
        finally:self.local.running=False
        with self.store.transaction():self.store.db.execute('UPDATE runtime_queue SET status=? WHERE id=?',(status,selected[0]))
        self.wake.set()
        with self.condition:self.condition.notify_all()
    def _lock(self,chain):
        with self.guard:return self.locks.setdefault(chain,threading.Lock())
    def _op(self,chain,actor,name,p=None):return self.core.operate(name,{'chain_id':chain,'actor':actor,'allow_write':True,'allow_model':True},p or {})['result']
    def step(self,chain_id,actor,payload=None):return self.run(chain_id,actor,{**(payload or {}),'max_steps':1})
    def run(self,chain_id,actor,payload=None):
        from .service import require,CoreError
        p=payload or {};max_steps=int(p.get('max_steps',8));require(1<=max_steps<=128,'runtime max_steps 1..128');lock=self._lock(chain_id);require(lock.acquire(blocking=False),'runtime already processing chain','runtime_busy',409);steps=[];model_calls=0;self.local.running=True
        try:
            with self.store.transaction():
                s=self.store.state(chain_id);require(actor in s['owners'],'chain access denied','forbidden',403)
                if not s['pending'] and any(m['message_id'] not in s['delivered'] for m in s['inbox']):
                    s['_actor']=actor;self.core._dispatch('source.ingest',s,actor,{'request':{'messages':[{'role':'user','content':''}]}});s=self.store.state(chain_id)
                eligible=set(s['pending']);policy=s.get('runtime',{});enabled=s['run_state']=='running'
            if not enabled:return {'chain_id':chain_id,'steps':[],'status':s['run_state']}
            if p.get('form_all') and eligible:self._op(chain_id,actor,'wb.form',{'source_ids':[],'reason':'batch-at-end'})
            editor_mode=p.get('editor_mode',policy.get('editor_mode','program'));require(editor_mode in ('model','program'),'unknown runtime editor mode')
            self._op(chain_id,actor,'decision.maintain')
            for step in range(max_steps):
                with self.store.transaction():
                    s=self.store.state(chain_id);s['_actor']=actor
                    if s['run_state']!='running' or self.core._closed:break
                    if not s['pending'] or s['pending'][0] not in eligible:break
                    if s.get('provider_inflight'):steps.append({'action':'publication-deferred','reason':'Worker in flight'});break
                    root=s['pending'][0];source=self.core._source(s,root);existing=[j for j in s['jobs'].values() if j['source_id']==root and j['status'] not in ('published','superseded','abandoned','failed')];job=existing[-1] if existing else None
                    failed=[j for j in s['jobs'].values() if j['source_id']==root and j['status']=='failed']
                    if job is None and failed:
                        require(failed[-1]['source_snapshot']['source_ids']!=source['source_ids'],'Failed Editor scope requires explicit recovery or a fresh edit.prepare; no automatic retry','editor_recovery_required',409)
                        for prior in failed:prior['status']='superseded';prior['superseded_reason']='new source version, not feedback repair'
                        self.store.save(s)
                    threshold=int(policy.get('seal_chars',1))
                    frozen={self.store.get(wid,'wb')['source_id'] for wid in hierarchy.level(s,1)['wbs']}
                    if p.get('automatic_event') and not p.get('flush_pending') and not job and root not in frozen and sum(len(u['text']) for u in source['units'])<threshold:
                        roots=[];count=0
                        for candidate in s['pending']:
                            if candidate not in eligible:break
                            if candidate in frozen:break
                            roots.append(candidate);count+=sum(len(u['text']) for u in self.core._source(s,candidate)['units'])
                            if count>=threshold:break
                        if count<threshold and not s['open_tools'] and not any(w['status']=='declared' and w['due_at']>time.time() for w in s.get('waits',{}).values()):break
                        if len(roots)>1:self.core._dispatch('wb.form',s,actor,{'source_ids':roots,'reason':'runtime-accumulated-boundary'});s=self.store.state(chain_id);source=self.core._source(s,root)
                    if not job:
                        if editor_mode=='program':job_id=self.core._dispatch('edit.program',s,actor,{'source_id':root,'rules':s['config'].get('program_rules',['deduplicate_exact'])})['job_id']
                        else:
                            rules=s['config'].get('program_rules',['deduplicate_exact']);program=program_ops(source,rules);handled={op['target'] for op in program};chunks=[];chunk=[];chars=0;budget=int(policy.get('editor_chars',4096))
                            for unit in source['units']:
                                if unit['unit_id'] in handled:continue
                                if chunk and chars+len(unit['text'])>budget:chunks.append(chunk);chunk=[];chars=0
                                chunk.append(unit['unit_id']);chars+=len(unit['text'])
                            if chunk:chunks.append(chunk)
                            assignments=[{'editor':'editor-'+str(i+1),'unit_ids':units} for i,units in enumerate(chunks)]
                            job_id=self.core._dispatch('edit.prepare',s,actor,{'source_id':root,'assignments':assignments,'program_rules':rules})['job_id']
                    else:job_id=job['job_id']
                if editor_mode=='model':
                    with self.store.transaction():
                        blocking=p.get('continue_worker') or p.get('blocking_worker');started=self.core.now()
                    batch=self._op(chain_id,actor,'edit.run_all',{'job_id':job_id,'max_workers':int(policy.get('editor_workers',2))});model_calls+=len(batch.get('results',[]))
                    if blocking and batch.get('results'):
                        from .wait_windows import serial_wait_segments
                        ended=self.core.now()
                        with self.store.transaction():events=copy.deepcopy(self.store.state(chain_id)['events'])
                        for a,b in serial_wait_segments(events,started,ended):
                            self._op(chain_id,actor,'harness.event',{'type':'interval','kind':'serial_wait','start':a,'end':b,'clock':'core-epoch-perf-counter','reason':'Worker held for Editor batch outside natural windows','job_id':job_id})
                with self.store.transaction():
                    s=self.store.state(chain_id);s['_actor']=actor;job=s['jobs'][job_id]
                    if s['run_state']!='running' or self.core._closed:steps.append({'action':'candidate-held','job_id':job_id,'run_state':s['run_state']});break
                    if self.core._source(s,root)['open_calls']:steps.append({'action':'candidate-ready-wait-native-results','job_id':job_id});break
                    if s.get('provider_inflight'):steps.append({'action':'publication-deferred','job_id':job_id});break
                    publication=self.core._dispatch('cut.publish',s,actor,{'job_id':job_id,'expected_cut':job['base_cut'],'expected_candidate_revision':self.core._candidate(job)['revision'],'idempotency_key':'runtime:'+job_id});steps.append({'action':'published','job_id':job_id,**publication})
            with self.store.transaction():running=self.store.state(chain_id)['run_state']=='running'
            if running and not self.core._closed and p.get('continue_worker') and (eligible or not p.get('automatic_event')):
                response=self._worker_loop(chain_id,actor,p.get('worker_request_id','runtime:'+uid('step')),p,editor_mode);model_calls+=response.get('runtime_provider_calls',0);steps.append({'action':'worker','result':response})
            with self.store.transaction():
                s=self.store.state(chain_id);s['runtime_last_step']={'steps':steps,'time':time.time()};self.store.save(s)
            status=s['run_state'] if s['run_state']!='running' else ('candidate-held' if any(step['action']=='candidate-held' for step in steps) else (steps[-1]['result'].get('status','completed') if steps and steps[-1]['action']=='worker' else 'completed'))
            return {'chain_id':chain_id,'steps':steps,'status':status,'provider_called':bool(model_calls),'provider_call_count':model_calls,'_provider_called':bool(model_calls)}
        finally:self.local.running=False;lock.release()
    def _collaboration_return_policy(self,parent,request,p):
        """Freeze legacy dispatch policies, and reject per-run policy changes.

        Caller holds the Store transaction. Earlier dispatch records retained
        the policy in child tasks; use those records rather than a run default.
        """
        from .service import require
        values=[]
        if 'return_lifecycle' in request:values.append(request['return_lifecycle'])
        for branch in request['branches']:
            if 'return_lifecycle' in branch:values.append(branch['return_lifecycle'])
            task=self.store.state(branch['chain_id']).get('collaboration_task',{})
            if task.get('sender')==parent['chain_id'] and task.get('request_id')==request['request_id'] and 'return_lifecycle' in task:values.append(task['return_lifecycle'])
        require(all(value in ('P','V') for value in values) and len(set(values))<=1,'Stored collaboration return policies conflict','collaboration_policy_conflict',409)
        lifecycle=values[0] if values else 'V'
        require('return_lifecycle' not in p or p['return_lifecycle']==lifecycle,'Run cannot change dispatched return_lifecycle','collaboration_policy_conflict',409)
        if request.get('return_lifecycle')!=lifecycle or any(branch.get('return_lifecycle')!=lifecycle for branch in request['branches']):
            request['return_lifecycle']=lifecycle
            for branch in request['branches']:branch['return_lifecycle']=lifecycle
            self.store.save(parent)
        return lifecycle
    def collaborate(self,chain_id,actor,p):
        from .service import require
        request_id=p['request_id']
        with self.store.transaction():
            parent=self.store.state(chain_id);require(actor in parent['owners'],'chain access denied','forbidden',403);require(request_id in parent.get('collaborations',{}),'request not found')
            self._collaboration_return_policy(parent,parent['collaborations'][request_id],p)
        lock=self._lock('collaboration-request:'+chain_id+':'+request_id)
        if not lock.acquire(blocking=False):
            # Child timers and repeated host requests observe the same execution;
            # they do not race its branch or turn runtime_busy into a task failure.
            with self.store.transaction():return {**copy.deepcopy(self.store.state(chain_id)['collaborations'][request_id]),'execution_state':'in-progress'}
        try:return self._collaborate_locked(chain_id,actor,p)
        finally:lock.release()
    def _collaborate_locked(self,chain_id,actor,p):
        from .service import require
        request_id=p['request_id']
        with self.store.transaction():
            parent=self.store.state(chain_id);require(actor in parent['owners'],'chain access denied','forbidden',403);return_lifecycle=self._collaboration_return_policy(parent,parent['collaborations'][request_id],p);request=copy.deepcopy(parent['collaborations'][request_id]);require(request is not None,'request not found')
        if request['status'] in ('completed','cancelled'):return request
        if request['status']=='failed':
            if not p.get('retry_failed'):return request
            require(p.get('acknowledge_possible_duplicate') is True,'Explicit acknowledgement required before a new failed-branch attempt','recovery_confirmation',403)
            for branch in request['branches']:
                if branch['status']=='failed':
                    branch['status']='pending';branch['attempt']=branch.get('attempt',0)+1
                    with self.store.transaction():
                        child=self.store.state(branch['chain_id'])
                        for old_job in child['jobs'].values():
                            if old_job['status']=='failed':old_job['status']='abandoned'
                        self.store.put('collaboration_retry',{'request_id':request_id,'branch':branch['chain_id'],'attempt':branch['attempt'],'actor':actor});self.store.save(child)
            with self.store.transaction():
                parent=self.store.state(chain_id);parent['collaborations'][request_id]=copy.deepcopy(request);self.store.save(parent)
        def execute_unchecked(branch):
            if branch.get('status')=='completed':return branch
            child=branch['chain_id'];state=self._op(child,actor,'chain.get')
            if state['run_state']!='running':return {**branch,'status':state['run_state']}
            worked=self.run(child,actor,{'max_steps':p.get('max_steps',8),'editor_mode':p.get('editor_mode','model'),'continue_worker':True,'worker_request_id':'collaboration:'+request_id+':'+child+':attempt:'+str(branch.get('attempt',0))})
            with self.store.transaction():run_state=self.store.state(child)['run_state']
            if run_state!='running':return {**branch,'status':run_state}
            worker=next((x['result'] for x in worked['steps'] if x['action']=='worker'),None)
            if worker is None:return {**branch,'status':'waiting','reason':'candidate-held-without-worker-admission'}
            if worker.get('status') in ('waiting-tools','budget-exhausted','waiting','stopped'):return {**branch,'status':worker['status'],'pending_calls':worker.get('pending_calls',[])}
            message=worker['response']['choices'][0]['message'];text=message.get('content') or '';require(not message.get('tool_calls'),'branch requires host tool processing before result return','branch_tool_pending',409)
            self.run(child,actor,{'max_steps':p.get('max_steps',8),'editor_mode':p.get('editor_mode','model'),'continue_worker':False})
            with self.store.transaction():
                sender=self.store.state(chain_id)
                if sender['collaborations'][request_id]['status']=='cancelled':return {**branch,'status':'cancelled'}
                state=self.store.state(child)
                if state['run_state']!='running':return {**branch,'status':state['run_state']}
                message_id='collaboration-result:'+request_id+':'+child
                from .communication import receive_message
                receipt=receive_message(self.core,sender,{'message_id':message_id,'text':text,'lifecycle':return_lifecycle,'sender':child,'request_id':request_id,'result_source_id':worker['source_id']})
                if receipt['created']:sender['events'].append({'type':'collaboration_result','request_id':request_id,'branch':child,'source_id':worker['source_id'],'time':time.time()})
                self.store.save(sender)
            return {**branch,'status':'completed','result_message_id':message_id,'result_source_id':worker['source_id'],'text':text}
        def execute(branch):
            try:return execute_unchecked(branch)
            except Exception as error:
                failure={**branch,'status':'failed','error':{'type':type(error).__name__,'code':getattr(error,'code','branch_failed')}}
                with self.store.transaction():self.store.put('collaboration_failure',{'request_id':request_id,'sender':chain_id,**failure})
                return failure
        with ThreadPoolExecutor(max_workers=max(1,min(4,len(request['branches'])))) as executor:branches=list(executor.map(execute,request['branches']))
        with self.store.transaction():
            sender=self.store.state(chain_id);row=sender['collaborations'][request_id];row['branches']=branches
            if row['status']!='cancelled':row['status']='failed' if any(x['status'] in ('failed','stopped','cancelled') for x in branches) else ('completed' if all(x['status']=='completed' for x in branches) else 'waiting');self.store.save(sender)
        self.notify(chain_id,actor,{'type':'collaboration-result','request_id':request_id});return copy.deepcopy(row)

    def register_tool(self,name,executor):
        """Host-only callable registration; model/schema text never supplies executable code."""
        self.executors[name]=executor
        return lambda:self.executors.pop(name,None)
    def _worker_loop(self,chain,actor,base_key,p,editor_mode):
        from .service import require,CoreError
        network_calls=0
        for _ in range(int(p.get('worker_tool_rounds',8))):
            with self.store.transaction():
                s=self.store.state(chain);progress=s.setdefault('runtime_worker_runs',{}).setdefault(base_key,{'round':0,'phase':'new'})
                if s['run_state']!='running':return {'status':s['run_state'],'pending_calls':[],'resume_key':base_key,'runtime_provider_calls':network_calls}
                if progress['phase']=='completed':return {**copy.deepcopy(progress['result']),'runtime_provider_calls':0}
                if progress['phase']=='waiting':
                    has_open=any(self.core._source(s,root)['open_calls'] for root in s['pending'])
                    if not has_open:progress['round']+=1;progress['phase']='new'
                budget=int(s.get('runtime',{}).get('worker_turns',64));used=s.get('runtime_worker_budget_used',0)
                if used>=budget:return {'status':'budget-exhausted','pending_calls':[],'resume_key':base_key,'used':used,'budget':budget,'runtime_provider_calls':network_calls}
                round_no=progress['round'];cached=progress.get('result') if progress['phase']=='waiting' else None;self.store.save(s)
            if cached:result=cached
            else:
                envelope=self.core.operate('provider.continue',{'chain_id':chain,'actor':actor,'allow_write':True,'allow_model':True},{'idempotency_key':base_key+':'+str(round_no),'policy':p.get('schedule_policy','wait')});result=envelope['result'];network_calls+=int(envelope['evidence']['provider_called'])
                if envelope['evidence']['provider_called']:
                    with self.store.transaction():
                        budget_state=self.store.state(chain);budget_state['runtime_worker_budget_used']=budget_state.get('runtime_worker_budget_used',0)+1;self.store.save(budget_state)
            require('response' in result,'Worker attempt failed or indeterminate; explicit new work/recovery required','worker_indeterminate',409)
            calls=result['response']['choices'][0]['message'].get('tool_calls') or []
            if not calls:
                with self.store.transaction():s=self.store.state(chain);s['runtime_worker_runs'][base_key].update(phase='completed',result=result);self.store.save(s)
                return {**result,'runtime_provider_calls':network_calls}
            with self.store.transaction():s=self.store.state(chain);s['runtime_worker_runs'][base_key].update(phase='waiting',result=result);self.store.save(s)
            with self.store.transaction():
                state=self.store.state(chain)
                if state['run_state']!='running':return {'status':state['run_state'],'pending_calls':calls,**result,'runtime_provider_calls':network_calls}
            known=set(self.core._tool_operations())|{'sic_source_read','sic_source_search','sic_operation_directory'};broker=getattr(self.core,'host_tools',None)
            runnable=[call for call in calls if call['function']['name'] in known or call['function']['name'] in self.executors or broker and broker.available(chain,actor,call['function']['name'])]
            if not runnable:return {'status':'waiting-tools','pending_calls':calls,**result,'runtime_provider_calls':network_calls}
            started=threading.Event()
            def execute(call):
                self.local.running=True
                call_id=call['id'];name=call['function']['name'];self._op(chain,actor,'tools.authorize',{'name':name})
                with self.store.transaction():
                    state=self.store.state(chain);old=state.setdefault('tool_results',{}).get(call_id)
                    if old and old.get('status')!='paused-not-dispatched':return old
                    state['tool_results'][call_id]={'status':'running'};self.store.save(state)
                remote=name not in known and name not in self.executors
                if not remote:self._op(chain,actor,'harness.event',{'type':'tool_start','call_id':call_id,'actual_executor':'SIC-host'});started.set()
                try:
                    arguments=json.loads(call['function']['arguments']);require(isinstance(arguments,dict),'tool arguments must be object')
                    context={'chain_id':chain,'actor':actor,'call_id':call_id}
                    value=self._op(chain,actor,'tools.invoke',{'name':name,'arguments':arguments}) if name in known else (broker.execute(context,name,arguments,on_started=started.set) if remote else self.executors[name](context,arguments))
                    text=json.dumps(value,ensure_ascii=False,allow_nan=False);status='failed' if isinstance(value,dict) and value.get('isError') is True else 'completed'
                except Exception as error:
                    if getattr(error,'code','')=='host_tool_paused':
                        with self.store.transaction():
                            state=self.store.state(chain);state['tool_results'][call_id]={'status':'paused-not-dispatched'};self.store.save(state)
                        raise
                    if getattr(error,'code','') in ('host_tool_indeterminate','host_tool_cancelled'):
                        with self.store.transaction():
                            state=self.store.state(chain);state['tool_results'][call_id]={'status':error.code};self.store.save(state)
                        raise
                    text=json.dumps({'error':type(error).__name__,'result_status':'failed-or-indeterminate'});status='failed'
                if not remote:self._op(chain,actor,'harness.event',{'type':'tool_end','call_id':call_id,'status':status,'actual_executor':'SIC-host'})
                with self.store.transaction():
                    state=self.store.state(chain);object_id=self.store.put('tool_result',{'call_id':call_id,'name':name,'content':text,'status':status});record={'status':status,'object_id':object_id,'delivered':False};state['tool_results'][call_id]=record;self.store.save(state)
                return record
            with ThreadPoolExecutor(max_workers=max(1,min(4,len(runnable)))) as executor:
                futures=[executor.submit(execute,call) for call in runnable];started.wait(1)
                # Use the natural tool window for a real single-round Editor, while execution continues.
                network_calls+=self._edit_during_tool(chain,actor,editor_mode) or 0
                try:records=[future.result() for future in futures]
                except CoreError as error:
                    if error.code!='host_tool_paused':raise
                    return {'status':'waiting','pending_calls':calls,**result,'runtime_provider_calls':network_calls}
            with self.store.transaction():
                s=self.store.state(chain);s['_actor']=actor;messages=[]
                for call in runnable:
                    record=s['tool_results'][call['id']]
                    if record.get('object_id') and not record.get('delivered'):messages.append({'role':'tool','tool_call_id':call['id'],'content':self.store.get(record['object_id'],'tool_result')['content']})
                if messages:
                    self.core._dispatch('source.ingest',s,actor,{'request':{'messages':messages}});s=self.store.state(chain)
                    for message in messages:s['tool_results'][message['tool_call_id']]['delivered']=True
                    self.store.save(s)
                remaining=[call for call in calls if not s.get('tool_results',{}).get(call['id'],{}).get('delivered')]
                if remaining:return {'status':'waiting-tools','pending_calls':remaining,**result,'runtime_provider_calls':network_calls}
                progress=s['runtime_worker_runs'][base_key];progress['round']+=1;progress['phase']='new';self.store.save(s)
            self._publish_ready(chain,actor)
        return {'status':'waiting-tools','pending_calls':[],'reason':'worker-tool-round-budget','resume_key':base_key,'runtime_provider_calls':network_calls}
    def _edit_during_tool(self,chain,actor,mode):
        with self.store.transaction():
            state=self.store.state(chain);state['_actor']=actor
            if state['run_state']!='running':return 0
            if not state['pending']:return
            root=state['pending'][0];jobs=[j for j in state['jobs'].values() if j['source_id']==root and j['status'] not in ('published','superseded','abandoned','failed')]
            job=jobs[-1] if jobs else None
            if not job:job_id=self.core._dispatch('edit.prepare' if mode=='model' else 'edit.program',state,actor,{'source_id':root})['job_id']
            else:job_id=job['job_id']
        if mode=='model':return len(self._op(chain,actor,'edit.run_all',{'job_id':job_id}).get('results',[]))
        return 0
    def _publish_ready(self,chain,actor):
        with self.store.transaction():
            state=self.store.state(chain);state['_actor']=actor
            if state['run_state']!='running':return
            if not state['pending']:return
            root=state['pending'][0];jobs=[j for j in state['jobs'].values() if j['source_id']==root and j['status'] not in ('published','superseded','abandoned','failed')]
            if not jobs or self.core._source(state,root)['open_calls']:return
            job=jobs[-1]
            if all(a['editor'] in job['responses'] for a in job['assignments']):self.core._dispatch('cut.publish',state,actor,{'job_id':job['job_id'],'expected_cut':job['base_cut'],'idempotency_key':'runtime:'+job['job_id']})
