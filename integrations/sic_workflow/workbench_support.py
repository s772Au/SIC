"""Presentation helpers for official components; no local copy becomes authority."""
import json

from lfx.schema import Data, DataFrame
from sic_workflow.v2_client import CoreClient


def context_of(component):
    previous = component.previous.data if getattr(component, "previous", None) else {}
    context = dict(previous.get("context", {}))
    if getattr(component, "chain_id", ""):
        context["chain_id"] = component.chain_id
    return context


def object_value(value, kind=dict):
    parsed = value if isinstance(value, (dict, list)) else json.loads(value or ("{}" if kind is dict else "[]"))
    if not isinstance(parsed, kind):
        raise ValueError(f"Expected JSON {kind.__name__}")
    return parsed


def perform(component, operation, payload=None, *, context=None):
    previous = getattr(component, 'previous', None)
    previous = previous.data if previous else {}
    if previous.get('ok') is False:
        return {'ok': False, 'operation': operation, 'context': context or context_of(component),
                'error': {'code': 'upstream_failed', 'message': 'Previous stage failed; this stage was not executed.'},
                'evidence': {'source': 'frontend-dependency-gate'}}
    envelope = CoreClient().execute(operation, context or context_of(component), payload or {},
                                    allow_write=getattr(component, "allow_write", False),
                                    allow_model=getattr(component, "allow_model", False))
    component.status = envelope
    return envelope


def cached(component, loader):
    # Official build invokes _pre_run_setup before output methods. Multiple
    # visible outputs must never duplicate a write/model request within a run.
    if getattr(component, "_workbench_result", None) is None:
        component._workbench_result = loader()
    return component._workbench_result


def reset(component):
    component._workbench_result = None


def table(rows, columns=None):
    return DataFrame(rows if rows else [{key: "" for key in (columns or ["status"])}])


def error_rows(envelope):
    return [{"status": "not executed", "code": envelope.get("error", {}).get("code", "unknown"),
             "message": envelope.get("error", {}).get("message", "No verified result")}]


def text(value):
    return value if isinstance(value, str) else json.dumps(value, ensure_ascii=False, sort_keys=True)
