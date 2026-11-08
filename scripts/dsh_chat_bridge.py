"""OpenAI Chat full-history bridge for DSH and other chat clients.

Client must supply x-sic-bridge-session on every request. The bridge owns history
deduplication; callers continue sending full history. Runtime state is in memory.
Upstream responses are buffered, including when compatible SSE is requested.
"""
import argparse
import copy
from decimal import Decimal, InvalidOperation
import json
import os
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from sic_demo import SICClient


def canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def argument_object(raw):
    """Compare valid JSON objects losslessly, without float rounding or duplicate keys."""
    if not isinstance(raw, str):
        raise ValueError("tool arguments must be JSON text")

    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError("duplicate tool argument key")
            result[key] = value
        return result

    def reject_constant(value):
        raise ValueError("non-finite tool argument number: " + value)

    value = json.loads(raw, object_pairs_hook=pairs, parse_int=Decimal, parse_float=Decimal,
                       parse_constant=reject_constant)
    if not isinstance(value, dict):
        raise ValueError("tool arguments must be an object")

    def typed(item):
        if item is None:
            return ("null",)
        if isinstance(item, bool):
            return ("boolean", item)
        if isinstance(item, str):
            return ("string", item)
        if isinstance(item, Decimal):
            if not item.is_finite():
                raise ValueError("non-finite tool argument number")
            sign, digits, exponent = item.as_tuple()
            digits = list(digits)
            if not any(digits):
                return ("number", sign, [0], 0)
            while digits[-1] == 0:
                digits.pop()
                exponent += 1
            return ("number", sign, digits, exponent)
        if isinstance(item, list):
            return ("array", [typed(part) for part in item])
        if isinstance(item, dict):
            return ("object", [(key, typed(part)) for key, part in sorted(item.items())])
        raise ValueError("unsupported tool argument value")

    return typed(value)


def generated_assistant_equivalent(expected, received):
    """Narrow pi-ai replay equivalences, only for the last bridge-owned reply.

    Do not use this for earlier history, protected/user/tool messages or request
    forwarding. Nonempty metadata, IDs, tool names/order and text stay strict.
    """
    if not isinstance(expected, dict) or not isinstance(received, dict):
        return False
    if expected.get("role") != "assistant" or received.get("role") != "assistant":
        return False
    left, right = copy.deepcopy(expected), copy.deepcopy(received)
    try:
        for message in (left, right):
            if message.get("refusal", "missing") is None:
                message.pop("refusal")
            if message.get("annotations") == []:
                message.pop("annotations")
            # Installed pi-ai inserts this empty field for compatible reasoning models.
            if message.get("reasoning_content", "missing") in (None, ""):
                message.pop("reasoning_content", None)
            # pi-ai emits tool_calls only when the reconstructed call list is nonempty.
            if message.get("tool_calls", "missing") is None or message.get("tool_calls") == []:
                message.pop("tool_calls", None)
            calls = message.get("tool_calls")
            if calls:
                if not isinstance(calls, list):
                    return False
                if message.get("content") in (None, ""):
                    message["content"] = None
                for call in calls:
                    if not isinstance(call, dict) or call.get("type") != "function":
                        return False
                    function = call.get("function")
                    if not isinstance(function, dict):
                        return False
                    function["arguments"] = argument_object(function.get("arguments"))
        return canonical(left) == canonical(right)
    except (ValueError, TypeError, InvalidOperation, RecursionError):
        return False


def history_parts(messages):
    if not isinstance(messages, list) or not messages:
        raise ValueError("messages must be a non-empty array")
    protected, groups, group, pending = [], [], [], set()
    started = False
    for message in messages:
        if not isinstance(message, dict):
            raise ValueError("invalid message")
        role = message.get("role")
        if role in ("system", "developer"):
            if started:
                raise ValueError("late protected instruction is unsupported")
            protected.append(message)
            continue
        started = True
        if role not in ("user", "assistant", "tool"):
            raise ValueError("unsupported message role")
        if pending and role != "tool":
            raise ValueError("tool-call batch is incomplete")
        if role == "tool":
            identifier = message.get("tool_call_id")
            if identifier not in pending:
                raise ValueError("unmatched or duplicate tool result")
            pending.remove(identifier)
        calls = message.get("tool_calls", [])
        if calls:
            if role != "assistant" or not isinstance(calls, list):
                raise ValueError("invalid tool batch")
            identifiers = [call.get("id") for call in calls]
            if any(not isinstance(x, str) or not x for x in identifiers) or len(set(identifiers)) != len(identifiers):
                raise ValueError("tool IDs must be unique non-empty strings")
            pending.update(identifiers)
        group.append(message)
        if not pending:
            groups.append(group)
            group = []
    if pending:
        raise ValueError("request ends with incomplete tool-call batch")
    return protected, groups


