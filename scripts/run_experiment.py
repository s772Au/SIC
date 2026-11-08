"""Immutable synthetic baseline/SIC runner. Defaults to owned loopback mocks.

Real Chat Completions execution needs an explicit endpoint, pinned model, API key
environment variable and --allow-network. Never resumes previous run directories.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import threading
import time
import urllib.error
from urllib.parse import urlsplit
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from sic_demo import SICClient

ROOT = Path(__file__).resolve().parents[1]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


class Mock(BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        if body.get("tools"):
            # Identity editing is deliberate: this tests publication, not compression quality.
            choice = {"finish_reason": "tool_calls", "message": {"tool_calls": [{"function": {
                "name": "sic_edit_wb", "arguments": json.dumps({"edits": [{
                    "target_id": body["tools"][0]["function"]["parameters"]["properties"]["edits"]["items"]["properties"]["target_id"]["enum"][0],
                    "op": "KEEP", "risk": "low"}]})}}]}}
        else:
            text = "\n".join(m.get("content", "") for m in body["messages"])
            match = re.search(r"FACT answer=([^\n]+)", text)
            choice = {"finish_reason": "stop", "message": {"role": "assistant", "content": match.group(1) if match else "unknown"}}
        payload = json.dumps({"choices": [choice]}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_):
        pass


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def score(records):
    answered = [r for r in records if r.get("status") == "answered"]
    return {"metric": "exact-match-strip-v1", "total": len(records),
            "answered": len(answered), "missing_or_failed": len(records) - len(answered),
            "correct": sum(r["answer"].strip() == r["expected"].strip() for r in answered),
            "quality_claim": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cases", type=Path, default=ROOT / "benchmarks/demo-cases.json")
    parser.add_argument("--provider", choices=["mock", "chat-completions"], default="mock")
    parser.add_argument("--endpoint", help="full /chat/completions URL")
    parser.add_argument("--model")
    parser.add_argument("--api-key-env", default="SIC_EXPERIMENT_API_KEY")
    parser.add_argument("--allow-network", action="store_true")
    parser.add_argument("--max-tokens", type=int, default=4096)
    args = parser.parse_args()
    if args.provider != "mock":
        endpoint = urlsplit(args.endpoint or "")
        if not args.allow_network or endpoint.scheme not in ("http", "https") or not endpoint.hostname or not args.model:
            parser.error("real provider requires --allow-network, --endpoint and --model")
        if endpoint.port == 8341 or endpoint.username or endpoint.password or endpoint.query or endpoint.fragment:
            parser.error("8341, URL credentials, query strings and fragments are forbidden")
        if not os.environ.get(args.api_key_env):
            parser.error("API key environment variable is unset")
    if args.max_tokens < 1:
        parser.error("max-tokens must be positive")
    cases_bytes = args.cases.read_bytes()
    cases = json.loads(cases_bytes)["cases"]
    ids = [case["id"] for case in cases]
    if not cases or len(set(ids)) != len(ids):
        parser.error("case IDs must be non-empty and unique")
    binary_name = "sic-proxy.exe" if os.name == "nt" else "sic-proxy"
    exe = next((path for path in (ROOT / "bin" / binary_name, ROOT / "target/release" / binary_name,
                                 ROOT / "target/debug" / binary_name) if path.exists()), None)
    if exe is None:
        parser.error("build SIC first: cargo build")
    args.out.mkdir(parents=True, exist_ok=False)
    write_json(args.out / "cases.json", {"cases": cases})
    mock = process = None
    records = []
    env = {k: v for k, v in os.environ.items() if not k.startswith("SIC_")}
    try:
        if args.provider == "mock":
            mock = ThreadingHTTPServer(("127.0.0.1", 0), Mock)
            threading.Thread(target=mock.serve_forever, daemon=True).start()
            endpoint = f"http://127.0.0.1:{mock.server_port}/v1/chat/completions"
            model, api_key = "synthetic-identity-mock-v1", "local-mock"
        else:
            endpoint, model, api_key = args.endpoint, args.model, os.environ[args.api_key_env]
        if not endpoint.endswith("/chat/completions"):
            raise ValueError("endpoint must end in /chat/completions")
        port = free_port()
        token = os.urandom(24).hex()
        env.update(SIC_BIND=f"127.0.0.1:{port}", SIC_DB=f"sqlite:///{(args.out.resolve() / 'run.db').as_posix()}?mode=rwc",
                   SIC_ADMIN_TOKEN=token, SIC_COMPRESSER_URL=endpoint, SIC_COMPRESSER_MODEL=model,
                   SIC_COMPRESSER_API_KEY=api_key, SIC_COMPRESSER_MAX_TOKENS=str(args.max_tokens),
                   SIC_WORKER_UPSTREAM_BASE_URL=endpoint.removesuffix("/chat/completions"), SIC_WORKER_UPSTREAM_API_KEY=api_key)
        write_json(args.out / "manifest.json", {"schema": "sic-run-v1", "provider": args.provider,
            "endpoint": endpoint, "model": model, "max_output_tokens": args.max_tokens,
            "cases_input_sha256": digest(cases_bytes), "binary_sha256": digest(exe.read_bytes()),
            "runner_sha256": digest(Path(__file__).read_bytes()), "client_sha256": digest((ROOT / "scripts/sic_demo.py").read_bytes()),
            "arms": ["baseline", "sic"], "seed": "not set; provider determinism unknown",
            "purpose": "synthetic pipeline check", "physical_adoption": "unknown", "status": "running"})
        with (args.out / "service.log").open("wb") as log:
            process = subprocess.Popen([str(exe)], cwd=ROOT, env=env, stdout=log, stderr=log)
            client = SICClient(f"http://127.0.0.1:{port}", token)
            for _ in range(100):
                if process.poll() is not None:
                    raise RuntimeError("SIC exited before readiness; see service.log")
                try:
                    client.call("GET", "/api/v1/ui/bootstrap")
                    break
                except (OSError, urllib.error.URLError):
                    time.sleep(.1)
            else:
                raise RuntimeError("SIC startup timeout")
            for index, case in enumerate(cases):
                for arm in ("baseline", "sic"):
                    item = {"case_id": case["id"], "arm": arm, "expected": case["expected"], "status": "failed"}
                    try:
                        session = client.call("POST", "/api/v1/sessions", {"name": f"demo-{index}-{arm}"})["id"]
                        request = {"model": model, "stream": False, "temperature": 0, "max_tokens": args.max_tokens,
                                   "messages": [{"role": "user", "content": case["question"]}]}
                        headers = {}
                        if arm == "sic":
                            item["ingestion"] = client.ingest(session, case["units"])
                            headers = {"x-sic-history-mode": "tail-only", "x-sic-session-id": session}
                            item["assembly"] = client.call("POST", f"/api/v1/sessions/{session}/worker/assemble-chat", {"request": request})
                        else:
                            request["messages"].insert(0, {"role": "user", "content": "".join(case["units"])})
                        item["worker_request"] = request
                        reply = client.call("POST", "/v1/chat/completions", request, headers)
                        item["worker_response"] = reply
                        item["answer"] = reply["choices"][0]["message"]["content"]
                        if not isinstance(item["answer"], str):
                            raise ValueError("non-text answer")
                        item["status"] = "answered"
                    except Exception as error:
                        item["error"] = type(error).__name__ + ": " + str(error)
                    records.append(item)
                    write_json(args.out / "records.json", records)
        write_json(args.out / "scores.json", {arm: score([r for r in records if r["arm"] == arm]) for arm in ("baseline", "sic")})
        write_json(args.out / "completion.json", {"status": "complete" if all(r["status"] == "answered" for r in records) else "completed-with-failures",
                   "quality_claim": False, "physical_adoption": "unknown"})
    finally:
        if process is not None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        if mock is not None:
            mock.shutdown()
            mock.server_close()
        # Exclude mutable SQLite WAL files; canonical JSON contains all scored observations.
        hashes = {p.name: digest(p.read_bytes()) for p in sorted(args.out.iterdir()) if p.is_file() and p.suffix in (".json", ".log")}
        write_json(args.out / "checksums.json", hashes)
    print(f"Artifacts written to: {args.out.name}")
    if any(r["status"] != "answered" for r in records):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
