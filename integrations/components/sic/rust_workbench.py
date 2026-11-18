"""Langflow control for the operations advertised by the running Rust Core."""

from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import BoolInput, CodeInput, DataInput, DropdownInput, StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.rust_workbench import RustWorkbench
from sic_workflow.v2_client import resolve_values
from sic_workflow.workbench_support import cached, context_of, error_rows, object_value, reset, table, text


class SICRustWorkbench(Component):
    display_name = "SIC Rust 实时工作台"
    description = "从当前 Rust Core 读取操作及参数；写入、模型和撤销权限分别确认。"
    icon = "Workflow"
    name = "SICRustWorkbench"
    inputs = [
        DataInput(name="previous", display_name="前一步", required=False),
        DropdownInput(name="group", display_name="操作分组（切换以刷新）",
                      options=["全部", "chain", "source", "wb", "edit", "cut", "hierarchy", "memory",
                               "decision", "runtime", "tools", "tool", "socket", "signal", "peer",
                               "board", "accounting", "security", "experiment", "legacy", "worker",
                               "message", "config"], value="全部", real_time_refresh=True),
        DropdownInput(name="operation_name", display_name="Rust 操作", options=["capabilities"],
                      value="capabilities", real_time_refresh=True),
        StrInput(name="chain_id", display_name="工作路线 ID（创建路线时留空）", value=""),
        CodeInput(name="context_json", display_name="上下文补充（JSON）", value="{}", advanced=True),
        CodeInput(name="payload_json", display_name="操作参数（JSON）", value="{}",
                  info="选择操作后查看参数说明；可使用 $context.source_id 等前一步绑定。"),
        BoolInput(name="allow_write", display_name="允许本次写入", value=False),
        BoolInput(name="allow_model", display_name="允许本次模型调用", value=False),
        BoolInput(name="allow_revoke", display_name="允许本次安全撤销", value=False),
    ]
    outputs = [
        Output(name="result", display_name="实际结果", method="run_action", group_outputs=True),
        Output(name="catalog", display_name="实时能力和参数", method="catalog", group_outputs=True),
    ]

    def _pre_run_setup(self):
        reset(self)

    def update_build_config(self, config, value, field_name=None):
        if field_name not in {"group", "operation_name"}:
            return config
        group = value if field_name == "group" else config["group"].get("value", "全部")
        selected = value if field_name == "operation_name" else config["operation_name"].get("value", "capabilities")
        catalog = RustWorkbench().capabilities()
        operations = catalog.get("result", {}).get("operations", []) if catalog.get("ok") else []
        if operations:
            config["group"]["options"] = ["全部"] + sorted({item["operation"].split(".", 1)[0] for item in operations})
        available = [item for item in operations if group == "全部" or item["operation"].split(".", 1)[0] == group]
        options = ["capabilities"] + sorted(item["operation"] for item in available)
        config["operation_name"]["options"] = options
        if selected not in options:
            selected = "capabilities"
        config["operation_name"]["value"] = selected
        spec = next((item for item in operations if item["operation"] == selected), None)
        if spec:
            config["payload_json"]["info"] = text(spec.get("payload", {}))
        elif not catalog.get("ok"):
            config["payload_json"]["info"] = catalog.get("error", {}).get("message", "无法读取 Rust 能力目录")
        return config

    def _act(self):
        previous = self.previous.data if self.previous else {}
        if previous.get("ok") is False:
            return {"ok": False, "operation": self.operation_name, "context": previous.get("context", {}),
                    "error": {"code": "upstream_failed", "message": "前一步失败；本次操作未执行。"}}
        context = context_of(self)
        context.update(object_value(self.context_json))
        if self.chain_id:
            context["chain_id"] = self.chain_id
        payload = resolve_values(object_value(self.payload_json), context)
        result = RustWorkbench().execute(self.operation_name, context, payload,
                                         allow_write=self.allow_write, allow_model=self.allow_model,
                                         allow_revoke=self.allow_revoke)
        self.status = result
        return result

    def run_action(self) -> Data:
        return Data(data=cached(self, self._act))

    def catalog(self) -> DataFrame:
        # Read-only and independent of the selected action; asking for this
        # output must never repeat a write or a model call.
        catalog = RustWorkbench().capabilities()
        if not catalog.get("ok"):
            return table(error_rows(catalog))
        operations = catalog["result"]["operations"]
        if self.operation_name != "capabilities":
            operations = [item for item in operations if item["operation"] == self.operation_name]
        elif self.group != "全部":
            operations = [item for item in operations if item["operation"].split(".", 1)[0] == self.group]
        return table([{"操作": item["operation"], "写入": item["write"], "模型": item["model"],
                       "参数": text(item.get("payload", {}))} for item in operations])
