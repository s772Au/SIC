"""Both official readers expose the real immutable Socket delivery provenance."""
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT), str(ROOT/'integrations'), str(ROOT/'scripts')]
from components.sic.memory import SICMemory
from sic_workflow.workbench_graph import memory_graph
from v2_experiment_fixture import OwnedFixture


class CommunicationReader(unittest.TestCase):
    def test_p_and_v_receipts_are_readable_without_new_calls_or_mutation(self):
        saved = {key: os.environ.get(key) for key in ('SIC_CORE_URL', 'SIC_CORE_TOKEN', 'SIC_CORE_ACTOR')}
        try:
            with tempfile.TemporaryDirectory(prefix='sic-comm-reader-') as directory, OwnedFixture(Path(directory)/'core.db') as own:
                os.environ.update(SIC_CORE_URL=own.client.origin, SIC_CORE_TOKEN=own.client.token, SIC_CORE_ACTOR='host')
                client = own.client
                target = client.call('chain.create')['chain_id']
                def publish(text):
                    sid = client.call('source.ingest', {'request': {'messages': [{'role': 'user', 'content': text}]}})['source_id']
                    job = client.call('edit.program', {'source_id': sid})['job_id']
                    return client.call('cut.publish', {'job_id': job, 'expected_cut': client.call('chain.get')['cut_seq'], 'idempotency_key': job})
                publish('Owned initial state')
                checkpoint = client.call('chain.checkpoint')['checkpoint_id']
                socket = client.call('socket.create', {'checkpoint_id': checkpoint, 'message_acl': ['host']})['socket_id']
                sender = client.call('chain.create')['chain_id']
                for lifecycle in ('P', 'V'):
                    client.call('socket.message', {'socket_id': socket, 'message_id': 'owned-'+lifecycle, 'lifecycle': lifecycle, 'text': 'Owned '+lifecycle+' message'})
                client.context['chain_id'] = target
                publication = publish('Owned next state')
                before = client.call('chain.get')
                component = SICMemory()
                component.set(chain_id=target, action='读取通信来源片段')
                component._pre_run_setup()
                lf = component.navigate().data
                self.assertTrue(lf['ok'], lf)
                lg = memory_graph.invoke({'chain_id': target, 'action': 'communication', 'object_id': publication['cut_id']})
                self.assertEqual(lg['status'], 'readonly')
                self.assertEqual(lg['result']['result'], lf['result'])
                rows = json.loads(lf['result']['value'])['communications']
                self.assertEqual({row['message_id'] for row in rows}, {'owned-P', 'owned-V'})
                self.assertTrue(all(row['sender'] == sender and row['received_at'] > 0 and row['receive_receipt_id'] for row in rows))
                self.assertEqual(client.call('chain.get'), before)
                self.assertFalse(own.model_requests)
        finally:
            for key, value in saved.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value


if __name__ == '__main__':
    unittest.main()
