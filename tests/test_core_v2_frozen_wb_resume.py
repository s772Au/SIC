"""Full-history resumed turns must preserve formed WB boundaries; no network."""
import copy,json,unittest
from unittest.mock import patch
from sic_core import Core,CoreError
from sic_core.bridge import Bridge

class OwnedProvider:
    def __init__(self):self.calls=[];self.next_tool_call=False
    def capabilities(self):return {'test_double':True,'live_provider_verified':False}
    def call(self,role,request):
        self.calls.append((role,copy.deepcopy(request)))
        if role=='worker' and self.next_tool_call:
            self.next_tool_call=False
            return {'requested_model':'owned','returned_model':'owned','response':{'id':'owned-tool','model':'owned','choices':[{'index':0,'finish_reason':'tool_calls','message':{'role':'assistant','content':'','tool_calls':[{'id':'owned-native-read','type':'function','function':{'name':'read','arguments':'{"file_path":"owned.txt"}'}}]}}]}}
        content='not JSON' if role=='editor' else 'owned final answer'
        return {'requested_model':'owned','returned_model':'owned','response':{'id':'owned','model':'owned','choices':[{'index':0,'finish_reason':'stop','message':{'role':'assistant','content':content}}],'usage':{'prompt_tokens':1,'completion_tokens':1,'total_tokens':2}}}

