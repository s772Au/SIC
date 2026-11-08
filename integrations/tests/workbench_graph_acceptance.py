"""Real Core HTTP + official LangGraph persistent checkpoint acceptance."""
import copy
import json
import os
from pathlib import Path
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT), str(ROOT / 'integrations'), str(ROOT / 'scripts')]
from langgraph.checkpoint.memory import InMemorySaver
from langgraph.types import Command
from sic_workflow.workbench_graph import builder, memory_graph
from sic_workflow import domain_graphs as domains
from sic_workflow.accounting_graph import graph as accounting_graph
from v2_experiment_fixture import OwnedFixture


def main():
    saved = {k: os.environ.get(k) for k in ('SIC_CORE_URL', 'SIC_CORE_TOKEN', 'SIC_CORE_ACTOR')}
    try:
        with tempfile.TemporaryDirectory(prefix='sic-studio-') as directory, OwnedFixture(Path(directory) / 'core.db') as fixture:
            os.environ.update(SIC_CORE_URL=fixture.client.origin, SIC_CORE_TOKEN=fixture.client.token, SIC_CORE_ACTOR='host')
            client = fixture.client
            client.call('chain.create', {'creators': []})
            chain = client.context['chain_id']
            source = client.call('source.ingest', {'format': 'openai-chat', 'request': {'messages': [{'role': 'user', 'content': 'Actual evidence for Studio candidate review'}]}})['source_id']
            job = client.call('edit.prepare', {'source_id': source})['job_id']
            client.call('edit.run_all', {'job_id': job})
            original = copy.deepcopy(client.call('edit.candidate.get', {'job_id': job})['original_responses'])
            graph = builder.compile(checkpointer=InMemorySaver())
            form = {'action': 'revise', 'content': 'Studio reviewed evidence', 'reason': 'Verified source', 'persistent': ['Studio P'], 'volatile': ['Studio V'], 'instant': {'operator_note': 'Studio I'}, 'allow_write': True}
            config = {'configurable': {'thread_id': 'owned-candidate-acceptance'}}
            paused = graph.invoke({'chain_id': chain, 'job_id': job, 'form': form}, config)
            assert paused.get('__interrupt__'), paused
            changed = dict(paused['__interrupt__'][0].value['form'], content='Edited at persistent interrupt')
            result = graph.invoke(Command(resume=changed), config)
            assert result['result']['ok'], result
            assert result['after']['result']['revision'] == 1
            assert result['after']['result']['original_responses'] == original
            assert result['after']['result']['ops'][0]['content'] == changed['content']
            assert graph.get_state(config).values['after']['result']['revision'] == 1
            for action in ('validate', 'publish'):
                value = graph.invoke({'chain_id': chain, 'job_id': job, 'form': {'action': action, 'expected_revision': 1, 'allow_write': True}}, config)
                assert value.get('__interrupt__')
                value = graph.invoke(Command(resume=value['__interrupt__'][0].value['form']), config)
                assert value['result']['ok'], value
            fixed = copy.deepcopy(client.call('cut.get')['view']['cbs'][0])
            assert changed['content'] in fixed['body'] and 'Studio P' in fixed['body']
            for action, obj in (('timeline', ''), ('source_mapping', source), ('cb_mapping', fixed['cb_id']), ('ops', fixed['cb_id']), ('proposal', fixed['cb_id']), ('decision', ''), ('ledger', '')):
                value = memory_graph.invoke({'chain_id': chain, 'action': action, 'object_id': obj})
                assert value['result']['ok'], value
            assert client.call('cut.get')['view']['cbs'][0] == fixed
            def domain_action(name, model, loader, request, form):
                g = domains.make_graph(name, model, loader, request, InMemorySaver())
                cfg = {'configurable': {'thread_id': name}}
                answer = g.invoke({'chain_id': chain, 'form': form}, cfg)
                if answer.get('__interrupt__'):
                    answer = g.invoke(Command(resume=answer['__interrupt__'][0].value['form']), cfg)
                assert answer['result']['ok'], answer['result'].get('error')
                return answer['result']['result']
            domain_action('decision', domains.DecisionForm, 'decision.get', domains.decision_request,
                          {'action': 'failure', 'text': 'Alternative failed on owned evidence', 'evidence_source': source, 'allow_write': True})
            domain_action('configuration', domains.ConfigurationForm, 'config.get', domains.configuration_request,
                          {'action': 'update', 'deduplicate_exact': True, 'drop_empty': True, 'normalize_newlines': True, 'allow_write': True})
            domain_action('runtime', domains.RuntimeForm, 'runtime.get', domains.runtime_request,
                          {'action': 'configure', 'enabled': False, 'allow_write': True})
            checkpoint = domain_action('checkpoint', domains.RouteForm, 'chain.get', domains.route_request,
                                       {'action': 'checkpoint', 'allow_write': True})['checkpoint_id']
            socket = domain_action('socket', domains.RouteForm, 'chain.get', domains.route_request,
                                   {'action': 'socket', 'checkpoint_id': checkpoint, 'message_acl': ['host'], 'signal_acl': ['host'], 'spawn_acl': ['host'], 'allow_write': True})['socket_id']
            domain_action('dispatch', domains.RouteForm, 'chain.get', domains.route_request,
                          {'action': 'dispatch', 'socket_ids': [socket], 'question': 'Inspect source evidence and return a useful result', 'request_id': 'owned-studio-peer', 'allow_write': True})
            peer = domain_action('peer_work', domains.RouteForm, 'chain.get', domains.route_request,
                                 {'action': 'run_peer_work', 'request_id': 'owned-studio-peer', 'editor_mode': 'model', 'allow_write': True, 'allow_model': True})
            assert peer, peer
            domain_action('board', domains.RouteForm, 'chain.get', domains.route_request,
                          {'action': 'claim', 'task_id': 'owned-studio-task', 'description': 'Inspect actual peer result', 'allow_write': True})
            ledger = accounting_graph.invoke({'chain_id': chain, 'action': 'ledger'})
            assert ledger['result']['ok'] and ledger['charge_rows'] and ledger['unknown']
            # Program-close pending return/V materials before a new explicit WB.
            for _ in range(4):
                if not client.call('cut.get')['view']['pending']:
                    break
                client.call('runtime.run', {'max_steps': 8, 'continue_worker': False, 'editor_mode': 'program'})
            assert not client.call('cut.get')['view']['pending']
            fresh = domain_action('source', domains.SourceForm, 'chain.get', domains.source_request,
                                  {'action': 'ingest', 'working_text': 'New Studio editor scope', 'allow_write': True})['source_id']
            task = domain_action('editor_prepare', domains.EditorForm, 'chain.get', domains.editor_request,
                                 {'action': 'prepare', 'source_id': fresh, 'readonly_reads': [{'kind': 'cb', 'object_id': fixed['cb_id'], 'offset': 0, 'limit': 128}], 'allow_write': True})['job_id']
            domain_action('editor_run', domains.EditorForm, 'chain.get', domains.editor_request,
                          {'action': 'run_all', 'job_id': task, 'allow_write': True, 'allow_model': True})
            report = {'status': 'passed', 'candidate_interrupt_resume_effective': True, 'immutable_original_and_cb': True,
                      'navigation_directions': 7, 'owned_model_calls': len(fixture.model_requests), 'real_provider_calls': 0,
                      'typed_domain_transactions': 11, 'typed_accounting_ledger': True,
                      'official_langgraph_sdk': True, 'studio_browser_gui_verified': False}
            dest = ROOT / '.local/integration-test-results/workbench-graph-acceptance.json'
            dest.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
            print(json.dumps(report))
    finally:
        for key, value in saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


if __name__ == '__main__':
    main()
