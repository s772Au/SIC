import json

from lfx.custom.custom_component.component import Component
from lfx.io import BoolInput, CodeInput, DataInput, DropdownInput, Output
from lfx.schema import Data
from sic_workflow.client import OPERATIONS, SicClient


class SICOperation(Component):
    display_name = "SIC Legacy Rust Operation"
    description = "Inspect Rust history, edit drafts, or explicitly enable one single-pass model request."
    icon = "Workflow"
    name = "SICOperation"
    inputs = [
        DropdownInput(name="operation_name", display_name="Operation", options=list(OPERATIONS), value="bootstrap"),
        DataInput(name="previous", display_name="Previous step", required=False,
                  info="Connect another SIC node; carries IDs without copying its historical result."),
        CodeInput(name="context_json", display_name="Context overrides (JSON)", value="{}",
                       info='IDs, for example {"session_id":"...", "draft_id":"..."}.'),
        CodeInput(name="payload_json", display_name="Editable request (JSON)", value="{}",
                       info="Only the API request body/query. Read the integration guide for operation examples."),
        BoolInput(name="allow_write", display_name="Allow write", value=False,
                  info="Explicitly enable state changes on this node. Re-running a write can create another record."),
        BoolInput(name="allow_model", display_name="Allow model request", value=False,
                  info="Only draft_single_pass: sends one real upstream request and may incur cost. Requires Allow write too."),
    ]
    outputs = [Output(name="result", display_name="API result + context", method="execute_operation")]

    def execute_operation(self) -> Data:
        previous = self.previous.data if self.previous else {}
        context = dict(previous.get("context", {}))
        context_values = self.context_json if isinstance(self.context_json, dict) else json.loads(self.context_json or "{}")
        payload_values = self.payload_json if isinstance(self.payload_json, dict) else json.loads(self.payload_json or "{}")
        if not isinstance(context_values, dict) or not isinstance(payload_values, dict):
            raise ValueError("Context and request must be JSON objects")
        context.update(context_values)
        result = SicClient().execute(self.operation_name, context,
                                     payload_values, allow_write=self.allow_write, allow_model=self.allow_model)
        self.status = result
        return Data(data=result)
