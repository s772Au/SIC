"""Durable host pull boundaries. All execution here is an explicit test double."""
import concurrent.futures,json,sqlite3,tempfile,time,unittest,threading
from pathlib import Path
from sic_core import Core,CoreError
from sic_core.host_tools import HostToolBroker

class HostBrokerTests(unittest.TestCase):
    def setUp(self):
        self.core=Core();self.broker=HostToolBroker(self.core,{'host_tools_enabled':True,'host_tool_allowlist':['fixture'],'host_tool_wait_seconds':2,'host_tool_lease_seconds':2});self.core.host_tools=self.broker
        self.root=self.op('chain.create',{},None)['chain_id']
        tool={'name':'fixture','descriptor':'local fixture','schema':{'type':'function','function':{'name':'fixture','parameters':{'type':'object','properties':{}}}},'discover_acl':['host'],'expand_acl':['host'],'execute_acl':['host'],'expanded':True}
        self.op('tools.configure',{'tools':[tool]})
        source=self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'parent source'}]}})['source_id']
        job=self.op('edit.program',{'source_id':source,'rules':[]})['job_id'];self.op('cut.publish',{'job_id':job,'expected_cut':0,'idempotency_key':'base'})
        cp=self.op('chain.checkpoint',{})['checkpoint_id'];socket=self.op('socket.create',{'checkpoint_id':cp,'spawn_acl':['host']})['socket_id']
        row=self.op('collaboration.dispatch',{'socket_ids':[socket],'request_id':'request','question':'peer task'})
        with self.core.store.transaction():self.peer=self.core.store.state(self.root)['collaborations']['request']['branches'][0]['chain_id']
        self.broker.register('host','parent-session',self.root,{'host_id':'host-one','tools':['fixture']})
        self.pool=concurrent.futures.ThreadPoolExecutor(2)
    def tearDown(self):self.broker.close();self.pool.shutdown(wait=True);self.core.close()
    def op(self,name,payload,chain='root'):
        return self.core.operate(name,{'actor':'host','chain_id':getattr(self,'root',None) if chain=='root' else chain,'allow_write':True,'allow_model':False},payload)['result']
    def dispatch(self,call='call-one'):
        return self.pool.submit(self.broker.execute,{'chain_id':self.peer,'actor':'host','call_id':call},'fixture',{'value':1})
    def pull(self):
        deadline=time.monotonic()+1
        while time.monotonic()<deadline:
            result=self.broker.action('host',{'action':'pull','host_id':'host-one'})['task']
            if result:return result
            time.sleep(.005)
        self.fail('host task was not queued')
    def action(self,task,action,**p):return self.broker.action('host',{'action':action,'host_id':'host-one','task_id':task['task_id'],'lease':task['lease'],**p})
    def begin(self,t):
        self.action(t,'begin',peer_session_id='independent-peer-session',host_time_ms=time.time()*1000)
        return self.action(t,'dispatch',host_time_ms=time.time()*1000)
    def test_reservation_and_policy_denial_are_not_actual_tool_execution(self):
        future=self.dispatch();task=self.pull()
        self.action(task,'begin',peer_session_id='independent-peer-session')
        with self.core.store.transaction():self.assertEqual(self.core.store.state(self.peer)['open_tools'],{})
        with self.assertRaises(CoreError):self.action(task,'complete',result='fake success')
        result={'isError':True,'content':'host policy denied before dispatch'}
        self.action(task,'complete',result=result,is_error=True)
        self.assertEqual(future.result(1),result)
        with self.core.store.transaction():self.assertFalse(any(e['type']=='tool_start' for e in self.core.store.state(self.peer)['events']))
    def test_actual_begin_events_and_idempotent_completion_no_reexecution(self):
        future=self.dispatch();task=self.pull()
        with self.core.store.transaction():self.assertEqual(self.core.store.state(self.peer)['open_tools'],{})
        self.begin(task)
        with self.core.store.transaction():self.assertIn('call-one',self.core.store.state(self.peer)['open_tools'])
        value={'content':'actual host result'};self.action(task,'complete',result=value);self.assertEqual(future.result(1),value)
        self.assertTrue(self.action(task,'complete',result=value)['replayed'])
        self.assertEqual(self.dispatch().result(1),value)
        self.assertIsNone(self.broker.action('host',{'action':'pull','host_id':'host-one'})['task'])
        with self.assertRaises(CoreError):self.action(task,'complete',result={'changed':True})
    def test_scope_actor_parent_and_host_lease_are_not_interchangeable(self):
        self.assertFalse(self.broker.available(self.root,'host','fixture'))
        self.assertFalse(self.broker.available(self.peer,'intruder','fixture'))
        future=self.dispatch();task=self.pull()
        with self.assertRaises(CoreError):self.broker.action('intruder',{'action':'begin','host_id':'host-one','task_id':task['task_id'],'lease':task['lease'],'peer_session_id':'peer'})
        with self.assertRaises(CoreError):self.action({**task,'lease':'other'},'begin',peer_session_id='peer')
        self.begin(task);self.action(task,'complete',result='done');self.assertEqual(future.result(1),'done')
    def test_disconnect_is_durable_indeterminate_and_never_reclaimed(self):
        future=self.dispatch();task=self.pull();self.begin(task)
        self.action(task,'indeterminate')
        with self.assertRaises(CoreError) as e:future.result(1)
        self.assertEqual(e.exception.code,'host_tool_indeterminate')
        self.assertIsNone(self.broker.action('host',{'action':'pull','host_id':'host-one'})['task'])
        with self.assertRaises(CoreError):self.action(task,'complete',result='late ambiguous result')
        with self.assertRaises(CoreError):self.dispatch().result(1)
    def test_expired_lease_never_reissues_or_adopts_late_completion(self):
        future=self.dispatch();task=self.pull();self.begin(task)
        with self.core.store.transaction():
            value=self.broker._load('host_tool_tasks',task['task_id']);value['lease_until']=0;self.broker._save('host_tool_tasks',task['task_id'],value)
        with self.assertRaises(CoreError):future.result(1)
        with self.assertRaises(CoreError):self.action(task,'complete',result='late')
        self.assertIsNone(self.broker.action('host',{'action':'pull','host_id':'host-one'})['task'])
    def test_cancelled_parent_stops_peer_and_blocks_result_adoption(self):
        future=self.dispatch();task=self.pull();self.begin(task)
        self.op('runtime.recover',{'action':'cancel_collaboration','request_id':'request','confirm':True})
        self.assertEqual(self.action(task,'status')['status'],'cancelled')
        with self.assertRaises(CoreError):future.result(1)
        with self.assertRaises(CoreError):self.action(task,'complete',result='cancelled result')
    def test_failed_host_tool_result_preserves_error_not_success(self):
        future=self.dispatch();task=self.pull();self.begin(task);result={'isError':True,'content':'permission denied'}
        self.action(task,'complete',result=result,is_error=True)
        self.assertEqual(future.result(1),result)
        with self.core.store.transaction():self.assertEqual(self.broker._load('host_tool_tasks',task['task_id'])['status'],'failed')
    def test_completed_and_indeterminate_receipts_survive_reopened_database(self):
        first=self.dispatch();done=self.pull();self.begin(done);self.action(done,'complete',result='durable result');first.result(1)
        second=self.dispatch('unknown');unknown=self.pull();self.begin(unknown);self.action(unknown,'indeterminate')
        with self.assertRaises(CoreError):second.result(1)
        with tempfile.TemporaryDirectory() as directory:
            path=str(Path(directory)/'core.db')
            copy=sqlite3.connect(path)
            try:self.core.store.db.backup(copy)
            finally:copy.close()
            recovered=Core(path);broker=HostToolBroker(recovered,self.broker.policy);recovered.host_tools=broker
            try:
                self.assertEqual(broker.execute({'chain_id':self.peer,'actor':'host','call_id':'call-one'},'fixture',{'value':1}),'durable result')
                with self.assertRaises(CoreError):broker.execute({'chain_id':self.peer,'actor':'host','call_id':'unknown'},'fixture',{'value':1})
                self.assertIsNone(broker.action('host',{'action':'pull','host_id':'host-one'})['task'])
            finally:recovered.close()
    def test_new_host_generation_replaces_live_registration_without_reexecuting_claimed_task(self):
        future=self.dispatch();task=self.pull();self.begin(task)
        registered=self.broker.register('host','parent-session',self.root,{'host_id':'host-two','tools':['fixture']})
        self.assertGreater(registered['generation'],0)
        self.assertTrue(self.broker.available(self.peer,'host','fixture'))
        with self.assertRaises(CoreError):future.result(1)
        with self.assertRaises(CoreError):self.broker.action('host',{'action':'pull','host_id':'host-one'})
        with self.assertRaises(CoreError):self.broker.register('host','parent-session',self.root,{'host_id':'host-one','tools':['fixture']})
        self.assertIsNone(self.broker.action('host',{'action':'pull','host_id':'host-two'})['task'])
        with self.assertRaises(CoreError):self.action(task,'complete',result='old generation late result')
        with self.assertRaises(CoreError):self.dispatch().result(1)
        with self.core.store.transaction():self.assertEqual(self.broker._load('host_tool_tasks',task['task_id'])['status'],'indeterminate')
    def test_same_generation_refresh_and_distinct_sessions_do_not_steal_each_other(self):
        self.broker.register('host','parent-session',self.root,{'host_id':'host-one','tools':['fixture']})
        self.assertTrue(self.broker.available(self.peer,'host','fixture'))
        other=self.op('chain.create',{},None)['chain_id']
        self.broker.register('host','other-session',other,{'host_id':'unrelated','tools':['fixture']})
        self.assertTrue(self.broker.available(self.peer,'host','fixture'))
    def test_current_route_revocation_after_reservation_blocks_dispatch_durably(self):
        future=self.dispatch();task=self.pull();self.action(task,'begin',peer_session_id='peer')
        self.op('tools.configure',{'tools':[]})
        with self.assertRaises(CoreError):self.action(task,'dispatch')
        with self.assertRaises(CoreError):future.result(1)
        with self.core.store.transaction():
            self.assertEqual(self.broker._load('host_tool_tasks',task['task_id'])['status'],'cancelled')
            self.assertFalse(any(e['type']=='tool_start' for e in self.core.store.state(self.peer)['events']))
    def test_current_deployment_revocation_after_dispatch_blocks_result_adoption(self):
        future=self.dispatch();task=self.pull();self.begin(task)
        self.broker.policy['host_tools_enabled']=False;self.broker.policy['host_tool_allowlist']=[]
        with self.assertRaises(CoreError):self.action(task,'complete',result='not authorized now')
        with self.assertRaises(CoreError):future.result(1)
        with self.core.store.transaction():self.assertNotIn('result',self.broker._load('host_tool_tasks',task['task_id']))
    def test_current_route_revocation_after_dispatch_blocks_result_and_replay(self):
        future=self.dispatch();task=self.pull();self.begin(task)
        self.op('tools.configure',{'tools':[]})
        with self.assertRaises(CoreError):self.action(task,'complete',result='not authorized now')
        with self.assertRaises(CoreError):future.result(1)
        self.assertFalse(self.broker.available(self.peer,'host','fixture'))
    def test_same_host_updated_tool_offering_revokes_reserved_task(self):
        future=self.dispatch();task=self.pull();self.action(task,'begin',peer_session_id='peer')
        self.broker.policy['host_tool_allowlist'].append('other')
        self.broker.register('host','parent-session',self.root,{'host_id':'host-one','tools':['other']})
        with self.assertRaises(CoreError):self.action(task,'dispatch')
        with self.assertRaises(CoreError):future.result(1)
    def test_explicit_host_close_keeps_claimed_outcome_unknown_and_forbids_late_adoption(self):
        future=self.dispatch();task=self.pull();self.begin(task)
        self.assertTrue(self.broker.action('host',{'action':'close','host_id':'host-one'})['closed'])
        self.assertTrue(self.broker.action('host',{'action':'close','host_id':'host-one'})['replayed'])
        with self.assertRaises(CoreError):future.result(1)
        with self.assertRaises(CoreError):self.action(task,'complete',result='late closed-host result')
        with self.assertRaises(CoreError):self.broker.action('host',{'action':'pull','host_id':'host-one'})
        with self.assertRaises(CoreError):self.broker.register('host','parent-session',self.root,{'host_id':'host-one','tools':['fixture']})
        with self.core.store.transaction():
            record=self.broker._load('host_tool_tasks',task['task_id']);self.assertEqual(record['status'],'indeterminate');self.assertEqual(record['reason'],'host-closed')
            self.assertFalse(any(e['type']=='tool_end' for e in self.core.store.state(self.peer)['events']))
        self.broker.register('host','parent-session',self.root,{'host_id':'host-two','tools':['fixture']})
        self.assertIsNone(self.broker.action('host',{'action':'pull','host_id':'host-two'})['task'])
        with self.assertRaises(CoreError):self.dispatch().result(1)
    def test_close_queued_task_cancels_without_fabricating_dispatch(self):
        future=self.dispatch();deadline=time.monotonic()+1
        while time.monotonic()<deadline:
            if self.broker.action('host',{'action':'pending','host_id':'host-one'})['queued']:break
            time.sleep(.005)
        self.broker.action('host',{'action':'close','host_id':'host-one'})
        with self.assertRaises(CoreError):future.result(1)
        with self.core.store.transaction():
            self.assertFalse(self.core.store.state(self.peer)['open_tools'])
            self.assertFalse(any(e['type'] in ('tool_start','tool_end') for e in self.core.store.state(self.peer)['events']))
    def test_reopened_broker_status_expires_lease_without_old_execute_thread(self):
        future=self.dispatch();task=self.pull();self.begin(task)
        with tempfile.TemporaryDirectory() as directory:
            path=str(Path(directory)/'core.db');connection=sqlite3.connect(path)
            try:self.core.store.db.backup(connection)
            finally:connection.close()
            # Release only the original fixture's waiter; the saved DB remains running.
            self.action(task,'indeterminate')
            with self.assertRaises(CoreError):future.result(1)
            recovered=Core(path);broker=HostToolBroker(recovered,self.broker.policy);recovered.host_tools=broker
            command={'host_id':'host-one','task_id':task['task_id'],'lease':task['lease']}
            try:
                with recovered.store.transaction():
                    saved=broker._load('host_tool_tasks',task['task_id']);self.assertEqual(saved['status'],'running');saved['lease_until']=0;broker._save('host_tool_tasks',task['task_id'],saved)
                self.assertEqual(broker.action('host',{'action':'status',**command})['status'],'indeterminate')
                with recovered.store.transaction():
                    saved=broker._load('host_tool_tasks',task['task_id']);self.assertEqual(saved['status'],'indeterminate');self.assertEqual(saved['reason'],'host-lease-expired')
                    self.assertFalse(any(e['type']=='tool_end' for e in recovered.store.state(self.peer)['events']))
                with self.assertRaises(CoreError):broker.action('host',{'action':'complete',**command,'result':'late old native output'})
                self.assertIsNone(broker.action('host',{'action':'pull','host_id':'host-one'})['task'])
            finally:recovered.close()
    def test_child_pending_timer_observes_inflight_collaboration_without_duplicate_execution(self):
        editor_started=threading.Event();release=threading.Event()
        class HeldEditor:
            def __init__(self):self.workers=0;self.editors=0;self.guard=threading.Lock()
            def call(self,role,request):
                with self.guard:
                    if role=='editor':self.editors+=1;number=self.editors
                    else:self.workers+=1;number=self.workers
                if role=='editor':
                    if number==1:editor_started.set();release.wait(4)
                    message={'role':'assistant','content':json.dumps({'ops':[],'pvi':{}})}
                elif number==1:message={'role':'assistant','content':None,'tool_calls':[{'id':'single-native','type':'function','function':{'name':'fixture','arguments':'{}'}}]}
                else:message={'role':'assistant','content':'Used actual owned tool result'}
                return {'requested_model':'owned','returned_model':'owned','usage':None,'response':{'choices':[{'finish_reason':'tool_calls' if role=='worker' and number==1 else 'stop','message':message}]}}
        provider=HeldEditor();self.core.provider=provider
        # Configure explicitly before starting the event pump, then install the
        # equivalent standing grant so the first actual timer races the batch.
        self.op('runtime.configure',{'enabled':False,'editor_mode':'model','worker_mode':'host','seal_chars':10000,'max_pending_seconds':.03},chain=self.peer)
        with self.core.store.transaction():
            state=self.core.store.state(self.peer);state['runtime']['enabled']=True;state['runtime_model_authorized']=True;self.core.store.save(state)
        future=self.pool.submit(self.core.runtime.collaborate,self.root,'host',{'request_id':'request','editor_mode':'model'})
        try:
            self.assertTrue(editor_started.wait(2));self.core.runtime.start();deadline=time.monotonic()+2;observed=False
            while time.monotonic()<deadline:
                with self.core.store.transaction():rows=self.core.store.db.execute("SELECT event,status FROM runtime_queue WHERE chain_id=?",(self.peer,)).fetchall()
                if any(json.loads(event).get('type')=='pending_timeout' and status=='completed' for event,status in rows):observed=True;break
                time.sleep(.005)
            self.assertTrue(observed,'real pending timer must complete while original Editor is held')
            self.assertEqual(provider.editors,1);self.assertEqual(provider.workers,0)
            self.assertNotEqual(self.op('collaboration.status',{})['requests']['request']['status'],'failed')
            release.set();task=self.pull();self.begin(task);self.action(task,'complete',result={'content':'owned native result'})
            result=future.result(4);self.assertEqual(result['status'],'completed');self.assertEqual(provider.workers,2)
            with self.core.store.transaction():
                root=self.core.store.state(self.root);peer=self.core.store.state(self.peer)
                self.assertEqual(sum(m.get('request_id')=='request' for m in root['inbox']),1)
                self.assertEqual(sum(e['type']=='tool_start' and e['call_id']=='single-native' for e in peer['events']),1)
                self.assertEqual(sum(e['type']=='tool_end' and e['call_id']=='single-native' for e in peer['events']),1)
                self.assertFalse(any(b.get('error',{}).get('code')=='runtime_busy' for b in root['collaborations']['request']['branches']))
        finally:release.set()
    def test_runtime_adopts_native_error_content_without_marking_tool_success(self):
        class WorkerFixture:
            def __init__(self):self.calls=0
            def call(self,role,request):
                self.calls+=1
                message={'role':'assistant','content':'Observed owned native failure'} if self.calls>1 else {'role':'assistant','content':None,'tool_calls':[{'id':'native-error-call','type':'function','function':{'name':'fixture','arguments':'{}'}}]}
                return {'requested_model':'owned-fixture','returned_model':'owned-fixture','response':{'choices':[{'finish_reason':'stop' if self.calls>1 else 'tool_calls','message':message}]},'usage':None}
        self.core.provider=WorkerFixture()
        future=self.pool.submit(self.core.operate,'runtime.run',{'actor':'host','chain_id':self.peer,'allow_write':True,'allow_model':True},{'editor_mode':'program','continue_worker':True})
        task=self.pull();self.begin(task);wire={'isError':True,'content':[{'type':'text','text':'owned native tool failed'}]}
        self.action(task,'complete',result=wire,is_error=True);future.result(3)
        with self.core.store.transaction():
            state=self.core.store.state(self.peer);record=state['tool_results']['native-error-call'];result=self.core.store.get(record['object_id'],'tool_result')
            self.assertEqual(record['status'],'failed');self.assertEqual(result['status'],'failed');self.assertEqual(json.loads(result['content']),wire)
            self.assertTrue(any(e['type']=='tool_end' and e['status']=='failed' for e in state['events']))
if __name__=='__main__':unittest.main()
