from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import DataInput, StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.workbench_support import cached, context_of, error_rows, perform, reset, table, text


class SICWorkspace(Component):
    display_name = "SIC 正文与协议对照"
    description = "只读核查来源正文、角色和工具关联，及其固定CB结果；这里不修改原文或协议。"
    icon = "Columns2"
    name = "SICWorkspace"
    inputs = [DataInput(name="previous", display_name="前一步", required=False),
              StrInput(name="chain_id", display_name="工作路线", value=""),
              StrInput(name="source_id", display_name="来源WB", value="", info="可沿前一步context继承source_id。")]
    outputs = [Output(name="result", display_name="来源与固定视图", method="read_workspace", group_outputs=True),
               Output(name="comparison", display_name="正文 / 协议 / CB 对照表", method="comparison", group_outputs=True),
               Output(name="protocol", display_name="只读协议控制", method="protocol", group_outputs=True)]

    def _pre_run_setup(self):
        reset(self)

    def _load(self):
        context = context_of(self)
        source_id = self.source_id or context.get("source_id")
        if not source_id:
            return {"ok": False, "context": context, "error": {"code": "source_required", "message": "请选择来源WB；原始存档只读。"}}
        source = perform(self, "source.read", {"source_id": source_id}, context=context)
        if not source.get("ok"):
            return source
        cut = perform(self, "cut.get", context=context)
        if not cut.get("ok"):
            return cut
        return {"ok": True, "operation": "workspace.inspect", "context": {**source["context"], "source_id": source_id},
                "result": {"source": source["result"]["source"], "cut": cut["result"]},
                "evidence": {"source": "sic-core-v2", "read_only": True, "editable_original": False}}

    def read_workspace(self) -> Data:
        return Data(data=cached(self, self._load))

    def comparison(self) -> DataFrame:
        envelope = cached(self, self._load)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        source = envelope["result"]["source"]
        messages = source.get("sidecar", {}).get("messages", [])
        source_id = envelope["context"]["source_id"]
        cbs = [cb for cb in envelope["result"]["cut"].get("view", {}).get("cbs", []) if source_id in cb.get("source_ids", [cb.get("source_id")])]
        rows = []
        for unit in source.get("units", []):
            anchor = unit.get("anchor", {})
            message_index = anchor.get("message", -1)
            message = messages[message_index] if 0 <= message_index < len(messages) else {}
            rows.append({"来源WB": source_id, "单元": unit.get("unit_id"), "原始正文（只读）": unit.get("text", ""),
                         "角色（只读）": message.get("role", "unknown"), "工具关联（只读）": text({key: message[key] for key in ("tool_call_id", "tool_calls") if key in message}),
                         "来源锚": text(anchor), "固定CB": ", ".join(cb["cb_id"] for cb in cbs),
                         "固定CB正文（只读）": "\n\n".join(cb.get("body", "") for cb in cbs),
                         "状态": "已发布 / 不可改写" if cbs else "尚未发布 / 编辑须通过候选"})
        return table(rows)

    def protocol(self) -> DataFrame:
        envelope = cached(self, self._load)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        source = envelope["result"]["source"]
        sidecar = source.get("sidecar", {})
        rows = [{"类别": "原生控制", "位置": key, "内容（只读）": text(value), "Editor可改": False}
                for key, value in sidecar.items() if key != "messages"]
        for index, message in enumerate(sidecar.get("messages", [])):
            fields = {key: value for key, value in message.items() if key != "content"}
            rows.append({"类别": "受保护指令" if message.get("role") in ("system", "developer") else "角色 / 关联",
                         "位置": f"messages[{index}]", "内容（只读）": text({**fields, **({"content": message.get("content")} if message.get("role") in ("system", "developer") else {})}),
                         "Editor可改": False})
        rows.append({"类别": "未闭合调用", "位置": "source.open_calls", "内容（只读）": text(source.get("open_calls", [])), "Editor可改": False})
        return table(rows)