class Bridge:
    def __init__(self, client, seal_bytes=65536):
        if seal_bytes < 1:
            raise ValueError("seal-bytes must be positive")
        self.client, self.seal_bytes = client, seal_bytes
        self.states, self.lock = {}, threading.Lock()

    def chat(self, key, request):
        if not key or len(key) > 128:
            raise ValueError("x-sic-bridge-session is required (1-128 characters)")
        if request.get("n", 1) != 1 or request.get("functions") or request.get("function_call"):
            raise ValueError("only one choice and modern tool_calls are supported")
        messages = request.get("messages")
        protected, groups = history_parts(messages)
        if not groups:
            raise ValueError("active tail is required")
        fingerprint = canonical(request)
        # A per-bridge lock serializes ingestion and delivery: no competing cursors.
        with self.lock:
            state = self.states.get(key)
            if state is None:
                session = self.client.call("POST", "/api/v1/sessions", {"name": "DSH bridge " + key})["id"]
                state = {"session": session, "expected": [], "archived": len(protected),
                         "protected": copy.deepcopy(protected), "failed": False}
                self.states[key] = state
            if state["failed"]:
                raise ValueError("session stopped after failed delivery/ingestion; inspect SIC and use a new bridge session")
            if state.get("fingerprint") == fingerprint:
                return copy.deepcopy(state["reply"]), state["session"]
            expected = state["expected"]
            old_prefix = expected[:-1] if expected else []
            prefix_matches = canonical(messages[:len(old_prefix)]) == canonical(old_prefix)
            last_reply_matches = not expected or (len(messages) >= len(expected) and
                generated_assistant_equivalent(expected[-1], messages[len(old_prefix)]))
            if canonical(protected) != canonical(state["protected"]) or not prefix_matches or not last_reply_matches:
                raise ValueError("history prefix mismatch; start a new bridge session for forks or edits")
            tail = messages[state["archived"]:]
            _, tail_groups = history_parts(tail)
            # Keep the latest complete group live; never split a tool call/result group.
            older = tail_groups[:-1]
            units = [canonical(group) + "\n" for group in older]
            try:
                if units and len("".join(units).encode()) >= self.seal_bytes:
                    self.client.ingest(state["session"], units)
                    state["archived"] += sum(len(group) for group in older)
                outbound = copy.deepcopy(request)
                outbound["messages"] = protected + messages[state["archived"]:]
                outbound["stream"] = False
                outbound.pop("stream_options", None)
                reply = self.client.call("POST", "/v1/chat/completions", outbound, {
                    "x-sic-session-id": state["session"], "x-sic-history-mode": "tail-only"})
                choice = reply["choices"][0]
                assistant = choice["message"]
                if assistant.get("role") != "assistant" or choice.get("finish_reason") not in ("stop", "tool_calls", "length"):
                    raise ValueError("unsupported upstream completion")
                state["expected"] = copy.deepcopy(messages + [assistant])
                state["fingerprint"], state["reply"] = fingerprint, copy.deepcopy(reply)
                return reply, state["session"]
            except Exception:
                # A lost acknowledgement may have published a WB; never guess/retry it.
                state["failed"] = True
                raise


def handler(bridge):
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            if self.path != "/v1/chat/completions":
                self.send_error(404)
                return
            try:
                length = int(self.headers.get("content-length", "0"))
                if not 0 < length <= 16 * 1024 * 1024:
                    raise ValueError("request requires content-length within 16 MiB")
                request = json.loads(self.rfile.read(length))
                reply, session = bridge.chat(self.headers.get("x-sic-bridge-session"), request)
                if request.get("stream"):
                    choice = reply["choices"][0]
                    delta = {k: v for k, v in choice["message"].items() if k in ("role", "content", "tool_calls", "refusal")}
                    if delta.get("tool_calls"):
                        delta["tool_calls"] = [dict(call, index=index) for index, call in enumerate(delta["tool_calls"])]
                    chunk = {"id": reply.get("id", "sic-bridge"), "object": "chat.completion.chunk",
                             "created": reply.get("created", int(time.time())),
                             "model": reply.get("model", request.get("model")), "choices": [
                                 {"index": 0, "delta": delta, "finish_reason": None}]}
                    final = dict(chunk, choices=[{"index": 0, "delta": {}, "finish_reason": choice["finish_reason"]}])
                    payload = ("data: " + json.dumps(chunk) + "\n\ndata: " + json.dumps(final) + "\n\ndata: [DONE]\n\n").encode()
                    content_type = "text/event-stream"
                else:
                    payload, content_type = json.dumps(reply).encode(), "application/json"
                status = 200
            except ValueError as error:
                status, content_type, payload, session = 409, "application/json", json.dumps({"error": str(error)}).encode(), None
            except Exception:
                status, content_type, payload, session = 502, "application/json", b'{"error":"SIC bridge failed; inspect service and use a new session"}', None
            self.send_response(status)
            self.send_header("content-type", content_type)
            self.send_header("content-length", str(len(payload)))
            if session:
                self.send_header("x-sic-session-id", session)
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, *_):
            pass
    return Handler


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sic-url", default="http://127.0.0.1:8787")
    parser.add_argument("--port", type=int, default=8788)
    parser.add_argument("--seal-bytes", type=int, default=65536)
    args = parser.parse_args()
    service = ThreadingHTTPServer(("127.0.0.1", args.port), handler(Bridge(SICClient(args.sic_url, os.environ["SIC_ADMIN_TOKEN"]), args.seal_bytes)))
    print(f"DSH Chat bridge: http://127.0.0.1:{service.server_port}/v1 ; full history + x-sic-bridge-session required", flush=True)
    service.serve_forever()
