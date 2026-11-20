"""Exercise actual Pi Worker, native bash and Editor stream against a real Core."""
import argparse
from contextlib import contextmanager
from hashlib import sha256
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
from queue import Queue
import secrets
import shlex
import subprocess
import sys
import tempfile
from threading import Event, Thread
from urllib.request import Request, build_opener, ProxyHandler
from uuid import uuid4


@contextmanager
def core_service(binary, root, native_grants=()):
    db = root / "core.db"
    actor = "pi-tool-wait-test"
    token = secrets.token_urlsafe(32)
    if not db.exists():
        subprocess.run([binary, "init", db], check=True, stdout=subprocess.DEVNULL)
    env = {**os.environ, "SIC_TEST_CORE_TOKEN": token}
    command = [binary, "serve", db, "127.0.0.1:0", actor, "SIC_TEST_CORE_TOKEN"]
    if native_grants:
        grants = root / "native-grants.json"
        entries = []
        for index, (chain_id, credential) in enumerate(native_grants):
            env_name = f"SIC_TEST_NATIVE_GRANT_{index}"
            env[env_name] = credential
            entries.append({"chain_id": chain_id, "token_env": env_name})
        grants.write_text(json.dumps(entries), encoding="utf-8")
        command.extend(["--harness-grants", grants])
    with subprocess.Popen(
        command, env=env, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    ) as process:
        try:
            lines = Queue()
            Thread(target=lambda: lines.put(process.stdout.readline()), daemon=True).start()
            banner = lines.get(timeout=15)
            if "listening on " not in banner:
                raise RuntimeError(f"Core failed to start: {banner!r}")
            endpoint = "http://" + banner.split("listening on ", 1)[1].strip()

            def call(operation, payload, chain_id=None):
                body = {"operation": operation, "payload": payload, "context": {
                    "actor": actor, "chain_id": chain_id, "allow_write": True,
                    "allow_model": False, "allow_revoke": False}}
                request = Request(endpoint + "/api/v2/operations", json.dumps(body).encode(),
                                  {"authorization": "Bearer " + token,
                                   "content-type": "application/json"})
                with build_opener(ProxyHandler({})).open(request, timeout=10) as response:
                    return json.load(response)["result"]

            call.endpoint = endpoint
            yield call
        finally:
            process.terminate()
            process.wait(timeout=10)


