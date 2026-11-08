"""Import/save/read v2 flows in an existing official runtime. Never starts services.

Runtime binding and visual QA are reported separately from API persistence.
"""
import argparse
import copy
import json
import os
from pathlib import Path
import sys
from urllib.request import Request, build_opener, ProxyHandler

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from sic_workflow.runtime_auth import langflow_access_token


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--langflow-url", default="http://127.0.0.1:17861")
    parser.add_argument("--langgraph-url", default="http://127.0.0.1:12026")
    args = parser.parse_args()
    opener = build_opener(ProxyHandler({}))
    headers = {"Content-Type": "application/json"}

    def request(base, path, method="GET", body=None):
        data = None if body is None else json.dumps(body, ensure_ascii=False).encode()
        scoped_headers = headers if base == args.langflow_url else {"Content-Type": "application/json"}
        with opener.open(Request(base.rstrip("/") + path, data, scoped_headers, method=method), timeout=120) as response:
            return json.load(response)

    token = langflow_access_token(args.langflow_url, opener)
    headers["Authorization"] = "Bearer " + token
    temporary_key = request(args.langflow_url, "/api/v1/api_key/", "POST", {"name": "sic-core-local-acceptance"})
    headers["x-api-key"] = temporary_key["api_key"]
    try:
        assert request(args.langflow_url, "/health")["status"] == "ok"
        imported = []
        for path in sorted((ROOT / "flows").glob("core-*.json")):
            flow = json.loads(path.read_text(encoding="utf-8"))
            record = request(args.langflow_url, "/api/v1/flows/", "POST", flow)
            saved = request(args.langflow_url, "/api/v1/flows/" + record["id"])
            assert len(saved["data"]["nodes"]) == len(flow["data"]["nodes"])
            imported.append({"file": path.name, "id": record["id"], "nodes": len(saved["data"]["nodes"])})
        # Actual official persistence edit: only an unexecuted working-input field.
        native = next(item for item in imported if item["file"] == "core-native-to-cut.json")
        saved = request(args.langflow_url, "/api/v1/flows/" + native["id"])
        edited = copy.deepcopy(saved["data"])
        template = edited["nodes"][2]["data"]["node"]["template"]
        payload = json.loads(template["payload_json"]["value"])
        payload["request"]["messages"][-1]["content"] = "Official editor API saved this unexecuted working material"
        template["payload_json"]["value"] = json.dumps(payload)
        request(args.langflow_url, "/api/v1/flows/" + native["id"], "PATCH", {"data": edited})
        read_back = request(args.langflow_url, "/api/v1/flows/" + native["id"])
        assert read_back["data"]["nodes"][2]["data"]["node"]["template"]["payload_json"]["value"] == template["payload_json"]["value"]
        # Read-only capabilities probe never starts a service or performs a write/model call.
        from generate_core_flows import make_core_flow
        probe = make_core_flow("SIC Core — runtime binding status", [{"operation": "capabilities"}])
        record = request(args.langflow_url, "/api/v1/flows/", "POST", probe)
        output = request(args.langflow_url, "/api/v1/run/" + record["id"], "POST", {"input_request": {
            "input_value": "", "input_type": "any", "output_type": "any", "output_component": "SICAction-1"}})
        assert output.get("outputs")
        # These response shapes belong to official Langflow, not the core API.
        probe_path = ROOT.parent / ".local/integration-test-results/v2-official-probe.json"
        probe_path.parent.mkdir(parents=True, exist_ok=True)
        probe_path.write_text(json.dumps(output, ensure_ascii=False, indent=2), encoding="utf-8")
        stage_output = output["outputs"][0]["outputs"][0]
        results = stage_output.get("results", {})
        leaf = (results.get("result") or stage_output.get("outputs", {}).get("result")
                or stage_output.get("artifacts", {}).get("result", {}).get("raw"))
        if leaf is None:
            raise ValueError("Official runtime returned no SIC Action output")
        envelope = leaf.get("data", leaf.get("message", leaf)) if isinstance(leaf, dict) else leaf
        if isinstance(envelope, str):
            envelope = json.loads(envelope)
        assistants = request(args.langgraph_url, "/assistants/search", "POST", {})
        graph_ids = sorted({item["graph_id"] for item in assistants})
        report = {"status": "official-api-import-edit-read-verified", "visual_ui_verified": False,
                  "visual_ui_scope": "API-only script; independent browser verification is reported separately",
                  "imported": imported, "saved_working_material_verified": True,
                  "capabilities_probe_flow": record["id"], "runtime_capabilities_probe": envelope,
                  "v2_core_bound_to_langflow": bool(envelope.get("ok")), "langgraph_registered_graphs": graph_ids,
                  "v2_langgraph_registered": "sic_core" in graph_ids,
                  "persistent_runtime_switch_performed_by_this_test": False, "model_requests_from_this_test": 0,
                  "runtime_binding_issue": None if envelope.get("ok") and "sic_core" in graph_ids else "Current runtime is not fully bound to the v2 core"}
        destination = ROOT.parent / ".local/integration-test-results"
        destination.mkdir(parents=True, exist_ok=True)
        (destination / "v2-official-api.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({key: value for key, value in report.items() if key not in {"imported", "runtime_capabilities_probe", "capabilities_probe_flow"}}, ensure_ascii=False))
    finally:
        request(args.langflow_url, "/api/v1/api_key/" + temporary_key["id"], "DELETE")
        headers.pop("x-api-key", None)


if __name__ == "__main__":
    main()
