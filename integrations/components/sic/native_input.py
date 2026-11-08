from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import BoolInput, CodeInput, DataInput, DropdownInput, MultilineInput, StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.workbench_support import cached, error_rows, object_value, perform, reset, table, text


class SICNativeInput(Component):
    display_name = "SIC 新工作材料"
    description = "将新材料送入真实解码器；原始存档随后只读。宿主保护字段不交给Editor编辑。R07–11。"
    icon = "FileInput"
    name = "SICNativeInput"
    inputs = [DataInput(name="previous", display_name="前一步", required=False),
              StrInput(name="chain_id", display_name="工作路线", value=""),
              DropdownInput(name="input_mode", display_name="输入方式", options=["工作正文", "原生请求（高级）"], value="工作正文"),
              DropdownInput(name="native_format", display_name="原生格式", options=["openai-chat", "openai-responses", "anthropic-messages"], value="openai-chat"),
              CodeInput(name="working_text", display_name="本次新正文（代码/JSON字符原样）", value="", info="只作为正文读取，不执行代码。"),
              MultilineInput(name="protected_text", display_name="宿主保护指令（新路线可选）", value="", info="不是对旧保护前缀的修改入口；冲突由核心拒绝。"),
              CodeInput(name="native_request", display_name="完整原生输入对象（高级）", value="{}", advanced=True),
              BoolInput(name="allow_write", display_name="允许保存本次新来源", value=False)]
    outputs = [Output(name="result", display_name="已保存来源与WB身份", method="ingest", group_outputs=True),
               Output(name="units", display_name="解码后的正文单元", method="units", group_outputs=True)]

    def _pre_run_setup(self):
        reset(self)

    def _act(self):
        if self.input_mode == "原生请求（高级）":
            request = object_value(self.native_request)
        elif self.native_format == "openai-responses":
            request = {"input": [{"role": "user", "content": self.working_text}]}
            if self.protected_text:
                request["instructions"] = self.protected_text
        else:
            request = {"messages": [{"role": "user", "content": self.working_text}]}
            if self.protected_text:
                if self.native_format == "anthropic-messages":
                    request["system"] = self.protected_text
                else:
                    request["messages"].insert(0, {"role": "system", "content": self.protected_text})
        return perform(self, "source.ingest", {"format": self.native_format, "request": request})

    def ingest(self) -> Data:
        return Data(data=cached(self, self._act))

    def units(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        return table([{"来源WB": envelope["result"].get("source_id"), "单元": unit["unit_id"], "正文（存档后只读）": unit["text"],
                       "原生位置": text(unit.get("anchor", {})), "下一步": "检查正文/协议后形成编辑候选"}
                      for unit in envelope["result"].get("units", [])])
