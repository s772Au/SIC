from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import BoolInput, DataInput, DropdownInput, IntInput, MultilineInput, StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.workbench_support import cached, context_of, error_rows, perform, reset, table, text


class SICBoard(Component):
    display_name = "SIC 团队公示牌与任务认领"
    description = "共享任务、认领者与进度；CAS冲突不覆盖。每次覆写以最新I呈现，旧CB不改。R24/R55。"
    icon = "ClipboardList"
    name = "SICBoard"
    inputs = [DataInput(name="previous", display_name="前一步", required=False),
              StrInput(name="chain_id", display_name="当前路线", value=""),
              StrInput(name="board_id", display_name="共享公示牌（空则当前）", value=""),
              DropdownInput(name="action", display_name="公示牌动作", options=["查看任务", "认领或更新自己的任务"], value="查看任务"),
              IntInput(name="expected_revision", display_name="已核对公示牌版本", value=0),
              StrInput(name="task_id", display_name="任务ID", value=""),
              MultilineInput(name="description", display_name="任务书 / 进度说明", value=""),
              DropdownInput(name="task_status", display_name="任务状态", options=["claimed", "working", "completed", "accepted", "rejected"], value="claimed"),
              BoolInput(name="allow_write", display_name="允许本次认领/更新", value=False)]
    outputs = [Output(name="result", display_name="共享公示牌版本", method="run_board", group_outputs=True),
               Output(name="tasks", display_name="任务 / 认领者 / 进度", method="tasks", group_outputs=True)]

    def _pre_run_setup(self):
        reset(self)

    def _act(self):
        payload = {"board_id": self.board_id} if self.board_id else {}
        if self.action == "查看任务":
            return perform(self, "board.get", payload)
        # Component.description is class metadata; the identically named input
        # must be read through the official input accessor.
        payload.update(task_id=self.task_id, expected_revision=self.expected_revision,
                       description=self.get_input('description').value, status=self.task_status)
        return perform(self, "board.claim", payload)

    def run_board(self) -> Data:
        return Data(data=cached(self, self._act))

    def tasks(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        result = envelope["result"]
        tasks = result.get("value", {}).get("tasks", {})
        rows = [{"共享板": result.get("board_id"), "版本": result.get("revision"), "任务": task_id,
                 "任务书 / 进度": task.get("description"), "认领路线": task.get("claimant"), "状态": task.get("status"),
                 "活动位置": "下次cut最新I；不写旧CB"} for task_id, task in tasks.items()]
        return table(rows or [{"共享板": result.get("board_id"), "版本": result.get("revision"), "状态": "暂无任务"}])
