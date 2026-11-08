"""V2 core adapter. All capabilities and permission gates come from the core."""
import copy
import json
import os
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import Request, build_opener, ProxyHandler


class CoreClient:
    def __init__(self, base_url=None, timeout=120):
        self.base_url = (base_url or os.environ.get("SIC_CORE_URL", "http://127.0.0.1:18888")).rstrip("/")
        url = urlsplit(self.base_url)
        if (url.scheme != "http" or url.hostname not in {"localhost", "127.0.0.1", "::1"}
                or url.username or url.password or url.query or url.fragment or url.path not in {"", "/"}):
            raise ValueError("SIC_CORE_URL must be a local HTTP origin without credentials or path")
        self.timeout = timeout
        self.opener = build_opener(ProxyHandler({}))

    def _request(self, path, body=None):
        headers = {"Content-Type": "application/json"}
        token = os.environ.get("SIC_CORE_TOKEN", "")
        if token:
            headers["Authorization"] = "Bearer " + token
        encoded = None if body is None else json.dumps(body, ensure_ascii=False).encode("utf-8")
        req = Request(self.base_url + path, encoded, headers, method="GET" if body is None else "POST")
        try:
            with self.opener.open(req, timeout=self.timeout) as response:
                result = json.load(response)
        except HTTPError as exc:
            try:
                result = json.load(exc)
            except (ValueError, UnicodeError):
                result = {"ok": False, "error": {"code": "http_error", "message": f"HTTP {exc.code}"}}
            result["http_status"] = exc.code
        except (URLError, TimeoutError, ConnectionError):
            result = {"ok": False, "error": {"code": "core_unavailable",
                      "message": "V2 core is unavailable from this runtime. Check its local binding and private environment; this stage has not executed."}}
        # Credentials are read only here, never in node inputs or graph state.
        if token:
            result = json.loads(json.dumps(result).replace(token, "[redacted]"))
        return result

    def capabilities(self):
        return self._request("/api/v2/capabilities")

    def execute(self, operation, context=None, payload=None, *, allow_write=False, allow_model=False):
        context, payload = copy.deepcopy(context or {}), copy.deepcopy(payload or {})
        if not isinstance(context, dict) or not isinstance(payload, dict):
            raise ValueError("Context and payload must be JSON objects")
        context.setdefault("actor", os.environ.get("SIC_CORE_ACTOR", "host"))
        caps = self.capabilities()
        if caps.get("ok") is False:
            return {**caps, "operation": operation, "context": context}
        if operation == "capabilities":
            return {"ok": True, "operation": operation, "context": context, "result": caps,
                    "evidence": {"source": "sic-core-v2", "model_quality_verified": False}}
        catalog = caps.get("operations", [])
        if isinstance(catalog, dict):
            catalog = [{"operation": key, **value} for key, value in catalog.items()]
        spec = next((item for item in catalog if item.get("operation") == operation), None)
        if spec is None:
            return self._failure(operation, context, "capability_missing", "Core does not advertise this operation")
        if spec.get("write") and allow_write is not True:
            return self._failure(operation, context, "write_disabled", "Enable this action's write permission")
        if spec.get("model") and allow_model is not True:
            return self._failure(operation, context, "model_disabled", "Enable this action's model request permission")
        # Editable JSON cannot override these independent UI gates.
        context["allow_write"] = allow_write is True
        context["allow_model"] = allow_model is True
        result = self._request("/api/v2/operations", {"operation": operation, "context": context, "payload": payload})
        clean_context = {key: value for key, value in context.items() if key not in {"allow_write", "allow_model"}}
        clean_context.update({key: value for key, value in result.get("context", {}).items()
                              if key not in {"allow_write", "allow_model"}})
        if result.get("ok") and isinstance(result.get("result"), dict):
            for key in ("chain_id", "source_id", "wb_id", "job_id", "cut_id", "cb_id", "checkpoint_id", "socket_id", "request_id", "board_id", "context_id", "level"):
                if key in result["result"]:
                    clean_context[key] = result["result"][key]
            if operation.startswith("edit.candidate.") and "revision" in result["result"]:
                clean_context["candidate_revision"] = result["result"]["revision"]
            body = result['result']
            if 'job' in body:
                for key in ('context_id', 'level', 'wb_id'):
                    if key in body['job']:
                        clean_context[key] = body['job'][key]
                if 'base_cut' in body['job']:
                    clean_context['reviewed_cut'] = body['job']['base_cut']
            for key in ('base_cut', 'sequence', 'cut_seq'):
                if key in body:
                    clean_context['reviewed_cut'] = body[key]
        return {**result, "operation": operation, "context": clean_context,
                "evidence": {**result.get("evidence", {}), "source": "sic-core-v2", "capability": spec,
                             "model_quality_verified": False, "orchestration_checkpoint_is_cb": False}}

    @staticmethod
    def _failure(operation, context, code, message):
        return {"ok": False, "operation": operation, "context": context,
                "error": {"code": code, "message": message}, "evidence": {"source": "frontend-permission-gate"}}


def resolve_values(value, context):
    """Resolve exact $context.key placeholders, without evaluating expressions."""
    if isinstance(value, str) and value.startswith("$context."):
        key = value[len("$context."):]
        if key not in context:
            raise ValueError(f"Missing context binding: {key}")
        return copy.deepcopy(context[key])
    if isinstance(value, dict):
        return {key: resolve_values(item, context) for key, item in value.items()}
    if isinstance(value, list):
        return [resolve_values(item, context) for item in value]
    return value


def bind_result(envelope, bindings):
    """Explicit output bindings: context key -> path inside result, e.g. chain.id."""
    context = dict(envelope.get("context", {}))
    if envelope.get("ok"):
        for key, path in bindings.items():
            value = envelope.get("result", {})
            for segment in path.split("."):
                value = value[int(segment)] if isinstance(value, list) else value[segment]
            context[key] = copy.deepcopy(value)
    envelope["context"] = context
    return envelope
