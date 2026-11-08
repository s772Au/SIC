"""Local loopback smoke test. Never contacts an external gateway."""

import json
import os
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EXE = Path(os.environ.get("SIC_TEST_EXE", ROOT / "target" / "debug" / "sic-proxy.exe"))
TOKEN = "local-smoke-token"
requests_seen = []
worker_seen = []


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class MockCompressor(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers["content-length"])
        request = json.loads(self.rfile.read(length))
        requests_seen.append(request)
        assert len(request["tools"]) == 1
        assert request["tools"][0]["function"]["name"] == "sic_edit_wb"
        if "Capacity probe" in request["messages"][1]["content"]:
            payload = b'{"error":{"code":"context_length_exceeded","message":"mock limit"}}'
            self.send_response(400)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        if len(requests_seen) == 1:
            assert "Current task: inspect facts." in request["messages"][1]["content"]
            choice = {
                "finish_reason": "tool_calls",
                "message": {"tool_calls": [{"function": {
                    "name": "sic_edit_wb",
                    "arguments": json.dumps({"edits": [{
                        "target_id": "wb", "op": "COMPRESS", "risk": "low",
                        "content": "Preserved fact; proposed action remains proposed."
                    }]})
                }}]},
            }
        elif len(requests_seen) == 2:
            choice = {"finish_reason": "length", "message": {"tool_calls": []}}
        else:
            ids = request["tools"][0]["function"]["parameters"]["properties"]["edits"]["items"]["properties"]["target_id"]["enum"]
            assert ids == ["u000001", "u000002"]
            assert "已完成" in request["messages"][1]["content"]
            assert "待处理" in request["messages"][1]["content"]
            choice = {"finish_reason": "tool_calls", "message": {"tool_calls": [{"function": {
                "name": "sic_edit_wb",
                "arguments": json.dumps({"edits": [{"target_id": "u000002", "op": "COMPRESS",
                                                  "risk": "low", "content": "待处理：保留来源"}]})
            }}]}}
        payload = json.dumps({"choices": [choice]}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_args):
        pass


class MockWorker(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers["content-length"])
        payload = self.rfile.read(length)
        worker_seen.append({"path": self.path, "body": payload,
                            "authorization": self.headers.get("authorization")})
        if self.path == "/v1/chat/completions":
            status, content_type, output = 400, "application/json", b'{"error":"mock upstream 400"}'
        elif self.path == "/v1/responses":
            status, content_type = 200, "text/event-stream"
            output = b'data: {"type":"response.output_text.delta","delta":"ok"}\n\ndata: [DONE]\n\n'
        else:
            status, content_type, output = 200, "application/json", b'{"content":[]}'
        self.send_response(status)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(output)))
        self.end_headers()
        self.wfile.write(output)

    def log_message(self, *_args):
        pass


