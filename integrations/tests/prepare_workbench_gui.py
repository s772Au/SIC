"""Prepare a NEW model-delivered candidate GUI entry, never execute its flow."""
import json
from pathlib import Path
import sys
from urllib.request import Request, build_opener, ProxyHandler

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT), str(ROOT / 'integrations')]
from generate_workbench_flows import make_workbench_flow, candidate_steps
from sic_workflow.runtime_auth import langflow_access_token
from sic_workflow.v2_client import CoreClient


def main():
    client = CoreClient()
    # This preparer must only run against our explicit offline fixture binding.
    probe = client.execute('chain.create', payload={'creators': []}, allow_write=True)
    assert probe.get('ok'), probe
    context = probe['context']
    profiles = client.execute('config.get', context)['result']
    roles = profiles.get('safe_default_roles', {})
    assert roles and all(role.get('model') == 'openai/sic-offline-fixture' for role in roles.values()), 'Owned fixture roles required'
    source = client.execute('source.ingest', context, {'format': 'openai-chat', 'request': {'messages': [
        {'role': 'system', 'content': 'Host-owned protected instruction: never change native tool controls.'},
        {'role': 'user', 'content': 'Fresh GUI evidence. Review this returned candidate using the typed form.'},
        {'role': 'user', 'content': 'Second source remains independent evidence.'}]}}, allow_write=True)
    assert source['ok'], source
    prepared = client.execute('edit.prepare', source['context'], {'source_id': source['context']['source_id']}, allow_write=True)
    assert prepared['ok'], prepared
    delivered = client.execute('edit.run_all', prepared['context'], {'job_id': prepared['context']['job_id']}, allow_write=True, allow_model=True)
    assert delivered['ok'], delivered
    current = client.execute('edit.candidate.get', delivered['context'], {'job_id': delivered['context']['job_id']})
    assert current['ok'] and current['result']['ready'], current
    flow = make_workbench_flow('SIC GUI — 已交付候选版本修订与固定发布', candidate_steps(current['context']), connect=True)
    opener, base = build_opener(ProxyHandler({})), 'http://127.0.0.1:17861'
    access = langflow_access_token(base, opener)
    request = Request(base + '/api/v1/flows/', json.dumps(flow, ensure_ascii=False).encode(),
                      {'Content-Type': 'application/json', 'Authorization': 'Bearer ' + access}, method='POST')
    with opener.open(request, timeout=120) as response:
        saved = json.load(response)
    metadata = {'url': base + '/flow/' + saved['id'], 'flow_id': saved['id'], 'context': current['context'],
                'candidate_revision': 0, 'expected_cut': 0, 'writes_default_off': True, 'model_gates_off': True,
                'preparation_owned_model_calls': 1, 'real_provider_calls': 0, 'isolated_from_dsh': True,
                'studio_input': {'chain_id': current['context']['chain_id'], 'job_id': current['context']['job_id'],
                                 'form': {'action': 'read'}}}
    destination = ROOT / '.local/integration-test-results/workbench-gui-entry.json'
    destination.write_text(json.dumps(metadata, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps({'url': metadata['url'], 'nodes': 7, 'write_gates': 'off', 'real_provider_calls': 0}))


if __name__ == '__main__':
    main()
