"""Owned loopback endpoint and HTTP transport for new-core experiments only."""
import copy
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import threading
import time
from urllib.error import HTTPError
from urllib.request import Request, build_opener, ProxyHandler
import uuid


class Transport:
    def __init__(self, origin, token):
        self.origin, self.token = origin, token
        self.context = {"actor": "host"}
        self.trace = []
        self.opener = build_opener(ProxyHandler({}))

    def call(self, operation, payload=None):
        body = {"operation": operation, "context": {**self.context, "allow_write": True, "allow_model": True}, "payload": payload or {}}
        request = Request(self.origin + "/api/v2/operations", json.dumps(body, ensure_ascii=False).encode(),
                          {"Content-Type": "application/json", "Authorization": "Bearer " + self.token}, method="POST")
        start = time.time()
        try:
            with self.opener.open(request, timeout=180) as response:
                envelope = json.load(response)
        except HTTPError as error:
            envelope = json.load(error)
        self.trace.append({"operation": operation, "payload": copy.deepcopy(payload or {}), "started_at": start,
                           "ended_at": time.time(), "response": envelope})
        if not envelope.get("ok"):
            raise RuntimeError(json.dumps(envelope).replace(self.token, "[redacted]"))
        self.context.update(envelope.get("context", {}))
        for key in ("source_id", "job_id", "cut_id", "cb_id", "chain_id"):
            if key in envelope["result"]:
                self.context[key] = envelope["result"][key]
        return envelope["result"]


class OwnedFixture:
    """Always owns both endpoint and database. Never accepts an external model URL."""
    def __init__(self, db_path, shared_model=None, editor_keep=False, research_answers=False, media_profile=False):
        self.db_path = Path(db_path)
        self.shared_model = shared_model
        self.editor_keep = editor_keep
        self.research_answers = research_answers
        self.media_profile = media_profile
        self.model_requests = shared_model.model_requests if shared_model else []
        self.saved = {}

    def __enter__(self):
        from sic_core.service import Core
        from sic_core.server import make_server
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                start = time.time()
                try:
                    editor_input = json.loads(request["messages"][-1]["content"])
                except (ValueError, KeyError, TypeError):
                    editor_input = {}
                if "editable_text_units" in editor_input:
                    response = {"ops": [{"op": "COMPRESS", "target": unit["unit_id"],
                                         "content": " ".join(unit["text"].split()[:12]),
                                         "reason": "Deterministic owned fixture; not a semantic-quality result"}
                                        for unit in editor_input["editable_text_units"]], "pvi": {"P": [], "V": [], "I": {}}}
                    if owner.editor_keep:
                        response['ops'] = []
                    content = json.dumps(response, ensure_ascii=False)
                    role = "editor"
                else:
                    content = "Owned fixture Worker completed this turn. This is not a scored answer."
                    if owner.research_answers:
                        content = "OWNED_FINAL_POST_EDIT_RESPONSE" if "SIC_RESEARCH_FINAL_QUESTION" in json.dumps(request,ensure_ascii=False) else "OWNED_INTERMEDIATE_RESPONSE"
                    role = "worker"
                # Declared synthetic usage, reproducible from captured request/response.
                prompt = max(1, (len(json.dumps(request, ensure_ascii=False)) + 3) // 4)
                completion = max(1, (len(content) + 3) // 4)
                value = {"id": "owned-experiment-fixture", "object": "chat.completion", "created": 1, "model": request["model"],
                         "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": content}}],
                         "usage": {"prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": prompt + completion,
                                   "prompt_tokens_details": {"cached_tokens": 0}, "completion_tokens_details": {"reasoning_tokens": 0}}}
                body = json.dumps(value, ensure_ascii=False).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                owner.model_requests.append({"role": role, "request": request, "response": value,
                                             "started_at": start, "ended_at": time.time()})

        keys = ("SIC_V2_FIXTURE_TOKEN", "SIC_V2_FIXTURE_KEY", "NO_PROXY", "LITELLM_LOCAL_MODEL_COST_MAP")
        self.saved = {key: os.environ.get(key) for key in keys}
        os.environ["SIC_V2_FIXTURE_TOKEN"] = uuid.uuid4().hex
        os.environ["SIC_V2_FIXTURE_KEY"] = uuid.uuid4().hex
        os.environ["NO_PROXY"] = "localhost,127.0.0.1,::1"
        os.environ["LITELLM_LOCAL_MODEL_COST_MAP"] = "true"
        if self.shared_model:
            self.model = self.shared_model.model
        else:
            self.model = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            threading.Thread(target=self.model.serve_forever, daemon=True).start()
        self.request_start = len(self.model_requests)
        self.profile = {"model": "openai/gpt-4o" if self.media_profile else "openai/sic-experiment-fixture", "api_base": f"http://127.0.0.1:{self.model.server_port}/v1",
                        "api_key_env": "SIC_V2_FIXTURE_KEY", "route_id": "owned-fixture-v1", "tokenizer_model": "gpt-4o", "max_input_tokens": 100000,
                        "max_output_tokens": 4096, "context_window": 128000, "tokenizer_status": "fixture-tokenizer-not-provider-verified"}
        config = {"tokens": {"SIC_V2_FIXTURE_TOKEN": "host"}, "roles": {"worker": self.profile, "editor": self.profile}, "profiles": {}}
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        self.core = Core(str(self.db_path), config)
        self.server = make_server(self.core, config, port=0)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.client = Transport(f"http://127.0.0.1:{self.server.server_port}", os.environ["SIC_V2_FIXTURE_TOKEN"])
        return self

    def __exit__(self, *args):
        if args and args[0] is not None:
            # Keep failure calls/artifacts; never only a final success summary.
            for name, value in (("partial-trace.json", self.client.trace),
                                ("partial-model-http.json", self.model_requests[self.request_start:])):
                (self.db_path.parent / name).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        self.server.shutdown()
        self.server.server_close()
        self.core.close()
        if not self.shared_model:
            self.model.shutdown()
            self.model.server_close()
        for key, value in self.saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
