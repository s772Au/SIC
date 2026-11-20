"""Verify the native Codex launcher, scoped Core grants and real App Server without paid requests."""

import argparse
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
from queue import Queue
import secrets
import subprocess
import sys
import tempfile
from threading import Thread

from pi_tool_wait_acceptance import core_service

INTEGRATIONS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(INTEGRATIONS))
from codex.loopback import emit_local_response
from codex.processes import AppServer, stop
from codex.sic_responses import plain_user


@contextmanager
def app(cli, root, endpoint, core, chain, grant, key):
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(('CODEX_', 'OPENAI_')) and not k.lower().endswith('_proxy')}
    env.update(SIC_CODEX_CORE_GRANT=grant, SIC_CODEX_PROVIDER_KEY=key)
    command = [sys.executable, '-m', 'codex.native_launch', '--codex', cli, '--core', core,
               '--provider', endpoint, '--model', 'sic-fixture-model', '--state', root, '--chain', chain,
               '--batch', '2', '--auto-threshold', '8000', '--', '-c', 'approval_policy="never"',
               '-c', 'sandbox_mode="read-only"', 'app-server', '--listen', 'stdio://']
    proc = subprocess.Popen(command, cwd=INTEGRATIONS, env=env, stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, text=True, encoding='utf-8')
    queue = Queue()

    def reader():
        try:
            for line in proc.stdout:
                queue.put(json.loads(line))
        finally:
            queue.put(None)

    pump = Thread(target=reader, daemon=True)
    pump.start()
    try:
        server = AppServer(proc, queue)
        server.rpc('initialize', {'clientInfo': {'name': 'sic-native-acceptance', 'version': '0.1.0'}})
        server.rpc('initialized', {}, notification=True)
        yield server
    finally:
        proc.stdin.close()
        try:
            proc.wait(timeout=8)
        except subprocess.TimeoutExpired:
            stop(proc)
        pump.join(timeout=5)
        if not pump.is_alive():
            proc.stdout.close()


def exercise(cli, binary, root):
    grant, key = (secrets.token_urlsafe(24) for _ in range(2))
    original = 'NATIVE_USER_WITNESS Repeated questions retain separate native occurrences. ' * 22
    trigger = 'TRIGGER_NEXT_AUTOMATIC_COMPACTION ' + original
    assistant = 'NATIVE_ASSISTANT_WITNESS Done with the previous exact request.'
    tool_witness = 'NATIVE_CODEX_TOOL_WITNESS'
    tail = 'Uncovered Worker question after the completed cut.'
    worker_inputs, edits = [], []

    class Provider(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            assert self.headers.get('Authorization') == 'Bearer ' + key
            assert self.path == '/v1/responses'
            frame = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            if frame.get('instructions', '').startswith('SIC Editor.'):
                units = json.loads(plain_user(frame['input'][-1]))['units']
                source = '\n'.join(u['text'] for u in units)
                assert '<environment_context>' not in source
                if 'NATIVE_USER_WITNESS' in source:
                    assert 'NATIVE_ASSISTANT_WITNESS' in source
                if not edits:
                    assert tool_witness in source
                edits.append(source)
                body = json.dumps({'status': 'completed', 'output': [{'type': 'message', 'content': [
                    {'type': 'output_text', 'text': f'SIC retained evidence {len(edits)}.'}]}]}).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(body)
                return
            items = frame['input']
            worker_inputs.append(items)
            latest = next((text for item in reversed(items) if (text := plain_user(item))), '')
            if len(worker_inputs) == 1:
                tools = list(frame.get('tools', []))
                tools.extend(tool for item in items if item.get('type') == 'additional_tools' for tool in item.get('tools', []))
                names = [(None, tool['name']) for tool in tools if tool.get('name') == 'exec_command']
                names.extend((group['name'], tool['name']) for group in tools for tool in group.get('tools', [])
                             if tool.get('name') == 'exec_command')
                [(namespace, name)] = names
                command = f'Write-Output {tool_witness}' if os.name == 'nt' else f'printf %s {tool_witness}'
                emit_local_response(self, frame, 'Checking the native tool path.', input_tokens=120,
                                    calls=[('sic_native_exec_1', namespace, name, {'cmd': command})])
                return
            if len(worker_inputs) == 2:
                output = next(item for item in items if item.get('call_id') == 'sic_native_exec_1'
                              and item.get('type') == 'function_call_output')
                assert tool_witness in json.dumps(output)
            emit_local_response(self, frame, assistant, input_tokens=12000 if latest == trigger else 120)

    with core_service(binary, root) as core:
        chain = core('chain.create', {})['chain_id']
    project = root / 'project'
    project.mkdir()
    opts = {'cwd': str(project), 'model': 'sic-fixture-model', 'modelProvider': 'sic_local'}
    with ThreadingHTTPServer(('127.0.0.1', 0), Provider) as provider:
        thread = Thread(target=provider.serve_forever, daemon=True)
        thread.start()
        endpoint = f'http://127.0.0.1:{provider.server_port}/v1'
        try:
            with core_service(binary, root, [(chain, grant)]) as core:
                with app(cli, root, endpoint, core.endpoint, chain, grant, key) as native:
                    sid = native.rpc('thread/start', opts)['thread']['id']
                    for _ in range(4):
                        native.turn(sid, original)
                        native.rpc('thread/compact/start', {'threadId': sid})
                        native.finished()
                    native.turn(sid, trigger)
                    native.turn(sid, tail)
                    assert len(edits) == 8 and any(plain_user(item) == tail for item in worker_inputs[-1])
                    selected = core('cut.get', {}, chain)
                    assert [cb['level'] for cb in selected['view']['cbs']] == [3, 1]
            with core_service(binary, root, [(chain, grant)]) as core:
                with app(cli, root, endpoint, core.endpoint, chain, grant, key) as native:
                    native.rpc('thread/resume', {**opts, 'threadId': sid})
                    native.turn(sid, 'Continue on the cold restarted session.')
                    projected = [plain_user(item) for item in worker_inputs[-1]]
                    assert original not in projected and trigger not in projected and tail in projected
                    assert all(cb['body'] in projected for cb in selected['view']['cbs'])
                    assert core('cut.get', {}, chain)['cut_id'] == selected['cut_id'] and len(edits) == 8
        finally:
            provider.shutdown()
            thread.join(timeout=5)
    print(json.dumps({'result': 'PASS', 'native_automatic_and_manual': True, 'levels': [3, 1],
                      'native_duplicate_prompt_occurrences': 4, 'original_tool_result_visible_to_editor': True,
                      'scoped_core': True,
                      'cold_restart': True, 'paid_requests': 0}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('codex', type=Path)
    parser.add_argument('core', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sic-codex-launcher-', ignore_cleanup_errors=True) as directory:
        exercise(args.codex.resolve(), args.core.resolve(), Path(directory))