def main():
    if not EXE.exists():
        raise SystemExit("Build first: cargo build")
    mock_port, worker_port, sic_port = free_port(), free_port(), free_port()
    mock = ThreadingHTTPServer(("127.0.0.1", mock_port), MockCompressor)
    mock_worker = ThreadingHTTPServer(("127.0.0.1", worker_port), MockWorker)
    thread = threading.Thread(target=mock.serve_forever, daemon=True)
    thread.start()
    threading.Thread(target=mock_worker.serve_forever, daemon=True).start()
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def call(method, path, body=None, admin=False, extra_headers=None):
        headers = {"content-type": "application/json"}
        headers.update(extra_headers or {})
        if admin:
            headers["x-sic-admin-token"] = TOKEN
        payload = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(
            f"http://127.0.0.1:{sic_port}{path}", payload, headers, method=method
        )
        try:
            with opener.open(request, timeout=10) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    def call_raw(path, payload, session_id, history_mode=None):
        headers = {"content-type": "application/json", "authorization": "Bearer local-worker-test",
                   "x-sic-session-id": session_id}
        if history_mode:
            headers["x-sic-history-mode"] = history_mode
        request = urllib.request.Request(
            f"http://127.0.0.1:{sic_port}{path}", payload,
            headers, method="POST"
        )
        try:
            with opener.open(request, timeout=10) as response:
                return response.status, {k.lower(): v for k, v in response.headers.items()}, response.read()
        except urllib.error.HTTPError as error:
            return error.code, {k.lower(): v for k, v in error.headers.items()}, error.read()

    with tempfile.TemporaryDirectory(prefix="sic-local-smoke-") as temp:
        env = os.environ.copy()
        env.update({
            "SIC_BIND": f"127.0.0.1:{sic_port}",
            "SIC_DB": f"sqlite:///{Path(temp, 'smoke.db').as_posix()}?mode=rwc",
            "SIC_ADMIN_TOKEN": TOKEN,
            "SIC_COMPRESSER_URL": f"http://127.0.0.1:{mock_port}/mock",
            "SIC_COMPRESSER_MODEL": "local-mock",
            "SIC_COMPRESSER_API_KEY": "unused-local-mock",
            "SIC_WORKER_UPSTREAM_BASE_URL": f"http://127.0.0.1:{worker_port}/v1",
        })
        process = subprocess.Popen(
            [str(EXE)], cwd=ROOT, env=env,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        try:
            for _ in range(100):
                try:
                    if call("GET", "/api/v1/ui/bootstrap")[0] == 200:
                        break
                except urllib.error.URLError:
                    time.sleep(0.05)
            else:
                raise AssertionError("local SIC service did not start")
            _, catalog = call("GET", "/api/v1/test-catalog")
            assert catalog["defaultGatewayUrl"] == ""
            assert catalog["gatewayStatus"].startswith("testing-only-unstable-route")
            _, session = call("POST", "/api/v1/sessions", {"name": "smoke"}, True)
            _, block = call("POST", f"/api/v1/sessions/{session['id']}/blocks",
                            {"source": "Fact occurred. Proposed action not completed."}, True)
            _, first = call("POST", "/api/v1/drafts", {"block_id": block["id"]}, True)
            _, second = call("POST", "/api/v1/drafts", {"block_id": block["id"]}, True)
            _, listed = call("GET", f"/api/v1/blocks/{block['id']}/drafts")
            assert {d["id"] for d in listed["drafts"]} == {first["id"], second["id"]}
            status, mount = call("PUT", f"/api/v1/sessions/{session['id']}/mount",
                                 {"base_revision": 0, "content": "Current task: inspect facts."}, True)
            assert status == 200 and mount["draft_revision"] == 1
            call("POST", f"/api/v1/sessions/{session['id']}/mount/publish", {}, True)

            seen_before = len(requests_seen)
            status, preflight = call("GET", f"/api/v1/drafts/{first['id']}/single-pass/preflight")
            assert status == 200 and len(requests_seen) == seen_before
            assert preflight["model_configured"] and preflight["within_local_byte_limit"]
            assert preflight["token_admission"] == "unknown"
            assert preflight["wb_source_bytes"] == len("Fact occurred. Proposed action not completed.")
            assert preflight["cb_root_count"] == 0 and preflight["published_mount_bytes"] > 0

            status, good = call("POST", f"/api/v1/drafts/{first['id']}/single-pass", {}, True)
            assert status == 200 and good.get("model_requests") == 1, (status, good)
            assert good["tool_feedback_rounds"] == 0 and not good["committed"]
            _, jobs = call("GET", f"/api/v1/drafts/{first['id']}/jobs")
            assert jobs["jobs"][0]["status"] == "applied"
            assert jobs["jobs"][0]["request_sha256"] == preflight["request_sha256"]
            assert jobs["jobs"][0]["response_sha256"]
            _, saved = call("GET", f"/api/v1/drafts/{first['id']}")
            assert saved["revision"] == 1 and saved["status"] == "open"
            status, _ = call("POST", f"/api/v1/drafts/{first['id']}/commit", {}, True,
                             {"x-sic-cb-sha256": "0" * 64})
            assert status == 409
            _, still_open = call("GET", f"/api/v1/drafts/{first['id']}")
            assert still_open["status"] == "open"
            _, validation = call("POST", f"/api/v1/drafts/{first['id']}/validate")
            _, committed = call("POST", f"/api/v1/drafts/{first['id']}/commit", {}, True,
                                {"x-sic-cb-sha256": validation["cb_sha256"]})
            _, cb = call("GET", f"/api/v1/commits/{committed['id']}")
            assert cb["content"] == "Preserved fact; proposed action remains proposed."
            assert cb["status"] == "committed"
            _, before = call("GET", f"/api/v1/sessions/{session['id']}/projection")
            assert not before["published_cbs"] and len(before["pending_wbs"]) == 1
            status, mount = call("PUT", f"/api/v1/sessions/{session['id']}/mount",
                                 {"base_revision": 1, "content": "Unpublished change."}, True)
            assert status == 200 and mount["draft_revision"] == 2
            _, still_unpublished = call("GET", f"/api/v1/sessions/{session['id']}/projection")
            assert still_unpublished["dynamic_mount"]["content"] == "Current task: inspect facts."
            call("POST", f"/api/v1/sessions/{session['id']}/mount/publish", {}, True)
            _, mount_history = call("GET", f"/api/v1/sessions/{session['id']}/mount/revisions")
            assert len(mount_history["revisions"]) == 2
            assert mount_history["revisions"][0]["published_at"]
            call("POST", f"/api/v1/commits/{committed['id']}/publish", {}, True)
            _, after = call("GET", f"/api/v1/sessions/{session['id']}/projection")
            assert len(after["published_cbs"]) == 1 and not after["pending_wbs"]
            assert after["dynamic_mount"]["content"] == "Unpublished change."
            assert after["order"] == ["published_cb", "dynamic_mount", "pending_wb"]
            assert after["worker_adopted"] is False

            status, bad = call("POST", f"/api/v1/drafts/{second['id']}/single-pass", {}, True)
            assert status == 502 and "incomplete" in bad["error"]
            _, failed_jobs = call("GET", f"/api/v1/drafts/{second['id']}/jobs")
            assert failed_jobs["jobs"][0]["status"] == "failed"
            _, untouched = call("GET", f"/api/v1/drafts/{second['id']}")
            assert untouched["revision"] == 0 and untouched["status"] == "open"
            _, manual = call("PUT", f"/api/v1/drafts/{second['id']}", {
                "base_revision": 0, "edits": [{"target_id": "wb", "op": "COMPRESS",
                                               "risk": "low", "content": "Alternative CB"}]
            }, True)
            assert manual["revision"] == 1
            _, alternative = call("POST", f"/api/v1/drafts/{second['id']}/commit", {}, True)
            status, _ = call("POST", f"/api/v1/commits/{alternative['id']}/publish", {}, True)
            assert status == 409
            status, _ = call("POST", "/api/v1/drafts", {"block_id": block["id"]}, True)
            assert status == 409
            status, _ = call("POST", f"/api/v1/sessions/{session['id']}/blocks", {
                "source": "abc", "units": ["ab", "x"]
            }, True)
            assert status == 400
            _, unit_block = call("POST", f"/api/v1/sessions/{session['id']}/blocks", {
                "source": "已完成\n待处理", "units": ["已完成\n", "待处理"]
            }, True)
            _, block_list = call("GET", f"/api/v1/sessions/{session['id']}/blocks")
            assert [b["id"] for b in block_list["blocks"]] == [block["id"], unit_block["id"]]
            assert block_list["blocks"][1]["bytes"] == len("已完成\n待处理".encode())
            _, unit_view = call("GET", f"/api/v1/blocks/{unit_block['id']}/units")
            assert [u["id"] for u in unit_view["units"]] == ["u000001", "u000002"]
            _, page = call("GET", f"/api/v1/sessions/{session['id']}/archive/wb/{unit_block['id']}?offset=1&max_bytes=4")
            assert page["offset"] == 3 and page["text"] == "完"
            _, unit_draft = call("POST", "/api/v1/drafts", {"block_id": unit_block["id"]}, True)
            _, unit_preflight = call("GET", f"/api/v1/drafts/{unit_draft['id']}/single-pass/preflight")
            assert unit_preflight["source_unit_count"] == 2
            assert unit_preflight["cb_root_count"] == 1 and unit_preflight["cb_context_bytes"] > 0
            status, _ = call("POST", f"/api/v1/drafts/{unit_draft['id']}/single-pass", {}, True)
            assert status == 200
            _, unit_commit = call("POST", f"/api/v1/drafts/{unit_draft['id']}/commit", {}, True)
            _, unit_cb = call("GET", f"/api/v1/commits/{unit_commit['id']}")
            assert unit_cb["content"] == "已完成\n待处理：保留来源"
            call("POST", f"/api/v1/commits/{unit_commit['id']}/publish", {}, True)
            _, timeline = call("GET", f"/api/v1/sessions/{session['id']}/timeline")
            assert [c["id"] for c in timeline["commits"]] == [committed["id"], alternative["id"], unit_commit["id"]]
            _, before_promotion = call("GET", f"/api/v1/sessions/{session['id']}/roots")
            child_ids = [r["id"] for r in before_promotion["roots"]]
            assert child_ids == [committed["id"], unit_commit["id"]]
            _, candidates = call("GET", f"/api/v1/sessions/{session['id']}/promotion-candidates")
            assert candidates["candidate_count"] == 1
            assert candidates["candidates"][0]["child_ids"] == child_ids
            assert candidates["candidates"][0]["child_count"] == 2
            assert candidates["upstream_requested"] is False
            status, _ = call("POST", f"/api/v1/sessions/{session['id']}/promotions", {
                "child_ids": child_ids[:1], "content": "invalid", "risk": "low", "reason": "test"
            }, True)
            assert status == 400
            status, _ = call("POST", f"/api/v1/sessions/{session['id']}/promotions", {
                "child_ids": list(reversed(child_ids)), "content": "invalid order",
                "risk": "low", "reason": "test"
            }, True)
            assert status == 409
            promotion_input = {
                "child_ids": child_ids, "content": "Two adjacent CBs, with original sources retained.",
                "risk": "medium", "reason": "Manual structural smoke test"
            }
            status, promotion_preview = call("POST", f"/api/v1/sessions/{session['id']}/promotions/preview",
                                             promotion_input)
            assert status == 200 and promotion_preview["written"] is False
            assert promotion_preview["roots_before"] == 2 and promotion_preview["roots_after"] == 1
            assert promotion_preview["source_bytes"] > promotion_preview["summary_bytes"]
            status, promoted = call("POST", f"/api/v1/sessions/{session['id']}/promotions",
                                    promotion_input, True)
            assert status == 200 and promoted["level"] == 2
            assert promoted["sha256"] == promotion_preview["summary_sha256"]
            assert call("POST", f"/api/v1/sessions/{session['id']}/promotions/preview",
                        promotion_input)[0] == 409
            _, node = call("GET", f"/api/v1/nodes/{promoted['id']}")
            assert node["child_ids"] == child_ids and "[child:" in node["source"]
            _, after_promotion = call("GET", f"/api/v1/sessions/{session['id']}/projection")
            assert len(after_promotion["published_cbs"]) == 1
            assert after_promotion["published_cbs"][0]["id"] == promoted["id"]
            _, no_candidates = call("GET", f"/api/v1/sessions/{session['id']}/promotion-candidates")
            assert no_candidates["candidate_count"] == 0
            chat_request = {"model": "mock", "messages": [
                {"role": "system", "content": "Host instructions"},
                {"role": "user", "content": "Live tail"}
            ]}
            status, assembly = call("POST", f"/api/v1/sessions/{session['id']}/worker/assemble-chat",
                                    {"request": chat_request})
            assert status == 200 and assembly["status"] == "assembled-not-sent"
            assert assembly["root_ids"] == [promoted["id"]]
            assembled = assembly["request"]["messages"]
            assert [m["role"] for m in assembled] == ["system", "system", "user", "user", "user"]
            assert assembled[0]["content"] == "Host instructions"
            assert promoted["id"] in assembled[2]["content"]
            assert assembled[-1]["content"] == "Live tail"
            status, _ = call("POST", f"/api/v1/sessions/{session['id']}/worker/assemble-chat",
                             {"request": {"messages": [
                                 {"role": "user", "content": "tail"},
                                 {"role": "developer", "content": "late"}
                             ]}})
            assert status == 400
            _, found = call("GET", f"/api/v1/sessions/{session['id']}/archive/search?q=Preserved")
            assert any(hit["kind"] == "cb" and hit["id"] == committed["id"]
                       for hit in found["matches"])
            with ThreadPoolExecutor(max_workers=8) as executor:
                results = list(executor.map(lambda i: call(
                    "PUT", f"/api/v1/sessions/{session['id']}/mount",
                    {"base_revision": 2, "content": f"Concurrent draft {i}"}, True
                )[0], range(8)))
            assert results.count(200) == 1 and results.count(409) == 7, results
            _, gap_session = call("POST", "/api/v1/sessions", {"name": "pending-gap"}, True)
            gap_blocks = []
            for source in ("first", "unpublished", "third"):
                _, gap_block = call("POST", f"/api/v1/sessions/{gap_session['id']}/blocks",
                                    {"source": source}, True)
                gap_blocks.append(gap_block)
            gap_commits = []
            gap_publish_statuses = []
            for gap_block in (gap_blocks[0], gap_blocks[2]):
                _, gap_draft = call("POST", "/api/v1/drafts", {"block_id": gap_block["id"]}, True)
                call("PUT", f"/api/v1/drafts/{gap_draft['id']}", {
                    "base_revision": 0, "edits": [{"target_id": "wb", "op": "COMPRESS",
                                                   "risk": "low", "content": "summary"}]
                }, True)
                _, gap_commit = call("POST", f"/api/v1/drafts/{gap_draft['id']}/commit", {}, True)
                gap_commits.append(gap_commit)
                gap_publish_statuses.append(call("POST", f"/api/v1/commits/{gap_commit['id']}/publish", {}, True)[0])
            assert gap_publish_statuses == [200, 409]
            _, gap_roots = call("GET", f"/api/v1/sessions/{gap_session['id']}/roots")
            assert len(gap_roots["roots"]) == 1
            _, gap_candidates = call("GET", f"/api/v1/sessions/{gap_session['id']}/promotion-candidates")
            assert gap_candidates["candidate_count"] == 0
            _, middle_draft = call("POST", "/api/v1/drafts", {"block_id": gap_blocks[1]["id"]}, True)
            call("PUT", f"/api/v1/drafts/{middle_draft['id']}", {
                "base_revision": 0, "edits": [{"target_id": "wb", "op": "COMPRESS",
                                               "risk": "low", "content": "middle summary"}]
            }, True)
            _, middle_commit = call("POST", f"/api/v1/drafts/{middle_draft['id']}/commit", {}, True)
            assert call("POST", f"/api/v1/commits/{middle_commit['id']}/publish", {}, True)[0] == 200
            assert call("POST", f"/api/v1/commits/{gap_commits[1]['id']}/publish", {}, True)[0] == 200
            _, gap_candidates = call("GET", f"/api/v1/sessions/{gap_session['id']}/promotion-candidates?limit=1")
            assert gap_candidates["candidate_count"] == 3
            assert len(gap_candidates["candidates"]) == 1
            assert gap_candidates["candidates"][0]["child_count"] == 3
            assert len(requests_seen) == 3
            _, capacity_block = call("POST", f"/api/v1/sessions/{gap_session['id']}/blocks",
                                     {"source": "Capacity probe"}, True)
            _, capacity_draft = call("POST", "/api/v1/drafts", {"block_id": capacity_block["id"]}, True)
            status, rejected = call("POST", f"/api/v1/drafts/{capacity_draft['id']}/single-pass", {}, True)
            assert status == 502 and rejected["error"] == "compresser context capacity rejected"
            _, capacity_jobs = call("GET", f"/api/v1/drafts/{capacity_draft['id']}/jobs")
            assert capacity_jobs["jobs"][0]["provider_http_status"] == 400
            assert capacity_jobs["jobs"][0]["response_sha256"]
            assert capacity_jobs["jobs"][0]["status"] == "failed"
            _, capacity_draft_after = call("GET", f"/api/v1/drafts/{capacity_draft['id']}")
            assert capacity_draft_after["revision"] == 0
            raw = b'{"model":"mock","stream":true,"input":"hello"}'
            status, relay_headers, response = call_raw("/v1/responses", raw, session["id"])
            assert status == 200 and b"data: [DONE]" in response
            assert relay_headers["content-type"] == "text/event-stream"
            relay_id = relay_headers["x-sic-relay-id"]
            _, observed = call("GET", f"/api/v1/relay-events/{relay_id}")
            assert observed["upstream_http_status"] == 200
            assert observed["upstream_http_response_observed"] and not observed["cb_adopted"]
            assert observed["physical_adoption"] == "unknown"
            status, _, response = call_raw("/v1/chat/completions", raw, session["id"])
            assert status == 400 and response == b'{"error":"mock upstream 400"}'
            tail_only = json.dumps(chat_request).encode()
            status, projected_headers, response = call_raw(
                "/v1/chat/completions", tail_only, session["id"], "tail-only")
            assert status == 400 and response == b'{"error":"mock upstream 400"}'
            projected_request = json.loads(worker_seen[-1]["body"])
            assert len(projected_request["messages"]) == 5
            assert promoted["id"] in projected_request["messages"][2]["content"]
            _, projected_event = call("GET", f"/api/v1/relay-events/{projected_headers['x-sic-relay-id']}")
            assert projected_event["local_projection"]["root_ids"] == [promoted["id"]]
            responses_request = {"model": "mock", "instructions": "Host instructions",
                                 "input": [{"role": "user", "content": "Live tail"}],
                                 "stream": True, "store": False}
            status, preview = call("POST", f"/api/v1/sessions/{session['id']}/worker/assemble-responses",
                                   {"request": responses_request})
            assert status == 200 and preview["request"]["instructions"] == "Host instructions"
            assert preview["request"]["input"][-1] == {"role": "user", "content": "Live tail"}
            status, response_headers, response = call_raw(
                "/v1/responses", json.dumps(responses_request).encode(), session["id"], "tail-only")
            assert status == 200 and b"data: [DONE]" in response
            projected_responses = json.loads(worker_seen[-1]["body"])
            assert projected_responses["input"][0]["role"] == "developer"
            assert promoted["id"] in projected_responses["input"][1]["content"]
            _, response_event = call("GET", f"/api/v1/relay-events/{response_headers['x-sic-relay-id']}")
            assert response_event["local_projection"]["root_ids"] == [promoted["id"]]
            invalid_stateful = dict(responses_request, previous_response_id="resp_old")
            status, _, _ = call_raw("/v1/responses", json.dumps(invalid_stateful).encode(),
                                    session["id"], "tail-only")
            assert status == 400
            assert [r["path"] for r in worker_seen] == [
                "/v1/responses", "/v1/chat/completions", "/v1/chat/completions", "/v1/responses"
            ]
            assert worker_seen[0]["body"] == raw and worker_seen[1]["body"] == raw
            assert all(r["authorization"] == "Bearer local-worker-test" for r in worker_seen)
            print("local smoke passed: edit core, archive, promotion, concurrent revisions, Worker SSE/400 and opt-in Chat/Responses projection")
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
            mock.shutdown()
            mock_worker.shutdown()


if __name__ == "__main__":
    main()
