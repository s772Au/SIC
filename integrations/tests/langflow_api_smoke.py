"""Import actual flow JSON and execute the read-only chain on a local Langflow server."""
import json
import os
from pathlib import Path
import sys
from urllib.request import Request, build_opener, ProxyHandler
from urllib.error import HTTPError

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from sic_workflow.client import SicClient

opener = build_opener(ProxyHandler({}))
base = os.environ.get("LANGFLOW_URL", "http://127.0.0.1:17860").rstrip("/")
headers = {"Content-Type": "application/json"}


def request(path, method="GET", body=None):
    data = None if body is None else json.dumps(body).encode()
    try:
        with opener.open(Request(base + path, data=data, method=method, headers=headers), timeout=90) as response:
            return json.load(response)
    except HTTPError as exc:
        raise RuntimeError(f"Langflow HTTP {exc.code}: {exc.read(2048).decode(errors='replace')}") from None


def main():
    # The dedicated acceptance instance explicitly enables local auto-login.
    # Other instances can set LANGFLOW_ACCESS_TOKEN instead.
    token = os.environ.get("LANGFLOW_ACCESS_TOKEN")
    if not token:
        token = request("/api/v1/auto_login")["access_token"]
    headers["Authorization"] = "Bearer " + token
    temporary_key = request("/api/v1/api_key/", "POST", {"name": "sic-offline-acceptance"})
    headers["x-api-key"] = temporary_key["api_key"]
    assert request("/health")["status"] == "ok"
    categories = request("/api/v1/all")
    assert any("sic" == name.lower() for name in categories)
    sessions = SicClient().execute("bootstrap")["result"]["sessions"]
    session_id = sessions[0]["id"]
    imported = []
    for filename in ("observe.json", "edit-lifecycle.json", "operations.json"):
        flow = json.loads((ROOT / "flows" / filename).read_text(encoding="utf-8"))
        for node in flow["data"]["nodes"]:
            template = node["data"]["node"]["template"]
            template["context_json"]["value"] = template["context_json"]["value"].replace("REPLACE_SESSION_ID", session_id)
        result = request("/api/v1/flows/", "POST", flow)
        imported.append({"file": filename, "id": result["id"], "nodes": len(flow["data"]["nodes"])})
    result = request("/api/v1/run/" + imported[0]["id"], "POST", {
        "input_request": {"input_value": "", "input_type": "any", "output_type": "any", "output_component": "SICOperation-5"}})
    assert result.get("outputs"), "No Langflow output returned"
    request("/api/v1/api_key/" + temporary_key["id"], "DELETE")
    headers.pop("x-api-key", None)
    report = {"status": "passed", "langflow_url": base, "health": "ok", "sic_discovery": True,
              "imported": imported, "read_only_run": True, "provider_calls": 0}
    destination = ROOT.parent / ".local/integration-test-results"
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "langflow-api-result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": "passed", "imported_flows": len(imported), "read_only_run": True, "provider_calls": 0}))


if __name__ == "__main__":
    main()
