"""Exercise both official adapters against an isolated real Rust Core."""

import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch
from uuid import uuid4

from langgraph.checkpoint.memory import InMemorySaver
from langgraph.types import Command
from lfx.custom.utils import build_custom_component_template
from components.sic.accounting import SICAccounting
from components.sic.rust_workbench import SICRustWorkbench
from sic_workflow.accounting_graph import graph as accounting_graph
from sic_workflow.rust_graph import builder
from sic_workflow.rust_workbench import RustWorkbench
from sic_workflow.v2_client import CoreClient


class WrongBackend(unittest.TestCase):
    def test_python_backend_is_not_mistaken_for_rust(self):
        class PythonCore:
            def capabilities(self):
                return {"api_version": "2", "implementation": "sic-python-core", "operations": []}
        result = RustWorkbench(PythonCore()).capabilities()
        self.assertFalse(result["ok"])
        self.assertEqual(result["error"]["code"], "wrong_backend")

    def test_rust_langflow_source_form_only_offers_supported_protocol(self):
        import json
        flow = json.loads((Path(__file__).resolve().parents[1] / "flows" / "workbench-rust.json")
                          .read_text(encoding="utf-8"))
        native_format = flow["data"]["nodes"][1]["data"]["node"]["template"]["native_format"]
        self.assertEqual(native_format["options"], ["openai-chat"])


@unittest.skipUnless(os.environ.get("SIC_RUST_TEST_BINARY"), "set SIC_RUST_TEST_BINARY to a built Rust Core")
class RustFrontendIntegration(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix="sic-rust-frontends-")
        cls.saved = {key: os.environ.get(key) for key in ("SIC_CORE_URL", "SIC_CORE_TOKEN", "SIC_CORE_ACTOR")}
        binary = os.environ["SIC_RUST_TEST_BINARY"]
        database = str(Path(cls.directory.name) / "core.db")
        flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
        subprocess.run([binary, "init", database], check=True, capture_output=True, creationflags=flags)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        os.environ.update(SIC_CORE_URL=f"http://127.0.0.1:{port}",
                          SIC_CORE_TOKEN="test-only-" + uuid4().hex + uuid4().hex,
                          SIC_CORE_ACTOR="host")
        cls.process = subprocess.Popen([binary, "serve", database, f"127.0.0.1:{port}", "host", "SIC_CORE_TOKEN"],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=flags)
        for _ in range(100):
            if RustWorkbench().capabilities().get("ok"):
                return
            if cls.process.poll() is not None:
                break
            time.sleep(0.1)
        cls.tearDownClass()
        raise RuntimeError("isolated Rust Core did not start")

    @classmethod
    def tearDownClass(cls):
        cls.process.terminate()
        try:
            cls.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            cls.process.kill()
            cls.process.wait(timeout=10)
        for key, value in cls.saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        cls.directory.cleanup()

    def langflow(self, operation, context=None, payload=None, *, allow_write=False):
        import json
        component = SICRustWorkbench()
        component.set(operation_name=operation, group="全部", chain_id=(context or {}).get("chain_id", ""),
                      context_json=json.dumps(context or {}), payload_json=json.dumps(payload or {}),
                      allow_write=allow_write, allow_model=False, allow_revoke=False)
        return component, component.run_action().data

    def studio(self, operation, chain_id="", payload=None, *, allow_write=False):
        graph = builder.compile(checkpointer=InMemorySaver())
        config = {"configurable": {"thread_id": uuid4().hex}}
        state = graph.invoke({"operation": operation, "chain_id": chain_id, "payload": payload or {},
                              "allow_write": allow_write, "allow_model": False, "allow_revoke": False}, config)
        if "__interrupt__" in state:
            state = graph.invoke(Command(resume=state["__interrupt__"][0].value["form"]), config)
        return state["result"]

    def test_live_catalog_and_authentication_failure(self):
        caps = RustWorkbench().capabilities()
        self.assertTrue(caps["ok"], caps)
        ops = {item["operation"]: item for item in caps["result"]["operations"]}
        self.assertIn("signal.status", ops)
        self.assertIn("payload", ops["source.text"])
        with patch.dict(os.environ, {"SIC_CORE_TOKEN": "not-the-token"}):
            result = RustWorkbench().execute("chain.get")
        self.assertFalse(result["ok"])
        self.assertEqual(result["error"]["code"], "unauthorized")
        self.assertEqual(result["http_status"], 401)

    def test_langflow_refreshes_dropdown_and_parameter_hints_from_rust(self):
        component = SICRustWorkbench()
        schema, _ = build_custom_component_template(component)
        config = component.update_build_config(schema["template"], "signal", "group")
        self.assertIn("signal.status", config["operation_name"]["options"])
        config["group"]["value"] = "signal"
        config = component.update_build_config(config, "signal.status", "operation_name")
        self.assertIn("route_id", config["payload_json"]["info"])

    def test_langflow_and_studio_share_real_rust_chain(self):
        _, denied = self.langflow("chain.create")
        self.assertEqual(denied["error"]["code"], "write_disabled")
        component, created = self.langflow("chain.create", payload={}, allow_write=True)
        self.assertTrue(created["ok"], created)
        # Rendering the catalog output must not repeat the create operation.
        component.catalog()
        chain_id = created["context"]["chain_id"]
        ingested = self.studio("source.ingest", chain_id,
                               {"format": "openai-chat", "request": {"messages": [
                                   {"role": "user", "content": "Shared Rust frontend evidence"}]}}, allow_write=True)
        self.assertTrue(ingested["ok"], ingested)
        _, prepared = self.langflow("edit.program", ingested["context"],
                                    {"source_id": ingested["context"]["source_id"], "rules": ["deduplicate_exact"]},
                                    allow_write=True)
        self.assertTrue(prepared["ok"], prepared)
        job = prepared["context"]["job_id"]
        validated = self.studio("edit.validate", chain_id, {"job_id": job})
        self.assertTrue(validated["ok"], validated)
        published = self.studio("cut.publish", chain_id,
                                {"job_id": job, "expected_cut": 0, "idempotency_key": uuid4().hex},
                                allow_write=True)
        self.assertTrue(published["ok"], published)
        _, read = self.langflow("cut.get", {"chain_id": chain_id})
        self.assertTrue(read["ok"], read)
        self.assertEqual(read["result"]["sequence"], 1)
        self.assertIn("Shared Rust frontend evidence", str(read["result"]["view"]))
        accounting = SICAccounting()
        accounting.set(chain_id=chain_id, scope="chain")
        ledger = accounting.run_accounting().data
        self.assertTrue(ledger["ok"], ledger)
        self.assertEqual(ledger["operation"], "accounting.get")
        self.assertIn("stats", ledger["result"])
        accounting.scope_details()
        accounting.timing_details()
        accounting.unknowns()
        studio_ledger = accounting_graph.invoke({"chain_id": chain_id, "action": "ledger"})
        self.assertEqual(studio_ledger["status"], "readonly")
        self.assertEqual(studio_ledger["result"]["operation"], "accounting.get")
        unsupported = SICAccounting()
        unsupported.set(chain_id=chain_id, scope="task")
        self.assertEqual(unsupported.run_accounting().data["error"]["code"], "capability_missing")


if __name__ == "__main__":
    unittest.main()
