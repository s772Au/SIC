"""Offline boundaries for the explicit real-Harness runner; never calls a gateway."""
import copy,io,json,sys,tempfile,unittest
from pathlib import Path
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from scripts import run_dsh_live_tests as runner

class MeteredProviderBoundaries(unittest.TestCase):
    def provider(self,folder,limit=1):
        with patch.object(runner,'LiteLLMProvider') as factory:
            value=runner.MeteredProvider(folder,{'worker':{'model':'unit-alias'}},limit)
        return value
    def test_cap_refuses_an_extra_attempt_without_delegate_or_new_request(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);provider=self.provider(root);provider.delegate.call.return_value={'returned_model':'unit-alias','usage':{},'requested_model':'unit-alias'}
            provider.call('worker',{'messages':[{'role':'user','content':'first'}]})
            with self.assertRaisesRegex(ValueError,'call cap'):provider.call('worker',{'messages':[{'role':'user','content':'extra'}]})
            provider.delegate.call.assert_called_once();self.assertEqual(len(provider.calls),1);self.assertEqual(len(list((root/'calls').glob('*.request.json'))),1);self.assertFalse((root/'calls/0002.request.json').exists())
    def test_failed_delegate_counts_toward_cap_and_is_persisted(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);provider=self.provider(root);provider.delegate.call.side_effect=RuntimeError('unit offline failure')
            with patch.object(runner.os.environ,'get',return_value=''),self.assertRaisesRegex(RuntimeError,'unit offline failure'):provider.call('editor',{'messages':[]})
            with self.assertRaisesRegex(ValueError,'call cap'):provider.call('editor',{'messages':[]})
            provider.delegate.call.assert_called_once();rows=json.loads((root/'provider-attempts.json').read_text(encoding='utf-8'));self.assertEqual(len(rows),1);self.assertEqual(rows[0]['status'],'failed');self.assertEqual(rows[0]['attempt'],1);self.assertGreaterEqual(rows[0]['ended_at'],rows[0]['started_at']);self.assertFalse((root/'calls/0001.response.json').exists())
    def test_actual_saved_request_sampling_is_fixed_without_mutating_input(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);provider=self.provider(root);provider.delegate.call.return_value={'returned_model':'unit-alias'}
            request={'messages':[{'role':'user','content':'nested immutable input'}],'temperature':0.9,'top_p':0.2,'metadata':{'case':'unit'}};original=copy.deepcopy(request)
            provider.call('worker',request);saved=json.loads((root/'calls/0001.request.json').read_text(encoding='utf-8'));actual=provider.delegate.call.call_args.args[1]
            self.assertEqual(saved,actual);self.assertEqual(saved['temperature'],0);self.assertEqual(saved['top_p'],1);self.assertEqual(request,original);self.assertIsNot(actual,request);self.assertIsNot(actual['messages'],request['messages'])

class LiveCliValidationBoundaries(unittest.TestCase):
    def test_missing_optin_or_invalid_route_or_cap_rejects_before_secret_or_plan(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);base=['runner','--dsh-command',str(root/'not-executed.exe'),'--prepared',str(root/'not-read-plan.json'),'--output',str(root/'not-created'),'--model','unit-alias']
            variants=[[],['--allow-test-gateway','--endpoint','https://example.test/v1'],['--allow-test-gateway','--endpoint','http://user:pass@127.0.0.1:8341/v1'],['--allow-test-gateway','--max-model-calls','0']]
            real_get=runner.os.environ.get
            def no_secret(key,*args):
                if key=='SIC_LIVE_PROVIDER_KEY':raise AssertionError('validation must precede any secret read')
                return real_get(key,*args)
            for extra in variants:
                with self.subTest(extra=extra),patch.object(sys,'argv',base+extra),patch.object(runner.os.environ,'get',side_effect=no_secret),patch.object(Path,'read_text',side_effect=AssertionError('plan must not be read')) as read,patch.object(runner,'MeteredProvider') as provider,patch.object(runner.subprocess,'Popen') as child,patch.object(sys,'stderr',new_callable=io.StringIO) as stderr:
                    with self.assertRaises(SystemExit) as raised:runner.main()
                    self.assertEqual(raised.exception.code,2);read.assert_not_called();provider.assert_not_called();child.assert_not_called();self.assertNotIn('user:pass',stderr.getvalue());self.assertFalse((root/'not-created').exists())

if __name__=='__main__':unittest.main()