def exercise(pi, root, core, tool_first, shell):
    root.mkdir()
    chain = core("chain.create", {})["chain_id"]
    call = lambda op, payload: core(op, payload, chain)
    editor_text = "Plan recorded by the SIC Editor."
    worker_text = "I will use the delayed native shell to read a witness and then report it."
    witness = "PI_LATE_TOOL_WITNESS"
    model = "sic-pi-tool-wait"
    tool_id = "call_sic_gate"
    errors = []
    timeline = []
    tool_started = Event()
    tool_finished = Event()
    published = Event()
    ready = {}
    records = {}

    class Endpoint(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def reply(self, body):
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(body).encode())

        def fail(self, error):
            errors.append(f"{self.path}: {error!r}")
            self.send_error(500, str(error).replace("\n", " ")[:400])

        def do_GET(self):
            try:
                assert self.path == "/delay"
                timeline.append("native-tool-started")
                tool_started.set()
                if not tool_first:
                    assert published.wait(12), "native shell timed out waiting for publication"
                self.reply(witness)
            except Exception as error:
                self.fail(error)

        def stream(self, delta, finish):
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.end_headers()
            for choice in ({"index": 0, "delta": {"role": "assistant", **delta}, "finish_reason": None},
                           {"index": 0, "delta": {}, "finish_reason": finish}):
                packet = {"id": "pi-sic-test", "created": 1, "model": model,
                          "object": "chat.completion.chunk", "choices": [choice]}
                self.wfile.write(("data: " + json.dumps(packet) + "\n\n").encode())
            self.wfile.write(b"data: [DONE]\n\n")

        def do_POST(self):
            try:
                payload = json.loads(self.rfile.read(int(self.headers["content-length"])))
                if self.path == "/freeze":
                    assert witness not in json.dumps(payload)
                    assert payload["request"]["messages"][-1]["stopReason"] == "toolUse"
                    receipt = call("native.begin", payload)
                    assert receipt["binding"]["tool_wait"]["result_ownership"] == "native-host-next-boundary"
                    state = call("cut.get", {})
                    assert state["sequence"] == 0 and not state["pending"]
                    ready.update(receipt)
                    self.reply(receipt)
                elif self.path == "/publish":
                    assert payload["original"] == worker_text and payload["replacement"] == editor_text
                    assert payload["source_id"] == ready["source_id"]
                    assert payload["expected_source_digest"] == ready["source_digest"]
                    identity = {key: payload[key] for key in ("source_id", "expected_source_digest")}
                    unit, = ready["binding"]["editable_unit_ids"]
                    call("native.commit", {**identity, "expected_version": 0, "edits": [{
                        "unit_id": unit, "expected_text": worker_text, "replacement": editor_text}]})
                    result = call("native.publish", {**identity, "expected_version": 1, "expected_cut": 0,
                        "idempotency_key": "publish-first-editor",
                        "ops": [{"op": "COMPRESS", "target": unit, "content": editor_text}], "pvi": {}})
                    records["cut"] = call("cut.get", {})
                    assert records["cut"]["sequence"] == 1
                    assert witness not in json.dumps(result["source_request"])
                    assert result["request"]["messages"][-1]["content"][0]["text"] == editor_text
                    timeline.append("core-published")
                    published.set()
                    self.reply(result)
                elif self.path == "/tool-finished":
                    assert payload["callId"] == tool_id and not payload["isError"]
                    timeline.append("native-tool-finished")
                    tool_finished.set()
                    self.reply({})
                elif self.path == "/v1/chat/completions":
                    messages = payload["messages"]
                    assert payload["model"] == model and payload["stream"] is True
                    assert "bash" in [tool["function"]["name"] for tool in payload["tools"]]
                    if "SIC_EDITOR_TOOL_WAIT_FIXTURE" in json.dumps(messages):
                        assert messages[:-1] == records["initial"]["messages"]
                        assert payload["tools"] == records["initial"]["tools"]
                        assert worker_text in messages[-1]["content"] and witness not in json.dumps(messages)
                        assert tool_started.wait(12), "Editor never overlapped a real shell invocation"
                        if tool_first:
                            assert tool_finished.wait(12), "Editor blocked the shell"
                        timeline.append("editor-responded")
                        self.stream({"content": json.dumps({"replacement": editor_text})}, "stop")
                    elif "initial" not in records:
                        records["initial"] = payload
                        timeline.append("worker-initial")
                        program = f"import urllib.request; print(urllib.request.urlopen({endpoint + '/delay'!r}, timeout=15).read().decode())"
                        python = shlex.quote(Path(sys.executable).as_posix())
                        command = f"{python} -c {shlex.quote(program)}"
                        self.stream({"content": worker_text, "tool_calls": [{"index": 0, "id": tool_id,
                            "type": "function", "function": {"name": "bash", "arguments": json.dumps({
                                "command": command, "timeout": 20})}}]}, "tool_calls")
                    else:
                        assert published.is_set() and tool_finished.is_set()
                        prefix = records["initial"]["messages"]
                        assert messages[:len(prefix)] == prefix and payload["tools"] == records["initial"]["tools"]
                        assistant, result = messages[len(prefix):]
                        assert assistant["role"] == "assistant" and assistant["content"] == editor_text
                        assert assistant["tool_calls"][0]["id"] == result["tool_call_id"] == tool_id
                        assert result["role"] == "tool" and witness in result["content"]
                        timeline.append("worker-continued")
                        self.stream({"content": "PI_WORKER_ACCEPTED_PUBLISHED_CB_AND_TOOL_RESULT"}, "stop")
                else:
                    raise AssertionError("Unexpected request path")
            except Exception as error:
                self.fail(error)

    with ThreadingHTTPServer(("127.0.0.1", 0), Endpoint) as server:
        endpoint = f"http://127.0.0.1:{server.server_port}"
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            config = root / "config"
            config.mkdir()
            (config / "models.json").write_text(json.dumps({"providers": {"sic-fixture": {
                "baseUrl": endpoint + "/v1", "api": "openai-completions", "apiKey": "fixture",
                "models": [{"id": model}]}}}), encoding="utf-8")
            settings = {"compaction": {"enabled": False}}
            if shell:
                settings["shellPath"] = str(shell.resolve())
            (config / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
            session = uuid4().hex
            env = {**{key: value for key, value in os.environ.items() if not key.lower().endswith("_proxy")},
                   "PI_CODING_AGENT_DIR": str(config), "SIC_TEST_PI_SESSION": session,
                   "SIC_TEST_PI_BRIDGE": endpoint, "NO_PROXY": "127.0.0.1,localhost",
                   "PI_SKIP_VERSION_CHECK": "1", "PI_TELEMETRY": "0"}
            command = [*pi, "--mode", "json", "--provider", "sic-fixture", "--model", model,
                       "--session-dir", root / "sessions", "--session-id", session, "--no-extensions",
                       "-e", Path(__file__).with_name("pi_tool_wait_fixture.ts"),
                       "Use the local tool to verify the witness."]
            result = subprocess.run(command, cwd=root, env=env, timeout=45, stdin=subprocess.DEVNULL,
                                    text=True, capture_output=True)
            assert not errors, (errors, timeline, result.stderr[-1500:], result.stdout[-2500:])
            assert result.returncode == 0 and "PI_WORKER_ACCEPTED_PUBLISHED_CB_AND_TOOL_RESULT" in result.stdout, (
                timeline, result.returncode, result.stderr[-1500:], result.stdout[-2500:])
            expected_order = ["native-tool-finished", "editor-responded", "core-published"] if tool_first else [
                "editor-responded", "core-published", "native-tool-finished"]
            assert timeline == ["worker-initial", "native-tool-started", *expected_order, "worker-continued"]
            cut = call("cut.get", {})
            assert cut == records["cut"] and cut["view"]["cbs"][0]["body"] == editor_text
            event_id = ready["binding"]["tool_wait"]["boundary"]["assistant_message_id"]
            session_file, = (root / "sessions").glob("*.jsonl")
            entries = [json.loads(line) for line in session_file.read_text(encoding="utf-8").splitlines()]
            assistant = next(entry for entry in entries if entry.get("id") == event_id)
            assert assistant["message"]["content"][0]["text"] == worker_text
            assert any(entry.get("message", {}).get("role") == "toolResult" and
                       witness in json.dumps(entry["message"]) for entry in entries)
            cut_hash = sha256(json.dumps(cut["cut"], sort_keys=True).encode()).hexdigest()
            return {"timeline": timeline, "immutable_cut_sha256": cut_hash,
                    "shared_prefix_and_native_tools": True, "late_result_outside_cb": True,
                    "worker_continued": True, "original_session_kept": True}
        finally:
            server.shutdown()
            thread.join(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pi", type=Path, required=True)
    parser.add_argument("--node", type=Path, help="Node executable when --pi specifies the JS entrypoint")
    parser.add_argument("--shell", type=Path, help="Native Bash executable for the Pi tool")
    parser.add_argument("--core", type=Path, required=True)
    args = parser.parse_args()
    pi = [args.pi.absolute()]
    if args.node:
        pi.insert(0, args.node.absolute())
    with tempfile.TemporaryDirectory(prefix="sic-pi-tool-wait-") as temp:
        root = Path(temp)
        with core_service(args.core.resolve(), root) as call:
            output = {name: exercise(pi, root / name, call, tool_first, args.shell)
                      for name, tool_first in (("tool_first", True), ("editor_first", False))}
    print(json.dumps({"result": "PASS", "paid_requests": 0, **output}, indent=2))


if __name__ == "__main__":
    main()
