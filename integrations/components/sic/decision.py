from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import BoolInput, CodeInput, DataInput, DropdownInput, IntInput, MultilineInput, StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.workbench_support import cached, context_of, error_rows, object_value, perform, reset, table, text


class SICDecision(Component):
    display_name = "SIC 四视角决策工作台"
    description = "维护当前工作、失败原因、候选线索与开放式遗漏发现。目录归I，历史依据保持固定。R02/R26–27。"
    icon = "ListTree"
    name = "SICDecision"
    inputs = [DataInput(name="previous", display_name="前一步", required=False),
              StrInput(name="chain_id", display_name="工作路线", value=""),
              DropdownInput(name="action", display_name="工作台动作", options=["查看四视角", "从新材料运行维护", "记录正在推进", "记录失败与原因", "加入候选与线索", "加入新发现方向", "标记完成"], value="查看四视角"),
              IntInput(name="expected_revision", display_name="已核对目录版本", value=0),
              StrInput(name="item_id", display_name="已有事项ID（可选）", value=""),
              MultilineInput(name="event_text", display_name="工作 / 原因 / 候选及线索", value=""),
              StrInput(name="evidence_source", display_name="依据来源WB（可选）", value=""),
              StrInput(name="evidence_unit", display_name="依据单元（可选）", value=""),
              CodeInput(name="extra_evidence", display_name="附加依据（数组，高级）", value="[]", advanced=True),
              BoolInput(name="allow_write", display_name="允许本次目录维护", value=False)]
    outputs = [Output(name="result", display_name="目录版本与维护证据", method="run_decision", group_outputs=True),
               Output(name="views", display_name="四视角工作台", method="views", group_outputs=True)]

    def _pre_run_setup(self):
        reset(self)

    def _act(self):
        context = context_of(self)
        if self.action == "查看四视角":
            return perform(self, "decision.get", context=context)
        if self.action == "从新材料运行维护":
            result = perform(self, "decision.maintain", context=context)
        else:
            kind = {"记录正在推进": "work", "记录失败与原因": "failure", "加入候选与线索": "candidate",
                    "加入新发现方向": "overlooked", "标记完成": "complete"}[self.action]
            evidence = object_value(self.extra_evidence, list)
            if self.evidence_source:
                evidence.append({"source_id": self.evidence_source, **({"unit_id": self.evidence_unit} if self.evidence_unit else {})})
            event = {"kind": kind, "text": self.event_text, "evidence": evidence}
            if self.item_id:
                event["target" if kind == "complete" else "id"] = self.item_id
            result = perform(self, "decision.update", {"expected_revision": self.expected_revision, "events": [event]}, context=context)
        if not result.get("ok"):
            return result
        result["maintenance_result"] = result["result"]
        latest = perform(self, "decision.get", context=context)
        if not latest.get("ok"):
            return latest
        return {**latest, "maintenance_result": result["maintenance_result"]}

    def run_decision(self) -> Data:
        return Data(data=cached(self, self._act))

    def views(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        result = envelope["result"]
        rows = []
        for view, purpose in [("Working on", "当前推进"), ("Failed Before", "失败方向与原因"), ("Not Tried Yet", "候选与线索"), ("Overlooked", "开放式遗漏发现")]:
            items = result.get("views", {}).get(view, [])
            if not items:
                rows.append({"视角": view, "用途": purpose, "事项": "暂无记录", "依据": "", "状态": "当前I视图", "版本": result.get("revision")})
            for item in items:
                rows.append({"视角": view, "用途": purpose, "事项ID": item.get("id"), "事项": item.get("text"),
                             "依据": text(item.get("evidence", [])), "状态": item.get("status"), "版本": result.get("revision"),
                             "生效边界": "Worker采用以发布cut的I为准"})
        rows.append({"视角": "Overlooked", "用途": "发现提示", "事项": result.get("discovery_prompt", "What have I missed?"),
                     "依据": "目录不是候选全集；机械更新不证明语义质量", "版本": result.get("revision")})
        return table(rows)
