import json,tempfile,time,unittest
from pathlib import Path
from sic_core import Core,CoreError

class NoTraffic:
    def __init__(self):self.calls=0
    def call(self,*args):self.calls+=1;raise RuntimeError('owned failed fixture')
    def capabilities(self):return {'owned_fixture':True}

class PendingFlushTests(unittest.TestCase):
    def setUp(self):
        self.directory=tempfile.TemporaryDirectory();self.path=str(Path(self.directory.name)/'core.db');self.provider=NoTraffic();self.core=Core(self.path,provider=self.provider);self.chain=self.op('chain.create',{})['chain_id']
    def tearDown(self):self.core.close();self.directory.cleanup()
    def op(self,name,p=None):return self.core.operate(name,{'actor':'host','chain_id':getattr(self,'chain',None),'allow_write':True,'allow_model':True},p or {})['result']
    def configure(self,**kw):return self.op('runtime.configure',{'enabled':True,'editor_mode':'program','worker_mode':'host','seal_chars':10000,'max_pending_seconds':.08,**kw})
    def source(self):return self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'short owned material'}]}})['source_id']
    def until(self,predicate,seconds=3):
        deadline=time.monotonic()+seconds
        while time.monotonic()<deadline:
            if predicate():return True
            time.sleep(.01)
        return False
    def state(self):return self.op('chain.get')
    def test_terminal_short_block_flushes_without_more_input_or_provider(self):
        self.configure();self.source()
        self.assertTrue(self.until(lambda:bool(self.state()['cbs'])))
        self.assertEqual(self.state()['pending'],[]);self.assertEqual(self.provider.calls,0)
        count=len(self.state()['events']);time.sleep(.12)
        self.assertEqual(len(self.state()['cbs']),1)
        self.assertEqual(sum(e['type']=='pending_timeout' for e in self.state()['events']),1)
    def test_empty_enabled_runtime_never_calls_or_enqueues_timeout(self):
        self.configure();time.sleep(.12)
        self.assertEqual(self.provider.calls,0);self.assertFalse(self.state()['cbs'])
        self.assertFalse(any(e['type']=='pending_timeout' for e in self.state()['events']))
    def test_explicit_work_end_flushes_before_long_deadline(self):
        self.configure(max_pending_seconds=3600);self.source();self.op('harness.event',{'type':'work_end'})
        self.assertTrue(self.until(lambda:bool(self.state()['cbs'])))
        self.assertFalse(any(e['type']=='pending_timeout' for e in self.state()['events']))
    def test_pending_age_survives_restart(self):
        self.configure(enabled=False,max_pending_seconds=.05);self.source()
        # Persist an enabled but unstarted runtime, as a stopped host would leave it.
        with self.core.store.transaction():
            state=self.core.store.state(self.chain);state['runtime']['enabled']=True;self.core.store.save(state)
        self.core.close();time.sleep(.06);self.core=Core(self.path,provider=self.provider);self.core.runtime.start()
        self.assertTrue(self.until(lambda:bool(self.state()['cbs'])))
        event=next(e for e in self.state()['events'] if e['type']=='pending_timeout')
        self.assertLess(event['due_at'],event['time']);self.assertEqual(self.provider.calls,0)
    def test_failed_editor_timeout_has_no_automatic_retry_even_after_restart(self):
        self.configure(editor_mode='model');self.source()
        self.assertTrue(self.until(lambda:self.provider.calls==1))
        self.assertTrue(self.until(lambda:bool(self.state().get('runtime_last_error'))))
        time.sleep(.15);self.assertEqual(self.provider.calls,1)
        self.core.close();self.core=Core(self.path,provider=self.provider);self.core.runtime.start();time.sleep(.15)
        self.assertEqual(self.provider.calls,1);self.assertTrue(self.state()['pending'])
    def test_invalid_timeout_rolls_back_configuration(self):
        for value in (0,-1,True,float('inf'),86401):
            with self.assertRaises(CoreError):self.configure(max_pending_seconds=value)
    def test_shutdown_before_due_does_not_flush_or_invoke(self):
        self.configure(max_pending_seconds=.2);self.source();self.core.runtime.stop();time.sleep(.23)
        self.assertTrue(self.state()['pending']);self.assertFalse(self.state()['cbs']);self.assertEqual(self.provider.calls,0)

if __name__=='__main__':unittest.main()
