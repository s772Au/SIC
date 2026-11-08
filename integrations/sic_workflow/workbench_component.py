"""Official Component base preserving typed table edges and JSON-safe artifacts.

LFX 1.13.1 classifies artifacts using ``self.status`` before the result. Our
status is a Core envelope, which otherwise labels a Table as an object and
leaves its pandas instance in the HTTP response. Override only the display
artifact; the official Table result and graph-edge value remain unchanged.
"""
import json
from lfx.custom.custom_component.component import Component as OfficialComponent
from lfx.schema import DataFrame


class WorkbenchComponent(OfficialComponent):
    def _build_artifact(self, result):
        if isinstance(result, DataFrame):
            rows = json.loads(result.to_json(orient='records', force_ascii=False))
            return {'repr': str(result), 'raw': rows, 'type': 'array'}
        return super()._build_artifact(result)
