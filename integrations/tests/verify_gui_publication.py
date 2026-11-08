"""Read-only persisted facts after the root agent's actual Langflow GUI run.

Never runs a flow, writes Core, or calls a model. GUI observation belongs to
the operator; this script independently checks the saved form against SQLite.
"""
import argparse
import hashlib
import json
from pathlib import Path
import sqlite3
import sys
from urllib.request import Request, build_opener, ProxyHandler

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT), str(ROOT / 'integrations')]
from sic_workflow.runtime_auth import langflow_access_token


def canonical_digest(value):
    return hashlib.sha256(json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--screenshot', type=Path, help='Root-provided actual GUI screenshot, hashed only')
    args = parser.parse_args()
    metadata = json.loads((ROOT / '.local/integration-test-results/workbench-gui-entry.json').read_text(encoding='utf-8'))
    context = metadata['context']
    opener, base = build_opener(ProxyHandler({})), 'http://127.0.0.1:17861'
    access = langflow_access_token(base, opener)
    with opener.open(Request(base + '/api/v1/flows/' + metadata['flow_id'], headers={'Authorization': 'Bearer ' + access}), timeout=30) as response:
        flow = json.load(response)
    node = next(n for n in flow['data']['nodes'] if n['id'] == 'SICCandidate-2')
    saved = {key: value.get('value') for key, value in node['data']['node']['template'].items() if isinstance(value, dict)}
    expected_pvi = {'P': [saved['persistent_text']] if saved['persistent_text'] else [],
                    'V': [saved['volatile_text']] if saved['volatile_text'] else [], 'I': json.loads(saved['instant_views'])}
    assert saved['allow_write'] is True
    publication = next(n for n in flow['data']['nodes'] if n['id'] == 'SICCandidate-4')
    assert publication['data']['node']['template']['allow_write']['value'] is True
    db = sqlite3.connect((ROOT / '.local/core-ui-demo.db').resolve().as_uri() + '?mode=ro', uri=True)
    db.execute('PRAGMA query_only=ON')
    db.execute('BEGIN')
    objects = {}

    def get(object_id, kind):
        row = db.execute('SELECT kind,data,digest FROM objects WHERE id=?', (object_id,)).fetchone()
        assert row and row[0] == kind
        value = json.loads(row[1])
        assert canonical_digest(value) == row[2], 'Immutable object digest mismatch'
        objects[object_id] = {'kind': kind, 'sha256': row[2]}
        return value

    try:
        state = json.loads(db.execute('SELECT state FROM chains WHERE id=?', (context['chain_id'],)).fetchone()[0])
        job = state['jobs'][context['job_id']]
        assert job['status'] == 'published'
        candidate = job['candidate']
        assert candidate['revision'] == saved['expected_revision'] + 1 == 1
        assert candidate['pvi'] == expected_pvi
        operation = candidate['ops'][saved['operation_number'] - 1]
        assert operation['content'] == saved['content'] and operation['reason'] == saved['reason']
        cb = get(job['cb_id'], 'cb')
        assert cb['candidate_revision'] == candidate['revision']
        assert cb['candidate_revision_id'] == candidate['revision_id']
        revision = get(candidate['revision_id'], 'candidate_revision')
        assert revision['ops'] == candidate['ops'] and revision['pvi'] == expected_pvi
        assert saved['content'] in cb['body'] and all(part in cb['body'] for part in expected_pvi['P'])
        assert any(op.get('content') == saved['content'] and op.get('reason') == saved['reason'] for op in cb['ops'])
        cut = get(state['current_cut'], 'cut')
        assert cb['cb_id'] in cut['cbs']
        assert all(cut['instant'][key] == value for key, value in expected_pvi['I'].items())
        if expected_pvi['V']:
            volatile = get(cut['volatile_source'], 'source')
            assert all(part in '\n'.join(unit['text'] for unit in volatile['units']) for part in expected_pvi['V'])
        for response in job['responses'].values():
            raw = get(response['response_id'], 'provider_response')
            original = json.loads(raw['response']['choices'][0]['message']['content'])
            assert response['ops'] == original['ops'] and response['pvi'] == original.get('pvi', {})
            assert response['ops'] != candidate['ops'], 'GUI revision should differ from original model delivery'
        assert cb['source_ids'] and cb['wb_id'] and cb['index']
        report = {'status': 'passed', 'visual_observation_source': 'root agent actual official Langflow GUI run',
                  'independent_readonly_db_verification': True, 'verifier_used_browser': False,
                  'flow_id': metadata['flow_id'], 'chain_id': context['chain_id'], 'job_id': context['job_id'],
                  'cb_id': cb['cb_id'], 'cut_id': cut['cut_id'], 'candidate_revision': candidate['revision'],
                  'typed_content_reason_match': True, 'persistent_content_match': True,
                  'volatile_next_wb_match': True, 'instant_current_views_match': True,
                  'original_delivery_matches_immutable_raw_provider_response': True,
                  'fixed_cb_ops_have_source_wb_mapping': True, 'objects_checked': objects,
                  'saved_typed_values': {key: saved[key] for key in ('content', 'reason', 'persistent_text', 'volatile_text', 'instant_views')},
                  'flow_rerun_by_verifier': False, 'core_http_requests': 0, 'model_calls': 0,
                  'langgraph_studio_gui_verified': False}
        if args.screenshot:
            assert args.screenshot.is_file()
            report['root_screenshot'] = {'path': str(args.screenshot), 'bytes': args.screenshot.stat().st_size,
                                         'sha256': hashlib.sha256(args.screenshot.read_bytes()).hexdigest()}
        destination = ROOT / '.local/integration-test-results/langflow-gui-publication.json'
        destination.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
        print(json.dumps({k: report[k] for k in ('status', 'independent_readonly_db_verification', 'candidate_revision', 'typed_content_reason_match', 'persistent_content_match', 'volatile_next_wb_match', 'instant_current_views_match', 'original_delivery_matches_immutable_raw_provider_response', 'flow_rerun_by_verifier', 'core_http_requests', 'model_calls', 'langgraph_studio_gui_verified')}))
    finally:
        db.rollback()
        db.close()


if __name__ == '__main__':
    main()
