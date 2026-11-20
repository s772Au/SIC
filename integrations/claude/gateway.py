"""Single-session loopback entry for an Anthropic Messages provider."""

from contextlib import contextmanager
from hmac import compare_digest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from threading import Thread
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener
from uuid import uuid4


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def endpoint(url, loopback_only=False):
    parts = urlsplit(url)
    local = parts.scheme == "http" and parts.hostname == "127.0.0.1" and parts.port
    if (parts.username or parts.password or parts.query or parts.fragment
            or (loopback_only and (not local or parts.path not in ("", "/")))
            or (not local and (parts.scheme != "https" or not parts.hostname))):
        raise ValueError("Expected a dedicated Core loopback URL or a fixed HTTPS provider URL")
    return url.rstrip("/")


def open_request(url, body, headers, timeout):
    opener = build_opener(ProxyHandler({}), NoRedirect())
    data = json.dumps(body, ensure_ascii=False).encode() if body is not None else None
    return opener.open(Request(url, data=data, headers=headers), timeout=timeout)


def core_operator(base, grant):
    url = endpoint(base, loopback_only=True) + "/api/v2/harness/operations"

    def operate(operation, payload):
        body = {"operation": operation, "payload": payload}
        headers = {"Authorization": "Bearer " + grant, "Content-Type": "application/json"}
        with open_request(url, body, headers, 20) as answer:
            return json.load(answer)

    return operate


def provider_headers(native, key):
    allowed = {"user-agent", "x-app"}
    return {"X-Api-Key": key, "Content-Type": "application/json",
            **{name: value for name, value in native.items()
               if name.lower() in allowed or name.lower().startswith(("anthropic-", "x-claude-code-"))}}


def editor_call(url, key, native_headers, frame):
    # Keep the native model and Messages encoding. The Editor only receives the archive and its assignment.
    request = {name: frame[name] for name in ("model", "max_tokens", "messages")}
    request["system"] = "You are the SIC Editor. Work only on the authorized archive assignment."
    with open_request(url, request, provider_headers(native_headers, key), 600) as answer:
        result = json.load(answer)
    if result.get("stop_reason") != "end_turn" or any(part.get("type") != "text" for part in result["content"]):
        raise ValueError("The Messages provider did not complete an Editor delivery")
    return "".join(part["text"] for part in result["content"])


def emit_checkpoint(handler, frame, checkpoint):
    message = {"id": "msg_" + uuid4().hex, "type": "message", "role": "assistant", "model": frame["model"],
               "content": [], "stop_reason": None, "stop_sequence": None,
               "usage": {"input_tokens": 0, "output_tokens": 0}}
    text = {"type": "text", "text": checkpoint}
    end = {"stop_reason": "end_turn", "stop_sequence": None}
    if not frame.get("stream"):
        handler.reply(200, json.dumps({**message, **end, "content": [text]}).encode())
        return
    handler.send_response(200)
    handler.send_header("Content-Type", "text/event-stream")
    handler.end_headers()
    events = [
        ("message_start", {"message": message}),
        ("content_block_start", {"index": 0, "content_block": {**text, "text": ""}}),
        ("content_block_delta", {"index": 0, "delta": {**text, "type": "text_delta"}}),
        ("content_block_stop", {"index": 0}),
        ("message_delta", {"delta": end, "usage": {"output_tokens": 0}}),
        ("message_stop", {}),
    ]
    for kind, payload in events:
        handler.wfile.write(f"event: {kind}\ndata: {json.dumps({'type': kind, **payload})}\n\n".encode())
        handler.wfile.flush()


@contextmanager
def listen(projection, provider, key, worker_key, hook_key):
    provider = endpoint(provider)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def end_headers(self):
            self.sent = True
            super().end_headers()

        def reply(self, status, body, content_type="application/json"):
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def fail(self, status, kind, message):
            self.reply(status, json.dumps({"type": "error", "error": {"type": kind, "message": message}}).encode())

        def body(self):
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length <= 64 * 1024 * 1024:
                raise ValueError("Invalid Messages frame size")
            return json.loads(self.rfile.read(length))

        def do_POST(self):
            self.dispatch()

        def do_GET(self):
            self.dispatch()

        def dispatch(self):
            self.sent = False
            self.connection.settimeout(30)
            path = urlsplit(self.path).path
            if self.command == "POST" and path == "/sic/arm":
                if not self.authorized("Authorization", "Bearer " + hook_key):
                    return self.fail(401, "authentication_error", "Invalid SIC hook credential")
                try:
                    frame = self.body()
                    marker = projection.arm(frame["session_id"], frame["trigger"])
                except (PermissionError, ValueError, KeyError, TypeError):
                    return self.fail(403, "permission_error", "SIC hook is outside the bound session")
                return self.reply(200, marker.encode(), "text/plain")
            if not self.authorized("x-api-key", worker_key):
                return self.fail(401, "authentication_error", "Invalid SIC session credential")
            try:
                if self.command == "GET" and path == "/v1/models":
                    return self.forward(None)
                if self.command != "POST" or path not in ("/v1/messages", "/v1/messages/count_tokens"):
                    return self.fail(404, "not_found_error", "Unsupported Messages route")
                frame = self.body()
                if path == "/v1/messages/count_tokens":
                    return self.forward(frame)
                if not self.authorized("x-claude-code-session-id", projection.session):
                    return self.fail(403, "permission_error", "Messages request is outside the bound session")
                request_class = self.headers.get("x-claude-code-request-class")
                if request_class in ("auxiliary", "subagent", "workflow") or self.headers.get("x-claude-code-agent-id"):
                    return self.forward(frame)
                if request_class not in ("main", "compaction"):
                    return self.fail(403, "permission_error", "Claude Code did not declare its request class")
                edit = lambda value: editor_call(self.upstream_url(), key, self.headers, value)
                mode, result = projection.route(frame, editor=edit, require_compaction=request_class == "compaction")
                if mode == "compact":
                    return emit_checkpoint(self, frame, result)
                return self.forward(result)
            except (BrokenPipeError, ConnectionResetError):
                return
            except (OSError, RuntimeError, ValueError, TypeError, KeyError, AttributeError) as error:
                if not self.sent:
                    self.fail(409, "invalid_request_error", f"SIC request stopped ({type(error).__name__})")

        def authorized(self, header, expected):
            values = self.headers.get_all(header, [])
            return len(values) == 1 and compare_digest(values[0].encode(), expected.encode())

        def upstream_url(self):
            path = urlsplit(self.path)
            return provider + path.path + ("?" + path.query if path.query else "")

        def forward(self, frame):
            try:
                response = open_request(self.upstream_url(), frame, provider_headers(self.headers, key), 600)
            except HTTPError as error:
                if 300 <= error.code < 400:
                    return self.fail(502, "api_error", "Messages upstream redirects are disabled")
                response = error
            with response:
                self.send_response(response.code)
                for name, value in response.headers.items():
                    if (name.lower() in {"content-type", "content-encoding", "request-id", "retry-after", "x-should-retry"}
                            or name.lower().startswith("anthropic-ratelimit-")):
                        self.send_header(name, value)
                self.end_headers()
                while chunk := response.read1(32768):
                    self.wfile.write(chunk)
                    self.wfile.flush()

    class Server(ThreadingHTTPServer):
        daemon_threads = False

    with Server(("127.0.0.1", 0), Handler) as server:
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}"
        finally:
            server.shutdown()
            thread.join()
