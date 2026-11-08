"""Narrow actual native SDK replay forms; no network or provider calls."""
import copy,unittest
from sic_core import Core,CoreError
from sic_core.bridge import Bridge
from sic_core.history import generated_assistant_equivalent


def assistant(content=None):
    return {'role':'assistant','content':content,'reasoning_content':'preserved reasoning','tool_calls':[{'index':0,'id':'native-read','type':'function','function':{'name':'read','arguments':'{ "file_path": "owned.txt", "offset": 1 }'}}]}

def native(message):
    result=copy.deepcopy(message)
    for call in result['tool_calls']:call.pop('index',None)
    result['content']=None;result['tool_calls'][0]['function']['arguments']='{"offset":1,"file_path":"owned.txt"}'
    return result

class NativeHistoryTests(unittest.TestCase):
    def test_latest_tool_reply_empty_forms_and_exact_positional_transport_index(self):
        for content in (None,'',[]):
            before=assistant(content);after=native(before);saved=copy.deepcopy((before,after))
            self.assertTrue(generated_assistant_equivalent(before,after));self.assertEqual((before,after),saved)
    def test_wrong_or_unknown_transport_index_never_equivalent(self):
        for bad in (None,True,False,'0',-1,1,0.0):
            before=assistant();before['tool_calls'][0]['index']=bad
            self.assertFalse(generated_assistant_equivalent(before,native(before)))
        before=assistant();before['tool_calls'].append({'index':1,'id':'second','type':'function','function':{'name':'read','arguments':'{}'}})
        after=native(before);self.assertTrue(generated_assistant_equivalent(before,after));after['tool_calls'].reverse();self.assertFalse(generated_assistant_equivalent(before,after))
    def test_semantic_text_reasoning_metadata_identity_and_argument_changes_rejected(self):
        before=assistant()
        for mutate in [lambda x:x.update(content='nonempty'),lambda x:x.update(reasoning_content='changed'),lambda x:x.pop('reasoning_content'),lambda x:x.update(annotations=[{'url':'different'}]),lambda x:x.update(unknown=None),lambda x:x['tool_calls'][0].update(id='other'),lambda x:x['tool_calls'][0]['function'].update(name='write'),lambda x:x['tool_calls'][0]['function'].update(arguments='{"file_path":"owned.txt","offset":2}'),lambda x:x['tool_calls'][0]['function'].update(arguments='{"offset":1,"offset":2}'),lambda x:x['tool_calls'][0].update(unknown=None)]:
            after=native(before);mutate(after);self.assertFalse(generated_assistant_equivalent(before,after))
        self.assertFalse(generated_assistant_equivalent({'role':'assistant','content':None},{'role':'assistant','content':[]}))
        self.assertFalse(generated_assistant_equivalent(assistant([{'type':'text','text':''}]),native(assistant())))
    def test_only_latest_generated_reply_is_permitted_to_normalize(self):
        core=Core();bridge=Bridge(core,{'bridge':{'allow_write':True,'allow_model':True}})
        try:
            generated=assistant('');base=[{'role':'system','content':'protected'}, {'role':'user','content':'owned request'}]
            binding={'history':base+[generated],'generated_index':2}
            resumed=base+[native(generated),{'role':'tool','tool_call_id':'native-read','content':'owned tool result'}]
            bridge._check_history(binding,resumed)
            for index in (0,1):
                changed=copy.deepcopy(resumed);changed[index]['content']+='changed'
                with self.assertRaises(CoreError):bridge._check_history(binding,changed)
            # Once admitted, prior assistant/tool messages form the strict prefix.
            strict={'history':resumed+[{'role':'assistant','content':'new reply'}],'generated_index':4}
            changed=copy.deepcopy(strict['history']);changed[2]['tool_calls'][0]['index']=0
            with self.assertRaises(CoreError):bridge._check_history(strict,changed)
            changed=copy.deepcopy(strict['history']);changed[3]['content']='changed result'
            with self.assertRaises(CoreError):bridge._check_history(strict,changed)
        finally:bridge.close();core.close()

if __name__=='__main__':unittest.main()
