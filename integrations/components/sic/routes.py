from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import BoolInput, DataInput, DropdownInput, IntInput, MultilineInput, StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.workbench_support import cached, context_of, error_rows, perform, reset, table, text


class SICRoutes(Component):
    display_name = "SIC 路线、历史点与协作"
    description = "查看真实CB路线与历史checkpoint，派出平级分身、运行并回传。Studio线程不是Agent分叉。R47–56。"
    icon = "GitFork"
    name = "SICRoutes"
    inputs = [DataInput(name="previous", display_name="前一步", required=False),
              StrInput(name="chain_id", display_name="工作路线", value=""),
              DropdownInput(name="action", display_name="路线动作", options=["浏览我的路线", "浏览DSH会话映射", "查看路线与历史点", "固定历史checkpoint", "从历史点平级分叉", "从空白创建路线", "建立历史接收点", "派发问题至历史分身", "查看协作进展", "运行分身并回传", "投递当前信箱", "发送Signal"], value="浏览我的路线", real_time_refresh=True),
              StrInput(name="session_key", display_name="选中宿主会话（空则只列目录）", value=""),
              StrInput(name="cut_id", display_name="要固定的历史cut（空则当前）", value=""),
              StrInput(name="checkpoint_id", display_name="历史checkpoint", value=""),
              MultilineInput(name="creator_ids", display_name="从空白创建：创建者路线（每行一个）", value=""),
              StrInput(name="socket_id", display_name="当前信箱 / Signal接收点", value=""),
              MultilineInput(name="socket_ids", display_name="历史分身接收点（每行一个）", value=""),
              MultilineInput(name="question", display_name="问题 / 消息正文", value=""),
              DropdownInput(name="lifecycle", display_name="请求 / 消息的延续方式", options=["P", "V"], value="V"),
              DropdownInput(name="return_lifecycle", display_name="结果回传至", options=["P", "V"], value="V"),
              StrInput(name="request_id", display_name="稳定请求号（重送沿用）", value="question-1"),
              IntInput(name="max_steps", display_name="每次有限执行步数", value=8),
              DropdownInput(name="signal", display_name="宿主控制动作", options=["run", "wait", "wake", "stop"], value="wake"),
              MultilineInput(name="message_acl", display_name="允许投递者（每行一个）", value=""),
              MultilineInput(name="signal_acl", display_name="允许控制者（每行一个）", value=""),
              MultilineInput(name="spawn_acl", display_name="允许读取历史并派生者（每行一个）", value="", info="消息投递权限不自动授予历史读取。"),
              BoolInput(name="allow_write", display_name="允许本次路线操作", value=False),
              BoolInput(name="allow_model", display_name="允许运行分身模型", value=False)]
    outputs = [Output(name="result", display_name="真实路线 / 协作结果", method="run_route", group_outputs=True),
               Output(name="routes", display_name="路线与历史点表", method="routes", group_outputs=True),
               Output(name="requests", display_name="问题 / 分身 / 回传进展", method="requests", group_outputs=True)]
    for _field in inputs:
        if _field.name not in {"previous", "chain_id", "action", "allow_write"}:
            _field.show = False

    def _pre_run_setup(self):
        reset(self)

    def update_build_config(self, build_config, field_value, field_name=None):
        if field_name != "action":
            return build_config
        relevant = {
            "浏览DSH会话映射": {"session_key"},
            "固定历史checkpoint": {"cut_id"}, "从历史点平级分叉": {"checkpoint_id"}, "从空白创建路线": {"creator_ids"},
            "建立历史接收点": {"checkpoint_id", "message_acl", "signal_acl", "spawn_acl"},
            "派发问题至历史分身": {"socket_id", "socket_ids", "question", "lifecycle", "return_lifecycle", "request_id"},
            "运行分身并回传": {"request_id", "max_steps", "allow_model"},
            "投递当前信箱": {"socket_id", "question", "lifecycle", "request_id"}, "发送Signal": {"socket_id", "signal"}}
        for key in ("session_key", "cut_id", "checkpoint_id", "creator_ids", "socket_id", "socket_ids", "question", "lifecycle", "return_lifecycle", "request_id", "max_steps", "signal", "message_acl", "signal_acl", "spawn_acl", "allow_model"):
            build_config[key]["show"] = key in relevant.get(field_value, set())
        return build_config

    @staticmethod
    def _lines(value):
        return [line.strip() for line in value.splitlines() if line.strip()]

    def _act(self):
        context = context_of(self)
        if self.action == '浏览DSH会话映射':
            from sic_workflow.bridge_client import session_bindings
            return session_bindings(context, self.session_key)
        checkpoint = self.checkpoint_id or context.get("checkpoint_id")
        socket = self.socket_id or context.get("socket_id")
        op, payload = {
            "浏览我的路线": ("chain.list", {}),
            "查看路线与历史点": ("chain.get", {}),
            "固定历史checkpoint": ("chain.checkpoint", {"cut_id": self.cut_id} if self.cut_id else {}),
            "从历史点平级分叉": ("chain.fork", {"checkpoint_id": checkpoint}),
            "从空白创建路线": ("chain.create", {"creators": self._lines(self.creator_ids)}),
            "建立历史接收点": ("socket.create", {"checkpoint_id": checkpoint, "message_acl": self._lines(self.message_acl), "signal_acl": self._lines(self.signal_acl), "spawn_acl": self._lines(self.spawn_acl)}),
            "派发问题至历史分身": ("collaboration.dispatch", {"socket_ids": self._lines(self.socket_ids) or ([socket] if socket else []), "question": self.question,
                                                   "lifecycle": self.lifecycle, "return_lifecycle": self.return_lifecycle, "request_id": self.request_id}),
            "查看协作进展": ("collaboration.status", {}),
            "运行分身并回传": ("collaboration.run", {"request_id": self.request_id, "max_steps": self.max_steps}),
            "投递当前信箱": ("socket.message", {"socket_id": socket, "text": self.question, "lifecycle": self.lifecycle, "message_id": self.request_id}),
            "发送Signal": ("socket.signal", {"socket_id": socket, "signal": self.signal}),
        }[self.action]
        return perform(self, op, payload, context=context)

    def run_route(self) -> Data:
        return Data(data=cached(self, self._act))

    def routes(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        result = envelope["result"]
        rows = []
        for session in result.get('sessions', []):
            rows.append({'宿主会话': session['session_key'], '路线': session['chain_id'], '桥阶段': session.get('phase'),
                         '编辑错误': session.get('editor_error'), '对象': '身份授权会话映射；选择后可继续Core只读导航'})
        for route in result.get("routes", []):
            rows.append({"路线": route["chain_id"], "运行状态": route["run_state"], "当前cut": route.get("cut_id"),
                         "平级来源checkpoint": route.get("origin_checkpoint"), "空白创建者": text(route.get("creators", [])), "对象": "逻辑CB路线"})
        if "chain_id" in result:
            rows.append({"路线": result["chain_id"], "运行状态": result.get("run_state", "见核心返回"), "当前cut": result.get("current_cut", result.get("cut_id")),
                         "平级来源checkpoint": result.get("origin_checkpoint"), "空白创建者": text(result.get("creators", [])), "对象": result.get("relationship", "路线")})
        for checkpoint in result.get("checkpoints", []):
            rows.append({"路线": checkpoint["chain_id"], "当前cut": checkpoint["cut_id"], "历史checkpoint": checkpoint["checkpoint_id"],
                         "对象": "SIC历史发布点（不是编排checkpoint）"})
        if result.get("checkpoint_id"):
            rows.append({"历史checkpoint": result["checkpoint_id"], "当前cut": result.get("cut_id"), "对象": "已固定的逻辑历史点；物理KV未知"})
        return table(rows)

    def requests(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        result = envelope["result"]
        requests = result.get("requests", {})
        if result.get("request_id"):
            requests = {result["request_id"]: result}
        rows = []
        for request_id, request in requests.items():
            for branch in request.get("branches", []) or [{}]:
                rows.append({"问题号": request_id, "问题": request.get("question", ""), "总状态": request.get("status"),
                             "平级分身": branch.get("chain_id"), "接收点": branch.get("socket_id"), "分身状态": branch.get("status"),
                             "结果 / 消费依据": text({key: value for key, value in branch.items() if key not in {"chain_id", "socket_id", "status"}})})
        if not rows:
            rows.append({"状态": result.get("run_state", result.get("delivery", "没有协作记录")),
                         "说明": result.get("endpoint_semantics", "分身实际完成工作和回传后才算协作完成；创建空链不算。")})
        return table(rows)
