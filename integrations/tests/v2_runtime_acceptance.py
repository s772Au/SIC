"""Real running official Langflow + persistent LangGraph API acceptance.

No process starts, no desktop interaction, no model requests. Requires the
private SIC_CORE_TOKEN in this process and both runtimes' environments.
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
from sic_workflow.v2_client import CoreClient
from sic_workflow.runtime_auth import langflow_access_token


def action_output(output):
    stage = output["outputs"][0]["outputs"][0]
    leaf = (stage.get("results", {}).get("result") or stage.get("outputs", {}).get("result")
            or stage.get("artifacts", {}).get("result", {}).get("raw"))
    if leaf is None:
        raise AssertionError("No official SIC Action result")
    leaf = leaf.get("data", leaf.get("message", leaf)) if isinstance(leaf, dict) else leaf
    return json.loads(leaf) if isinstance(leaf, str) else leaf


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--langflow-url", default="http://127.0.0.1:17861")
    parser.add_argument("--langgraph-url", default="http://127.0.0.1:12026")
    args = parser.parse_args()
    opener = build_opener(ProxyHandler({}))
    lf_headers = {"Content-Type": "application/json"}

    def request(base, path, body=None, method=None):
        headers = lf_headers if base == args.langflow_url else {"Content-Type": "application/json"}
        data = None if body is None else json.dumps(body, ensure_ascii=False).encode()
        with opener.open(Request(base + path, data, headers, method=method or ("GET" if body is None else "POST")), timeout=180) as response:
            return json.load(response)

    core = CoreClient()
    caps = core.capabilities()
    assert caps.get("api_version") == "2", caps
    assert all("fixture" in role["model"] for role in caps["adapters"]["roles"].values()), "Only own local fixture is allowed"
    token = langflow_access_token(args.langflow_url, opener)
    lf_headers["Authorization"] = "Bearer " + token
    key = request(args.langflow_url, "/api/v1/api_key/", {"name": "sic-v2-running-acceptance"})
    lf_headers["x-api-key"] = key["api_key"]
    report = {"status": "running", "real_provider_calls": 0, "model_requests_from_this_test": 0,
              "visual_ui_verified_by_this_script": False, "langflow": [], "langgraph": {}}
    try:
        for filename in ("core-native-to-cut.json", "core-editor-to-cut.json"):
            flow = json.loads((ROOT / "flows" / filename).read_text(encoding="utf-8"))
            flow["name"] += " — running API acceptance"
            for node in flow["data"]["nodes"]:
                node["data"]["node"]["template"]["allow_write"]["value"] = True
            created = request(args.langflow_url, "/api/v1/flows/", flow)
            edited = copy.deepcopy(flow["data"])
            if filename == "core-native-to-cut.json":
                template = edited["nodes"][2]["data"]["node"]["template"]
                payload = json.loads(template["payload_json"]["value"])
                payload["request"]["messages"][-1]["content"] = "Langflow saved working material marker"
            else:
                template = edited["nodes"][5]["data"]["node"]["template"]
                payload = json.loads(template["payload_json"]["value"])
                payload["ops"][0]["content"] = "Langflow reviewed op marker"
                payload["pvi"]["I"]["decision"] = "Langflow reviewed instant marker"
            template["payload_json"]["value"] = json.dumps(payload)
            request(args.langflow_url, "/api/v1/flows/" + created["id"], {"data": edited}, "PATCH")
            saved = request(args.langflow_url, "/api/v1/flows/" + created["id"])
            assert saved["data"] == edited
            count = len(edited["nodes"])
            output = request(args.langflow_url, "/api/v1/run/" + created["id"], {"input_request": {
                "input_value": "", "input_type": "any", "output_type": "any", "output_component": f"SICAction-{count}"}})
            envelope = action_output(output)
            assert envelope.get("ok"), envelope
            context = envelope["context"]
            view = core.execute("cut.get", context)["result"]
            marker = "Langflow saved working material marker" if count == 11 else "Langflow reviewed op marker"
            assert marker in view["view"]["cbs"][0]["body"]
            if count == 12:
                assert view["view"]["instant"]["decision"] == "Langflow reviewed instant marker"
            report["langflow"].append({"file": filename, "flow_id": created["id"], "chain_id": context["chain_id"],
                                       "stages": count, "saved_edit_effective": True, "cut_id": view["cut_id"]})

        registered = request(args.langgraph_url, "/assistants/search", {})
        assert "sic_core" in {item["graph_id"] for item in registered}
        actions = [
            {"operation": "chain.create", "payload": {"creators": []}, "allow_write": True},
            {"operation": "source.ingest", "payload": {"format": "openai-chat", "request": {"messages": [
                {"role": "user", "content": "Original evidence"}, {"role": "user", "content": "Second evidence"}]}}, "allow_write": True},
            {"operation": "edit.prepare", "payload": {"source_id": "$context.source_id"}, "allow_write": True},
            {"operation": "edit.submit", "payload": {"job_id": "$context.job_id", "editor": "editor-1", "ops": [
                {"op": "REPLACE", "target": "u1", "content": "Before review"}], "pvi": {"P": ["Before P"], "V": ["Before V"], "I": {"operator_note": "before"}}}, "allow_write": True},
            {"operation": "edit.validate", "payload": {"job_id": "$context.job_id"}},
            {"operation": "cut.publish", "payload": {"job_id": "$context.job_id", "expected_cut": 0, "idempotency_key": "$context.job_id"}, "allow_write": True},
            {"operation": "cut.get"}, {"operation": "provider.preview", "payload": {"format": "openai-chat"}},
        ]
        thread = request(args.langgraph_url, "/threads", {})["thread_id"]
        run_path = "/threads/" + thread + "/runs/wait"
        state = request(args.langgraph_url, run_path, {"assistant_id": "sic_core", "input": {"actions": actions}, "config": {"recursion_limit": 300}})
        pauses = []
        while state.get("__interrupt__"):
            action = copy.deepcopy(state["__interrupt__"][0]["value"]["action"])
            pauses.append(action["operation"])
            if action["operation"] == "edit.submit":
                action["payload"]["ops"][0]["content"] = "LangGraph reviewed op marker"
                action["payload"]["pvi"] = {"P": ["LangGraph reviewed P marker"], "V": ["LangGraph reviewed V marker"],
                                            "I": {"operator_note": "LangGraph reviewed I marker"}}
            state = request(args.langgraph_url, run_path, {"assistant_id": "sic_core", "command": {"resume": action}, "config": {"recursion_limit": 300}})
        assert state["status"] == "completed", state
        assert pauses == ["chain.create", "source.ingest", "edit.prepare", "edit.submit", "cut.publish"], pauses
        persisted = request(args.langgraph_url, "/threads/" + thread + "/state")
        assert persisted["values"]["status"] == "completed"
        assert len(persisted["values"]["trace"]) == 8
        history = request(args.langgraph_url, "/threads/" + thread + "/history", {"limit": 100})
        assert len(history) > 8
        context = state["context"]
        first = core.execute("cut.get", context)["result"]
        first_cb = copy.deepcopy(first["view"]["cbs"][0])
        assert "LangGraph reviewed op marker" in first_cb["body"] and "LangGraph reviewed P marker" in first_cb["body"]
        assert first["view"]["instant"]["operator_note"] == "LangGraph reviewed I marker"
        assert "LangGraph reviewed V marker" in json.dumps(first["view"]["pending"])
        negative = []
        for operation in ("cb.update", "op.update", "source.update"):
            denied = core._request("/api/v2/operations", {"operation": operation, "context": {**context, "allow_write": True},
                                                       "payload": {"cb_id": first_cb["cb_id"], "body": "forbidden", "ops": []}})
            assert denied.get("ok") is False
            negative.append(operation)
        # A correction is a new P/I communication on the next WB, never a write to old CB/op.
        source_id = first["pending"][0]
        prepared = core.execute("edit.prepare", context, {"source_id": source_id}, allow_write=True)
        correction_context = prepared["context"]
        job = correction_context["job_id"]
        assert core.execute("edit.submit", correction_context, {"job_id": job, "editor": "editor-1", "ops": [],
            "pvi": {"P": ["New persistent correction; first CB remains fixed"], "V": [], "I": {"operator_note": "Corrected current I"}}}, allow_write=True)["ok"]
        assert core.execute("edit.validate", correction_context, {"job_id": job})["ok"]
        assert core.execute("cut.publish", correction_context, {"job_id": job, "expected_cut": 1, "idempotency_key": job}, allow_write=True)["ok"]
        corrected = core.execute("cut.get", correction_context)["result"]
        assert corrected["view"]["cbs"][0] == first_cb
        assert "New persistent correction" in corrected["view"]["cbs"][1]["body"]
        assert corrected["view"]["instant"]["operator_note"] == "Corrected current I"
        report["langgraph"] = {"thread_id": thread, "chain_id": context["chain_id"], "trace_stages": 8,
                               "write_review_pauses": pauses, "persisted_history_records": len(history),
                               "resume_op_pvi_changes_effective": True, "immutable_update_denials": negative,
                               "old_cb_and_ops_unchanged": True, "new_pvi_correction_effective": True}
        report["status"] = "passed"
        destination = ROOT.parent / ".local/integration-test-results"
        destination.mkdir(parents=True, exist_ok=True)
        (destination / "v2-runtime-acceptance.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({"status": "passed", "langflow_full_flows": [item["stages"] for item in report["langflow"]],
                          "langgraph_persistent_stages": 8, "langgraph_write_pauses": len(pauses),
                          "op_pvi_resume_edits_effective": True, "immutable_update_denials": len(negative),
                          "old_cb_ops_unchanged": True, "model_requests": 0, "visual_ui_verified_by_this_script": False}))
    finally:
        request(args.langflow_url, "/api/v1/api_key/" + key["id"], method="DELETE")


if __name__ == "__main__":
    main()
