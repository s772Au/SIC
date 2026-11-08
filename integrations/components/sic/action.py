import json

from lfx.custom.custom_component.component import Component
from lfx.io import BoolInput, CodeInput, DataInput, StrInput, Output
from lfx.schema import Data
from sic_workflow.v2_client import CoreClient, bind_result, resolve_values


class SICAction(Component):
    display_name = "SIC Core Action"
    description = "V2 core action. Published CB and protocol sidecars are read-only. Corrections use new P/V/I."
    icon = "Workflow"
    name = "SICAction"
    inputs = [
        StrInput(name="operation_name", display_name="Core operation", value="capabilities"),
        DataInput(name="previous", display_name="Previous stage", required=False),
        CodeInput(name="context_json", display_name="Context / authorized actor (JSON)", value="{}"),
        CodeInput(name="payload_json", display_name="Editable working material / configuration (JSON)", value="{}"),
        CodeInput(name="bindings_json", display_name="Save result paths to context (JSON)", value="{}",
                  info='For example {"chain_id":"id"}. Payload values can refer to $context.chain_id.'),
        BoolInput(name="allow_write", display_name="Allow this write", value=False),
        BoolInput(name="allow_model", display_name="Allow this model request", value=False),
        BoolInput(name="continue_after_error", display_name="Inspect after previous failure", value=False,
                  info="Default stops the chain. Enable only for intentional diagnostic/read operations."),
    ]
    outputs = [Output(name="result", display_name="Stage result / state / evidence", method="run_action")]

    @staticmethod
    def _object(value):
        result = value if isinstance(value, dict) else json.loads(value or "{}")
        if not isinstance(result, dict):
            raise ValueError("Fields must be JSON objects")
        return result

    def run_action(self) -> Data:
        previous = self.previous.data if self.previous else {}
        if previous.get("ok") is False and not self.continue_after_error:
            result = {**previous, "blocked_operation": self.operation_name, "blocked_by_previous_failure": True}
        else:
            context = dict(previous.get("context", {}))
            context.update(self._object(self.context_json))
            payload = resolve_values(self._object(self.payload_json), context)
            result = CoreClient().execute(self.operation_name, context, payload,
                                          allow_write=self.allow_write, allow_model=self.allow_model)
            result = bind_result(result, self._object(self.bindings_json))
        self.status = result
        return Data(data=result)
