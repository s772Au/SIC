"""Official Studio graph: real v2 actions routed through visible mechanism stages."""
from typing import Any, TypedDict

from langgraph.graph import StateGraph, START, END
from langgraph.types import interrupt
from sic_workflow.v2_client import CoreClient, bind_result, resolve_values


class CoreState(TypedDict, total=False):
    actions: list[dict[str, Any]]
    context: dict[str, Any]
    cursor: int
    capabilities: dict[str, Any]
    pending_action: dict[str, Any]
    reviewed_action: dict[str, Any]
    trace: list[dict[str, Any]]
    result: dict[str, Any]
    status: str


def inspect_capabilities(state):
    caps = CoreClient().capabilities()
    return {"capabilities": caps, "status": "ready" if caps.get("ok", True) else "capability_error",
            "cursor": state.get("cursor", 0), "trace": state.get("trace", [])}


def prepare_action(state):
    actions = state.get("actions") or [{"operation": "capabilities"}]
    cursor = state.get("cursor", 0)
    action = dict(actions[cursor])
    context = dict(state.get("context", {}))
    context.update(resolve_values(action.get("context", {}), context))
    action["context"] = context
    action["payload"] = resolve_values(action.get("payload", {}), context)
    return {"pending_action": action}


def review_authorized_material(state):
    action = state["pending_action"]
    catalog = state["capabilities"].get("operations", [])
    if isinstance(catalog, dict):
        catalog = [{"operation": key, **value} for key, value in catalog.items()]
    spec = next((item for item in catalog if item.get("operation") == action["operation"]), {})
    if spec.get("write"):
        reviewed = interrupt({"stage": stage_for(action["operation"]), "capability": spec,
                              "immutable": "Published CB, persistent operations and protocol control are read-only",
                              "message": "Review authorized material/configuration; resume with the complete action JSON.",
                              "action": action})
        if not isinstance(reviewed, dict) or reviewed.get("operation") != action["operation"]:
            raise ValueError("Resume must preserve the reviewed operation")
        action = reviewed
    return {"reviewed_action": action}


def execute_stage(state):
    action = state["reviewed_action"]
    envelope = CoreClient().execute(action["operation"], action.get("context"), action.get("payload"),
                                    allow_write=action.get("allow_write", False),
                                    allow_model=action.get("allow_model", False))
    envelope = bind_result(envelope, action.get("bindings", {}))
    return {"result": envelope, "context": envelope["context"],
            "trace": state.get("trace", []) + [{"stage": stage_for(action["operation"]), **envelope}],
            "cursor": state.get("cursor", 0) + 1, "status": "running" if envelope.get("ok") else "failed"}


def stage_for(operation):
    if operation.startswith(("source.", "wb.")):
        return "decode_text_and_sidecar"
    if operation.startswith("edit."):
        return "program_and_editor_operations"
    if operation.startswith("cut."):
        return "immutable_cb_pvi_cut"
    if operation.startswith("provider."):
        return "encode_and_worker"
    if operation.startswith(("harness.", "runtime.", "frontier.")):
        return "cache_and_tool_scheduler"
    if operation.startswith(("chain.", "socket.", "collaboration.")):
        return "peer_chain_and_socket"
    if operation.startswith(("board.", "tools.", "index.", "memory.", "decision.")):
        return "current_views_and_dual_index"
    return "configuration_and_metering"


def next_action(state):
    if state.get("status") == "failed" or state.get("cursor", 0) >= len(state.get("actions") or [{}]):
        return "finish"
    return "prepare_action"


def finish(state):
    return {"status": "failed" if state.get("status") in {"failed", "capability_error"} else "completed"}


builder = StateGraph(CoreState)
builder.add_node("inspect_actual_capabilities", inspect_capabilities)
builder.add_node("prepare_action", prepare_action)
builder.add_node("review_authorized_material", review_authorized_material)
builder.add_node("finish", finish)
stages = ["decode_text_and_sidecar", "program_and_editor_operations", "immutable_cb_pvi_cut", "encode_and_worker",
          "cache_and_tool_scheduler", "peer_chain_and_socket", "current_views_and_dual_index", "configuration_and_metering"]
for name in stages:
    builder.add_node(name, execute_stage)
    builder.add_conditional_edges(name, next_action, {"prepare_action": "prepare_action", "finish": "finish"})
builder.add_edge(START, "inspect_actual_capabilities")
builder.add_conditional_edges("inspect_actual_capabilities", lambda s: "finish" if s.get("status") == "capability_error" else "prepare_action")
builder.add_edge("prepare_action", "review_authorized_material")
builder.add_conditional_edges("review_authorized_material", lambda s: stage_for(s["reviewed_action"]["operation"]), {key: key for key in stages})
builder.add_edge("finish", END)
graph = builder.compile()
