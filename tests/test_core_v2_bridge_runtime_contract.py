"""Bridge/shared Runtime recovery and transport boundary regressions; no network."""
import json,time,unittest
from unittest.mock import patch
from sic_core import Core, CoreError
from sic_core.bridge import Bridge
from sic_core.adapters import digest
from sic_core.wait_windows import serial_wait_segments

class BridgeRuntimeContractTests(unittest.TestCase):
    def setUp(self):
        self.core=Core()
        self.bridge=Bridge(self.core,{'bridge':{'allow_write':True,'allow_model':True,'editor_mode':'program','seal_bytes':1}})
        binding,lease=self.bridge._claim('host','review')
        self.chain=binding['chain_id'];self.bridge._release('host','review',lease)
        with self.core.store.transaction():
            state=self.core.store.state(self.chain);state['runtime']['enabled']=False;self.core.store.save(state)
    def tearDown(self):
        self.bridge.close();self.core.close()
    def op(self,name,payload):
        return self.core.operate(name,{'actor':'host','chain_id':self.chain,'allow_write':True,'allow_model':False},payload)['result']
    def test_actual_sdk_null_refusal_container_is_replayable(self):
        raw={'choices':[{'finish_reason':'stop','message':{'role':'assistant','content':'ok','provider_specific_fields':{'refusal':None}}}]}
        self.assertEqual(self.bridge._reply(raw)['choices'][0]['message'],{'role':'assistant','content':'ok'})
        raw['choices'][0]['message']['provider_specific_fields']={'unknown_control':'value'}
        with self.assertRaises(CoreError):self.bridge._reply(raw)
    def test_editor_recovery_refuses_live_shared_runtime(self):
        lock=self.core.runtime._lock(self.chain);lock.acquire()
        try:
            with self.assertRaises(CoreError) as rejected:
                self.bridge.recover('host','review',{'action':'retry_editor','confirm':True})
            self.assertEqual(rejected.exception.code,'runtime_busy')
        finally:lock.release()
    def test_editor_recovery_reopens_failed_admitted_turn_without_model_call(self):
        fp=digest({'test':'admitted'})
        with self.core.store.transaction():
            binding=self.bridge._binding('host','review');binding.update(phase='failed',current_fingerprint=fp,editor_error={'code':'editor_failed'})
            self.bridge._save('host','review',binding)
            self.bridge._save_turn('host','review',fp,{'phase':'failed','attempt':0,'round':0})
        self.bridge.recover('host','review',{'action':'retry_editor','confirm':True})
        with self.core.store.transaction():
            self.assertEqual(self.bridge._turn('host','review',fp)['phase'],'ingested')
            self.assertIsNone(self.bridge._binding('host','review')['editor_error'])
    def test_blocking_drive_retries_returned_publication_deferred(self):
        self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'ready'}]}})
        calls=[]
        def run(chain,actor,payload):
            calls.append(payload)
            if len(calls)==1:return {'steps':[{'action':'publication-deferred','reason':'Worker in flight'}]}
            with self.core.store.transaction():
                state=self.core.store.state(chain);state['pending']=[];self.core.store.save(state)
            return {'steps':[{'action':'published'}]}
        with patch.object(self.core.runtime,'run',side_effect=run):
            self.bridge._drive('host','review',force=True,wait=True,blocking_worker=True)
        self.assertEqual(len(calls),2)
        self.assertTrue(all(not call['blocking_worker'] for call in calls))
        with self.core.store.transaction():
            intervals=[e for e in self.core.store.state(self.chain)['events'] if e.get('kind')=='serial_wait']
        self.assertTrue(intervals)
        self.assertGreater(sum(e['end']-e['start'] for e in intervals),0)
    def test_host_wait_complement_excludes_overlapping_tools_and_cancelled_wait(self):
        events=[{'type':'tool_start','call_id':'a','time':1},
                {'type':'tool_start','call_id':'a','time':3,'duplicate_observation':True},
                {'type':'wait','wait_id':'w','seconds':10,'time':4},
                {'type':'tool_end','call_id':'a','time':5},
                {'type':'wait_cancel','wait_id':'w','time':7},
                {'type':'tool_start','call_id':'b','time':9}]
        expected=[(0,1),(7,9)]
        self.assertEqual(serial_wait_segments(events,0,10),expected)
        with self.core.store.transaction():
            state=self.core.store.state(self.chain);state['events']=events;self.core.store.save(state)
        self.bridge._record_host_wait('host','review',0,10)
        with self.core.store.transaction():
            intervals=[e for e in self.core.store.state(self.chain)['events'] if e.get('kind')=='serial_wait']
        self.assertEqual([(e['start'],e['end']) for e in intervals],expected)
        self.assertEqual(len({e['wait_id'] for e in intervals}),1)
    def test_expired_wait_only_excludes_declared_duration(self):
        events=[{'type':'wait','wait_id':'w','seconds':3,'time':1}]
        self.assertEqual(serial_wait_segments(events,0,10),[(0,1),(4,10)])
        self.assertEqual(serial_wait_segments(events,2,3),[])
    def test_runtime_editor_wait_excludes_declaration_and_records_after_cancel(self):
        owner=self
        class Editor:
            def capabilities(self):return {'test_double':True}
            def call(self,role,request):
                units=json.loads(request['messages'][-1]['content'])['editable_text_units']
                time.sleep(.01)
                if cancel:owner.op('harness.event',{'type':'wait_cancel','wait_id':'natural'})
                time.sleep(.01)
                content=json.dumps({'ops':[{'op':'KEEP','target':u['unit_id']} for u in units],'pvi':{}})
                return {'requested_model':'fixture','returned_model':'fixture','response':{'model':'fixture','choices':[{'finish_reason':'stop','message':{'role':'assistant','content':content}}]}}
        self.core.provider=Editor()
        for cancel in (False,True):
            self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'editable input'}]}})
            self.op('harness.event',{'type':'wait','wait_id':'natural','seconds':100})
            with self.core.store.transaction():before=len(self.core.store.state(self.chain)['events'])
            self.core.runtime.run(self.chain,'host',{'editor_mode':'model','max_steps':1,'continue_worker':False,'blocking_worker':True})
            with self.core.store.transaction():
                events=self.core.store.state(self.chain)['events'][before:]
            intervals=[e for e in events if e.get('kind')=='serial_wait']
            if cancel:
                self.assertTrue(intervals)
                cancelled=next(e['time'] for e in events if e['type']=='wait_cancel')
                self.assertTrue(all(e['start']>=cancelled for e in intervals))
            else:self.assertEqual(intervals,[])

if __name__=='__main__':unittest.main()
