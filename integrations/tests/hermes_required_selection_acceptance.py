import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
from pi_tool_wait_acceptance import core_service

cli, binary = map(Path, sys.argv[1:3])
patched = len(sys.argv) >= 4
patched_root = sys.argv[3] if patched else None
required_binding = len(sys.argv) == 5 and sys.argv[4] == "required-binding"
old = "HERMES_ORIGINAL_WITNESS"
with tempfile.TemporaryDirectory(prefix="sic-hermes-selection-") as temporary:
    root = Path(temporary)
    with core_service(binary, root) as core:
        chain = core("chain.create", {})["chain_id"]
        begin = core("native.begin", {"format": "openai-chat",
            "request": {"messages": [{"role": "user", "content": old}]}}, chain)
        identity = {"source_id": begin["source_id"], "expected_source_digest": begin["source_digest"]}
        core("native.commit", {**identity, "expected_version": 0, "edits": [
            {"unit_id": "u1", "expected_text": old, "replacement": "HERMES_CB_WITNESS"}]}, chain)
        core("native.publish", {**identity, "expected_version": 1, "expected_cut": 0, "idempotency_key": "hermes",
            "ops": [{"op": "COMPRESS", "target": "u1", "content": "HERMES_CB_WITNESS"}], "pvi": {}}, chain)
        new = core("cut.get", {}, chain)["view"]["cbs"][0]["body"]
    reports = []

    class Endpoint(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            frame = json.loads(self.rfile.read(int(self.headers["content-length"])))
            reports.append({"path": self.path, "users": [m["content"] for m in frame["messages"] if m["role"] == "user"],
                            "tools": [f["function"]["name"] for f in frame.get("tools", [])],
                            "stream": frame.get("stream", False), "model": frame["model"]})
            model, answer = frame["model"], "HERMES_WORKER_ACCEPTED"
            usage = {"prompt_tokens": 300, "completion_tokens": 4, "total_tokens": 304}
            self.send_response(200)
            if frame.get("stream"):
                self.send_header("content-type", "text/event-stream")
                self.end_headers()
                choices = [({"role":"assistant","content":answer}, None), ({}, "stop")]
                for delta, finish in choices:
                    packet = {"id":"hermes-fixture","object":"chat.completion.chunk","created":1,"model":model,
                              "choices":[{"index":0,"delta":delta,"finish_reason":finish}],"usage":usage}
                    self.wfile.write(("data: " + json.dumps(packet) + "\n\n").encode())
                self.wfile.write(b"data: [DONE]\n\n")
                return
            self.send_header("content-type", "application/json")
            self.end_headers()
            packet = {"id":"hermes-fixture","object":"chat.completion","created":1,"model":model,
                      "choices":[{"index":0,"message":{"role":"assistant","content":answer},"finish_reason":"stop"}],
                      "usage":usage}
            self.wfile.write(json.dumps(packet).encode())

    with ThreadingHTTPServer(("127.0.0.1", 0), Endpoint) as provider:
        thread = Thread(target=provider.serve_forever, daemon=True)
        thread.start()
        try:
            working = root / "project"
            working.mkdir()
            private_home = root / "home"
            private_home.mkdir()
            hermes_home = private_home / "hermes"
            plugin = hermes_home / "plugins" / "sic_fixture"
            plugin.mkdir(parents=True)
            (plugin / "__init__.py").write_text("""
import os
from pathlib import Path
from agent.context_engine import ContextEngine

class SicFixture(ContextEngine):
    name = "sic_fixture"
    require_context_selection = True

    def bind_session_state(self, session_db, session_id):
        if Path(__file__).with_name("fail-binding").exists():
            raise ValueError("synthetic missing session grant")
        assert session_id

    def should_compress(self, prompt_tokens=None):
        return False

    def compress(self, messages, current_tokens=None, focus_topic=None, force=False, memory_context=""):
        return messages

    def update_from_response(self, usage):
        self.last_prompt_tokens = usage["prompt_tokens"]
        self.last_completion_tokens = usage["completion_tokens"]
        self.last_total_tokens = usage["total_tokens"]

    def select_context(self, request_messages, **kwargs):
        original = "HERMES_ORIGINAL_WITNESS"
        target = os.environ["SIC_HERMES_SELECTED_CB"]
        if any(m.get("content") == "HERMES_SELECTION_FAILURE_WITNESS" for m in request_messages):
            raise ValueError("synthetic context store unavailable")
        if any(m.get("content") == "HERMES_SELECTION_NO_CUT_WITNESS" for m in request_messages):
            return None
        if sum(m.get("role") == "user" and m.get("content") == original for m in request_messages) != 1:
            return request_messages
        return [{**m, "content": target} if m.get("role") == "user" and m.get("content") == original else m
                for m in request_messages]
""", encoding="utf-8")
            base_url = f"http://127.0.0.1:{provider.server_port}/v1"
            config = {"context":{"engine":"sic_fixture"}, "model":{"default":"sic-local","context_length":65536,
                                                                  "provider":"custom","base_url":base_url},
                      "memory": {"memory_enabled": False, "user_profile_enabled": False},
                      "display":{"interface":"cli"}}
            if required_binding:
                config["context"]["require_engine"] = True
            (hermes_home / "config.yaml").write_text(json.dumps(config), encoding="utf-8")
            env = {**{k: v for k, v in os.environ.items() if not k.lower().endswith("_proxy")},
                   "HOME": str(private_home), "HERMES_HOME": str(hermes_home), "SIC_HERMES_SELECTED_CB":new,
                   "OPENAI_API_KEY":"fixture", "OPENAI_BASE_URL":base_url,
                   "NO_PROXY":"127.0.0.1,localhost"}
            if patched:
                env["PYTHONPATH"] = patched_root
            fail_cases = ("HERMES_SELECTION_FAILURE_WITNESS", "HERMES_SELECTION_NO_CUT_WITNESS")

            def native(prompt, settings=env):
                return subprocess.run([cli,"chat","--cli","--oneshot","--quiet","--ignore-rules","--provider","custom",
                    "--base-url", base_url, "--api-key","fixture",
                    "--model","sic-local","--toolsets","file","--max-turns","2","-q",prompt],
                    cwd=working, env=settings, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=45)

            for prompt in (old, "Hermes normal input.", *fail_cases):
                before = len(reports)
                result = native(prompt)
                if patched and prompt in fail_cases:
                    assert len(reports) == before, reports[before:]
                    assert "Required context selection" in result.stderr + result.stdout, (
                        result.returncode, result.stdout[-3000:], result.stderr[-3000:])
                    continue
                assert result.returncode == 0 and "HERMES_WORKER_ACCEPTED" in result.stdout, (
                    result.returncode, result.stdout[-2600:], result.stderr[-6000:], reports)
            if required_binding:
                before = len(reports)
                (plugin / "fail-binding").touch()
                bound_fault = native(old)
                assert "Required context engine session binding failed" in bound_fault.stdout + bound_fault.stderr, (
                    bound_fault.returncode, bound_fault.stdout[-2500:], bound_fault.stderr[-2500:], reports[before:])
                config["context"]["engine"] = "sic_fixture_absent"
                (hermes_home / "config.yaml").write_text(json.dumps(config), encoding="utf-8")
                unavailable = native(old)
                assert "Required context engine is unavailable" in unavailable.stdout + unavailable.stderr, (
                    unavailable.returncode, unavailable.stdout[-2500:], unavailable.stderr[-2500:], reports[before:])
                assert len(reports) == before, reports[before:]
            primary = [r for r in reports if r["tools"]]
            assert len(primary) == (2 if patched else 4) and all(r["path"] == "/v1/chat/completions" and "read_file" in r["tools"] for r in primary), reports
            assert new in json.dumps(primary[0]["users"]) and old not in json.dumps(primary[0]["users"]), reports
            assert "Hermes normal input." in json.dumps(primary[1]["users"]) and new not in json.dumps(primary[1]["users"]), reports
            export_file = root / "native-sessions.jsonl"
            subprocess.run([cli, "sessions", "export", "--format", "jsonl", str(export_file)],
                           cwd=working, env=env, stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=25, check=True)
            transcript = export_file.read_text(encoding="utf-8")
            assert old in transcript and new not in transcript, "Hermes lost original transcript material"
            print(json.dumps({"result":"PASS","host":"Hermes v0.23.2 pinned release source",
                              "patched_selection_required":patched, "native_read_file_preserved":True,
                              "missing_engine_and_binding_fault_blocked":required_binding,
                              "fault_injection_provider_requests":len(primary)-2,
                              "selected_cb_sent_by_native_context_engine":True,"unbound_request_unchanged":True,
                              "native_transcript_kept_original":True,
                              "paid_requests":0}, indent=2))
        finally:
            provider.shutdown()
            thread.join(timeout=5)
