"""Shared, capability-driven adapter for the current Rust control plane."""

from sic_workflow.v2_client import CoreClient


class RustWorkbench:
    def __init__(self, client=None):
        self.client = client or CoreClient()

    def capabilities(self):
        caps = self.client.capabilities()
        if caps.get("ok") is False:
            return {**caps, "operation": "capabilities", "context": {}}
        if (str(caps.get("api_version")) != "2"
                or not str(caps.get("implementation", "")).startswith("sic-rust-core")):
            return self._error("capabilities", {}, "wrong_backend",
                               "This workbench requires the Rust Core v2 service.")
        operations = caps.get("operations")
        if not isinstance(operations, list) or any(
            not isinstance(item, dict) or not isinstance(item.get("operation"), str)
            or type(item.get("write")) is not bool or type(item.get("model")) is not bool
            for item in operations
        ):
            return self._error("capabilities", {}, "invalid_capabilities",
                               "The Rust Core returned an invalid operation catalog.")
        names = [item["operation"] for item in operations]
        if len(names) != len(set(names)):
            return self._error("capabilities", {}, "invalid_capabilities",
                               "The Rust Core advertised duplicate operations.")
        return {"ok": True, "operation": "capabilities", "context": {}, "result": caps,
                "evidence": {"source": "live-rust-core", "model_quality_verified": False}}

    def execute(self, operation, context=None, payload=None, *, allow_write=False,
                allow_model=False, allow_revoke=False):
        if context is not None and not isinstance(context, dict):
            raise ValueError("Context must be a JSON object")
        if payload is not None and not isinstance(payload, dict):
            raise ValueError("Payload must be a JSON object")
        context = dict(context or {})
        catalog = self.capabilities()
        if not catalog.get("ok"):
            return catalog
        if operation == "capabilities":
            catalog["context"] = context
            return catalog
        spec = next((item for item in catalog["result"]["operations"]
                     if item["operation"] == operation), None)
        if spec is None:
            return self._error(operation, context, "capability_missing",
                               "The connected Rust Core does not advertise this operation.")
        # This remains subject to the independent permission ceiling held by
        # the Rust host; values entered into a graph never grant host rights.
        context["allow_revoke"] = allow_revoke is True
        return self.client.execute(operation, context, payload or {},
                                   allow_write=allow_write is True,
                                   allow_model=allow_model is True)

    @staticmethod
    def _error(operation, context, code, message):
        return {"ok": False, "operation": operation, "context": context,
                "error": {"code": code, "message": message},
                "evidence": {"source": "rust-workbench"}}
