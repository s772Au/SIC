"""Focused regression for accumulated short WB formation in the new bridge."""
import copy,tempfile,unittest
from pathlib import Path
from sic_core import Core,CoreError
from sic_core.bridge import Bridge

class ShortStreamProvider:
    def __init__(self):self.calls=[]
    def capabilities(self):return {'test_double':True,'live_provider_verified':False}
    def call(self,role,request):
        self.calls.append((role,copy.deepcopy(request)))
        return {'requested_model':'local-fixture','returned_model':'local-fixture','response':{'id':'local-fixture','model':'local-fixture','choices':[{'index':0,'finish_reason':'stop','message':{'role':'assistant','content':'ok'}}],'usage':{'prompt_tokens':1,'completion_tokens':1,'total_tokens':2}}}

class BridgeFormationTests(unittest.TestCase):
    def test_work_end_flushes_short_pending_once_without_another_worker(self):
        provider=ShortStreamProvider();core=Core(provider=provider)
        bridge=Bridge(core,{'bridge':{'allow_write':True,'allow_model':True,'editor_mode':'program','seal_bytes':100000}})
        try:
            _,chain=bridge.chat('host','native-turn-end',{'model':'sic-worker','messages':[{'role':'user','content':'short final material'}]})
            with core.store.transaction():self.assertTrue(core.store.state(chain)['pending'])
            event={'type':'work_end','event_id':'native-end-once','observed_by':'dsh-native-turn-end','turn':1}
            first=bridge.events('host','native-turn-end',event)
            with core.store.transaction():
                state=core.store.state(chain);self.assertFalse(state['pending']);self.assertTrue(state['cbs'])
                self.assertEqual(sum(e['type']=='work_end' for e in state['events']),1)
                cbs=list(state['cbs'])
            self.assertEqual(bridge.events('host','native-turn-end',event),first)
            with core.store.transaction():self.assertEqual(core.store.state(chain)['cbs'],cbs)
            self.assertEqual(len(provider.calls),1)
            with self.assertRaises((CoreError,ValueError)):bridge.events('host','native-turn-end',{**event,'turn':2})
        finally:bridge.close();core.close()

    def test_work_end_cannot_hide_unresolved_native_tool_proposal(self):
        class ToolProvider(ShortStreamProvider):
            def call(self,role,request):
                self.calls.append((role,copy.deepcopy(request)))
                return {'requested_model':'local-fixture','returned_model':'local-fixture','response':{'id':'fixture','model':'local-fixture','choices':[{'index':0,'finish_reason':'tool_calls','message':{'role':'assistant','content':'','tool_calls':[{'id':'native-call','type':'function','function':{'name':'fixture','arguments':'{}'}}]}}]}}
        provider=ToolProvider();core=Core(provider=provider)
        bridge=Bridge(core,{'bridge':{'allow_write':True,'allow_model':True,'editor_mode':'program','seal_bytes':100000}})
        try:
            bridge.chat('host','unfinished',{'model':'sic-worker','messages':[{'role':'user','content':'call native tool'}],'tools':[{'type':'function','function':{'name':'fixture','parameters':{'type':'object','properties':{}}}}]})
            with self.assertRaises(CoreError) as caught:bridge.events('host','unfinished',{'type':'work_end','event_id':'premature'})
            self.assertEqual(caught.exception.code,'event_scope')
            self.assertEqual(len(provider.calls),1)
        finally:bridge.close();core.close()

    def test_editor_started_between_drive_and_worker_admission_is_awaited(self):
        provider=ShortStreamProvider();core=Core(provider=provider)
        bridge=Bridge(core,{'bridge':{'allow_write':True,'allow_model':True,'editor_mode':'program','seal_bytes':100000}})
        original=bridge._op;injected=[]
        def race(actor,binding,operation,payload,**kwargs):
            if operation=='provider.continue' and not injected:
                with core.store.transaction():source=core.store.state(binding['chain_id'])['pending'][0]
                context={'actor':actor,'chain_id':binding['chain_id'],'allow_write':True,'allow_model':False}
                injected.append(core.operate('edit.program',context,{'source_id':source})['result']['job_id'])
            return original(actor,binding,operation,payload,**kwargs)
        bridge._op=race
        try:
            request={'model':'sic-worker','messages':[{'role':'user','content':'Source ready before the admission race'}]}
            reply,chain=bridge.chat('host','admission-race',request)
            self.assertEqual(len(provider.calls),1)
            with core.store.transaction():self.assertEqual(core.store.state(chain)['jobs'][injected[0]]['status'],'published')
            self.assertEqual(reply,bridge.chat('host','admission-race',request)[0])
            self.assertEqual(len(provider.calls),1)
        finally:bridge.close();core.close()

    def test_actual_provider_failure_is_not_an_admission_retry(self):
        class FailedProvider(ShortStreamProvider):
            def call(self,role,request):
                self.calls.append((role,copy.deepcopy(request)))
                raise CoreError('provider_failed','owned provider attempt failed',502)
        provider=FailedProvider();core=Core(provider=provider)
        bridge=Bridge(core,{'bridge':{'allow_write':True,'allow_model':True,'editor_mode':'program','seal_bytes':100000}})
        try:
            with self.assertRaises(CoreError):bridge.chat('host','no-provider-retry',{'model':'sic-worker','messages':[{'role':'user','content':'one attempt'}]})
            self.assertEqual(len(provider.calls),1)
        finally:bridge.close();core.close()

    def test_short_first_source_does_not_starve_accumulated_stream(self):
        core=Core(provider=ShortStreamProvider());bridge=Bridge(core,{'bridge':{'allow_write':True,'allow_model':True,'editor_mode':'program','seal_bytes':10}});history=[]
        try:
            for _ in range(5):
                history.append({'role':'user','content':'hi'});reply,chain=bridge.chat('host','regression',{'model':'sic-worker','messages':history});history.append(reply['choices'][0]['message'])
            bridge.close()
            with core.store.transaction():
                state=core.store.state(chain);self.assertGreater(len(state['cbs']),0);self.assertLess(len(state['pending']),10)
                self.assertTrue(any(len(core.store.get(cb,'cb')['source_ids'])>1 for cb in state['cbs']))
        finally:
            bridge.close();core.close()

    def test_same_turn_is_receipted_but_identical_new_user_event_is_not_dropped(self):
        provider=ShortStreamProvider();core=Core(provider=provider)
        bridge=Bridge(core,{'bridge':{'allow_write':True,'allow_model':True,'editor_mode':'program','seal_bytes':100000}})
        try:
            request={'model':'sic-worker','store':False,'messages':[{'role':'user','content':'same text'}]}
            first,chain=bridge.chat('host','one',request)
            self.assertEqual(first,bridge.chat('host','one',request)[0]);self.assertEqual(len(provider.calls),1)
            second={**request,'messages':request['messages']+[first['choices'][0]['message'],{'role':'user','content':'same text'}]}
            bridge.chat('host','one',second);self.assertEqual(len(provider.calls),2)
            with core.store.transaction():
                state=core.store.state(chain)
                user_texts=[u['text'] for ids in state['groups'].values() for source_id in ids
                            for u in core.store.get(source_id,'source')['units'] if u['text']=='same text']
                self.assertEqual(len(user_texts),2)
            evidence=bridge.evidence('host','one');self.assertEqual(evidence['chain_id'],chain)
            self.assertEqual(len(evidence['projections']),1)
            self.assertIn('messages',evidence['projections'][0]['request'])
        finally:bridge.close();core.close()

    def test_binding_and_reply_survive_new_core_and_bridge_instances(self):
        with tempfile.TemporaryDirectory() as directory:
            database=str(Path(directory)/'core.db');provider=ShortStreamProvider()
            config={'bridge':{'allow_write':True,'allow_model':True,'editor_mode':'program','seal_bytes':100000}}
            core=Core(database,provider=provider);bridge=Bridge(core,config)
            request={'model':'sic-worker','messages':[{'role':'user','content':'persistent request'}]}
            try:first,chain=bridge.chat('host','restart',request)
            finally:bridge.close();core.close()
            replacement=ShortStreamProvider();core=Core(database,provider=replacement);bridge=Bridge(core,config)
            try:
                reply,new_chain=bridge.chat('host','restart',request)
                self.assertEqual(chain,new_chain);self.assertEqual(first,reply);self.assertEqual(replacement.calls,[])
                self.assertEqual(bridge.sessions('other'),[])
            finally:bridge.close();core.close()

    def test_changed_old_history_and_unowned_tool_event_are_rejected(self):
        core=Core(provider=ShortStreamProvider());bridge=Bridge(core,{'bridge':{'allow_write':True,'allow_model':True,'editor_mode':'program'}})
        try:
            request={'model':'sic-worker','messages':[{'role':'user','content':'original'}]}
            first,_=bridge.chat('host','strict',request)
            with self.assertRaisesRegex(Exception,'history'):
                bridge.chat('host','strict',{'model':'sic-worker','messages':[{'role':'user','content':'changed'},first['choices'][0]['message'],{'role':'user','content':'next'}]})
            with self.assertRaisesRegex(Exception,'delivered bridge proposal'):
                bridge.events('host','strict',{'type':'tool_start','call_id':'forged'})
        finally:bridge.close();core.close()

if __name__=='__main__':unittest.main()
