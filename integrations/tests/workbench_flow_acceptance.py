"""Execute the actual connected seven-node official LFX export, not a copy."""
import asyncio
import json
import os
from pathlib import Path
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT), str(ROOT / 'integrations'), str(ROOT / 'scripts')]
from lfx.load import load_flow_from_json
from generate_workbench_flows import make_workbench_flow, candidate_steps
from v2_experiment_fixture import OwnedFixture


def main():
    saved = {key: os.environ.get(key) for key in ('SIC_CORE_URL', 'SIC_CORE_TOKEN', 'SIC_CORE_ACTOR')}
    try:
        with tempfile.TemporaryDirectory(prefix='sic-lfx-workbench-') as directory, OwnedFixture(Path(directory) / 'core.db') as fixture:
            os.environ.update(SIC_CORE_URL=fixture.client.origin, SIC_CORE_TOKEN=fixture.client.token, SIC_CORE_ACTOR='host')
            client = fixture.client
            client.call('chain.create', {'creators': []})
            client.call('source.ingest', {'format': 'openai-chat', 'request': {'messages': [{'role': 'user', 'content': 'Real returned candidate for connected GUI flow'}]}})
            client.call('edit.prepare', {'source_id': client.context['source_id']})
            client.call('edit.run_all', {'job_id': client.context['job_id']})
            flow = make_workbench_flow('Connected official candidate acceptance', candidate_steps(client.context), connect=True)
            assert all(not node['data']['node']['template'].get('allow_write', {}).get('value', False) for node in flow['data']['nodes'])
            assert all(not node['data']['node']['template'].get('allow_model', {}).get('value', False) for node in flow['data']['nodes'])
            for node in flow['data']['nodes']:
                fields = node['data']['node']['template']
                if node['data']['type'] == 'SICCandidate':
                    fields['allow_write']['value'] = True
                if node['id'] == 'SICCandidate-2':
                    fields['content']['value'] = 'Actual connected flow reviewed content'
                    fields['persistent_text']['value'] = 'Connected P'
            graph = load_flow_from_json(flow)
            outputs = asyncio.run(graph.arun(inputs=[{}], outputs=['SICAccounting-7']))
            value = outputs[0].outputs[0].outputs['result']
            value = value.get('message', value.get('data', value)) if isinstance(value, dict) else value.data
            assert value['ok'], value
            cut = client.call('cut.get')
            assert len(cut['view']['cbs']) == 1
            assert 'Actual connected flow reviewed content' in cut['view']['cbs'][0]['body']
            assert 'Connected P' in cut['view']['cbs'][0]['body']
            assert len(fixture.model_requests) == 1, 'Flow must not call model again'
            report = {'status': 'passed', 'official_lfx_connected_nodes': 7, 'candidate_revision': 1,
                      'published_cb_count': 1, 'model_calls_during_flow': 0, 'owned_preparation_model_calls': 1,
                      'real_provider_calls': 0, 'default_export_gates_off': True, 'gui_visual_verified': False}
            (ROOT / '.local/integration-test-results/workbench-flow-acceptance.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
            print(json.dumps(report))
    finally:
        for key, value in saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


if __name__ == '__main__':
    main()
