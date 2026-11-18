"""Studio graph for the live Rust capability catalog and reviewed operations."""

from typing import Any, TypedDict

from langgraph.graph import END, START, StateGraph
from langgraph.types import interrupt
from pydantic import BaseModel, Field
from sic_workflow.rust_workbench import RustWorkbench


class RustInput(BaseModel):
    operation: str = Field(default="capabilities", description="查看实时能力，或输入 Rust Core 公布的操作名")
    chain_id: str = Field(default="", description="操作所属路线；创建路线时留空")
    payload: dict[str, Any] = Field(default_factory=dict, description="操作参数；先查看实时能力中的 payload")
    allow_write: bool = False
    allow_model: bool = False
    allow_revoke: bool = False


class RustState(TypedDict, total=False):
    operation: str
    chain_id: str
    payload: dict[str, Any]
    allow_write: bool
    allow_model: bool
    allow_revoke: bool
    capability: dict[str, Any]
    reviewed: dict[str, Any]
    result: dict[str, Any]
    status: str


def inspect(state):
    catalog = RustWorkbench().capabilities()
    if not catalog.get("ok"):
        return {"result": catalog, "status": "failed"}
    operation = state.get("operation", "capabilities")
    if operation == "capabilities":
        return {"result": catalog, "status": "completed"}
    spec = next((item for item in catalog["result"]["operations"] if item["operation"] == operation), None)
    if spec is None:
        return {"result": RustWorkbench._error(operation, {}, "capability_missing",
                                               "The connected Rust Core does not advertise this operation."),
                "status": "failed"}
    return {"capability": spec, "status": "ready"}


def review(state):
    form = {key: state.get(key, default) for key, default in (
        ("operation", "capabilities"), ("chain_id", ""), ("payload", {}),
        ("allow_write", False), ("allow_model", False), ("allow_revoke", False))}
    spec = state["capability"]
    if spec["write"] or spec["model"]:
        answer = interrupt({"capability": spec, "form": form,
                            "notice": "核对参数与权限；操作和路线不可在恢复时改变。"})
        if isinstance(answer, dict) and "form" in answer:
            answer = answer["form"]
        if (not isinstance(answer, dict) or answer.get("operation") != form["operation"]
                or answer.get("chain_id", "") != form["chain_id"]
                or not isinstance(answer.get("payload"), dict)
                or set(answer) - set(form)):
            raise ValueError("Resume must preserve the reviewed operation and chain and contain only form fields")
        form = answer
    return {"reviewed": form}


def execute(state):
    form = state["reviewed"]
    context = {"chain_id": form["chain_id"]} if form.get("chain_id") else {}
    result = RustWorkbench().execute(form["operation"], context, form.get("payload", {}),
                                     allow_write=form.get("allow_write") is True,
                                     allow_model=form.get("allow_model") is True,
                                     allow_revoke=form.get("allow_revoke") is True)
    return {"result": result, "status": "completed" if result.get("ok") else "failed"}


builder = StateGraph(RustState, input_schema=RustInput)
builder.add_node("inspect_rust_capabilities", inspect)
builder.add_node("review_rust_operation", review)
builder.add_node("execute_rust_operation", execute)
builder.add_edge(START, "inspect_rust_capabilities")
builder.add_conditional_edges("inspect_rust_capabilities", lambda state: "review" if state["status"] == "ready" else "done",
                              {"review": "review_rust_operation", "done": END})
builder.add_edge("review_rust_operation", "execute_rust_operation")
builder.add_edge("execute_rust_operation", END)
graph = builder.compile()
