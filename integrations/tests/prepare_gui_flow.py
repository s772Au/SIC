"""Prepare a fresh, isolated four-node GUI acceptance flow; never runs it."""
import json
from pathlib import Path
import sys
from urllib.request import Request, build_opener, ProxyHandler

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from generate_core_flows import make_core_flow
from sic_workflow.runtime_auth import langflow_access_token
from sic_workflow.v2_client import CoreClient


def main():
    client = CoreClient()
    context = client.execute("chain.create", payload={"creators": []}, allow_write=True)["context"]
    source = client.execute("source.ingest", context, {"format": "openai-chat", "request": {"messages": [
        {"role": "user", "content": "GUI isolated source: edit this working material"},
        {"role": "user", "content": "Unmentioned second unit remains KEEP"}]}}, allow_write=True)
    prepared = client.execute("edit.prepare", source["context"], {"source_id": source["context"]["source_id"]}, allow_write=True)
    assert prepared["ok"], prepared
    context = prepared["context"]
    actions = [
        {"label": "1 Edit op and P/V/I proposal (model OFF)", "operation": "edit.submit", "context": context,
         "payload": {"job_id": "$context.job_id", "editor": "editor-1", "ops": [{"op": "REPLACE", "target": "u1", "content": "GUI before edit"}],
                     "pvi": {"P": ["GUI persistent before"], "V": ["GUI volatile before"], "I": {"operator_note": "GUI instant before"}}}},
        {"label": "2 Validate candidate / scope / PVI", "operation": "edit.validate", "payload": {"job_id": "$context.job_id"}},
        {"label": "3 Publish once — immutable CB + op + cut", "operation": "cut.publish", "payload": {"job_id": "$context.job_id", "expected_cut": 0, "idempotency_key": "$context.job_id"}},
        {"label": "4 Read-only persisted CB / op / PVI / indices", "operation": "cut.get"},
    ]
    flow = make_core_flow("SIC Core — GUI isolated op/PVI acceptance", actions)
    opener = build_opener(ProxyHandler({}))
    base = "http://127.0.0.1:17861"
    token = langflow_access_token(base, opener)
    request = Request(base + "/api/v1/flows/", json.dumps(flow, ensure_ascii=False).encode(),
                      {"Content-Type": "application/json", "Authorization": "Bearer " + token}, method="POST")
    with opener.open(request, timeout=60) as response:
        result = json.load(response)
    metadata = {"url": base + "/flow/" + result["id"], "flow_id": result["id"], "context": context,
                "source_id": source["context"]["source_id"], "model_requests": 0, "writes_default_off": True,
                "expected_cut": 0, "isolated_from_dsh": True}
    destination = ROOT.parent / ".local/integration-test-results/gui-acceptance-flow.json"
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(metadata, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({"url": metadata["url"], "nodes": 4, "model_requests": 0, "write_gates": "off"}))


if __name__ == "__main__":
    main()
