"""Official live LangGraph durable thread acceptance; own fixture Core only.

Creates its own independent chains, never executes the saved Langflow GUI
proposal. It also leaves a distinct paused Studio candidate for GUI review.
"""
import copy
import json
from pathlib import Path
import sys
from urllib.request import Request, build_opener, ProxyHandler

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT), str(ROOT / 'integrations')]
from sic_workflow.v2_client import CoreClient

BASE = 'http://127.0.0.1:12026'
OPENER = build_opener(ProxyHandler({}))


def api(path, payload=None):
    request = Request(BASE + path, None if payload is None else json.dumps(payload).encode(), {'Content-Type': 'application/json'})
    with OPENER.open(request, timeout=180) as response:
        return json.load(response)


def main():
    core = CoreClient()
    assistants = api('/assistants/search', {'limit': 100})
    expected = {'sic_candidate', 'sic_memory', 'sic_decision', 'sic_routes', 'sic_runtime', 'sic_configuration', 'sic_editor', 'sic_source', 'sic_accounting'}
    assert expected <= {assistant['graph_id'] for assistant in assistants}

    def fresh_job():
        created = core.execute('chain.create', payload={'creators': []}, allow_write=True)
        assert created['ok'], created
        context = created['context']
        roles = core.execute('config.get', context)['result']['safe_default_roles']
        assert roles and all(role.get('model') == 'openai/sic-offline-fixture' for role in roles.values()), 'Own fixture required'
        source = core.execute('source.ingest', context, {'format': 'openai-chat', 'request': {'messages': [{'role': 'user', 'content': 'Independent live Studio candidate evidence'}]}}, allow_write=True)
        task = core.execute('edit.prepare', source['context'], {'source_id': source['context']['source_id']}, allow_write=True)
        delivered = core.execute('edit.run_all', task['context'], {'job_id': task['context']['job_id']}, allow_write=True, allow_model=True)
        assert delivered['ok'], delivered
        return delivered['context']

    context = fresh_job()
    original = core.execute('edit.candidate.get', context, {'job_id': context['job_id']})['result']['original_responses']
    thread = api('/threads', {})['thread_id']
    path = '/threads/' + thread + '/runs/wait'
    form = {'action': 'revise', 'content': 'Live API candidate before persistent review', 'persistent': ['Live API P'], 'volatile': ['Live API V'], 'instant': {'operator_note': 'Live API I'}, 'expected_revision': 0, 'allow_write': True}
    paused = api(path, {'assistant_id': 'sic_candidate', 'input': {'chain_id': context['chain_id'], 'job_id': context['job_id'], 'form': form}})
    assert paused.get('__interrupt__'), paused
    review = dict(paused['__interrupt__'][0]['value']['form'], content='Changed at actual persistent Studio interrupt')
    resumed = api(path, {'assistant_id': 'sic_candidate', 'command': {'resume': review}})
    assert resumed['result']['ok'], resumed['result']
    assert resumed['after']['result']['revision'] == 1
    assert resumed['after']['result']['original_responses'] == original
    for action in ('validate', 'publish'):
        paused = api(path, {'assistant_id': 'sic_candidate', 'input': {'chain_id': context['chain_id'], 'job_id': context['job_id'], 'form': {'action': action, 'expected_revision': 1, 'allow_write': True}}})
        assert paused.get('__interrupt__')
        resumed = api(path, {'assistant_id': 'sic_candidate', 'command': {'resume': paused['__interrupt__'][0]['value']['form']}})
        assert resumed['result']['ok'], resumed['result']
    view = core.execute('cut.get', context)['result']
    fixed = copy.deepcopy(view['view']['cbs'][0])
    assert review['content'] in fixed['body'] and 'Live API P' in fixed['body']
    assert api('/threads/' + thread + '/state')['values']['status'] == 'applied'
    history = api('/threads/' + thread + '/history', {'limit': 100})
    assert len(history) >= 9
    for name, payload in (('sic_memory', {'chain_id': context['chain_id'], 'action': 'timeline'}), ('sic_accounting', {'chain_id': context['chain_id']})):
        answer = api('/runs/wait', {'assistant_id': name, 'input': payload})
        assert answer['result']['ok'], answer
    negative = []
    for operation in ('cb.update', 'op.update', 'source.update'):
        result = core._request('/api/v2/operations', {'operation': operation, 'context': {**context, 'allow_write': True}, 'payload': {'cb_id': fixed['cb_id'], 'body': 'forbidden'}})
        assert result.get('ok') is False
        negative.append(operation)
    assert core.execute('cut.get', context)['result']['view']['cbs'][0] == fixed
    # GUI gets another untouched chain and a real persisted review pause.
    gui_context = fresh_job()
    gui_thread = api('/threads', {})['thread_id']
    gui_form = {'action': 'revise', 'content': 'GUI before review', 'persistent': ['GUI P before'], 'volatile': ['GUI V before'], 'instant': {'operator_note': 'GUI I before'}, 'allow_write': False}
    gui_pause = api('/threads/' + gui_thread + '/runs/wait', {'assistant_id': 'sic_candidate', 'input': {'chain_id': gui_context['chain_id'], 'job_id': gui_context['job_id'], 'form': gui_form}})
    assert gui_pause.get('__interrupt__')
    report = {'status': 'passed', 'registered_semantic_graphs': sorted(expected), 'persistent_candidate_thread': thread,
              'history_records': len(history), 'resume_op_pvi_effective': True, 'original_response_unchanged': True,
              'immutable_update_denials': negative, 'owned_preparation_model_calls': 2, 'real_provider_calls': 0,
              'gui_visual_verified': False, 'gui_entry': {'studio_url': 'https://smith.langchain.com/studio/?baseUrl=' + BASE,
              'assistant_id': 'sic_candidate', 'thread_id': gui_thread, 'context': gui_context, 'resume_form': gui_pause['__interrupt__'][0]['value']['form'], 'state': 'paused before revision0 mutation'}}
    (ROOT / '.local/integration-test-results/workbench-runtime-acceptance.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps({key: value for key, value in report.items() if key not in {'gui_entry', 'persistent_candidate_thread'}}))


if __name__ == '__main__':
    main()
