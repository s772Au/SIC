"""Pause is scheduling, not permission withdrawal or an unknown retry."""
import json,threading,unittest
from sic_core import CoreError
import test_core_v2_host_tools as host_fixtures

class SignalLifecycleTests(unittest.TestCase):
    def setUp(self):self.fixture=host_fixtures.HostBrokerTests();self.fixture.setUp()
    def tearDown(self):self.fixture.tearDown()
    def signal(self,value):
        f=self.fixture;cp=f.op('chain.checkpoint',{},chain=f.peer)['checkpoint_id'];socket=f.op('socket.create',{'checkpoint_id':cp,'signal_acl':['host']},chain=f.peer)['socket_id']
        return f.op('socket.signal',{'socket_id':socket,'signal':value})
    def test_pause_before_dispatch_is_known_nonadmission_and_inflight_completion_can_settle(self):
        f=self.fixture;future=f.dispatch();task=f.pull();f.action(task,'begin',peer_session_id='owned-native')
        self.signal('wait')
        with self.assertRaises(CoreError) as caught:f.action(task,'dispatch')
        self.assertEqual(caught.exception.code,'host_tool_paused')
        with f.core.store.transaction():
            current=f.broker._load('host_tool_tasks',task['task_id']);self.assertEqual(current['status'],'running');self.assertNotIn('dispatched_at',current)
        self.signal('wake');f.action(task,'dispatch');self.signal('wait')
        self.assertEqual(f.action(task,'status')['status'],'running')
        f.action(task,'complete',result={'content':'one settled execution'})
        self.assertEqual(future.result(1),{'content':'one settled execution'})
        with f.core.store.transaction():events=f.core.store.state(f.peer)['events'];self.assertEqual(sum(e['type']=='tool_start' for e in events),1);self.assertEqual(sum(e['type']=='tool_end' for e in events),1)
    def editor_pause(self,signal):
        f=self.fixture;started=threading.Event();release=threading.Event()
        class Provider:
            def __init__(self):self.roles=[]
            def call(self,role,request):
                self.roles.append(role)
                if role=='editor' and len(self.roles)==1:started.set();release.wait(3)
                content=json.dumps({'ops':[],'pvi':{}}) if role=='editor' else 'Owned peer final result'
                return {'requested_model':'owned','returned_model':'owned','usage':None,'response':{'choices':[{'finish_reason':'stop','message':{'role':'assistant','content':content}}]}}
        provider=Provider();f.core.provider=provider;p={'request_id':'request','editor_mode':'model'}
        future=f.pool.submit(f.core.runtime.collaborate,f.root,'host',p)
        try:self.assertTrue(started.wait(1));self.signal(signal)
        finally:release.set()
        result=future.result(2);self.assertNotIn('worker',provider.roles)
        return result,provider,p
    def test_paused_editor_is_held_then_same_request_wakes_without_repair_round(self):
        f=self.fixture;result,provider,p=self.editor_pause('wait');self.assertEqual(result['status'],'waiting')
        with f.core.store.transaction():job=next(iter(f.core.store.state(f.peer)['jobs'].values()));response=job['responses'];job_id=job['job_id']
        self.signal('wake');done=f.core.runtime.collaborate(f.root,'host',p);self.assertEqual(done['status'],'completed');self.assertEqual(provider.roles.count('worker'),1)
        with f.core.store.transaction():self.assertEqual(f.core.store.state(f.peer)['jobs'][job_id]['responses'],response)
    def test_stopped_editor_is_terminal_and_resend_does_not_start_work(self):
        f=self.fixture;result,provider,p=self.editor_pause('stop');self.assertEqual(result['status'],'failed');self.assertEqual(result['branches'][0]['status'],'stopped')
        calls=list(provider.roles);again=f.core.runtime.collaborate(f.root,'host',p);self.assertEqual(again['status'],'failed');self.assertEqual(provider.roles,calls)
    def test_waiting_never_preserves_withdrawn_execution_permission(self):
        f=self.fixture;future=f.dispatch();task=f.pull();f.action(task,'begin',peer_session_id='owned-native');self.signal('wait')
        f.op('tools.configure',{'tools':[]})
        self.assertEqual(f.action(task,'status')['status'],'cancelled')
        with self.assertRaises(CoreError):future.result(1)
        self.signal('wake')
        with self.assertRaises(CoreError):f.action(task,'dispatch')
        with f.core.store.transaction():self.assertFalse(any(e['type']=='tool_start' for e in f.core.store.state(f.peer)['events']))

if __name__=='__main__':unittest.main()
