"""Small, non-retrying, localhost-only API adapter shared by visual runtimes."""
import json
import os
from urllib.error import HTTPError
from urllib.parse import quote, urlencode, urlsplit
from urllib.request import Request, build_opener, ProxyHandler

# name: method, route, ID key, changes state
OPERATIONS = {
    "bootstrap": ("GET", "/ui/bootstrap", None, False),
    "session_create": ("POST", "/sessions", None, True),
    "blocks": ("GET", "/sessions/{id}/blocks", "session_id", False),
    "wb_create": ("POST", "/sessions/{id}/blocks", "session_id", True),
    "timeline": ("GET", "/sessions/{id}/timeline", "session_id", False),
    "wb_source": ("GET", "/blocks/{id}/source", "block_id", False),
    "wb_units": ("GET", "/blocks/{id}/units", "block_id", False),
    "drafts": ("GET", "/blocks/{id}/drafts", "block_id", False),
    "draft_create": ("POST", "/drafts", None, True),
    "draft_read": ("GET", "/drafts/{id}", "draft_id", False),
    "draft_edit": ("PUT", "/drafts/{id}", "draft_id", True),
    "draft_validate": ("POST", "/drafts/{id}/validate", "draft_id", False),
    "preflight": ("GET", "/drafts/{id}/single-pass/preflight", "draft_id", False),
    "draft_single_pass": ("POST", "/drafts/{id}/single-pass", "draft_id", True),
    "draft_commit": ("POST", "/drafts/{id}/commit", "draft_id", True),
    "commit_read": ("GET", "/commits/{id}", "commit_id", False),
    "commit_publish": ("POST", "/commits/{id}/publish", "commit_id", True),
    "draft_jobs": ("GET", "/drafts/{id}/jobs", "draft_id", False),
    "edit_job": ("GET", "/edit-jobs/{id}", "job_id", False),
    "mount_read": ("GET", "/sessions/{id}/mount", "session_id", False),
    "mount_edit": ("PUT", "/sessions/{id}/mount", "session_id", True),
    "mount_publish": ("POST", "/sessions/{id}/mount/publish", "session_id", True),
    "mount_revisions": ("GET", "/sessions/{id}/mount/revisions", "session_id", False),
    "projection": ("GET", "/sessions/{id}/projection", "session_id", False),
    "roots": ("GET", "/sessions/{id}/roots", "session_id", False),
    "promotion_candidates": ("GET", "/sessions/{id}/promotion-candidates", "session_id", False),
    "promotion_preview": ("POST", "/sessions/{id}/promotions/preview", "session_id", False),
    "promotion_commit": ("POST", "/sessions/{id}/promotions", "session_id", True),
    "node_read": ("GET", "/nodes/{id}", "node_id", False),
    "archive_search": ("GET", "/sessions/{id}/archive/search", "session_id", False),
    "archive_read": ("GET", "/sessions/{id}/archive/{kind}/{entry_id}", "session_id", False),
    "assemble_chat": ("POST", "/sessions/{id}/worker/assemble-chat", "session_id", False),
    "assemble_responses": ("POST", "/sessions/{id}/worker/assemble-responses", "session_id", False),
    "test_catalog": ("GET", "/test-catalog", None, False),
    "test_plan_read": ("GET", "/test-plans/{id}", "plan_id", False),
    "test_plan_edit": ("PUT", "/test-plans/{id}", "plan_id", True),
    "test_plan_validate": ("POST", "/test-plans/{id}/validate", "plan_id", False),
}
CREATED_IDS = {"session_create": "session_id", "wb_create": "block_id",
               "draft_create": "draft_id", "draft_commit": "commit_id", "promotion_commit": "node_id"}


class SicError(RuntimeError):
    pass


class SicClient:
    def __init__(self, base_url=None, timeout=30):
        self.base_url = (base_url or os.environ.get("SIC_API_URL", "http://127.0.0.1:8787")).rstrip("/")
        url = urlsplit(self.base_url)
        if (url.scheme != "http" or url.hostname not in {"localhost", "127.0.0.1", "::1"}
                or url.username or url.password or url.query or url.fragment or url.path not in {"", "/"}):
            raise ValueError("SIC_API_URL must be a local HTTP origin without credentials or a path")
        self.timeout = timeout
        self.opener = build_opener(ProxyHandler({}))

    def execute(self, operation, context=None, payload=None, *, allow_write=False, allow_model=False):
        if operation not in OPERATIONS:
            raise ValueError("Unknown SIC operation")
        method, path, id_key, writes = OPERATIONS[operation]
        if writes and not allow_write:
            raise ValueError("Enable this node's Allow write field to perform this operation")
        if operation == "draft_single_pass" and allow_model is not True:
            raise ValueError("Single-pass sends a real model request; explicitly enable Allow model request")
        context, payload = dict(context or {}), dict(payload or {})
        if id_key:
            if not context.get(id_key):
                raise ValueError(f"Missing {id_key} in context")
            path = path.replace("{id}", quote(str(context[id_key]), safe=""))
        if operation == "archive_read":
            for key in ("kind", "entry_id"):
                if not context.get(key):
                    raise ValueError(f"Missing {key} in context")
                path = path.replace("{" + key + "}", quote(str(context[key]), safe=""))
        if operation == "draft_create" and "block_id" not in payload:
            payload["block_id"] = context.get("block_id")
        headers = {"Content-Type": "application/json"}
        token = os.environ.get("SIC_ADMIN_TOKEN", "")
        if writes:
            if not token:
                raise ValueError("Set SIC_ADMIN_TOKEN in the runtime environment")
            headers["x-sic-admin-token"] = token
        if operation == "draft_commit" and context.get("validated_cb_sha256"):
            headers["x-sic-cb-sha256"] = context["validated_cb_sha256"]
        # Read-only POST routes do not require an admin token.
        body = None
        if method == "GET" and payload:
            path += "?" + urlencode(payload)
        elif method != "GET":
            body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        req = Request(self.base_url + "/api/v1" + path, data=body, method=method, headers=headers)
        try:
            with self.opener.open(req, timeout=self.timeout) as response:
                result = json.load(response)
        except HTTPError as exc:
            # Never include request headers or credential values in exported traces.
            detail = exc.read(2048).decode("utf-8", errors="replace")
            if token:
                detail = detail.replace(token, "[redacted]")
            raise SicError(f"SIC {operation}: HTTP {exc.code}: {detail}") from None
        if operation in CREATED_IDS:
            context[CREATED_IDS[operation]] = result["id"]
        if "revision" in result:
            context["last_revision"] = result["revision"]
        if operation == "draft_validate" and result.get("cb_sha256"):
            context["validated_cb_sha256"] = result["cb_sha256"]
        elif operation in {"draft_create", "draft_edit", "draft_single_pass"}:
            context.pop("validated_cb_sha256", None)
        if operation == "draft_single_pass" and result.get("job_id"):
            context["job_id"] = result["job_id"]
            if isinstance(result.get("draft"), dict) and "revision" in result["draft"]:
                context["last_revision"] = result["draft"]["revision"]
                context["draft_revision"] = result["draft"]["revision"]
        return {"context": context, "operation": operation, "result": result,
                "evidence": {"source": "sic-rust-api", "operation_can_request_model": operation == "draft_single_pass"}}
