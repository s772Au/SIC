"""Isolated Core and Codex App Server process entry points for Codex SIC."""

from contextlib import contextmanager
import json
import os
from queue import Queue
import subprocess
from threading import Thread
from time import monotonic
from urllib.request import Request, ProxyHandler, build_opener


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


@contextmanager
def core(binary, directory, token, archive_token=None, chain=None, address='127.0.0.1:0'):
    database = directory / 'core.db'
    actor = 'codex-sic-lab'
    env = {**os.environ, 'SIC_LAB_CORE_TOKEN': token}
    if not database.exists():
        subprocess.run([binary, 'init', database], check=True, timeout=25, env=env, capture_output=True)
    args = [binary, 'serve', database, address, actor, 'SIC_LAB_CORE_TOKEN']
    if archive_token is not None:
        if chain is None:
            raise ValueError('The archive token requires an owned Core chain')
        grants = directory / 'archive-grants.json'
        grants.write_text(json.dumps([{'chain_id': chain, 'token_env': 'SIC_LAB_ARCHIVE_TOKEN'}]), encoding='utf-8')
        env['SIC_LAB_ARCHIVE_TOKEN'] = archive_token
        args += ['--archive-grants', grants]
    with (directory / 'core.stderr.log').open('ab') as stderr:
        proc = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=stderr, text=True, encoding='utf-8', env=env)
        try:
            lines = Queue()
            Thread(target=lambda: lines.put(proc.stdout.readline()), daemon=True).start()
            ready = lines.get(timeout=20)
            endpoint = 'http://' + ready.strip().split('listening on ', 1)[1]
            http = build_opener(ProxyHandler({}))

            def operate(operation, payload, chain_id=chain):
                body = {'operation': operation, 'payload': payload, 'context': {
                    'actor': actor, 'chain_id': chain_id, 'allow_write': True, 'allow_model': False, 'allow_revoke': False,
                }}
                req = Request(endpoint + '/api/v2/operations', data=json.dumps(body, ensure_ascii=False).encode(),
                              headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
                with http.open(req, timeout=50) as response:
                    return json.load(response)['result']

            yield endpoint, operate
        finally:
            stop(proc)
            proc.stdout.close()


class AppServer:
    def __init__(self, process, events):
        self.process, self.events, self.sequence = process, events, 0

    def until(self, predicate, timeout=60):
        deadline = monotonic() + timeout
        while True:
            item = self.events.get(timeout=max(0, deadline - monotonic()))
            if item is None:
                raise RuntimeError('Codex App Server closed before its receipt')
            if predicate(item):
                return item

    def rpc(self, method, params, notification=False):
        frame = {'jsonrpc': '2.0', 'method': method, 'params': params}
        if not notification:
            self.sequence += 1
            frame['id'] = self.sequence
        self.process.stdin.write(json.dumps(frame, ensure_ascii=False) + '\n')
        self.process.stdin.flush()
        if not notification:
            response = self.until(lambda item: item.get('id') == frame['id'])
            if 'error' in response:
                raise RuntimeError(f'Codex {method} failed: {response["error"]}')
            return response['result']

    def turn(self, session, text, timeout=60, on_start=None):
        start = self.rpc('turn/start', {'threadId': session, 'input': [
            {'type': 'text', 'text': text, 'text_elements': []},
        ]})
        turn_id = start['turn']['id']
        if on_start:
            on_start(turn_id)
        return self.finished(turn_id, timeout)

    def finished(self, turn_id=None, timeout=60):
        completed_items = []

        def finished(item):
            params = item.get('params') or {}
            if item.get('method') == 'item/completed' and params.get('turnId') == turn_id:
                completed_items.append(params['item'])
            return item.get('method') == 'turn/completed' and (turn_id is None or params['turn']['id'] == turn_id)

        event = self.until(finished, timeout)
        turn = event['params']['turn']
        if turn['status'] != 'completed':
            raise RuntimeError(f'Codex turn failed: {turn.get("error")}')
        return {**turn, 'items': turn.get('items') or completed_items}


@contextmanager
def codex(binary, directory, configuration, variables, *, overrides=()):
    config = directory / 'codex'
    config.mkdir(exist_ok=True)
    path = config / 'config.toml'
    if path.exists() and path.read_text(encoding='utf-8') != configuration:
        raise ValueError('Codex configuration changed across restart')
    path.write_text(configuration, encoding='utf-8')
    environment = {name: value for name, value in os.environ.items()
                   if not name.startswith(('CODEX_', 'OPENAI_')) and not name.lower().endswith('_proxy')}
    environment.update({'CODEX_HOME': str(config), 'NO_PROXY': '127.0.0.1,localhost', **variables})
    queue = Queue()
    with (directory / 'codex.stderr.log').open('ab') as stderr:
        flags = [word for value in overrides for word in ('-c', value)]
        proc = subprocess.Popen([binary, *flags, 'app-server', '--listen', 'stdio://'], env=environment, cwd=directory,
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr, text=True, encoding='utf-8')

        def read():
            try:
                for line in proc.stdout:
                    queue.put(json.loads(line))
            finally:
                queue.put(None)

        reader = Thread(target=read, daemon=True)
        reader.start()
        try:
            app = AppServer(proc, queue)
            app.rpc('initialize', {'clientInfo': {'name': 'sic-codex-lab', 'version': '0.1.0'}})
            app.rpc('initialized', {}, notification=True)
            yield app
        finally:
            stop(proc)
            reader.join(timeout=3)
            if not reader.is_alive():
                proc.stdout.close()
            proc.stdin.close()
