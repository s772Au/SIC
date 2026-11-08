"""Typed candidate actions must work with a real empty KEEP model delivery."""
import os
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT), str(ROOT / 'integrations'), str(ROOT / 'scripts')]
from langgraph.checkpoint.memory import InMemorySaver
from langgraph.types import Command
from sic_workflow.workbench_graph import builder
from v2_experiment_fixture import OwnedFixture


class EmptyCandidate(unittest.TestCase):
    def test_pvi_only_append_remove_and_publish_real_empty_delivery(self):
        saved = {key: os.environ.get(key) for key in ('SIC_CORE_URL', 'SIC_CORE_TOKEN', 'SIC_CORE_ACTOR')}
        try:
            with tempfile.TemporaryDirectory(prefix='sic-empty-candidate-') as directory, OwnedFixture(Path(directory) / 'core.db', editor_keep=True) as fixture:
                os.environ.update(SIC_CORE_URL=fixture.client.origin, SIC_CORE_TOKEN=fixture.client.token, SIC_CORE_ACTOR='host')
                client = fixture.client
                client.call('chain.create', {'creators': []})
                client.call('source.ingest', {'format': 'openai-chat', 'request': {'messages': [{'role': 'user', 'content': 'Original implicit KEEP body'}]}})
                client.call('edit.prepare', {'source_id': client.context['source_id']})
                client.call('edit.run_all', {'job_id': client.context['job_id']})
                job, chain = client.context['job_id'], client.context['chain_id']
                original = client.call('edit.candidate.get', {'job_id': job})
                self.assertEqual(original['ops'], [])
                graph = builder.compile(checkpointer=InMemorySaver())
                cfg = {'configurable': {'thread_id': 'empty-candidate'}}

                def action(form):
                    state = graph.invoke({'chain_id': chain, 'job_id': job, 'form': form}, cfg)
                    self.assertIn('__interrupt__', state)
                    state = graph.invoke(Command(resume=state['__interrupt__'][0].value['form']), cfg)
                    self.assertTrue(state['result']['ok'], state['result'])
                    return state

                common = {'action': 'revise', 'allow_write': True, 'persistent': ['Added P without a content op'], 'volatile': [], 'instant': {'operator_note': 'PVI only'}}
                first = action({**common, 'edit_mode': 'pvi_only', 'expected_revision': 0})
                self.assertEqual(first['after']['result']['ops'], [])
                second = action({**common, 'edit_mode': 'append', 'op': 'FLAG', 'target': 'u1', 'content': 'Reviewed flag', 'expected_revision': 1})
                self.assertEqual(len(second['after']['result']['ops']), 1)
                third = action({**common, 'edit_mode': 'remove', 'operation_number': 1, 'expected_revision': 2})
                self.assertEqual(third['after']['result']['ops'], [])
                self.assertEqual(third['after']['result']['original_responses'], original['original_responses'])
                action({'action': 'validate', 'expected_revision': 3})
                action({'action': 'publish', 'expected_revision': 3, 'expected_cut': 0, 'allow_write': True})
                fixed = client.call('cut.get')['view']['cbs'][0]
                self.assertIn('Original implicit KEEP body', fixed['body'])
                self.assertIn('Added P without a content op', fixed['body'])
                self.assertEqual(len(fixture.model_requests), 1)
        finally:
            for key, value in saved.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value


if __name__ == '__main__':
    unittest.main()
