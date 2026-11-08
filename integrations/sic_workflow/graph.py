"""Optional LangGraph/Studio control plane; credentials never enter graph state."""
from typing import Any, TypedDict

from langgraph.graph import StateGraph, START, END
from langgraph.types import interrupt

from sic_workflow.client import OPERATIONS, SicClient


class State(TypedDict, total=False):
    operation: str
    context: dict[str, Any]
    payload: dict[str, Any]
    allow_model: bool
    capabilities: dict[str, Any]
    approved_action: dict[str, Any]
    result: dict[str, Any]
    projection: dict[str, Any]


def inspect(state: State):
    return {"capabilities": SicClient().execute("bootstrap")["result"]}


def review(state: State):
    operation = state.get("operation", "bootstrap")
    if operation not in OPERATIONS:
        raise ValueError("Unknown SIC operation")
    action = {"operation": operation, "context": state.get("context", {}), "payload": state.get("payload", {})}
    if operation == "draft_single_pass":
        action["allow_model"] = state.get("allow_model", False)
    if OPERATIONS[operation][3]:
        # Studio shows this editable JSON. A resumed action is validated again.
        action = interrupt({"message": "Review and resume with the complete action JSON to perform one write.",
                            "action": action})
        if not isinstance(action, dict) or action.get("operation") != operation:
            raise ValueError("Resume must preserve the reviewed operation")
    return {"approved_action": action}


def execute(state: State):
    action = state["approved_action"]
    result = SicClient().execute(action["operation"], action.get("context"), action.get("payload"),
                                allow_write=True, allow_model=action.get("allow_model", False))
    return {"result": result, "context": result["context"]}


def refresh(state: State):
    context = state.get("context", {})
    if context.get("session_id"):
        return {"projection": SicClient().execute("projection", context)["result"]}
    return {}


builder = StateGraph(State)
builder.add_node("inspect_capabilities", inspect)
builder.add_node("review_editable_action", review)
builder.add_node("execute_one_operation", execute)
builder.add_node("refresh_rust_projection", refresh)
builder.add_edge(START, "inspect_capabilities")
builder.add_edge("inspect_capabilities", "review_editable_action")
builder.add_edge("review_editable_action", "execute_one_operation")
builder.add_edge("execute_one_operation", "refresh_rust_projection")
builder.add_edge("refresh_rust_projection", END)
# Agent Server supplies its own checkpoint store; direct callers can compile
# builder with InMemorySaver. Checkpoints are orchestration state, not SIC history.
graph = builder.compile()
