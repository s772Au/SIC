"""Dispatched peer return policy survives every execution entry point."""
import copy,unittest
from unittest.mock import patch
from sic_core import Core,CoreError
from test_core_v2_mechanisms import MechanismProvider

class CollaborationPolicyTests(unittest.TestCase):
    def setUp(self):
        self.provider=MechanismProvider();self.core=Core(provider=self.provider)
        self.chain=self.op('chain.create')['chain_id']
        source=self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'historical retained context'}]}})['source_id']
        job=self.op('edit.program',{'source_id':source})['job_id']
        self.op('cut.publish',{'job_id':job,'expected_cut':0,'idempotency_key':'first'})
        checkpoint=self.op('chain.checkpoint')['checkpoint_id']
        self.socket=self.op('socket.create',{'checkpoint_id':checkpoint,'spawn_acl':['host'],'message_acl':['host'],'signal_acl':['host']})['socket_id']
    def tearDown(self):self.core.close()
    def op(self,name,p=None):
        return self.core.operate(name,{'actor':'host','chain_id':getattr(self,'chain',None),'allow_write':True,'allow_model':True},p or {})['result']
    def dispatch(self,lifecycle='P',request_id='policy'):
        return self.op('collaboration.dispatch',{'socket_ids':[self.socket],'question':'new question','request_id':request_id,'return_lifecycle':lifecycle})
    def run_peer(self,**extra):return self.op('collaboration.run',{'request_id':'policy','editor_mode':'program','max_steps':8,**extra})
    def assert_policy(self,lifecycle):
        state=self.op('chain.get');row=state['collaborations']['policy']
        self.assertEqual(row['return_lifecycle'],lifecycle)
        self.assertEqual([branch['return_lifecycle'] for branch in row['branches']],[lifecycle])
        self.assertEqual([message['lifecycle'] for message in state['inbox'] if message.get('request_id')=='policy'],[lifecycle])
        message=next(message for message in state['inbox'] if message.get('request_id')=='policy')
        self.assertIsInstance(message['received_at'],(int,float));self.assertTrue(message['received_event_id']);self.assertTrue(message['receive_receipt_id'])
        self.assertEqual(len([event for event in state['events'] if event.get('type')=='communication_received' and event.get('message_id')==message['message_id']]),1)
    def test_p_and_v_omitted_run_policy_and_resend_are_stable(self):
        for lifecycle in ('P','V'):
            with self.subTest(lifecycle=lifecycle):
                self.dispatch(lifecycle);self.assertEqual(self.run_peer()['status'],'completed');self.assert_policy(lifecycle)
                before=copy.deepcopy(self.op('chain.get')['inbox']);calls=len(self.provider.calls)
                self.run_peer();self.run_peer(return_lifecycle=lifecycle)
                self.assertEqual(len(self.provider.calls),calls);self.assertEqual(self.op('chain.get')['inbox'],before)
                if lifecycle=='P':
                    # New independent request scope for the V half.
                    self.tearDown();self.setUp()
    def test_mismatching_run_rejected_before_work_and_after_completion(self):
        self.dispatch('P')
        for completed in (False,True):
            if completed:self.run_peer()
            before=copy.deepcopy(self.op('chain.get'));calls=len(self.provider.calls)
            with self.assertRaises(CoreError) as raised:self.run_peer(return_lifecycle='V')
            self.assertEqual(raised.exception.code,'collaboration_policy_conflict')
            self.assertEqual(len(self.provider.calls),calls);self.assertEqual(self.op('chain.get'),before)
    def test_failed_retry_uses_original_policy_and_refuses_override(self):
        for lifecycle in ('P','V'):
            with self.subTest(lifecycle=lifecycle):
                self.dispatch(lifecycle)
                def fail_provider(role,request):
                    self.provider.calls.append((role,copy.deepcopy(request)));raise RuntimeError('isolated provider failure')
                with patch.object(self.provider,'call',side_effect=fail_provider):
                    self.assertEqual(self.run_peer()['status'],'failed')
                count=len(self.provider.calls);self.assertEqual(count,1)
                with self.assertRaises(CoreError):self.run_peer(retry_failed=True,acknowledge_possible_duplicate=True,return_lifecycle='V' if lifecycle=='P' else 'P')
                self.assertEqual(len(self.provider.calls),count)
                self.assertEqual(self.run_peer(retry_failed=True,acknowledge_possible_duplicate=True)['status'],'completed')
                self.assert_policy(lifecycle);self.assertEqual(self.op('chain.get')['collaborations']['policy']['branches'][0]['attempt'],1)
                before=len(self.provider.calls);self.run_peer();self.assertEqual(before,len(self.provider.calls))
                if lifecycle=='P':self.tearDown();self.setUp()
    def test_child_timer_entry_uses_frozen_parent_policy(self):
        row=self.dispatch('P');child=row['branches'][0]['chain_id']
        self.core.runtime.notify(child,'host',{'type':'pending_timeout'})
        with self.core.store.transaction():
            state=self.core.store.state(child);state['runtime']['editor_mode']='program';self.core.store.save(state)
            event=self.core.store.db.execute('SELECT id,chain_id,actor,event FROM runtime_queue WHERE chain_id=? ORDER BY rowid LIMIT 1',(child,)).fetchone()
        self.assertIsNotNone(event);self.core.runtime._handle_event(event)
        self.assertEqual(self.op('chain.get')['collaborations']['policy']['status'],'completed');self.assert_policy('P')
    def test_legacy_child_policy_migrates_without_defaulting_p_to_v(self):
        row=self.dispatch('P')
        with self.core.store.transaction():
            state=self.core.store.state(self.chain);old=state['collaborations']['policy'];del old['return_lifecycle']
            for branch in old['branches']:del branch['return_lifecycle']
            self.core.store.save(state)
        self.assertEqual(self.run_peer()['status'],'completed');self.assert_policy('P')
    def test_invalid_policy_rejected_without_creating_child(self):
        with self.core.store.transaction():before=self.core.store.db.execute('SELECT count(*) FROM chains').fetchone()[0]
        with self.assertRaises(CoreError):self.dispatch('I')
        with self.core.store.transaction():self.assertEqual(self.core.store.db.execute('SELECT count(*) FROM chains').fetchone()[0],before)
    def test_in_progress_duplicate_cannot_change_policy(self):
        self.dispatch('P');lock=self.core.runtime._lock('collaboration-request:'+self.chain+':policy');lock.acquire()
        try:
            self.assertEqual(self.run_peer()['execution_state'],'in-progress')
            with self.assertRaises(CoreError) as raised:self.run_peer(return_lifecycle='V')
            self.assertEqual(raised.exception.code,'collaboration_policy_conflict');self.assertEqual(self.provider.calls,[])
        finally:lock.release()
    def test_conflicting_legacy_child_policies_fail_closed(self):
        row=self.dispatch('P')
        with self.core.store.transaction():
            child=self.core.store.state(row['branches'][0]['chain_id']);child['collaboration_task']['return_lifecycle']='V';self.core.store.save(child)
        with self.assertRaises(CoreError) as raised:self.run_peer()
        self.assertEqual(raised.exception.code,'collaboration_policy_conflict');self.assertEqual(self.provider.calls,[])
    def test_parent_dispatch_event_uses_frozen_policy(self):
        self.dispatch('P')
        with self.core.store.transaction():
            state=self.core.store.state(self.chain);state.setdefault('runtime',{})['editor_mode']='program';self.core.store.save(state)
            event=self.core.store.db.execute('SELECT id,chain_id,actor,event FROM runtime_queue WHERE chain_id=? ORDER BY rowid DESC LIMIT 1',(self.chain,)).fetchone()
        self.assertIsNotNone(event);self.core.runtime._handle_event(event)
        self.assertEqual(self.op('chain.get')['collaborations']['policy']['status'],'completed');self.assert_policy('P')

if __name__=='__main__':unittest.main()
