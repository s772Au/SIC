from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import BoolInput, DataInput, DropdownInput, FloatInput, IntInput, MultilineInput, StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.frontier_forms import frontier_payload, frontier_rows
from sic_workflow.workbench_support import cached, context_of, error_rows, perform, reset, table, text


class SICRuntime(Component):
    display_name = "SIC 持续工作与编辑窗口"
    description = "查看真实事件与后台进度，配置有限工作循环，比较同cut编辑窗口。事件记录不等于工具已执行。R05/R11/R35–41。"
    icon = "Activity"
    name = "SICRuntime"
    inputs = [DataInput(name="previous", display_name="前一步", required=False),
              StrInput(name="chain_id", display_name="工作路线", value=""),
              DropdownInput(name="action", display_name="运行与调度", options=["查看持续运行", "配置持续循环", "执行有限循环", "记录宿主已确认事件", "取得推进决定", "比较同次编辑窗口", "人工处理不确定状态"], value="查看持续运行", real_time_refresh=True),
              BoolInput(name="enabled", display_name="启用后台事件驱动", value=False),
              DropdownInput(name="editor_mode", display_name="编辑分工", options=["program", "model"], value="program"),
              DropdownInput(name="worker_mode", display_name="Worker推进方式", options=["host", "automatic"], value="host"),
              IntInput(name="editor_workers", display_name="并发Editor上限", value=2),
              FloatInput(name="max_pending_seconds", display_name="短工作块最长等待秒数（大于0，至86400）", value=1.0),
              IntInput(name="seal_chars", display_name="累计工作块字符阈值", value=8192),
              IntInput(name="editor_chars", display_name="Editor有限背景字符预算", value=4096),
              IntInput(name="worker_chars", display_name="Worker有限背景字符预算", value=8192),
              IntInput(name="worker_turns", display_name="每轮外部任务自治Worker预算", value=64),
              IntInput(name="branch_budget", display_name="一次协作分身预算", value=4),
              IntInput(name="max_steps", display_name="本次有限步数", value=8),
              BoolInput(name="continue_worker", display_name="本轮允许Worker继续", value=False),
              DropdownInput(name="event_type", display_name="宿主事实事件", options=["tool_start", "tool_end", "wait", "wait_complete", "wait_cancel", "work_end"], value="wait"),
              StrInput(name="call_id", display_name="实际工具调用ID", value=""),
              StrInput(name="wait_id", display_name="等待声明ID", value="operator-wait-1"),
              FloatInput(name="seconds", display_name="预期等待秒数", value=1.0),
              DropdownInput(name="tool_status", display_name="工具结束状态", options=["completed", "failed", "cancelled"], value="completed"),
              MultilineInput(name="tool_error", display_name="失败 / 取消依据", value=""),
              BoolInput(name="host_confirmed", display_name="确认事件来自实际宿主执行", value=False),
              StrInput(name="job_id", display_name="本次编辑任务", value=""),
              DropdownInput(name="schedule_policy", display_name="赶不上时", options=["wait", "interleave"], value="wait"),
              IntInput(name="interleave_budget", display_name="每WB额外推进预算", value=1),
              IntInput(name="expected_cut", display_name="已核对cut序号", value=0),
              IntInput(name="position_budget", display_name="同次比较的位置预算", value=0),
              IntInput(name="length_budget", display_name="同次比较的长度预算（字节）", value=4096),
              DropdownInput(name="comparison_role", display_name="比较下一请求的角色", options=["worker", "editor"], value="worker", real_time_refresh=True),
              StrInput(name="frontier_editor", display_name="目标Editor身份（沿用其模型与私有背景）", value="editor-1"),
              StrInput(name="next_source_id", display_name="该Editor下一WB的source_id", value=""),
              BoolInput(name="next_all_units", display_name="下一WB完整编辑范围", value=True, real_time_refresh=True),
              MultilineInput(name="next_unit_ids", display_name="指定下一WB单元（每行一个；关闭完整范围后空白表示空scope）", value=""),
              DropdownInput(name="recovery_action", display_name="人工处理方式（不重发模型）", options=["resume_queue", "abandon_editor", "mark_worker_indeterminate", "cancel_collaboration"], value="resume_queue"),
              StrInput(name="request_id", display_name="协作请求ID", value=""),
              BoolInput(name="confirm_recovery", display_name="确认停自动运行并保留不确定事实", value=False),
              BoolInput(name="acknowledge_possible_duplicate", display_name="确认已派发请求结果不确定", value=False),
              BoolInput(name="allow_write", display_name="允许本次运行/政策/事件写入", value=False),
              BoolInput(name="allow_model", display_name="允许本轮或已选后台模型工作", value=False)]
    outputs = [Output(name="result", display_name="运行决定与真实状态", method="run_runtime", group_outputs=True),
               Output(name="events", display_name="真实事件 / 决定 / 进度", method="events", group_outputs=True),
               Output(name="window", display_name="同cut必要替换 / 附加编辑对照", method="window", group_outputs=True)]
    for _field in inputs:
        if _field.name not in {"previous", "chain_id", "action", "allow_write"}:
            _field.show = False

    def _pre_run_setup(self):
        reset(self)

    def update_build_config(self, build_config, field_value, field_name=None):
        if field_name not in {"action", "comparison_role", "next_all_units"}:
            return build_config
        action = field_value if field_name == "action" else build_config["action"].get("value", self.action)
        role = field_value if field_name == "comparison_role" else build_config["comparison_role"].get("value", "worker")
        all_units = field_value if field_name == "next_all_units" else build_config["next_all_units"].get("value", True)
        fields = {
            "配置持续循环": {"enabled", "editor_mode", "worker_mode", "editor_workers", "seal_chars", "max_pending_seconds", "editor_chars", "worker_chars", "worker_turns", "branch_budget", "max_steps", "allow_model"},
            "执行有限循环": {"editor_mode", "max_steps", "continue_worker", "allow_model"},
            "记录宿主已确认事件": {"event_type", "call_id", "wait_id", "seconds", "tool_status", "tool_error", "host_confirmed"},
            "取得推进决定": {"job_id", "schedule_policy", "interleave_budget"},
            "比较同次编辑窗口": {"job_id", "expected_cut", "position_budget", "length_budget", "comparison_role"},
            "人工处理不确定状态": {"recovery_action", "request_id", "job_id", "confirm_recovery", "acknowledge_possible_duplicate"},
        }.get(action, set())
        if action == "比较同次编辑窗口" and role == "editor":
            fields.update({"frontier_editor", "next_source_id", "next_all_units"})
            if not all_units:
                fields.add("next_unit_ids")
        for field in self.inputs:
            if field.name not in {"previous", "chain_id", "action", "allow_write"}:
                build_config[field.name]["show"] = field.name in fields
        return build_config

    def _act(self):
        context = context_of(self)
        job = self.job_id or context.get("job_id")
        if self.action == "查看持续运行":
            return perform(self, "runtime.get", context=context)
        if self.action == "人工处理不确定状态":
            return perform(self, "runtime.recover", {"action": self.recovery_action, "job_id": job,
                           "request_id": self.request_id, "confirm": self.confirm_recovery,
                           "acknowledge_possible_duplicate": self.acknowledge_possible_duplicate}, context=context)
        if self.action == "配置持续循环":
            return perform(self, "runtime.configure", {"enabled": self.enabled, "editor_mode": self.editor_mode, "worker_mode": self.worker_mode,
                           "editor_workers": self.editor_workers, "seal_chars": self.seal_chars, "max_pending_seconds": self.max_pending_seconds, "editor_chars": self.editor_chars,
                           "worker_chars": self.worker_chars, "worker_turns": self.worker_turns, "branch_budget": self.branch_budget, "max_steps": self.max_steps}, context=context)
        if self.action == "执行有限循环":
            return perform(self, "runtime.run", {"max_steps": self.max_steps, "continue_worker": self.continue_worker, "editor_mode": self.editor_mode}, context=context)
        if self.action == "记录宿主已确认事件":
            if not self.host_confirmed:
                return {"ok": False, "context": context, "error": {"code": "host_fact_required", "message": "这里仅记录已确认事实，不执行工具，也不把模型提议当成开始/结束。"}}
            payload = {"type": self.event_type}
            if self.event_type.startswith("tool_"):
                payload["call_id"] = self.call_id
                if self.event_type == "tool_end":
                    payload.update(status=self.tool_status, error=self.tool_error)
            elif self.event_type != "work_end":
                payload["wait_id"] = self.wait_id
                if self.event_type == "wait":
                    payload["seconds"] = self.seconds
            return perform(self, "harness.event", payload, context=context)
        if self.action == "取得推进决定":
            payload = {"policy": self.schedule_policy, "interleave_budget": self.interleave_budget}
            if job:
                payload["job_id"] = job
            return perform(self, "harness.schedule", payload, context=context)
        try:
            payload = frontier_payload({"job_id": job, "expected_cut": self.expected_cut, "position_budget": self.position_budget,
                                        "length_budget": self.length_budget, "comparison_role": self.comparison_role,
                                        "frontier_editor": self.frontier_editor, "next_source_id": self.next_source_id,
                                        "next_unit_ids": None if self.next_all_units else self.next_unit_ids.splitlines()})
        except ValueError as error:
            return {"ok": False, "context": context, "error": {"code": "frontier_scope_invalid", "message": str(error)}}
        return perform(self, "frontier.compare", payload, context=context)

    def run_runtime(self) -> Data:
        return Data(data=cached(self, self._act))

    def events(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        result = envelope["result"]
        rows = [{"顺序": i, "事件": event.get("type"), "时间": event.get("time"), "事件ID": event.get("event_id"),
                 "实际工具": event.get("call_id"), "依据": text({key: value for key, value in event.items() if key not in {"type", "time", "event_id", "call_id"}})}
                for i, event in enumerate(result.get("events", []))]
        for index, step in enumerate(result.get("steps", [])):
            rows.append({"顺序": index, "事件": step.get("action"), "依据": text(step)})
        rows.append({"事件": "当前决定 / 政策", "依据": text({key: value for key, value in result.items() if key not in {"events", "steps", "necessary_request", "additional_request"}}),
                     "证据边界": "宿主事实与模型派发区间分别记录；物理缓存命中不得由状态名推定"})
        return table(rows)

    def window(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        result = envelope["result"]
        if envelope["operation"] != "frontier.compare":
            return table([{"状态": "选择同次编辑窗口比较以查看成对请求；不把相邻历史请求混成该对照。"}])
        return table(frontier_rows(result))
