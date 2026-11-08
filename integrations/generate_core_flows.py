"""Build v2 official Langflow flows from the backend's published operation catalog."""
import copy
import json
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from lfx.custom.utils import build_custom_component_template
from components.sic.action import SICAction

ROOT = Path(__file__).resolve().parent
CATALOG = ROOT.parent / "sic_core/operations.json"


def make_core_flow(name, actions, connect=True):
    component = SICAction()
    component._code = (ROOT / "components/sic/action.py").read_text(encoding="utf-8")
    template, _ = build_custom_component_template(component)
    nodes, edges = [], []
    for i, action in enumerate(actions):
        node_id = f"SICAction-{i + 1}"
        schema = copy.deepcopy(template)
        schema["display_name"] = action.get("label", action["operation"])
        for key, value in {
            "operation_name": action["operation"], "context_json": json.dumps(action.get("context", {})),
            "payload_json": json.dumps(action.get("payload", {}), ensure_ascii=False),
            "bindings_json": json.dumps(action.get("bindings", {})),
            "allow_write": False, "allow_model": False,
        }.items():
            schema["template"][key]["value"] = value
        # Grouped rows keep large workflows navigable in the official canvas.
        x, y = (i % 5) * 460, (i // 5) * 820
        nodes.append({"id": node_id, "type": "genericNode", "position": {"x": x, "y": y},
                      "data": {"id": node_id, "type": "SICAction", "node": schema}})
        if connect and i:
            source = f"SICAction-{i}"
            sh = {"dataType": "SICAction", "id": source, "name": "result", "output_types": schema["outputs"][0]["types"]}
            th = {"fieldName": "previous", "id": node_id, "inputTypes": schema["template"]["previous"]["input_types"], "type": "other"}
            edges.append({"id": f"edge-{i}", "source": source, "target": node_id,
                          "sourceHandle": json.dumps(sh), "targetHandle": json.dumps(th),
                          "data": {"sourceHandle": sh, "targetHandle": th}, "animated": False})
    return {"name": name, "description": "SIC v2 real core actions. Writes/models disabled on import. CB/operation history read-only; corrections via P/V/I.",
            "is_component": False, "data": {"nodes": nodes, "edges": edges, "viewport": {"x": 0, "y": 0, "zoom": 0.5}}}


def main():
    catalog = json.loads(CATALOG.read_text(encoding="utf-8"))
    core = [
        {"label": "Actual capabilities / missing support", "operation": "capabilities"},
        {"label": "Create independent Agent chain", "operation": "chain.create", "payload": {"creators": []}},
        {"label": "Native API → text + protocol sidecar", "operation": "source.ingest", "payload": {"format": "openai-chat", "request": {"messages": [
            {"role": "system", "content": "Keep task constraints and protocol protected."},
            {"role": "user", "content": "First working material"}, {"role": "user", "content": "First working material"}]}}},
        {"label": "Read immutable source / sidecar", "operation": "source.read", "payload": {"source_id": "$context.source_id"}},
        {"label": "Active program rules (no model)", "operation": "edit.program", "payload": {"source_id": "$context.source_id", "rules": ["deduplicate_exact"]}},
        {"label": "Validate op / PVI candidate", "operation": "edit.validate", "payload": {"job_id": "$context.job_id"}},
        {"label": "Publish immutable CB + operations / cut", "operation": "cut.publish", "payload": {"job_id": "$context.job_id", "expected_cut": 0, "idempotency_key": "$context.job_id"}},
        {"label": "Read CB + persistent ops + two indices", "operation": "cut.get"},
        {"label": "Encode legal provider request (preview)", "operation": "provider.preview", "payload": {"format": "openai-chat"}},
        {"label": "Inspect full state and current views", "operation": "chain.get"},
        {"label": "Metering / unknown evidence", "operation": "usage.report"},
    ]
    groups = {
        "core-native-to-cut.json": make_core_flow("SIC Core — native to immutable CB", core),
    }
    editor = [
        {"operation": "capabilities", "label": "Actual model / adapter capabilities"},
        {"operation": "chain.create", "payload": {"creators": []}},
        {"operation": "source.ingest", "label": "Native source / protected protocol split", "payload": {"format": "openai-chat", "request": {"messages": [
            {"role": "user", "content": "Evidence from first part"}, {"role": "user", "content": "Evidence from second part"}]}}},
        {"operation": "source.read", "payload": {"source_id": "$context.source_id"}},
        {"operation": "edit.prepare", "label": "Two Editor scopes — one Agent", "payload": {"source_id": "$context.source_id", "assignments": [
            {"editor": "editor-1", "unit_ids": ["u1"]}, {"editor": "editor-2", "unit_ids": ["u2"]}]}},
        {"operation": "edit.submit", "label": "Review first complete op + P/V/I proposal", "payload": {"job_id": "$context.job_id", "editor": "editor-1", "ops": [
            {"op": "COMPRESS", "target": "u1", "content": "First evidence"}], "pvi": {"P": ["Confirmed first conclusion"], "V": ["Verify next step"], "I": {"decision": "Working on verification"}}}},
        {"operation": "edit.submit", "label": "Second Editor — same immutable base", "payload": {"job_id": "$context.job_id", "editor": "editor-2", "ops": [
            {"op": "FLAG", "target": "u2", "content": "Check this evidence"}], "pvi": {"P": [], "V": [], "I": {}}}},
        {"operation": "edit.validate", "label": "Combined ordered op / default KEEP / PVI preview", "payload": {"job_id": "$context.job_id"}},
        {"operation": "cut.publish", "label": "Publish once — never mutate old CB", "payload": {"job_id": "$context.job_id", "expected_cut": 0, "idempotency_key": "$context.job_id"}},
        {"operation": "cut.get", "label": "CB / op ledger / timeline / WB correspondence"},
        {"operation": "provider.preview", "label": "Encode CB + visible op directory + current views", "payload": {"format": "openai-chat"}},
        {"operation": "usage.report", "label": "Cost evidence (manual proposals do not claim model quality)"},
    ]
    groups["core-editor-to-cut.json"] = make_core_flow("SIC Core — multi Editor proposals / PVI", editor)
    prefixes = {
        "core-editor-pvi.json": ("SIC Core — Editor / PVI proposals", ("edit.", "cut.")),
        "core-scheduler-provider.json": ("SIC Core — tool windows / encode / model", ("harness.", "provider.", "runtime.", "frontier.")),
        "core-chain-socket.json": ("SIC Core — peer chains / Socket / board", ("chain.", "socket.", "board.", "collaboration.")),
        "core-config-index-cost.json": ("SIC Core — tools / dual index / configuration / cost / host revoke", ("tools.", "index.", "usage.", "config.", "security.", "memory.", "decision.", "ledger.", "resource.")),
        "core-source-inspection.json": ("SIC Core — immutable source discovery", ("source.", "wb.")),
    }
    for filename, (name, starts) in prefixes.items():
        actions = []
        for spec in catalog["operations"]:
            if spec["operation"].startswith(starts):
                payload = copy.deepcopy(spec["payload_example"])
                for key in ("source_id", "job_id", "checkpoint_id", "socket_id", "cut_id"):
                    if key in payload:
                        payload[key] = "$context." + key
                actions.append({"operation": spec["operation"], "label": spec["operation"] + " — " + spec["editable_scope"],
                                "payload": payload, "context": {"chain_id": "REPLACE_CHAIN_ID"}})
        groups[filename] = make_core_flow(name, actions, connect=False)
    for filename, flow in groups.items():
        (ROOT / "flows" / filename).write_text(json.dumps(flow, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(filename, len(flow["data"]["nodes"]))
    (ROOT / "core-operation-catalog.json").write_text(json.dumps(catalog, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
