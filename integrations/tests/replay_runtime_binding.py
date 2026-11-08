"""Replay official-runtime binding, or observe an author-provided DSH chain.

Default: a NEW owned fixture chain, LF publish -> LG read/revise/publish ->
LF read. --observe-chain performs no Core writes or model calls; it creates
only local official-runtime observation flows/threads. It never restarts a
service or touches existing GUI proposals.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import sys
import uuid
from urllib.error import HTTPError
from urllib.request import Request, build_opener, ProxyHandler

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT), str(ROOT / 'integrations')]
from generate_workbench_flows import candidate_steps, make_workbench_flow
from sic_workflow.bridge_client import session_bindings
from sic_workflow.runtime_auth import langflow_access_token
from sic_workflow.v2_client import CoreClient
from v2_runtime_acceptance import action_output


def digest(value):
    return hashlib.sha256(json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


class OfficialRuntimes:
    def __init__(self):
        self.lf = 'http://127.0.0.1:17861'
        self.lg = 'http://127.0.0.1:12026'
        self.opener = build_opener(ProxyHandler({}))
        self.lf_headers = {'Content-Type': 'application/json', 'Authorization': 'Bearer ' + langflow_access_token(self.lf, self.opener)}
        self.trace = []
        self.flows = []
        self.threads = []
        self.key = self.request(self.lf, '/api/v1/api_key/', {'name': 'sic-cross-runtime-replay'}, record=False)
        self.lf_headers['x-api-key'] = self.key['api_key']

    def request(self, base, path, body=None, method=None, record=True):
        headers = self.lf_headers if base == self.lf else {'Content-Type': 'application/json'}
        raw = None if body is None else json.dumps(body, ensure_ascii=False).encode()
        request = Request(base + path, raw, headers, method=method or ('GET' if body is None else 'POST'))
        try:
            with self.opener.open(request, timeout=180) as response:
                value = json.load(response)
        except HTTPError as error:
            try:
                value = json.load(error)
            except ValueError:
                value = {'error': 'non-JSON runtime error'}
            if record:
                self.trace.append({'base': base, 'path': path, 'http_status': error.code, 'response': value})
            raise RuntimeError(f'Official runtime HTTP {error.code}: {base}{path}; see private replay trace') from None
        if record:
            # Headers and login/API-key responses are deliberately excluded.
            self.trace.append({'base': base, 'path': path, 'response': value})
        return value

    def import_flow(self, flow):
        saved = self.request(self.lf, '/api/v1/flows/', flow)
        self.flows.append(saved['id'])
        assert self.request(self.lf, '/api/v1/flows/' + saved['id'])['data'] == flow['data']
        return saved['id']

    def lf_run(self, flow_id, output_node):
        raw = self.request(self.lf, '/api/v1/run/' + flow_id, {'input_request': {'input_value': '', 'input_type': 'any', 'output_type': 'any', 'output_component': output_node}})
        value = action_output(raw)
        assert value.get('ok'), value.get('error')
        return value

    def lg_run(self, graph_id, inputs, thread=None, resume=None):
        if thread is None:
            thread = self.request(self.lg, '/threads', {})['thread_id']
            self.threads.append(thread)
        body = {'assistant_id': graph_id, 'input': inputs} if resume is None else {'assistant_id': graph_id, 'command': {'resume': resume}}
        value = self.request(self.lg, '/threads/' + thread + '/runs/wait', body)
        return thread, value

    def close(self):
        self.request(self.lf, '/api/v1/api_key/' + self.key['id'], method='DELETE', record=False)


def core_call(core, operation, context, payload=None, write=False, model=False):
    value = core.execute(operation, context, payload or {}, allow_write=write, allow_model=model)
    assert value.get('ok'), value.get('error')
    return value


def source_and_job(core, context, marker):
    source = core_call(core, 'source.ingest', context, {'format': 'openai-chat', 'request': {'messages': [{'role': 'user', 'content': marker}]}}, write=True)
    task = core_call(core, 'edit.prepare', source['context'], {'source_id': source['context']['source_id']}, write=True)
    delivered = core_call(core, 'edit.run_all', task['context'], {'job_id': task['context']['job_id']}, write=True, model=True)
    return delivered['context']


def replay(core, runtime, nonce):
    roles = core.capabilities().get('adapters', {}).get('roles', {})
    assert roles and all(role.get('model') == 'openai/sic-offline-fixture' for role in roles.values()), 'Default replay requires the owned offline fixture; no real provider fallback'
    context = core_call(core, 'chain.create', {}, {'creators': []}, write=True)['context']
    context = source_and_job(core, context, nonce + ' LF source')
    chain = context['chain_id']
    steps = candidate_steps(context)
    steps[1]['values'].update(content=nonce + ' LF published edit', persistent_text=nonce + ' LF P', volatile_text='', instant_views=json.dumps({'operator_note': nonce + ' LF I'}))
    flow = make_workbench_flow('SIC cross-runtime replay ' + nonce, steps, connect=True)
    # Source export remains all-off; this explicit test copy alone is granted.
    for node in flow['data']['nodes']:
        if node['data']['type'] == 'SICCandidate':
            node['data']['node']['template']['allow_write']['value'] = True
    flow_id = runtime.import_flow(flow)
    runtime.lf_run(flow_id, 'SICAccounting-7')
    first = core_call(core, 'cut.get', context)['result']
    fixed_first = copy.deepcopy(first['view']['cbs'][0])
    assert nonce + ' LF published edit' in fixed_first['body'] and nonce + ' LF P' in fixed_first['body']
    _, read = runtime.lg_run('sic_memory', {'chain_id': chain, 'action': 'cb', 'object_id': fixed_first['cb_id'], 'limit': 8192})
    assert read['result']['ok'] and nonce + ' LF published edit' in read['page_text']
    context = source_and_job(core, {'chain_id': chain}, nonce + ' LG source')
    original = core_call(core, 'edit.candidate.get', context, {'job_id': context['job_id']})['result']['original_responses']
    form = {'action': 'revise', 'content': nonce + ' LG before review', 'persistent': [nonce + ' LG P'], 'volatile': [], 'instant': {'operator_note': nonce + ' LG I'}, 'expected_revision': 0, 'allow_write': True}
    thread, paused = runtime.lg_run('sic_candidate', {'chain_id': chain, 'job_id': context['job_id'], 'form': form})
    assert paused.get('__interrupt__')
    reviewed = dict(paused['__interrupt__'][0]['value']['form'], content=nonce + ' LG persisted resume edit')
    _, revised = runtime.lg_run('sic_candidate', None, thread, reviewed)
    assert revised['result']['ok'] and revised['after']['result']['original_responses'] == original
    for action in ('validate', 'publish'):
        _, paused = runtime.lg_run('sic_candidate', {'chain_id': chain, 'job_id': context['job_id'], 'form': {'action': action, 'expected_revision': 1, 'expected_cut': 1, 'allow_write': True}}, thread)
        assert paused.get('__interrupt__')
        _, applied = runtime.lg_run('sic_candidate', None, thread, paused['__interrupt__'][0]['value']['form'])
        assert applied['result']['ok'], applied['result'].get('error')
    final = core_call(core, 'cut.get', {'chain_id': chain})['result']
    assert len(final['view']['cbs']) == 2 and final['view']['cbs'][0] == fixed_first
    assert nonce + ' LG persisted resume edit' in final['view']['cbs'][1]['body']
    result = observe(core, runtime, chain, nonce)
    assert result['cb_count'] == 2
    result.update(mode='cross-runtime-fixture-replay', langflow_publish_seen_by_langgraph=True,
                  langgraph_publish_seen_by_langflow=True, same_chain_in_18888=True,
                  first_cb_unchanged_after_second_runtime_publish=True, original_editor_response_unchanged=True,
                  owned_fixture_model_calls=2, real_provider_calls=0)
    return result


def observe(core, runtime, chain, nonce, session_key=''):
    context = {'chain_id': chain}
    if session_key:
        binding = session_bindings(context, session_key)
        assert binding.get('ok') and binding['context']['chain_id'] == chain, 'Session binding must match the supplied authorized chain'
    initial = core_call(core, 'cut.get', context)['result']
    assert initial['view']['cbs'], 'No published CB on this chain; binding discovery alone is not published-memory acceptance'
    initial_ledger = core_call(core, 'ledger.export', context)['result']
    timeline = core_call(core, 'memory.timeline', context, {'offset': 0, 'limit': 100})['result']
    steps = [{'component': 'SICRoutes', 'values': {'chain_id': chain, 'action': '查看路线与历史点'}},
             {'component': 'SICMemory', 'values': {'chain_id': chain, 'action': 'CB形成时序', 'timeline_size': 100}},
             {'component': 'SICAccounting', 'values': {'chain_id': chain}}]
    flow = make_workbench_flow('SIC authorized observation ' + nonce, steps, connect=True)
    flow_id = runtime.import_flow(flow)
    lf_timeline = runtime.lf_run(flow_id, 'SICMemory-2')['result']
    assert lf_timeline == timeline, 'Core changed or Langflow points at another Core'
    thread, lg_timeline = runtime.lg_run('sic_memory', {'chain_id': chain, 'action': 'timeline', 'offset': 0, 'limit': 100})
    assert lg_timeline['result']['ok'] and lg_timeline['result']['result'] == timeline, 'Core changed or LangGraph points at another Core'
    lf_ledger = runtime.lf_run(flow_id, 'SICAccounting-3')['result']
    _, lg_ledger = runtime.lg_run('sic_accounting', {'chain_id': chain, 'action': 'ledger'})
    expected_ledger = core_call(core, 'ledger.export', context)['result']
    assert lf_ledger['calls'] == expected_ledger['calls'] == lg_ledger['result']['result']['calls']
    assert expected_ledger['calls'] == initial_ledger['calls'], 'Model/usage activity changed during observation; rerun after host work is quiescent'
    workspace_id = None
    if timeline['rows']:
        source_id = timeline['rows'][0]['source_ids'][0]
        workspace = make_workbench_flow('SIC authorized source/protocol observation ' + nonce,
            [{'component': 'SICWorkspace', 'values': {'chain_id': chain, 'source_id': source_id}}])
        workspace_id = runtime.import_flow(workspace)
        lf_workspace = runtime.lf_run(workspace_id, 'SICWorkspace-1')['result']
        expected_source = core_call(core, 'source.read', context, {'source_id': source_id})['result']['source']
        assert lf_workspace['source'] == expected_source
        assert lf_workspace['cut']['view']['cbs'] == initial['view']['cbs']
    # Observe every fixed CB and every op byte through Studio, not just row count.
    for cb in initial['view']['cbs']:
        for kind, expected in (('cb', cb['body']), ('ops', cb['ops'])):
            offset, pieces = 0, []
            while True:
                _, page = runtime.lg_run('sic_memory', {'chain_id': chain, 'action': kind, 'object_id': cb['cb_id'], 'offset': offset, 'limit': 8192}, thread)
                assert page['result']['ok'], page['result'].get('error')
                body = page['result']['result']
                pieces.append(body['value'])
                if body['next_offset'] is None:
                    break
                assert body['next_offset'] > offset
                offset = body['next_offset']
            assembled = ''.join(pieces)
            assert (json.loads(assembled) if kind == 'ops' else assembled) == expected
    final = core_call(core, 'cut.get', context)['result']
    assert final['cut_id'] == initial['cut_id'] and final['view']['cbs'] == initial['view']['cbs'], 'Observed chain changed during the read; repeat after host work is quiescent'
    assert core_call(core, 'ledger.export', context)['result']['calls'] == initial_ledger['calls']
    return {'mode': 'authorized-chain-readonly-observation', 'chain_id': chain, 'cut_id': initial['cut_id'],
            'cb_count': len(initial['view']['cbs']), 'cb_archive_sha256': digest(initial['view']['cbs']),
            'ledger_calls_sha256': digest(expected_ledger['calls']),
            'timeline_first_page_rows': len(timeline['rows']), 'timeline_total': timeline['total'],
            'ledger_call_count': len(expected_ledger['calls']), 'both_runtimes_match_core': True,
            'all_cb_bodies_and_ops_read_through_studio': True, 'explicit_core_write_actions_during_observation': 0, 'model_calls_during_observation': 0,
            'langflow_readonly_url': runtime.lf + '/flow/' + flow_id,
            'langflow_source_protocol_url': runtime.lf + '/flow/' + workspace_id if workspace_id else None,
            'studio_thread_id': thread, 'studio_url': 'https://smith.langchain.com/studio/?baseUrl=' + runtime.lg,
            'session_binding_checked': bool(session_key), 'gui_visual_verified': False}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--observe-chain', help='Author-provided real DSH chain; no Core writes/model calls')
    parser.add_argument('--session-key', default='', help='Optional exact authorized bridge binding to verify')
    parser.add_argument('--compare-report', type=Path, help='Prior private observation report; require same immutable chain facts after restart')
    args = parser.parse_args()
    assert not args.session_key or args.observe_chain, '--session-key requires --observe-chain'
    assert not args.compare_report or args.observe_chain, '--compare-report requires --observe-chain'
    nonce = 'replay-' + uuid.uuid4().hex[:12]
    output = ROOT / '.local/integration-test-results/runtime-replay' / nonce
    output.mkdir(parents=True, exist_ok=False)
    core, runtime = CoreClient('http://127.0.0.1:18888'), None
    try:
        runtime = OfficialRuntimes()
        result = observe(core, runtime, args.observe_chain, nonce, args.session_key) if args.observe_chain else replay(core, runtime, nonce)
        report = {'status': 'passed', 'bindings': {'core': core.base_url, 'langflow': runtime.lf, 'langgraph': runtime.lg}, **result}
        if args.compare_report:
            previous = json.loads(args.compare_report.read_text(encoding='utf-8'))
            assert previous.get('status') == 'passed', 'Baseline report must have passed'
            keys = ['chain_id', 'cut_id', 'cb_count', 'cb_archive_sha256', 'ledger_call_count']
            if 'ledger_calls_sha256' in previous:
                keys.append('ledger_calls_sha256')
            for key in keys:
                assert report[key] == previous[key], 'Persisted observation differs from baseline: ' + key
            report['baseline_comparison'] = {'status': 'passed', 'fields': keys, 'prior_report_sha256': hashlib.sha256(args.compare_report.read_bytes()).hexdigest()}
        (output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
        print(json.dumps({k: v for k, v in report.items() if k not in {'chain_id', 'cut_id', 'langflow_readonly_url', 'langflow_source_protocol_url', 'studio_thread_id'}}))
        print('Private report: ' + str((output / 'result.json').relative_to(ROOT)))
    except Exception as error:
        (output / 'failure.json').write_text(json.dumps({'status': 'failed', 'type': type(error).__name__, 'message': str(error), 'gui_visual_verified': False}, indent=2) + '\n', encoding='utf-8')
        raise
    finally:
        if runtime:
            (output / 'runtime-trace.json').write_text(json.dumps(runtime.trace, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
            runtime.close()


if __name__ == '__main__':
    main()
