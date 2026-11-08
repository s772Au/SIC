import io
import json
import os
from unittest import TestCase
from unittest.mock import patch

from sic_workflow.v2_client import CoreClient, bind_result, resolve_values


CATALOG = {"operations": [
    {"operation": "source.read", "write": False, "model": False},
    {"operation": "edit.run", "write": True, "model": True},
    {"operation": "chain.fork", "write": True, "model": False},
]}


class CoreContract(TestCase):
    def test_context_cannot_bypass_independent_write_or_model_gate(self):
        client = CoreClient()
        with patch.object(client, "capabilities", return_value=CATALOG), patch.object(client, "_request") as request:
            for write, model, code in [(False, True, "write_disabled"), (True, False, "model_disabled"), (True, 1, "model_disabled")]:
                result = client.execute("edit.run", {"allow_write": True, "allow_model": True}, allow_write=write, allow_model=model)
                self.assertEqual(result["error"]["code"], code)
            request.assert_not_called()

    def test_fork_preserves_core_identity_and_bindings(self):
        client = CoreClient()
        response = {"ok": True, "context": {"chain_id": "peer"}, "result": {"checkpoint_id": "cp"}}
        with patch.object(client, "capabilities", return_value=CATALOG), patch.object(client, "_request", return_value=response):
            result = client.execute("chain.fork", {"chain_id": "origin"}, allow_write=True)
        self.assertEqual(result["context"]["chain_id"], "peer")
        self.assertEqual(result["context"]["checkpoint_id"], "cp")
        self.assertFalse(result["evidence"]["orchestration_checkpoint_is_cb"])

    def test_bearer_only_in_transport_and_masked_if_echoed(self):
        with patch.dict(os.environ, {"SIC_CORE_TOKEN": "private-fixture-value"}):
            client = CoreClient()
            with patch.object(client.opener, "open", return_value=io.BytesIO(b'{"error":"private-fixture-value"}')) as request:
                result = client.capabilities()
            self.assertEqual(request.call_args.args[0].get_header("Authorization"), "Bearer private-fixture-value")
            self.assertNotIn("private-fixture-value", json.dumps(result))

    def test_unknown_operation_cannot_create_mutation_escape_hatch(self):
        client = CoreClient()
        with patch.object(client, "capabilities", return_value=CATALOG), patch.object(client, "_request") as request:
            result = client.execute("cb.update", allow_write=True)
        self.assertEqual(result["error"]["code"], "capability_missing")
        request.assert_not_called()

    def test_explicit_bindings_are_data_only_and_keep_exact_types(self):
        envelope = bind_result({"ok": True, "context": {}, "result": {"view": {"epoch": 3}}}, {"epoch": "view.epoch"})
        self.assertEqual(resolve_values({"expected_cut": "$context.epoch"}, envelope["context"]), {"expected_cut": 3})
        self.assertEqual(resolve_values("print('not executed')", {}), "print('not executed')")
        with self.assertRaises(ValueError):
            resolve_values("$context.missing", {})

    def test_api_origin_is_loopback_only(self):
        for url in ["https://localhost", "http://example.org", "http://localhost/secret", "http://a@localhost"]:
            with self.assertRaises(ValueError):
                CoreClient(url)
