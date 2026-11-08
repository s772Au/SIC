import copy

from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import BoolInput, CodeInput, DataInput, DropdownInput, IntInput, MultilineInput, StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.workbench_support import cached, context_of, error_rows, object_value, perform, reset, table, text


class SICCandidate(Component):
    display_name = "SIC 候选版本编辑"
    description = "修订尚未发布的op与P/V/I，保留模型原始交付；旧校验随版本失效。已发布CB与op不可改。R13–25/R61。"
    icon = "FilePenLine"
    name = "SICCandidate"
    inputs = [
        DataInput(name="previous", display_name="前一步", required=False),
        StrInput(name="chain_id", display_name="工作路线", value=""),
        StrInput(name="job_id", display_name="编辑任务", value=""),
        DropdownInput(name="action", display_name="候选操作", options=["读取候选", "追加操作", "修改一项操作", "修订操作与 P/V/I", "移除一项操作", "修改 P/V/I", "替换完整序列（高级）", "校验候选", "发布此版本"], value="读取候选", real_time_refresh=True),
        IntInput(name="expected_revision", display_name="已核对候选版本（-1沿用前一步）", value=0),
        IntInput(name="operation_number", display_name="修改/移除第几项（从1起）", value=1),
        DropdownInput(name="op", display_name="内容操作", options=["KEEP", "DELETE", "FLAG", "REORDER", "COMPRESS", "REPLACE", "ADD"], value="REPLACE"),
        StrInput(name="target", display_name="原文单元ID", value=""),
        MultilineInput(name="content", display_name="新正文或标记", value=""),
        MultilineInput(name="reason", display_name="编辑理由 / 信息损失说明", value=""),
        IntInput(name="order", display_name="重排位置", value=0),
        StrInput(name="anchor_target", display_name="ADD插入锚（可选）", value=""),
        DropdownInput(name="anchor_position", display_name="ADD锚位置", options=["before", "after"], value="after"),
        DropdownInput(name="lifecycle", display_name="生成内容的生命周期", options=["P", "V", "I"], value="P", info="操作账本本身始终持久；KEEP/DELETE/REORDER仅P。"),
        StrInput(name="view_key", display_name="I视图名称", value=""),
        DropdownInput(name="risk", display_name="信息损失风险", options=["none", "low", "medium", "high", "unknown"], value="unknown"),
        MultilineInput(name="persistent_text", display_name="P：本次固定内容", value="", info="空值表示此版本P为空；正文可含换行。"),
        MultilineInput(name="volatile_text", display_name="V：下一WB材料", value=""),
        CodeInput(name="instant_views", display_name="I：当前独立视图（JSON对象）", value="{}", info="不属于可编辑WB；下一覆写换版。决策四视角另有专门工作台。"),
        CodeInput(name="full_ops", display_name="完整候选op数组（高级）", value="[]", advanced=True),
        IntInput(name="expected_cut", display_name="已核对cut（-1沿用任务/前一步快照）", value=0),
        BoolInput(name="allow_write", display_name="允许本次修订/发布", value=False),
    ]
    outputs = [Output(name="result", display_name="候选 / 校验 / 发布结果", method="run_candidate", group_outputs=True),
               Output(name="operations", display_name="有序op与来源", method="operations", group_outputs=True),
               Output(name="originals", display_name="原始Editor交付（只读）", method="originals", group_outputs=True),
               Output(name="lifecycles", display_name="P / V / I去向", method="lifecycles", group_outputs=True)]
    for _field in inputs:
        if _field.name in {"operation_number", "op", "target", "content", "reason", "order", "anchor_target", "anchor_position", "lifecycle", "view_key", "risk", "persistent_text", "volatile_text", "instant_views", "full_ops", "expected_cut"}:
            _field.show = False

    def _pre_run_setup(self):
        reset(self)
        self._candidate_original = None

    def update_build_config(self, build_config, field_value, field_name=None):
        if field_name != "action":
            return build_config
        typed = field_value in {"追加操作", "修改一项操作", "修订操作与 P/V/I"}
        for key in ("op", "target", "content", "reason", "order", "anchor_target", "anchor_position", "lifecycle", "view_key", "risk"):
            build_config[key]["show"] = typed
        build_config["operation_number"]["show"] = field_value in {"修改一项操作", "修订操作与 P/V/I", "移除一项操作"}
        for key in ("persistent_text", "volatile_text", "instant_views"):
            build_config[key]["show"] = field_value in {"修改 P/V/I", "修订操作与 P/V/I"}
        build_config["full_ops"]["show"] = field_value == "替换完整序列（高级）"
        build_config["expected_cut"]["show"] = field_value == "发布此版本"
        return build_config

    def _act(self):
        context = context_of(self)
        job = self.job_id or context.get("job_id")
        if not job:
            return {"ok": False, "context": context, "error": {"code": "job_required", "message": "先选择已交付的编辑任务；不能以新空任务代替候选修订。"}}
        context["job_id"] = job
        original = perform(self, "edit.candidate.get", {"job_id": job}, context=context)
        if original.get("ok"):
            self._candidate_original = original["result"]
        if not original.get("ok") or self.action == "读取候选":
            return original
        candidate = original["result"]
        expected_revision = context.get("candidate_revision") if self.expected_revision == -1 else self.expected_revision
        if candidate["revision"] != expected_revision:
            return {"ok": False, "context": context, "error": {"code": "candidate_stale", "message": f"候选已是版本{candidate['revision']}，请重新读取；不自动覆盖。"}}
        if self.action == "校验候选":
            return perform(self, "edit.validate", {"job_id": job}, context=context)
        if self.action == "发布此版本":
            expected_cut = context.get('reviewed_cut') if self.expected_cut == -1 else self.expected_cut
            if expected_cut is None:
                return {'ok': False, 'context': context, 'error': {'code': 'cut_snapshot_required', 'message': 'Prepare/read the job or explicitly supply its reviewed cut; no blind latest-version overwrite.'}}
            return perform(self, "cut.publish", {"job_id": job, "expected_cut": expected_cut,
                           "expected_candidate_revision": expected_revision,
                           "idempotency_key": f"{job}:candidate:{expected_revision}"}, context=context)
        ops, pvi = copy.deepcopy(candidate["ops"]), copy.deepcopy(candidate.get("pvi", {}))
        if self.action == "修改 P/V/I":
            pvi = {"P": [self.persistent_text] if self.persistent_text else [],
                   "V": [self.volatile_text] if self.volatile_text else [], "I": object_value(self.instant_views)}
        elif self.action == "替换完整序列（高级）":
            ops = object_value(self.full_ops, list)
        elif self.action == "移除一项操作":
            if not 1 <= self.operation_number <= len(ops):
                raise ValueError("操作编号超出当前候选范围")
            ops.pop(self.operation_number - 1)
        else:
            op = {"op": self.op}
            op["lifecycle"] = self.lifecycle
            op["risk"] = self.risk
            if self.lifecycle == "I":
                op["view_key"] = self.view_key
            if self.op != "ADD":
                op["target"] = self.target
            if self.op in {"FLAG", "COMPRESS", "REPLACE", "ADD"}:
                op["content"] = self.content
            if self.reason:
                op["reason"] = self.reason
            if self.op == "REORDER":
                op["order"] = self.order
            if self.op == "ADD" and self.anchor_target:
                op["anchor"] = {"target": self.anchor_target, "position": self.anchor_position}
            if self.action == "追加操作":
                ops.append(op)
            else:
                if not 1 <= self.operation_number <= len(ops):
                    raise ValueError("操作编号超出当前候选范围")
                ops[self.operation_number - 1] = op
            if self.action == "修订操作与 P/V/I":
                pvi = {"P": [self.persistent_text] if self.persistent_text else [],
                       "V": [self.volatile_text] if self.volatile_text else [], "I": object_value(self.instant_views)}
        result = perform(self, "edit.candidate.update", {"job_id": job, "expected_revision": expected_revision,
                         "ops": ops, "pvi": pvi}, context=context)
        if result.get("ok"):
            result["presentation"] = {"submitted_ops": ops, "submitted_pvi": pvi,
                                      "original_responses_read_only": True, "old_validation_stale": True}
        return result

    def run_candidate(self) -> Data:
        return Data(data=cached(self, self._act))

    def operations(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        result = envelope["result"]
        ops = result.get("ops", result.get("material", {}).get("ops", envelope.get("presentation", {}).get("submitted_ops", [])))
        return table([{"序号": i + 1, "记录下标": i, "操作": op.get("op"), "目标": text(op.get("target", op.get("targets", ""))),
                       "内容": op.get("content", ""), "内容生命周期": op.get("lifecycle", "P"), "风险": op.get("risk", "unknown"),
                       "理由": op.get("reason", ""), "来源": op.get("origin", op.get("editor", "候选提案")),
                       "候选版本": result.get("revision", self.expected_revision)} for i, op in enumerate(ops)])

    def lifecycles(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        result = envelope["result"]
        pvi = result.get("pvi", envelope.get("presentation", {}).get("submitted_pvi", {}))
        material = result.get("material", {})
        if material:
            pvi = {"P": [material.get("body", "")], "V": material.get("volatile", []), "I": material.get("instant", {})}
        return table([{"生命周期": key, "去向": destination, "内容": text(pvi.get(key, {} if key == "I" else [])),
                       "旧CB可改": False} for key, destination in [("P", "本次CB固定"), ("V", "下一WB可再编辑"), ("I", "独立当前视图，不进入WB")]])

    def originals(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        original = getattr(self, "_candidate_original", None) or {}
        responses = original.get("original_responses", {})
        entries = responses.items() if isinstance(responses, dict) else enumerate(responses)
        return table([{"交付者": str(key), "原始交付（只读）": text(value), "人工修订是否冒充原始输出": False}
                      for key, value in entries])
