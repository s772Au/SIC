"""Loopback Responses entry for the scoped Codex projection."""

from contextlib import contextmanager
from hmac import compare_digest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from threading import Lock, Thread
from uuid import uuid4

from .sic_responses import plain_user


def emit_local_response(handler, frame, text, *, calls=(), input_tokens=0):
    """Emit native SSE for host-owned checkpoints and offline provider fixtures."""
    identity = uuid4().hex
    message = {'type': 'message', 'id': 'msg_' + identity, 'role': 'assistant', 'status': 'completed',
               'content': [{'type': 'output_text', 'text': text, 'annotations': []}]}
    response = {'id': 'resp_' + identity, 'object': 'response', 'model': frame['model'], 'status': 'in_progress',
                'output': []}
    handler.send_response(200)
    handler.send_header('Content-Type', 'text/event-stream')
    handler.end_headers()

    def event(kind, **payload):
        handler.wfile.write(('event: ' + kind + '\ndata: ' + json.dumps({'type': kind, **payload}) + '\n\n').encode())

    event('response.created', response=response)
    event('response.output_item.added', output_index=0, item={**message, 'content': [], 'status': 'in_progress'})
    event('response.output_text.delta', output_index=0, content_index=0, item_id=message['id'], delta=text)
    event('response.output_item.done', output_index=0, item=message)
    output = [message]
    for i, (call_id, namespace, name, arguments) in enumerate(calls, 1):
        call = {'type': 'function_call', 'id': 'fc_' + call_id, 'call_id': call_id, 'name': name,
                'namespace': namespace, 'arguments': json.dumps(arguments), 'status': 'completed'}
        event('response.output_item.added', output_index=i, item={**call, 'arguments': '', 'status': 'in_progress'})
        event('response.function_call_arguments.delta', output_index=i, item_id=call['id'], delta=call['arguments'])
        event('response.function_call_arguments.done', output_index=i, item_id=call['id'], arguments=call['arguments'])
        event('response.output_item.done', output_index=i, item=call)
        output.append(call)
    event('response.completed', response={**response, 'status': 'completed', 'output': output, 'usage': {
        'input_tokens': input_tokens, 'output_tokens': 0, 'total_tokens': input_tokens,
        'input_tokens_details': {'cached_tokens': 0}, 'output_tokens_details': {'reasoning_tokens': 0}}})


@contextmanager
def listen(gateway_key, instruction, worker, checkpoint, on_error, *, port=0, bind=None, drain=False):
    """`worker` streams the original provider response. `checkpoint` uses native SSE."""
    state = {'projection': None}
    gate = Lock()

    class Handler(BaseHTTPRequestHandler):
        sent = False

        def log_message(self, *_):
            pass

        def end_headers(self):
            self.sent = True
            super().end_headers()

        def do_POST(self):
            if self.path != '/v1/responses':
                self.send_error(404)
                return
            supplied = self.headers.get_all('Authorization', [])
            if len(supplied) != 1 or not compare_digest(supplied[0].encode(), ('Bearer ' + gateway_key).encode()):
                self.send_error(401)
                return
            try:
                length = int(self.headers['Content-Length'])
                if not 0 < length <= 64 * 1024 * 1024:
                    self.send_error(413)
                    return
                frame = json.loads(self.rfile.read(length))
                thread_id = self.headers.get('thread-id')
                with gate:
                    projection = state['projection']
                    if projection is None and bind is not None:
                        projection = state['projection'] = bind(thread_id, frame)
                    if projection is None:
                        projected = frame
                    else:
                        items = projection.check_request(thread_id, frame)
                        if items and plain_user(items[-1]) == instruction:
                            text = projection.compact(thread_id, frame, instruction)
                            checkpoint(self, frame, text)
                            return
                        projected = projection.worker(thread_id, frame)
                worker(self, frame, projected)
            except Exception as error:
                status = on_error(error)
                if status and not self.sent:
                    self.send_error(status)

    class Gateway(ThreadingHTTPServer):
        allow_reuse_address = True
        daemon_threads = not drain

    with Gateway(('127.0.0.1', port), Handler) as server:
        url = f'http://127.0.0.1:{server.server_port}/v1'
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield url, state
        finally:
            server.shutdown()
            thread.join(timeout=5)
