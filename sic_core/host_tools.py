"""Durable, explicitly scoped pull transport for actual Harness peer tools.

A claimed attempt is never automatically reissued. Expiry or ambiguous transport
outcomes stop that attempt; host completion is idempotent, not re-execution.
"""
import copy,json,threading,time
from .store import dumps,uid
from .adapters import digest

class HostToolBroker:
    def __init__(self,core,policy):
        self.core=core;self.store=core.store;self.policy=policy;self.closed=False;self.condition=threading.Condition()
        with self.store.transaction():
            self.store.db.execute('CREATE TABLE IF NOT EXISTS host_tool_hosts(id TEXT PRIMARY KEY,state TEXT NOT NULL)')
            self.store.db.execute('CREATE TABLE IF NOT EXISTS host_tool_tasks(id TEXT PRIMARY KEY,state TEXT NOT NULL)')
    def close(self):
        self.closed=True
        with self.condition:self.condition.notify_all()
    def _load(self,table,key):
        row=self.store.db.execute('SELECT state FROM '+table+' WHERE id=?',(key,)).fetchone()
        return json.loads(row[0]) if row else None
    def _save(self,table,key,value):
        self.store.db.execute('INSERT INTO '+table+' VALUES(?,?) ON CONFLICT(id) DO UPDATE SET state=excluded.state',(key,dumps(value)))
    def _check(self,condition,message,code='host_tool_denied'):
        from .service import require
        require(condition,message,code,409 if code!='host_tool_denied' else 403)
    def register(self,actor,session,chain,p):
        self._check(self.policy.get('host_tools_enabled') is True,'Peer host execution is disabled')
        names=p.get('tools');allowed=self.policy.get('host_tool_allowlist',[])
        self._check(isinstance(names,list) and names and all(isinstance(x,str) and x in allowed for x in names),'Host tool list must be explicitly allowed by deployment')
        host=p.get('host_id');self._check(isinstance(host,str) and 0<len(host)<256,'Host identity required')
        with self.store.transaction():
            s=self.store.state(chain);self._check(actor in s['owners'],'Host does not own bound chain')
            old=self._load('host_tool_hosts',host)
            self._check(old is None or (old['actor'],old['session_key'],old['chain_id'])==(actor,session,chain),'Host identity belongs to another session')
            self._check(old is None or not (old.get('superseded_by') or old.get('closed_at')),'A closed or superseded host generation cannot register again')
            generation=old.get('generation',0) if old else 0
            for (raw,) in self.store.db.execute('SELECT state FROM host_tool_hosts').fetchall():
                prior=json.loads(raw)
                if prior['host_id']==host or (prior['actor'],prior['session_key'],prior['chain_id'])!=(actor,session,chain):continue
                generation=max(generation,prior.get('generation',0)+1)
                if prior.get('superseded_by'):continue
                prior.update(superseded_by=host,expires_at=0);self._save('host_tool_hosts',prior['host_id'],prior)
                for (task_raw,) in self.store.db.execute('SELECT state FROM host_tool_tasks').fetchall():
                    task=json.loads(task_raw)
                    if task['host_id']!=prior['host_id'] or task['status'] not in ('queued','leased','running'):continue
                    task.update(status='cancelled' if task['status']=='queued' else 'indeterminate',ended_at=self.core.now(),reason='host-generation-replaced',automatic_redispatch=False)
                    self._save('host_tool_tasks',task['task_id'],task)
            value={'host_id':host,'generation':generation,'actor':actor,'session_key':session,'chain_id':chain,'tools':names,'expires_at':time.time()+float(self.policy.get('host_registration_seconds',120))}
            self._save('host_tool_hosts',host,value)
        with self.condition:self.condition.notify_all()
        return {'registered':True,'host_id':host,'generation':generation,'chain_id':chain,'tools':names}
    def _lineage(self,chain,actor):
        current=self.store.state(chain);seen=set()
        while current.get('collaboration_task'):
            self._check(current['chain_id'] not in seen,'Cyclic collaboration lineage');seen.add(current['chain_id'])
            self._check(actor in current['owners'] and current['run_state'] in ('running','waiting'),'Peer not owned or has stopped')
            self._check(not any(self.store.revoked(x) for group in current['groups'].values() for x in group),'Peer depends on revoked source')
            task=current['collaboration_task'];parent=self.store.state(task['sender']);request=parent.get('collaborations',{}).get(task['request_id'])
            self._check(request is not None and request['status'] not in ('cancelled','failed'),'Collaboration revoked or failed')
            self._check(any(b['chain_id']==current['chain_id'] for b in request['branches']),'Peer is not an authorized dispatched branch')
            current=parent
        self._check(bool(seen),'External broker execution requires an actual collaboration peer')
        self._check(actor in current['owners'],'Root route access denied')
        return current['chain_id']
    def _host_for(self,chain,actor,name):
        root=self._execution_root(chain,actor,name)
        hosts=[json.loads(x[0]) for x in self.store.db.execute('SELECT state FROM host_tool_hosts')]
        rows=[h for h in hosts if h['chain_id']==root and h['actor']==actor and not h.get('superseded_by') and not h.get('closed_at') and name in h['tools'] and h['expires_at']>time.time()]
        self._check(len(rows)==1,'No unique live host registered for this peer/tool')
        return rows[0]
    def _execution_root(self,chain,actor,name,require_running=True):
        self._check(self.policy.get('host_tools_enabled') is True and name in self.policy.get('host_tool_allowlist',[]),'Current deployment tool permission withdrawn')
        root=self._lineage(chain,actor);state=self.store.state(chain);tool=next((t for t in state['tools'] if t['name']==name),None)
        self._check(tool is not None and self.core._tool_acl(state,tool,'execute',actor),'Current route tool permission withdrawn')
        if require_running:self._check(state['run_state']=='running','Peer is waiting; no new native dispatch admitted','host_tool_paused')
        return root
    def available(self,chain,actor,name):
        if self.closed:return False
        try:
            with self.store.transaction():self._host_for(chain,actor,name)
            return True
        except Exception:return False
    def _event(self,task,event):
        s=self.store.state(task['chain_id']);s['_actor']=task['actor'];self.core._dispatch('harness.event',s,task['actor'],{**event,'call_id':task['call_id'],'observed_by':'dsh-peer-public-tool-runtime','host_task_id':task['task_id']})
    def execute(self,context,name,arguments,on_started=None):
        chain=context['chain_id'];actor=context['actor'];key='host_task_'+digest([chain,context['call_id']]);deadline=time.monotonic()+float(self.policy.get('host_tool_wait_seconds',120))
        with self.store.transaction():
            host=self._host_for(chain,actor,name);task=self._load('host_tool_tasks',key);fp=digest([name,arguments])
            if task:self._check(task['fingerprint']==fp,'Tool call identity reused with other arguments','host_tool_conflict')
            else:
                state=self.store.state(chain)
                outcome=next((v['result'] for v in state.get('runtime_worker_runs',{}).values() if v.get('phase')=='waiting' and any(c['id']==context['call_id'] for c in v.get('result',{}).get('response',{}).get('choices',[{'message':{}}])[0]['message'].get('tool_calls',[]) or [])),None)
                projection=None
                if outcome:
                    request=self.store.get(outcome['request_id'],'provider_request');response=self.store.get(outcome['response_id'],'provider_response')
                    projection={'request_id':outcome['request_id'],'response_id':outcome['response_id'],'call_id':outcome['call_id'],'request':request['request'],'response':response['response'],'cut_id':request.get('cut_id')}
                task={'task_id':key,'chain_id':chain,'actor':actor,'root_chain_id':host['chain_id'],'session_key':host['session_key'],'host_id':host['host_id'],'call_id':context['call_id'],'name':name,'arguments':copy.deepcopy(arguments),'fingerprint':fp,'status':'queued','created_at':self.core.now(),'provider_projection':projection}
                self._save('host_tool_tasks',key,task)
        reported=False
        while True:
            failure=None
            with self.store.transaction():
                task=self._load('host_tool_tasks',key)
                if task.get('dispatched_at') and not reported:
                    reported=True
                    if on_started:on_started()
                if task['status'] in ('completed','failed'):return copy.deepcopy(task['result'])
                if task['status'] in ('indeterminate','cancelled'):failure=task['status']
                if failure is None:
                    try:self._execution_root(chain,actor,name,require_running=False)
                    except Exception:task['status']='cancelled';task['ended_at']=self.core.now();failure='cancelled'
                if failure is None and (self.closed or self.core._closed or time.monotonic()>=deadline or task.get('lease_until',float('inf'))<time.time()):
                    task['status']='indeterminate';task['ended_at']=self.core.now();failure='indeterminate'
                if failure:self._save('host_tool_tasks',key,task)
            # Fail after commit: recording an uncertain outcome must not roll back.
            if failure:self._check(False,'Host tool outcome is '+failure+'; automatic redispatch forbidden','host_tool_'+failure)
            with self.condition:self.condition.wait(.05)
    def action(self,actor,p):
        result=self._action_transaction(actor,p)
        if result.get('_authorization_denied'):
            self._check(False,'Current host/tool authorization withdrawn; outcome is not adopted','host_tool_denied')
        if result.get('_attempt_indeterminate'):
            self._check(False,'Host tool lease expired; automatic redispatch and late adoption forbidden','host_tool_indeterminate')
        if result.get('_peer_paused'):self._check(False,'Peer is waiting; no native dispatch admitted','host_tool_paused')
        with self.condition:self.condition.notify_all()
        return result
    def _action_transaction(self,actor,p):
        action=p['action'];host_id=p['host_id']
        with self.store.transaction():
            host=self._load('host_tool_hosts',host_id);self._check(host and host['actor']==actor,'Unknown host ownership')
            self._check(not host.get('superseded_by'),'Host generation has been superseded')
            if action=='close':
                if host.get('closed_at'):return {'accepted':True,'closed':True,'replayed':True}
                host.update(closed_at=self.core.now(),expires_at=0);self._save('host_tool_hosts',host_id,host)
                for (raw,) in self.store.db.execute('SELECT state FROM host_tool_tasks').fetchall():
                    task=json.loads(raw)
                    if task['host_id']!=host_id or task['status'] not in ('queued','leased','running'):continue
                    task.update(status='cancelled' if task['status']=='queued' else 'indeterminate',ended_at=self.core.now(),reason='host-closed',automatic_redispatch=False)
                    self._save('host_tool_tasks',task['task_id'],task)
                return {'accepted':True,'closed':True,'generation':host.get('generation',0)}
            self._check(not host.get('closed_at'),'Host generation has been closed')
            if action=='pending':
                rows=[json.loads(r[0]) for r in self.store.db.execute('SELECT state FROM host_tool_tasks')]
                own=[t for t in rows if t['host_id']==host_id]
                root=self.store.state(host['chain_id'])
                return {'queued':sum(t['status']=='queued' for t in own),'running':sum(t['status'] in ('leased','running') for t in own),'collaboration_active':sum(r['status'] in ('pending','running','waiting') for r in root.get('collaborations',{}).values())}
            if action=='pull':
                host['expires_at']=time.time()+float(self.policy.get('host_registration_seconds',120));self._save('host_tool_hosts',host_id,host)
                selected=None
                for row in self.store.db.execute('SELECT state FROM host_tool_tasks ORDER BY rowid'):
                    task=json.loads(row[0])
                    if task['host_id']!=host_id:continue
                    if task['status'] in ('leased','running') and task['lease_until']<time.time():
                        task['status']='indeterminate';self._save('host_tool_tasks',task['task_id'],task)
                    if task['status']!='queued':continue
                    if task['name'] not in host['tools']:
                        task['status']='cancelled';self._save('host_tool_tasks',task['task_id'],task);continue
                    try:self._execution_root(task['chain_id'],actor,task['name'])
                    except Exception as error:
                        if getattr(error,'code','')=='host_tool_paused':continue
                        task['status']='cancelled';self._save('host_tool_tasks',task['task_id'],task);continue
                    selected=task;break
                if not selected:return {'task':None}
                selected.update(status='leased',lease=uid('host_lease'),lease_until=time.time()+float(self.policy.get('host_tool_lease_seconds',120)))
                self._save('host_tool_tasks',selected['task_id'],selected);return {'task':selected}
            task=self._load('host_tool_tasks',p['task_id']);self._check(task and task['host_id']==host_id and task.get('lease')==p.get('lease'),'Task lease/host mismatch')
            if task['status'] in ('leased','running') and task['lease_until']<time.time():
                task.update(status='indeterminate',ended_at=self.core.now(),reason='host-lease-expired',automatic_redispatch=False)
                self._save('host_tool_tasks',task['task_id'],task)
                if action=='status':return {'status':'indeterminate'}
                return {'_attempt_indeterminate':True}
            if action=='complete' and task['status'] in ('completed','failed'):
                try:
                    self._check(task['name'] in host['tools'],'Current host tool offering withdrawn')
                    self._execution_root(task['chain_id'],actor,task['name'],require_running=False)
                except Exception:return {'_authorization_denied':True}
            if action in ('begin','dispatch','complete','status') and task['status'] not in ('completed','failed','indeterminate','cancelled'):
                try:
                    self._check(task['name'] in host['tools'],'Current host tool offering withdrawn')
                    self._execution_root(task['chain_id'],actor,task['name'],require_running=action in ('begin','dispatch'))
                except Exception as error:
                    if getattr(error,'code','')=='host_tool_paused':return {'_peer_paused':True}
                    task.update(status='cancelled',ended_at=self.core.now(),reason='current-authorization-withdrawn',execution_outcome='unknown' if task.get('dispatched_at') else 'not-dispatched')
                    self._save('host_tool_tasks',task['task_id'],task)
                    if action=='status':return {'status':'cancelled'}
                    return {'_authorization_denied':True}
            if action=='status':
                try:self._lineage(task['chain_id'],actor)
                except Exception:
                    if task['status'] not in ('completed','failed','indeterminate'):task['status']='cancelled';self._save('host_tool_tasks',task['task_id'],task)
                return {'status':task['status']}
            if action=='begin':
                self._lineage(task['chain_id'],actor)
                self._check(task['status']=='leased' and task['lease_until']>=time.time(),'Task cannot begin twice or after lease expiry','host_tool_indeterminate')
                peer=p.get('peer_session_id');self._check(isinstance(peer,str) and peer!=host['session_key'],'Independent live Harness peer identity required')
                task.update(status='running',started_at=self.core.now(),peer_session_id=peer)
            elif action=='dispatch':
                self._lineage(task['chain_id'],actor)
                self._check(task['status']=='running' and not task.get('dispatched_at') and task['lease_until']>=time.time(),'Task dispatch must occur once after reservation','host_tool_indeterminate')
                task['dispatched_at']=self.core.now()
                self._event(task,{'type':'tool_start','host_time_ms':p.get('host_time_ms')})
            elif action=='complete':
                result=p['result'];fp=digest(result)
                if task['status'] in ('completed','failed'):
                    self._check(task.get('result_digest')==fp,'Conflicting completion receipt','host_tool_conflict');return {'accepted':True,'replayed':True}
                self._check(task['status']=='running' and task['lease_until']>=time.time(),'Ambiguous/cancelled/expired attempts may not be silently adopted','host_tool_indeterminate')
                self._check(task.get('dispatched_at') or p.get('is_error') is True,'A success requires actual Harness dispatch','host_tool_conflict')
                self._lineage(task['chain_id'],actor)
                task.update(status='failed' if p.get('is_error') else 'completed',result=copy.deepcopy(result),result_digest=fp,ended_at=self.core.now())
                if task.get('dispatched_at'):self._event(task,{'type':'tool_end','status':task['status'],'host_time_ms':p.get('host_time_ms')})
            elif action=='indeterminate':
                if task['status'] not in ('completed','failed','cancelled'):task['status']='indeterminate';task['ended_at']=self.core.now()
            else:self._check(False,'Unknown host transport action')
            self._save('host_tool_tasks',task['task_id'],task)
        with self.condition:self.condition.notify_all()
        return {'accepted':True,'status':task['status']}
