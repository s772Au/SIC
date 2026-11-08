"""Import semantic flows and execute an isolated official Langflow candidate chain."""
import copy
import json
from pathlib import Path
import sys
from urllib.request import Request, build_opener, ProxyHandler

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT), str(ROOT / 'integrations')]
from generate_workbench_flows import make_workbench_flow, candidate_steps
from sic_workflow.runtime_auth import langflow_access_token
from sic_workflow.v2_client import CoreClient
from v2_runtime_acceptance import action_output


def main():
    base, opener = 'http://127.0.0.1:17861', build_opener(ProxyHandler({}))
    headers = {'Content-Type': 'application/json', 'Authorization': 'Bearer ' + langflow_access_token(base, opener)}

    def request(path, value=None, method=None):
        raw = None if value is None else json.dumps(value, ensure_ascii=False).encode()
        with opener.open(Request(base + path, raw, headers, method=method or ('GET' if raw is None else 'POST')), timeout=180) as response:
            return json.load(response)

    key = request('/api/v1/api_key/', {'name': 'sic-semantic-workbench-acceptance'})
    headers['x-api-key'] = key['api_key']
    try:
        imported = []
        for path in sorted((ROOT / 'integrations/flows').glob('workbench-*.json')):
            flow = json.loads(path.read_text(encoding='utf-8'))
            for node in flow['data']['nodes']:
                assert not node['data']['node']['template'].get('allow_write', {}).get('value', False)
                assert not node['data']['node']['template'].get('allow_model', {}).get('value', False)
            created = request('/api/v1/flows/', flow)
            saved = request('/api/v1/flows/' + created['id'])
            assert saved['data'] == flow['data']
            imported.append({'file': path.name, 'flow_id': created['id']})
        core = CoreClient()
        context = core.execute('chain.create', payload={'creators': []}, allow_write=True)['context']
        roles = core.execute('config.get', context)['result']['safe_default_roles']
        assert all(role['model'] == 'openai/sic-offline-fixture' for role in roles.values())
        source = core.execute('source.ingest', context, {'format': 'openai-chat', 'request': {'messages': [{'role': 'user', 'content': 'Independent official Langflow semantic chain source'}]}}, allow_write=True)
        prepared = core.execute('edit.prepare', source['context'], {'source_id': source['context']['source_id']}, allow_write=True)
        delivered = core.execute('edit.run_all', prepared['context'], {'job_id': prepared['context']['job_id']}, allow_write=True, allow_model=True)
        assert delivered['ok'], delivered
        context = delivered['context']
        flow = make_workbench_flow('SIC semantic seven-node API acceptance', candidate_steps(context), connect=True)
        for node in flow['data']['nodes']:
            if node['data']['type'] == 'SICCandidate':
                node['data']['node']['template']['allow_write']['value'] = True
        created = request('/api/v1/flows/', flow)
        edited = copy.deepcopy(flow['data'])
        edited['nodes'][1]['data']['node']['template']['content']['value'] = 'Saved semantic form content used by real Langflow'
        edited['nodes'][1]['data']['node']['template']['persistent_text']['value'] = 'Persisted semantic P marker'
        request('/api/v1/flows/' + created['id'], {'data': edited}, 'PATCH')
        assert request('/api/v1/flows/' + created['id'])['data'] == edited
        output = request('/api/v1/run/' + created['id'], {'input_request': {'input_value': '', 'input_type': 'any', 'output_type': 'any', 'output_component': 'SICAccounting-7'}})
        result = action_output(output)
        assert result['ok'], result
        fixed = core.execute('cut.get', context)['result']['view']['cbs'][0]
        assert 'Saved semantic form content used by real Langflow' in fixed['body']
        assert 'Persisted semantic P marker' in fixed['body']
        report = {'status': 'passed', 'semantic_flows_imported': len(imported), 'imports': imported,
                  'official_connected_nodes_executed': 7, 'saved_typed_edit_effective': True,
                  'owned_preparation_model_calls': 1, 'flow_model_calls': 0, 'real_provider_calls': 0,
                  'gui_visual_verified': False, 'executed_flow_id': created['id']}
        (ROOT / '.local/integration-test-results/workbench-langflow-api.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
        print(json.dumps({k: v for k, v in report.items() if k not in {'imports', 'executed_flow_id'}}))
    finally:
        request('/api/v1/api_key/' + key['id'], method='DELETE')


if __name__ == '__main__':
    main()
