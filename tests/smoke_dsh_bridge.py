"""Real Rust + HTTP bridge + owned loopback model, no external calls."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from dsh_chat_bridge import Bridge, handler
from run_experiment import free_port
from sic_demo import SICClient

FIXTURES = json.loads((Path(__file__).parent / "fixtures/pi-ai-1.1.0-replay.json").read_text(encoding="utf-8"))
TOOL_REPLAY = next(sample for sample in FIXTURES["samples"] if sample["name"] == "tool-json-reformat")


class BridgeMock(BaseHTTPRequestHandler):
    worker_requests = 0

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["content-length"])))
        if request.get("tools"):
            identifier = request["tools"][0]["function"]["parameters"]["properties"]["edits"]["items"]["properties"]["target_id"]["enum"][0]
            message, reason = {"role": "assistant", "tool_calls": [{"function": {"name": "sic_edit_wb",
                "arguments": json.dumps({"edits": [{"target_id": identifier, "op": "KEEP", "risk": "low"}]})}}]}, "tool_calls"
        else:
            BridgeMock.worker_requests += 1
            if BridgeMock.worker_requests == 1:
                message, reason = TOOL_REPLAY["upstream_assistant"], "tool_calls"
            else:
                message, reason = {"role": "assistant", "content": "ok", "refusal": None, "annotations": []}, "stop"
        payload = json.dumps({"choices": [{"finish_reason": reason, "message": message}]}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_):
        pass


def main():
    model = ThreadingHTTPServer(("127.0.0.1", 0), BridgeMock)
    threading.Thread(target=model.serve_forever, daemon=True).start()
    port = free_port()
    env = {k: v for k, v in os.environ.items() if not k.startswith("SIC_")}
    with tempfile.TemporaryDirectory(prefix="sic-bridge-") as tmp:
        env.update(SIC_BIND=f"127.0.0.1:{port}", SIC_ADMIN_TOKEN="bridge-smoke",
                   SIC_DB=f"sqlite:///{Path(tmp, 'bridge.db').as_posix()}?mode=rwc",
                   SIC_COMPRESSER_URL=f"http://127.0.0.1:{model.server_port}/v1/chat/completions",
                   SIC_COMPRESSER_MODEL="mock", SIC_COMPRESSER_API_KEY="mock",
                   SIC_WORKER_UPSTREAM_BASE_URL=f"http://127.0.0.1:{model.server_port}/v1", SIC_WORKER_UPSTREAM_API_KEY="mock")
        executable = Path(os.environ.get("SIC_TEST_EXE", ROOT / "target/debug" / ("sic-proxy.exe" if os.name == "nt" else "sic-proxy")))
        process = subprocess.Popen([str(executable)], cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        bridge_server = None
        try:
            client = SICClient(f"http://127.0.0.1:{port}", "bridge-smoke")
            for _ in range(100):
                try:
                    client.call("GET", "/api/v1/ui/bootstrap")
                    break
                except OSError:
                    time.sleep(.1)
            else:
                raise RuntimeError("SIC did not start")
            bridge_server = ThreadingHTTPServer(("127.0.0.1", 0), handler(Bridge(client, 1)))
            threading.Thread(target=bridge_server.serve_forever, daemon=True).start()
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

            def chat(body):
                request = urllib.request.Request(f"http://127.0.0.1:{bridge_server.server_port}/v1/chat/completions",
                    json.dumps(body).encode(), {"content-type": "application/json", "x-sic-bridge-session": "smoke"})
                with opener.open(request, timeout=30) as response:
                    return response.read(), response.headers["x-sic-session-id"]

            request = {"model": "mock", "messages": [{"role": "system", "content": "HOST-PROTECTED"}, {"role": "user", "content": "first"}]}
            raw, session = chat(request)
            first = json.loads(raw)
            assert chat(request)[0] == raw  # cached identical retry
            assert first["choices"][0]["message"] == TOOL_REPLAY["upstream_assistant"]
            request["messages"] += [TOOL_REPLAY["pi_replayed_assistant"],
                {"role": "tool", "tool_call_id": "call_test", "content": "local tool result"},
                {"role": "user", "content": "second"}]
            second_raw, _ = chat(request)
            projection = client.call("GET", f"/api/v1/sessions/{session}/projection")
            assert len(projection["published_cbs"]) == 1
            first_blocks = client.call("GET", f"/api/v1/sessions/{session}/blocks")
            first_units = client.call("GET", f"/api/v1/blocks/{first_blocks['blocks'][0]['id']}/units")
            archived_messages = [message for unit in first_units["units"] for message in json.loads(unit["source"])]
            assert TOOL_REPLAY["pi_replayed_assistant"] in archived_messages
            assert TOOL_REPLAY["upstream_assistant"] not in archived_messages
            roots = client.call("GET", f"/api/v1/sessions/{session}/roots")
            assert "HOST-PROTECTED" not in json.dumps(roots)
            tool_group = [{"role": "assistant", "content": None, "tool_calls": [
                {"id": "tool-1", "type": "function", "function": {"name": "demo", "arguments": "{}"}},
                {"id": "tool-2", "type": "function", "function": {"name": "demo", "arguments": "{}"}}]},
                {"role": "tool", "tool_call_id": "tool-2", "content": "result 2"},
                {"role": "tool", "tool_call_id": "tool-1", "content": "result 1"}]
            second_assistant = json.loads(second_raw)["choices"][0]["message"]
            second_assistant.pop("refusal")
            second_assistant.pop("annotations")
            request["messages"] += [second_assistant] + tool_group + [{"role": "user", "content": "third"}]
            request["stream"] = True
            stream, _ = chat(request)
            assert b"data: [DONE]" in stream
            projection = client.call("GET", f"/api/v1/sessions/{session}/projection")
            assert len(projection["published_cbs"]) == 2
            blocks = client.call("GET", f"/api/v1/sessions/{session}/blocks")
            # All three tool messages remain one immutable source unit.
            last_block = blocks["blocks"][-1]
            units = client.call("GET", f"/api/v1/blocks/{last_block['id']}/units")
            assert any("tool-1" in unit["source"] and "tool-2" in unit["source"] for unit in units["units"])
            request["messages"][1]["content"] = "edited history"
            try:
                chat(request)
                raise AssertionError("edited prefix accepted")
            except urllib.error.HTTPError as error:
                assert error.code == 409
            print("DSH bridge smoke passed: actual pi-ai replay fixture, automatic WB/edit/publish and Worker projection, retry cache, protected prefix, buffered SSE, mismatch rejection")
        finally:
            if bridge_server:
                bridge_server.shutdown()
                bridge_server.server_close()
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            model.shutdown()
            model.server_close()


if __name__ == "__main__":
    main()
