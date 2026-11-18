"""Check official Langflow and LangGraph services against their shared Rust Core.

Uses a caller-supplied private Langflow login file. Creates an isolated test
flow and chain, never requests a model, and removes the test flow and API key.
"""
import argparse
import copy
import json
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import ProxyHandler, Request, build_opener


ROOT = Path(__file__).resolve().parents[1]


def extract_result(output):
    stage = output["outputs"][0]["outputs"][0]
    leaf = (stage.get("results", {}).get("result") or stage.get("outputs", {}).get("result")
            or stage.get("artifacts", {}).get("result", {}).get("raw"))
    if leaf is None:
        raise AssertionError("The official Langflow run returned no SIC result")
    leaf = leaf.get("data", leaf.get("message", leaf)) if isinstance(leaf, dict) else leaf
    return json.loads(leaf) if isinstance(leaf, str) else leaf


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--langflow-url", required=True)
    parser.add_argument("--langgraph-url", required=True)
    parser.add_argument("--auth-file", required=True)
    args = parser.parse_args()
    lf, lg = args.langflow_url.rstrip("/"), args.langgraph_url.rstrip("/")
    if not lf.startswith("http://127.0.0.1:") or not lg.startswith("http://127.0.0.1:"):
        raise ValueError("This test is limited to explicit IPv4 loopback origins")
    opener = build_opener(ProxyHandler({}))
    private = json.loads(Path(args.auth_file).read_text(encoding="utf-8-sig"))
    login = Request(lf + "/api/v1/login",
                    urlencode({"username": private["username"], "password": private["password"]}).encode(),
                    {"Content-Type": "application/x-www-form-urlencoded"}, method="POST")
    with opener.open(login, timeout=30) as response:
        token = json.load(response)["access_token"]
    lf_headers = {"Content-Type": "application/json", "Authorization": "Bearer " + token}

    def request(base, path, body=None, method=None):
        headers = lf_headers if base == lf else {"Content-Type": "application/json"}
        data = None if body is None else json.dumps(body, ensure_ascii=False).encode("utf-8")
        try:
            with opener.open(Request(base + path, data, headers, method=method or ("GET" if data is None else "POST")),
                             timeout=180) as response:
                raw = response.read()
                return json.loads(raw) if raw else None
        except HTTPError as error:
            detail = error.read().decode("utf-8", "replace")
            for secret in (token, lf_headers.get("x-api-key", "")):
                if secret:
                    detail = detail.replace(secret, "[redacted]")
            raise RuntimeError(f"Official runtime HTTP {error.code}: {detail}") from None

    key = request(lf, "/api/v1/api_key/", {"name": "sic-rust-workbench-acceptance"})
    lf_headers["x-api-key"] = key["api_key"]
    flow_id = None
    try:
        flow = json.loads((ROOT / "flows" / "workbench-rust.json").read_text(encoding="utf-8"))
        flow["name"] = "SIC Rust isolated official API acceptance"
        flow_id = request(lf, "/api/v1/flows/", flow)["id"]

        def run_flow(component="SICRustWorkbench-1"):
            output = request(lf, "/api/v1/run/" + flow_id, {"input_request": {
                "input_value": "", "input_type": "any", "output_type": "any",
                "output_component": component}})
            return extract_result(output)

        def update_node(index, values):
            saved = request(lf, "/api/v1/flows/" + flow_id)
            data = copy.deepcopy(saved["data"])
            template = data["nodes"][index]["data"]["node"]["template"]
            for name, value in values.items():
                if name == "operation_name" and value not in template[name]["options"]:
                    template[name]["options"].append(value)
                template[name]["value"] = value
            request(lf, "/api/v1/flows/" + flow_id, {"data": data}, "PATCH")

        def run_graph(operation, chain_id="", payload=None, write=False):
            thread = request(lg, "/threads", {})["thread_id"]
            path = "/threads/" + thread + "/runs/wait"
            state = request(lg, path, {"assistant_id": "sic_rust", "input": {
                "operation": operation, "chain_id": chain_id, "payload": payload or {}, "allow_write": write}})
            if write:
                assert state.get("__interrupt__"), state
            if state.get("__interrupt__"):
                form = state["__interrupt__"][0]["value"]["form"]
                state = request(lg, path, {"assistant_id": "sic_rust", "command": {"resume": form}})
            assert state["status"] == "completed" and state["result"]["ok"], state
            return state["result"]

        caps = run_flow()
        assert caps["ok"] and caps["result"]["implementation"].startswith("sic-rust-core"), caps
        operations = caps["result"]["operations"]
        assert any(item["operation"] == "signal.status" and "payload" in item for item in operations)

        assistants = request(lg, "/assistants/search", {})
        assert "sic_rust" in {item["graph_id"] for item in assistants}
        created = run_graph("chain.create", write=True)
        chain_id = created["context"]["chain_id"]

        update_node(0, {"operation_name": "chain.get", "chain_id": chain_id})
        read = run_flow()
        assert read["ok"] and read["context"]["chain_id"] == chain_id, read
        update_node(1, {"chain_id": chain_id, "working_text": "Official shared Rust workbench evidence", "allow_write": True})
        source = run_flow("SICNativeInput-2")
        assert source["ok"], source
        source_id = source["context"]["source_id"]
        prepared = run_graph("edit.program", chain_id, {"source_id": source_id, "rules": ["deduplicate_exact"]}, True)
        job_id = prepared["context"]["job_id"]
        run_graph("edit.validate", chain_id, {"job_id": job_id})
        run_graph("cut.publish", chain_id, {"job_id": job_id, "expected_cut": 0, "idempotency_key": job_id}, True)
        update_node(2, {"chain_id": chain_id, "source_id": source_id})
        workspace = run_flow("SICWorkspace-3")
        assert workspace["ok"] and "Official shared Rust workbench evidence" in json.dumps(workspace["result"]), workspace
        update_node(3, {"chain_id": chain_id})
        ledger = run_flow("SICAccounting-4")
        assert ledger["ok"] and ledger["operation"] == "accounting.get" and "stats" in ledger["result"], ledger
        studio_ledger = request(lg, "/runs/wait", {"assistant_id": "sic_accounting",
                                                    "input": {"chain_id": chain_id, "action": "ledger"}})
        assert studio_ledger["status"] == "readonly" and studio_ledger["result"]["operation"] == "accounting.get", studio_ledger
        print(json.dumps({"status": "passed", "langflow_rust_operations": len(operations),
                          "langgraph_rust_graph_registered": True,
                          "langgraph_write_reviewed_and_created": True,
                          "langflow_read_same_chain": True, "langflow_typed_source": True,
                          "langgraph_published": True, "langflow_workspace_read": True,
                          "both_read_rust_accounting": True, "model_requests": 0}))
    finally:
        if flow_id:
            request(lf, "/api/v1/flows/" + flow_id, method="DELETE")
        request(lf, "/api/v1/api_key/" + key["id"], method="DELETE")


if __name__ == "__main__":
    main()