class FrozenWBResumeTests(unittest.TestCase):
    def setUp(self):
        self.provider=OwnedProvider();self.core=Core(provider=self.provider)
        self.bridge=Bridge(self.core,{'bridge':{'allow_write':True,'allow_model':True,'editor_mode':'program','seal_bytes':1000}})
        binding,lease=self.bridge._claim('host','owned-native-session');self.chain=binding['chain_id'];self.bridge._release('host','owned-native-session',lease)
        with self.core.store.transaction():
            s=self.core.store.state(self.chain);s['runtime']['enabled']=False;self.core.store.save(s)
        # Explicit synchronous boundaries make the regression deterministic;
        # ingress, scheduling, editing, publication and request receipts are real.
        self.kick=patch.object(self.bridge,'kick');self.kick.start()
        self.history=[{'role':'user','content':'initial source evidence '*100}]
        reply,chain=self.bridge.chat('host','owned-native-session',{'model':'sic-worker','messages':self.history})
        self.history.append(reply['choices'][0]['message']);self.assertEqual(chain,self.chain)
        with self.core.store.transaction():
            s=self.core.store.state(self.chain);self.old_cb=s['cbs'][0];self.old_cb_data=copy.deepcopy(self.core.store.get(self.old_cb,'cb'));self.root=s['pending'][0]
    def tearDown(self):
        self.kick.stop();self.bridge.close();self.core.close()
    def op(self,name,payload,model=False):
        return self.core.operate(name,{'actor':'host','chain_id':self.chain,'allow_write':True,'allow_model':model},payload)['result']
    def state(self):
        with self.core.store.transaction():return self.core.store.state(self.chain)
    def freeze(self):return self.op('wb.form',{'source_ids':[self.root]})['wb_id']
    def assert_old_immutable(self):
        with self.core.store.transaction():self.assertEqual(self.core.store.get(self.old_cb,'cb'),self.old_cb_data)
    def test_real_full_history_resume_processes_formed_tail_separately_and_receipts_replay(self):
        wid=self.freeze()
        with self.core.store.transaction():before=copy.deepcopy(self.core.store.get(wid,'wb'))
        self.history.append({'role':'user','content':'late correction new source '*100})
        request={'model':'sic-worker','messages':self.history};reply,chain=self.bridge.chat('host','owned-native-session',request)
        self.assertEqual(chain,self.chain);self.assertEqual(len(self.provider.calls),2)
        s=self.state();self.assertEqual(len(s['cbs']),3)
        with self.core.store.transaction():
            self.assertEqual(self.core.store.get(wid,'wb'),before)
            matched=[self.core.store.get(cid,'cb') for cid in s['cbs'] if self.core.store.get(cid,'cb')['wb_id']==wid]
        self.assertEqual(len(matched),1);self.assertEqual(matched[0]['source_ids'],before['source_ids'])
        self.assertIn('late correction new source',json.dumps(self.provider.calls[-1][1]))
        self.assertEqual(self.bridge.chat('host','owned-native-session',request)[0],reply)
        self.assertEqual(len(self.provider.calls),2);self.assert_old_immutable()
    def test_failed_editor_tail_requires_explicit_recovery_without_membership_error_or_retry(self):
        job=self.op('edit.prepare',{'source_id':self.root})['job_id']
        with self.assertRaises(CoreError):self.op('edit.run',{'job_id':job,'editor':'editor-1'},model=True)
        frozen=self.state()['jobs'][job]['source_snapshot']
        self.history.append({'role':'user','content':'next real full-history input '*100})
        with self.assertRaises(CoreError) as rejected:self.bridge.chat('host','owned-native-session',{'model':'sic-worker','messages':self.history})
        self.assertEqual(rejected.exception.code,'editor_recovery_required')
        self.assertEqual(len(self.provider.calls),2) # one original Worker, one failed Editor
        s=self.state();self.assertEqual(len(s['pending']),2);self.assertEqual(s['jobs'][job]['status'],'failed');self.assertEqual(s['jobs'][job]['source_snapshot'],frozen)
        self.assertEqual(s['groups'][self.root],frozen['source_ids']);self.assertEqual(len(s['cbs']),1);self.assert_old_immutable()
    def test_automatic_runtime_honors_preformed_tail_before_later_material(self):
        wid=self.freeze();new=self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'new runtime source '*100}]}})['source_id']
        outcome=self.core.runtime.run(self.chain,'host',{'max_steps':2,'editor_mode':'program','continue_worker':False,'automatic_event':True})
        self.assertEqual(sum(x['action']=='published' for x in outcome['steps']),2)
        s=self.state();self.assertFalse(s['pending']);self.assertEqual(len(s['cbs']),3)
        with self.core.store.transaction():
            cb=next(self.core.store.get(cid,'cb') for cid in s['cbs'] if self.core.store.get(cid,'cb')['wb_id']==wid)
        self.assertEqual(cb['source_ids'],[self.root]);self.assertNotIn(new,cb['source_ids']);self.assertEqual(len(self.provider.calls),1);self.assert_old_immutable()
    def test_explicit_illegal_regrouping_is_still_rejected_atomically(self):
        self.freeze();self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'later'}]}});before=self.state()
        with self.assertRaises(CoreError) as rejected:self.op('wb.form',{'source_ids':before['pending']})
        self.assertIn('cannot change frozen job membership',str(rejected.exception));self.assertEqual(self.state(),before);self.assert_old_immutable()

    def test_native_tool_full_history_then_resumed_user_preserves_closed_group_and_wb(self):
        key='owned-resumed-tool-session';binding,lease=self.bridge._claim('host',key);chain=binding['chain_id'];self.bridge._release('host',key,lease)
        with self.core.store.transaction():
            s=self.core.store.state(chain);s['runtime']['enabled']=False;self.core.store.save(s)
        schema={'type':'function','function':{'name':'read','parameters':{'type':'object','properties':{'file_path':{'type':'string'}},'required':['file_path']}}}
        self.provider.next_tool_call=True;history=[{'role':'user','content':'read owned source '*100}]
        reply,_=self.bridge.chat('host',key,{'model':'sic-worker','messages':history,'tools':[schema]});history.append(reply['choices'][0]['message'])
        history.append({'role':'tool','tool_call_id':'owned-native-read','content':'owned successful read evidence '*100})
        reply,_=self.bridge.chat('host',key,{'model':'sic-worker','messages':history,'tools':[schema]});history.append(reply['choices'][0]['message'])
        ctx={'actor':'host','chain_id':chain,'allow_write':True,'allow_model':False}
        with self.core.store.transaction():
            s=self.core.store.state(chain);root=s['pending'][-1];old={cid:copy.deepcopy(self.core.store.get(cid,'cb')) for cid in s['cbs']}
        wid=self.core.operate('wb.form',ctx,{'source_ids':[root]})['result']['wb_id']
        with self.core.store.transaction():wb=copy.deepcopy(self.core.store.get(wid,'wb'))
        history.append({'role':'user','content':'same native session late correction '*100})
        reply,actual=self.bridge.chat('host',key,{'model':'sic-worker','messages':history,'tools':[schema]});self.assertEqual(actual,chain)
        self.assertEqual(len([r for r in self.provider.calls if r[0]=='worker']),4) # setUp plus these three actual admissions
        with self.core.store.transaction():
            s=self.core.store.state(chain);self.assertEqual(self.core.store.get(wid,'wb'),wb)
            for cid,value in old.items():self.assertEqual(self.core.store.get(cid,'cb'),value)
            self.assertEqual(self.bridge._binding('host',key)['history'][-2],history[-1])
            self.assertFalse(self.core._source(s,root)['open_calls'])

if __name__=='__main__':unittest.main()
