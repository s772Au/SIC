import io
import json
import os
from unittest import TestCase, main
from unittest.mock import patch

from sic_workflow.client import SicClient


class ClientContract(TestCase):
    def test_credentials_never_go_to_read_routes_or_result(self):
        with patch.dict(os.environ, {"SIC_ADMIN_TOKEN": "not-exportable"}):
            client = SicClient()
            with patch.object(client.opener, "open", return_value=io.BytesIO(b'{"ok":true}')) as request:
                result = client.execute("projection", {"session_id": "s"})
                self.assertNotIn("X-sic-admin-token", request.call_args.args[0].headers)
                self.assertNotIn("not-exportable", json.dumps(result))

    def test_no_write_without_explicit_gate(self):
        client = SicClient()
        with patch.object(client.opener, "open") as request:
            with self.assertRaises(ValueError):
                client.execute("session_create", payload={"name": "test"})
            request.assert_not_called()

    def test_single_pass_requires_separate_explicit_model_gate(self):
        client = SicClient()
        with patch.object(client.opener, "open") as request:
            for value in [False, "true", 1]:
                with self.assertRaises(ValueError):
                    client.execute("draft_single_pass", {"draft_id": "d"}, allow_write=True, allow_model=value)
            request.assert_not_called()

    def test_no_arbitrary_hosts(self):
        for url in ["http://example.com", "http://127.0.0.1@evil.com", "http://localhost/secret", "file:///x"]:
            with self.assertRaises(ValueError):
                SicClient(url)

    def test_query_and_identifier_encoding(self):
        client = SicClient("http://127.0.0.1:8787")
        with patch.object(client.opener, "open", return_value=io.BytesIO(b'{}')) as request:
            client.execute("archive_search", {"session_id": "a/b"}, {"q": "x&limit=500"})
            self.assertEqual(request.call_args.args[0].full_url,
                             "http://127.0.0.1:8787/api/v1/sessions/a%2Fb/archive/search?q=x%26limit%3D500")

    def test_validated_hash_is_sent_to_commit_and_invalidated_on_edit(self):
        with patch.dict(os.environ, {"SIC_ADMIN_TOKEN": "local"}):
            client = SicClient()
            context = {"draft_id": "d", "validated_cb_sha256": "hash"}
            with patch.object(client.opener, "open", return_value=io.BytesIO(b'{"id":"c"}')) as request:
                client.execute("draft_commit", context, allow_write=True)
                self.assertEqual(request.call_args.args[0].get_header("X-sic-cb-sha256"), "hash")
            with patch.object(client.opener, "open", return_value=io.BytesIO(b'{"revision":2}')):
                result = client.execute("draft_edit", context, {"base_revision": 1, "edits": []}, allow_write=True)
                self.assertNotIn("validated_cb_sha256", result["context"])


if __name__ == "__main__":
    main()
