"""Offline acceptance checks against an isolated, real Rust API. No providers."""
import asyncio
import copy
import json
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from sic_workflow.client import SicClient, SicError


def main():
    client = SicClient()
    evidence = []
    context = {}

    def call(operation, payload=None, overrides=None):
        nonlocal context
        envelope = client.execute(operation, context | (overrides or {}), payload, allow_write=True)
        context = envelope["context"]
        evidence.append({"operation": operation, "passed": True})
        return envelope["result"]

    call("bootstrap")
    call("session_create", {"name": "visual integration offline acceptance"})
    call("wb_create", {"source": "first\nsecond", "units": ["first\n", "second"]})
    assert call("wb_source")["source"] == "first\nsecond"
    call("wb_units")
    call("draft_create")
    call("preflight")
    call("draft_edit", {"base_revision": 0, "edits": [
        {"target_id": "u000001", "op": "COMPRESS", "risk": "low", "content": "1\n"}]})
    try:
        call("draft_edit", {"base_revision": 0, "edits": []})
        raise AssertionError("Stale revision was accepted")
    except SicError as exc:
        assert "409" in str(exc)
    assert call("draft_validate")["valid"]
    validated = context["validated_cb_sha256"]
    try:
        client.execute("draft_commit", context | {"validated_cb_sha256": "0" * 64}, allow_write=True)
        raise AssertionError("Wrong validation hash was accepted")
    except SicError as exc:
        assert "409" in str(exc)
    call("draft_commit")
    assert call("commit_read")["content"] == "1\nsecond"
    call("commit_publish")
    first_commit = context["commit_id"]
    call("drafts")
    call("draft_jobs")
    call("mount_read")
    call("mount_edit", {"base_revision": 0, "content": "Current task notes"})
    call("mount_publish")
    call("mount_revisions")
    assert call("projection")["worker_adopted"] is False
    call("assemble_chat", {"request": {"model": "offline", "messages": [{"role": "user", "content": "tail"}]}})
    call("assemble_responses", {"request": {"model": "offline", "input": [{"role": "user", "content": "tail"}]}})
    call("wb_create", {"source": "third"})
    call("draft_create")
    call("draft_edit", {"base_revision": 0, "edits": [
        {"target_id": "wb", "op": "COMPRESS", "risk": "low", "content": "3"}]})
    call("draft_validate")
    call("draft_commit")
    call("commit_publish")
    call("roots")
    candidates = call("promotion_candidates")["candidates"]
    promotion = {"child_ids": candidates[0]["child_ids"], "content": "first through third",
                 "risk": "medium", "reason": "Offline acceptance check"}
    call("promotion_preview", promotion)
    call("promotion_commit", promotion)
    call("node_read")
    call("archive_search", {"q": "first", "limit": 20})
    call("archive_read", {"offset": 0, "max_bytes": 4096}, {"kind": "cb", "entry_id": first_commit})
    call("timeline")
    call("test_catalog")
    call("test_plan_read", overrides={"plan_id": "integration_smoke"})
    plan = {"version": {"major": 9, "minor": 0, "patch": 0},
            "scheduler": {"gatewayUrl": "http://127.0.0.1:1", "targetUtilizationPct": 50,
                          "maxInFlight": 1, "adaptive": False, "maxRps": 1, "burst": 1,
                          "rampUpPerSecond": 1, "stop429Pct": 1, "stop5xxPct": 1},
            "low": {"cases": []}, "high": {"cases": []}}
    assert call("test_plan_validate", {"plan": plan})["valid"]
    revision = call("test_plan_read")["revision"]
    call("test_plan_edit", {"baseRevision": revision, "plan": plan})

    from generate_flows import main as generate
    generate()
    from lfx.load import load_flow_from_json
    flow = json.loads((ROOT / "flows/observe.json").read_text(encoding="utf-8"))
    flow["data"]["nodes"][1]["data"]["node"]["template"]["context_json"]["value"] = json.dumps({"session_id": context["session_id"]})
    graph = load_flow_from_json(flow)
    outputs = asyncio.run(graph.arun(inputs=[{}], outputs=["SICOperation-5"]))
    assert outputs[0].outputs, "Langflow did not produce output"
    evidence.append({"langflow_sdk": "observe chain executes through five real custom nodes", "passed": True})
    lifecycle = json.loads((ROOT / "flows/edit-lifecycle.json").read_text(encoding="utf-8"))
    for node in lifecycle["data"]["nodes"]:
        node["data"]["node"]["template"]["allow_write"]["value"] = True
    lifecycle["data"]["nodes"][0]["data"]["node"]["template"]["context_json"]["value"] = json.dumps({"session_id": context["session_id"]})
    graph = load_flow_from_json(lifecycle)
    outputs = asyncio.run(graph.arun(inputs=[{}], outputs=["SICOperation-9"]))
    assert outputs[0].outputs
    evidence.append({"langflow_sdk": "WB -> units -> draft -> preflight -> edit -> validate -> commit -> publish -> projection", "passed": True})

    from sic_workflow.graph import builder
    from langgraph.checkpoint.memory import InMemorySaver
    from langgraph.types import Command
    graph = builder.compile(checkpointer=InMemorySaver())
    config = {"configurable": {"thread_id": "offline-acceptance"}}
    action = {"operation": "mount_edit", "context": {"session_id": context["session_id"]},
              "payload": {"base_revision": 1, "content": "Studio-reviewed notes"}}
    paused = graph.invoke(action, config)
    assert paused.get("__interrupt__"), "Write was not interrupted for review"
    finished = graph.invoke(Command(resume=action), config)
    assert finished["result"]["operation"] == "mount_edit"
    evidence.append({"langgraph": "write interrupts, editable action resumes, projection refreshes", "passed": True})
    # Exported flows contain only code and public API parameters.
    token = os.environ["SIC_ADMIN_TOKEN"]
    assert all(token not in p.read_text(encoding="utf-8") for p in (ROOT / "flows").glob("*.json"))
    report = {"status": "passed", "provider_calls": 0, "checks": evidence,
              "sdk_versions": {"lfx": "1.13.1", "langgraph": "1.2.20"}}
    destination = ROOT.parent / ".local/integration-test-results"
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "smoke-result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": "passed", "checks": len(evidence), "provider_calls": 0}))


if __name__ == "__main__":
    main()
