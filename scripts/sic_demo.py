"""Small stdlib client for explicitly supplied local SIC sessions; no hidden retries."""
import argparse
import json
import os
from pathlib import Path
import urllib.request
from urllib.parse import urlsplit


class SICClient:
    def __init__(self, base_url, token):
        parsed = urlsplit(base_url)
        if parsed.scheme != "http" or parsed.hostname not in ("127.0.0.1", "localhost", "::1"):
            raise ValueError("demo client requires a loopback SIC URL")
        self.base = base_url.rstrip("/")
        self.token = token
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def call(self, method, path, body=None, headers=None, with_headers=False):
        merged = {"content-type": "application/json", "x-sic-admin-token": self.token}
        merged.update(headers or {})
        data = None if body is None else json.dumps(body, ensure_ascii=False).encode()
        request = urllib.request.Request(self.base + path, data, merged, method=method)
        with self.opener.open(request, timeout=330) as response:
            value = json.load(response)
            if with_headers:
                return value, {key.lower(): value for key, value in response.headers.items()}
            return value

    def ingest(self, session, units):
        """One WB -> one model response -> validated CB -> ordered publication.

        Errors stop immediately. Previously created WB/draft/job stay available for
        inspection; rerunning does not silently reuse or overwrite them.
        """
        block = self.call("POST", f"/api/v1/sessions/{session}/blocks",
                          {"source": "".join(units), "units": units})
        draft = self.call("POST", "/api/v1/drafts", {"block_id": block["id"]})
        prefix = f"/api/v1/drafts/{draft['id']}"
        preflight = self.call("GET", prefix + "/single-pass/preflight")
        edited = self.call("POST", prefix + "/single-pass")
        validation = self.call("POST", prefix + "/validate")
        if not validation["valid"]:
            raise RuntimeError("draft validation failed")
        committed = self.call("POST", prefix + "/commit", headers={
            "x-sic-cb-sha256": validation["cb_sha256"]})
        published = self.call("POST", f"/api/v1/commits/{committed['id']}/publish")
        return {"block": block, "draft": draft, "preflight": preflight,
                "edit": edited, "commit": committed, "publication": published}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", default="http://127.0.0.1:8787")
    parser.add_argument("--session", required=True, help="existing session ID")
    parser.add_argument("--units", required=True, type=Path, help="JSON array of source strings")
    parser.add_argument("--allow-model-request", action="store_true")
    args = parser.parse_args()
    if not args.allow_model_request:
        parser.error("--allow-model-request is required: ingest invokes the configured compressor")
    units = json.loads(args.units.read_text(encoding="utf-8"))
    if not isinstance(units, list) or not units or not all(isinstance(x, str) for x in units):
        parser.error("units must be a non-empty JSON string array")
    result = SICClient(args.base_url, os.environ["SIC_ADMIN_TOKEN"]).ingest(args.session, units)
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
