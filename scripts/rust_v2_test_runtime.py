"""Isolated Rust v2 service and bounded loopback fixture for dataset tests.

The service is always a child process with a new database. The mock provider
is owned by this process; real provider credentials remain in environment
variables and are never passed on the command line or written to artifacts.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import threading
import time
from urllib.error import HTTPError
from urllib.request import ProxyHandler, Request, build_opener


def sha256_file(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        for block in iter(lambda: source.read(8 * 1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


class CoreError(RuntimeError):
    def __init__(self, code, message, status=None):
        self.code = code
        self.status = status
        super().__init__(f"{code}: {message}")


class RustClient:
    """Authenticated local v2 transport; only the host supplies permissions."""

    def __init__(self, origin, token, actor="benchmark", timeout=180):
        if not re.fullmatch(r"http://127\.0\.0\.1:[0-9]+", origin):
            raise ValueError("Rust Core must use an owned IPv4 loopback origin")
        self.origin, self._token, self.actor, self.timeout = origin, token, actor, timeout
        self.opener = build_opener(ProxyHandler({}))

    def request(self, path, body=None):
        raw = None if body is None else json.dumps(body, ensure_ascii=False).encode("utf-8")
        if raw is not None and len(raw) > 16 * 1024 * 1024:
            raise ValueError("Rust Core request exceeds its 16 MiB HTTP body limit; split the source")
        request = Request(self.origin + path, data=raw, method="GET" if raw is None else "POST",
                          headers={"Authorization": "Bearer " + self._token,
                                   "Content-Type": "application/json"})
        try:
            with self.opener.open(request, timeout=self.timeout) as response:
                value = json.load(response)
        except HTTPError as exc:
            try:
                value = json.load(exc)
            except (ValueError, UnicodeError):
                value = {"error": {"code": "http_error", "message": "HTTP " + str(exc.code)}}
            error = value.get("error", {})
            raise CoreError(error.get("code", "http_error"),
                            str(error.get("message", "request failed")).replace(self._token, "[redacted]"),
                            exc.code) from None
        if isinstance(value, dict) and value.get("ok") is False:
            error = value.get("error", {})
            raise CoreError(error.get("code", "core_error"),
                            str(error.get("message", "request failed")).replace(self._token, "[redacted]"))
        return value

    def call(self, operation, payload=None, chain_id=None, allow_model=False):
        context = {"actor": self.actor, "allow_write": True, "allow_model": allow_model,
                   "allow_revoke": False}
        if chain_id is not None:
            context["chain_id"] = chain_id
        value = self.request("/api/v2/operations", {"operation": operation, "context": context,
                                                    "payload": payload or {}})
        if not isinstance(value.get("result"), dict):
            raise CoreError("invalid_response", "Core returned no result object")
        return value["result"]


def run_bounded_worker(client, chain_id, budget, max_calls):
    """Continue built-in Core tool results with a strict host-side call bound."""
    if type(max_calls) is not int or max_calls < 1:
        raise ValueError("Worker call limit must be a positive integer")
    attempts = []
    for _ in range(max_calls):
        budget.take()
        result = client.call("worker.run", {}, chain_id, True)
        if not result.get("attempt_id"):
            raise ValueError("Worker result has no persisted attempt")
        attempts.append(result["attempt_id"])
        if result.get("host_tasks"):
            raise ValueError("Worker requested external host tools; this benchmark has no host-tool executor")
        if not result.get("tool_calls"):
            if isinstance(result.get("content"), str) and result["content"].strip():
                return result, attempts
            raise ValueError("Worker returned neither a text answer nor tool calls")
    raise ValueError("Worker tool loop exceeded its explicit call limit")


def maintain_hierarchy(client, chain_id, fanout):
    """Publish deterministic higher CBs over complete consecutive batches."""
    if fanout == 0:
        return []
    if type(fanout) is not int or not 2 <= fanout <= 64:
        raise ValueError("hierarchy fanout must be 0 or an integer from 2 to 64")
    actions = []
    for lower_level in range(1, 128):
        levels = client.call("chain.get", chain_id=chain_id)["contexts"]["main"]["levels"]
        lower = levels.get(str(lower_level))
        if not lower:
            break
        upper = levels.get(str(lower_level + 1))
        rows, offset = [], 0
        if upper:
            while True:
                page = client.call("hierarchy.timeline", {"level": lower_level + 1, "kind": "wb",
                                   "offset": offset, "limit": 100}, chain_id)
                rows.extend(page["rows"])
                if page["next_offset"] is None:
                    break
                if page["next_offset"] <= offset:
                    raise ValueError("hierarchy timeline pagination did not advance")
                offset = page["next_offset"]
        if any(row.get("cb_id") is None and row.get("superseded_by") is None for row in rows):
            raise ValueError("unpaired higher WB requires explicit recovery")
        consumed = {member for row in rows for member in (row.get("members") or [])}
        remaining = [identifier for identifier in lower["cbs"] if identifier not in consumed]
        while len(remaining) >= fanout:
            members, remaining = remaining[:fanout], remaining[fanout:]
            formed = client.call("hierarchy.form", {"level": lower_level + 1, "cb_ids": members}, chain_id)
            job = client.call("edit.program", {"source_id": formed["source_id"],
                              "rules": ["deduplicate_exact"]}, chain_id)
            material = client.call("edit.validate", {"job_id": job["job_id"]}, chain_id)["material"]
            seq = client.call("chain.get", chain_id=chain_id)["cut_seq"]
            cut = client.call("cut.publish", {"job_id": job["job_id"], "expected_cut": seq,
                              "idempotency_key": "hierarchy-" + formed["wb_id"]}, chain_id)
            actions.append({"origin": "hierarchy-program", "level": lower_level + 1,
                            "job_id": job["job_id"], "wb_id": formed["wb_id"], "members": members,
                            "source_text": "\n".join(unit["text"] for unit in job["editable_units"]),
                            "candidate": material, "publication": {"cb_id": cut["cb_id"]}})
        levels = client.call("chain.get", chain_id=chain_id)["contexts"]["main"]["levels"]
        if not levels.get(str(lower_level + 1), {}).get("cbs"):
            break
    if actions:
        client.call("hierarchy.select", {"level": max(action["level"] for action in actions),
                    "roles": ["worker", "editor"]}, chain_id)
    return actions


class MockProvider:
    """A deterministic local provider. Outputs must never be treated as scores."""

    def __init__(self, capture_bodies=False, worker_tool_once=False):
        self.requests = []
        self.capture_bodies = capture_bodies
        self.worker_tool_once = worker_tool_once
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                try:
                    request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                    try:
                        last = json.loads(request["messages"][-1]["content"])
                        role_material = last.get("sic_role_material", {})
                    except (ValueError, TypeError, KeyError, IndexError, AttributeError):
                        role_material = {}
                    owner.requests.append(request if owner.capture_bodies else {
                        "model": request.get("model"),
                        "kind": "editor" if "current_new_wb_editable_units" in role_material else "worker_or_judge"})
                    if "current_new_wb_editable_units" in role_material:
                        message = {"role": "assistant", "content": None, "tool_calls": [{
                            "id": "fixture-edit", "type": "function", "function": {
                                "name": "sic_edit_blocks", "arguments": '{"ops":[],"pvi":{}}'}}]}
                        finish = "tool_calls"
                    else:
                        system = request.get("messages", [{}])[0].get("content", "")
                        is_judge = isinstance(system, str) and system.startswith("SIC fixed judge v1.")
                        if owner.worker_tool_once and not is_judge:
                            owner.worker_tool_once = False
                            message = {"role": "assistant", "content": None, "tool_calls": [{
                                "id": "fixture-memory", "type": "function", "function": {
                                    "name": "sic_memory_search", "arguments": '{"query":"fixture","scope":"pending","level":1}'}}]}
                            finish = "tool_calls"
                        else:
                            content = '{"score":0,"reason":"fixture only"}' if is_judge else "MOCK answer: plumbing only"
                            message, finish = {"role": "assistant", "content": content}, "stop"
                    response = {"model": "sic-benchmark-fixture", "choices": [{
                        "finish_reason": finish, "message": message}],
                        "usage": {"prompt_tokens": 10, "completion_tokens": 2}}
                    raw = json.dumps(response).encode("utf-8")
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(raw)))
                    self.end_headers()
                    self.wfile.write(raw)
                except Exception:
                    self.send_error(500, "fixture request failed")

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    @property
    def base(self):
        return f"http://127.0.0.1:{self.server.server_port}/v1"

    def __exit__(self, *_args):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


class OwnedRustCore:
    """Start and stop only an isolated, newly initialized Rust Core process."""

    REQUIRED = {"chain.create", "chain.get", "config.update", "source.ingest", "wb.form",
                "edit.prepare", "edit.run", "edit.program", "cut.publish", "worker.run",
                "accounting.get", "experiment.pack", "experiment.verify"}

    def __init__(self, binary, directory, model_config, extra_env=None):
        self.binary = Path(binary).resolve(strict=True)
        self.directory = Path(directory).resolve(strict=True)
        self.model_config = Path(model_config).resolve(strict=True)
        self.extra_env = dict(extra_env or {})
        self.process = self.log = None

    def __enter__(self):
        token = secrets.token_urlsafe(32)
        env = os.environ.copy()
        env.update(self.extra_env)
        env["SIC_EVAL_BEARER"] = token
        env["NO_PROXY"] = "localhost,127.0.0.1,::1"
        db = self.directory / "rust-core.sqlite"
        if db.exists():
            raise ValueError("refusing to reuse an existing experiment database")
        flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
        init = subprocess.run([str(self.binary), "init", str(db)], env=env, capture_output=True,
                              text=True, encoding="utf-8", errors="replace", timeout=60,
                              creationflags=flags)
        if init.returncode:
            raise RuntimeError("Rust Core init failed: " + init.stderr[-1000:].replace(token, "[redacted]"))
        log_path = self.directory / "rust-core.log"
        self.log = log_path.open("w", encoding="utf-8")
        command = [str(self.binary), "serve", str(db), "127.0.0.1:0", "benchmark", "SIC_EVAL_BEARER",
                   "--allow-model", "--model-config", str(self.model_config)]
        try:
            self.process = subprocess.Popen(command, env=env, stdout=self.log, stderr=subprocess.STDOUT,
                                            creationflags=flags)
            deadline = time.monotonic() + 30
            match = None
            while time.monotonic() < deadline:
                if self.process.poll() is not None:
                    raise RuntimeError("isolated Rust Core exited during startup; inspect rust-core.log")
                text = log_path.read_text(encoding="utf-8", errors="replace")
                match = re.search(r"SIC Rust core listening on (127\.0\.0\.1:[0-9]+)", text)
                if match:
                    break
                time.sleep(0.05)
            if not match:
                raise RuntimeError("isolated Rust Core did not announce a listening address")
            self.client = RustClient("http://" + match.group(1), token)
            health = self.client.request("/health")
            if health.get("service") != "sic-rust-core":
                raise RuntimeError("unexpected Core health response")
            caps = self.client.request("/api/v2/capabilities")
            available = {item.get("operation") for item in caps.get("operations", [])}
            if self.REQUIRED - available:
                raise RuntimeError("Rust Core is missing required operations: " +
                                   ", ".join(sorted(self.REQUIRED - available)))
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def __exit__(self, *_args):
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=10)
        if self.log is not None:
            self.log.close()


def mock_profile(base):
    profile = {"model": "openai/sic-benchmark-fixture", "api_base": base,
               "api_key_env": "SIC_RUST_EVAL_MOCK_KEY", "tokenizer_model": "host-o200k-surrogate",
               "tokenizer_status": "fixture-surrogate-not-provider-native", "context_window": 1000000,
               "max_input_tokens": 900000, "max_output_tokens": 4096,
               "default_output_tokens": 256, "timeout_seconds": 30}
    return {"roles": {"worker": profile, "editor": profile}}
