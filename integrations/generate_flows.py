"""Generate importable flows with the installed official Langflow template builder."""
import copy
import json
from pathlib import Path

from lfx.custom.utils import build_custom_component_template
from components.sic.operation import SICOperation

ROOT = Path(__file__).resolve().parent


def make_flow(name, steps, connect=True):
    code = (ROOT / "components/sic/operation.py").read_text(encoding="utf-8")
    component = SICOperation()
    component._code = code
    template, _ = build_custom_component_template(component)
    nodes, edges = [], []
    for i, (label, operation, payload, context) in enumerate(steps):
        node_id = f"SICOperation-{i + 1}"
        schema = copy.deepcopy(template)
        schema["display_name"] = label
        for field, value in {"operation_name": operation, "payload_json": json.dumps(payload, ensure_ascii=False),
                             "context_json": json.dumps(context), "allow_write": False}.items():
            schema["template"][field]["value"] = value
        nodes.append({"id": node_id, "type": "genericNode", "position": {"x": i * 420, "y": 120},
                      "data": {"id": node_id, "type": "SICOperation", "node": schema}})
        if connect and i:
            source = f"SICOperation-{i}"
            source_handle = {"dataType": "SICOperation", "id": source, "name": "result", "output_types": schema["outputs"][0]["types"]}
            target_handle = {"fieldName": "previous", "id": node_id, "inputTypes": schema["template"]["previous"]["input_types"], "type": "other"}
            edges.append({"id": f"edge-{i}", "source": source, "target": node_id,
                          "sourceHandle": json.dumps(source_handle), "targetHandle": json.dumps(target_handle),
                          "data": {"sourceHandle": source_handle, "targetHandle": target_handle}, "animated": False})
    return {"name": name, "description": "SIC Rust API control plane. Writes disabled until explicitly enabled per node.",
            "is_component": False, "data": {"nodes": nodes, "edges": edges, "viewport": {"x": 0, "y": 0, "zoom": 0.7}}}


def main():
    flows = {
        "observe.json": make_flow("SIC - observe", [
            ("Capabilities", "bootstrap", {}, {}),
            ("Timeline", "timeline", {}, {"session_id": "REPLACE_SESSION_ID"}),
            ("Local projection", "projection", {}, {}),
            ("Promotion candidates", "promotion_candidates", {}, {}),
            ("Archive search", "archive_search", {"q": "sample", "limit": 20}, {}),
        ]),
        "edit-lifecycle.json": make_flow("SIC - explicit draft lifecycle", [
            ("Create WB", "wb_create", {"source": "first\nsecond", "units": ["first\n", "second"]}, {"session_id": "REPLACE_SESSION_ID"}),
            ("Inspect source units", "wb_units", {}, {}),
            ("Create draft", "draft_create", {}, {}),
            ("Inspect preflight (no model)", "preflight", {}, {}),
            ("Edit draft revision", "draft_edit", {"base_revision": 0, "edits": [{"target_id": "u000001", "op": "COMPRESS", "risk": "low", "content": "1\n"}]}, {}),
            ("Validate draft", "draft_validate", {}, {}),
            ("Commit immutable CB", "draft_commit", {}, {}),
            ("Publish CB", "commit_publish", {}, {}),
            ("Inspect projection", "projection", {}, {}),
        ]),
        "operations.json": make_flow("SIC - operations workbench", [
            ("Single pass (explicit model request)", "draft_single_pass", {}, {"session_id": "REPLACE_SESSION_ID", "draft_id": "REPLACE_DRAFT_ID"}),
            ("Mount draft", "mount_read", {}, {"session_id": "REPLACE_SESSION_ID"}),
            ("Edit mount", "mount_edit", {"base_revision": 0, "content": "Editable working notes"}, {"session_id": "REPLACE_SESSION_ID"}),
            ("Publish mount", "mount_publish", {}, {"session_id": "REPLACE_SESSION_ID"}),
            ("Mount ledger", "mount_revisions", {}, {"session_id": "REPLACE_SESSION_ID"}),
            ("Promotion preview", "promotion_preview", {"child_ids": ["FIRST_ROOT", "SECOND_ROOT"], "content": "Reviewed summary", "risk": "medium", "reason": "Manual review"}, {"session_id": "REPLACE_SESSION_ID"}),
            ("Test catalog", "test_catalog", {}, {}),
            ("Plan validation", "test_plan_validate", {"plan": {}}, {"plan_id": "demo"}),
        ], connect=False),
    }
    for filename, flow in flows.items():
        (ROOT / "flows" / filename).write_text(json.dumps(flow, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"Generated {filename}: {len(flow['data']['nodes'])} nodes")


if __name__ == "__main__":
    main()
