"""Exercise a real Langflow node -> Rust -> own loopback mock. No provider."""
import asyncio
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from sic_workflow.client import SicClient


def main():
    observed = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            observed.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
            edits = {"edits": [{"target_id": "wb", "op": "COMPRESS", "risk": "low", "content": "mock-reviewed text"}]}
            result = {"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "tool_calls": [
                {"id": "offline-call", "type": "function", "function": {"name": "sic_edit_wb", "arguments": json.dumps(edits)}}]}}]}
            body = json.dumps(result).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    mock = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=mock.serve_forever, daemon=True).start()
    repository = ROOT.parent
    executable = repository / '.local/sic-integration-service.exe'
    if not executable.exists():
        executable = repository / 'target/debug/sic-proxy.exe'
    admin = uuid.uuid4().hex
    provider_key = uuid.uuid4().hex
    previous_environment = {k: os.environ.get(k) for k in ('SIC_API_URL', 'SIC_ADMIN_TOKEN')}
    with tempfile.TemporaryDirectory(dir=repository / '.local', prefix='single-pass-mock-') as directory:
        relative = Path(directory).relative_to(repository)
        env = os.environ.copy()
        for key in list(env):
            if key.startswith(('SIC_COMPRESSER_', 'SIC_WORKER_')):
                env.pop(key)
        env.update({'SIC_BIND': '127.0.0.1:18788', 'SIC_ADMIN_TOKEN': admin,
                    'SIC_DB': f"sqlite://{relative.as_posix()}/store.db?mode=rwc",
                    'SIC_COMPRESSER_URL': f'http://127.0.0.1:{mock.server_port}/v1/chat/completions',
                    'SIC_COMPRESSER_MODEL': 'own-loopback-mock', 'SIC_COMPRESSER_API_KEY': provider_key})
        process = subprocess.Popen([str(executable)], cwd=repository, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            os.environ.update({'SIC_API_URL': 'http://127.0.0.1:18788', 'SIC_ADMIN_TOKEN': admin})
            client = SicClient()
            for _ in range(100):
                try:
                    client.execute('bootstrap')
                    break
                except OSError:
                    time.sleep(0.1)
            context = client.execute('session_create', payload={'name': 'own mock only'}, allow_write=True)['context']
            context = client.execute('wb_create', context, {'source': 'Complete mock WB'}, allow_write=True)['context']
            context = client.execute('draft_create', context, allow_write=True)['context']
            from generate_flows import make_flow
            from lfx.load import load_flow_from_json
            flow = make_flow('Own mock single-pass', [('Explicit single pass', 'draft_single_pass', {}, context)])
            template = flow['data']['nodes'][0]['data']['node']['template']
            template['allow_write']['value'] = True
            template['allow_model']['value'] = True
            graph = load_flow_from_json(flow)
            outputs = asyncio.run(graph.arun(inputs=[{}], outputs=['SICOperation-1']))
            node_output = outputs[0].outputs[0]
            node_result = node_output.outputs['result']
            envelope = node_result.get('message', node_result.get('data', node_result)) if isinstance(node_result, dict) else node_result.data
            assert envelope['context']['job_id']
            assert envelope['context']['draft_revision'] == 1
            assert envelope['result']['model_requests'] == 1
            assert envelope['result']['committed'] is False
            assert len(observed) == 1
            assert observed[0]['tools'][0]['function']['name'] == 'sic_edit_wb'
            job = client.execute('edit_job', envelope['context'])['result']
            assert job['status'] == 'applied'
            assert client.execute('draft_read', context)['result']['status'] == 'open'
            exported = json.dumps(flow)
            assert admin not in exported and provider_key not in exported
            report = {'status': 'passed', 'model_calls_to_own_loopback_mock': 1, 'real_provider_calls': 0,
                      'draft_revision': envelope['context']['draft_revision'], 'job_id_returned': True,
                      'job_status': job['status'], 'auto_committed': False, 'credentials_in_export': False}
            destination = ROOT.parent / '.local/integration-test-results'
            destination.mkdir(parents=True, exist_ok=True)
            (destination / 'single-pass-mock-result.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
            print(json.dumps(report))
        finally:
            process.terminate()
            process.wait(timeout=10)
            for key, value in previous_environment.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value
    mock.shutdown()
    mock.server_close()


if __name__ == '__main__':
    main()
