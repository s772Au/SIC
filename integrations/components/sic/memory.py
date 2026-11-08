from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import DataInput, DropdownInput, IntInput, StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.workbench_support import cached, context_of, error_rows, perform, reset, table, text


class SICMemory(Component):
    display_name = "SIC 时序、来源与片段导航"
    description = "分别沿CB生成时序和CB↔WB来源导航，再读原文/正文/op片段；有限投影明确列未读档案。R18/R29–34。"
    icon = "LibraryBig"
    name = "SICMemory"
    inputs = [DataInput(name="previous", display_name="前一步", required=False),
              StrInput(name="chain_id", display_name="工作路线", value=""),
              DropdownInput(name="action", display_name="导航方向", options=["CB形成时序", "从WB找CB", "从CB找WB", "读取原文片段", "读取媒体原档片段", "读取通信来源片段", "读取冻结前缀计划", "读取实际模型请求", "读取固定CB片段", "读取持久op片段", "读取原始提案与修订链", "读取历史I片段", "有限背景投影"], value="CB形成时序", real_time_refresh=True),
              DropdownInput(name="cb_id", display_name="固定CB（刷新当前时序页）", options=[""], value="", refresh_button=True, combobox=True),
              DropdownInput(name="source_id", display_name="来源WB（刷新当前时序页）", options=[""], value="", refresh_button=True, combobox=True),
              StrInput(name="cut_id", display_name="历史I所属cut", value=""),
              StrInput(name="prefix_plan_id", display_name="冻结计划ID（只读；来自配置/Editor任务）", value=""),
              StrInput(name="provider_request_id", display_name="实际请求ID（只读；非准备计划）", value=""),
              StrInput(name="communication_id", display_name="通信来源WB / 快照cut（空读当前cut）", value="", info="只读来源、首次接收时点和收据；不是信箱消息ID。"),
              DropdownInput(name='view_scope', display_name='I读取范围', options=['当前cut默认','指定内部上下文/阶次','历史角色选择'],value='当前cut默认'),
              StrInput(name='context_id',display_name='I内部上下文',value='main'),
              IntInput(name='level',display_name='I阶次',value=1),
              StrInput(name='view_role',display_name='历史角色选择（如editor:alpha）',value='editor'),
              IntInput(name="offset", display_name="起始位置 / 记录下标", value=0),
              IntInput(name="timeline_size", display_name="本页时序条目（最多100）", value=20),
              IntInput(name="page_size", display_name="片段字符预算（最多8192）", value=2048),
              DropdownInput(name="role", display_name="投影供谁读取", options=["worker", "editor"], value="worker"),
              IntInput(name="max_chars", display_name="投影字符预算", value=4096),
              StrInput(name="query", display_name="优先查找线索", value="")]
    outputs = [Output(name="result", display_name="导航结果与可继续位置", method="navigate", group_outputs=True),
               Output(name="records", display_name="时序 / 对应 / 片段表", method="records", group_outputs=True)]

    def _pre_run_setup(self):
        reset(self)

    def update_build_config(self, build_config, field_value, field_name=None):
        if field_name == "action":
            visible = {"CB形成时序": {"offset", "timeline_size"}, "从WB找CB": {"source_id"}, "从CB找WB": {"cb_id"},
                       "读取冻结前缀计划": {"prefix_plan_id", "offset", "page_size"}, "读取实际模型请求": {"provider_request_id", "offset", "page_size"}, "读取原文片段": {"source_id", "offset", "page_size"}, "读取媒体原档片段": {"source_id", "offset", "page_size"}, "读取通信来源片段": {"communication_id", "offset", "page_size"}, "读取固定CB片段": {"cb_id", "offset", "page_size"},
                       "读取持久op片段": {"cb_id", "offset", "page_size"}, "读取原始提案与修订链": {"cb_id", "offset", "page_size"}, "读取历史I片段": {"cut_id", "offset", "page_size"},
                       "有限背景投影": {"role", "max_chars", "query"}}[field_value]
            if field_value=='读取历史I片段':visible |= {'view_scope','context_id','level','view_role'}
            for key in ("source_id", "cb_id", "cut_id", "communication_id", "prefix_plan_id", "provider_request_id", "offset", "timeline_size", "page_size", "role", "max_chars", "query",'view_scope','context_id','level','view_role'):
                build_config[key]["show"] = key in visible
            build_config["offset"]["display_name"] = "路线条目起点" if field_value == "CB形成时序" else "字符起点（按返回unit）"
        if field_name in {"source_id", "cb_id"}:
            page = perform(self, "memory.timeline", {"offset": self.offset, "limit": min(self.timeline_size, 100)})
            if page.get("ok"):
                rows = page["result"]["rows"]
                build_config["cb_id"]["options"] = [""] + [row["cb_id"] for row in rows]
                build_config["source_id"]["options"] = [""] + list(dict.fromkeys(source for row in rows for source in row["source_ids"]))
        return build_config

    def _act(self):
        context = context_of(self)
        source_id = self.source_id or context.get("source_id")
        cb_id = self.cb_id or context.get("cb_id")
        if self.action == "CB形成时序":
            operation, payload = "memory.timeline", {"offset": self.offset, "limit": self.timeline_size}
        elif self.action in {"从WB找CB", "从CB找WB"}:
            operation = "memory.locate"
            payload = {"source_id": source_id} if self.action == "从WB找CB" else {"cb_id": cb_id}
        elif self.action == "有限背景投影":
            operation, payload = "memory.project", {"role": self.role, "max_chars": self.max_chars, "query": self.query}
        elif self.action in {"读取冻结前缀计划", "读取实际模型请求"}:
            kind = "prefix_plan" if self.action == "读取冻结前缀计划" else "provider_request"
            object_id = self.prefix_plan_id if kind == "prefix_plan" else self.provider_request_id
            operation, payload = "memory.page", {"kind": kind, "object_id": object_id, "offset": self.offset, "limit": self.page_size}
        elif self.action == "读取通信来源片段":
            object_id = self.communication_id or context.get('cut_id')
            if not object_id:
                current = perform(self, 'chain.get', context=context)
                if not current.get('ok'):
                    return current
                object_id = current['result'].get('current_cut')
            if not object_id:
                return {'ok': False, 'context': context, 'error': {'code': 'cut_required', 'message': '尚无固定cut；也可明确选择通信来源WB。'}}
            operation, payload = 'memory.page', {'kind': 'communication', 'object_id': object_id, 'offset': self.offset, 'limit': self.page_size}
        else:
            kind = {"读取原文片段": "source", "读取媒体原档片段": "media", "读取固定CB片段": "cb", "读取持久op片段": "ops", "读取原始提案与修订链": "proposal", "读取历史I片段": "view"}[self.action]
            object_id = source_id if kind in {"source", "media"} else (self.cut_id or context.get("cut_id")) if kind == "view" else cb_id
            operation, payload = "memory.page", {"kind": kind, "object_id": object_id, "offset": self.offset, "limit": self.page_size}
            if kind=='view':
                if self.view_scope=='指定内部上下文/阶次':payload.update(context_id=self.context_id,level=self.level)
                elif self.view_scope=='历史角色选择':payload['role']=self.view_role
        return perform(self, operation, payload, context=context)

    def navigate(self) -> Data:
        return Data(data=cached(self, self._act))

    def records(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        result = envelope["result"]
        if envelope["operation"] == "memory.timeline":
            return table([{"当前路线位置": row["membership_index"], "固定CB": row["cb_id"], "原形成路线": row["formed_on_chain"],
                           "原形成顺序": row["original_sequence"], "前一CB": row["predecessor"], "后一CB": row["successor"],
                           "WB": row.get("wb_id"), "来源": text(row["source_ids"]), "正文线索": row["excerpt"],
                           "下页起点": result.get("next_offset")} for row in result["rows"]])
        if envelope["operation"] == "memory.locate":
            rows = []
            for row in result["rows"]:
                for unit in row.get("unit_index", []) or [{}]:
                    rows.append({"固定CB": row["cb_id"], "WB": row.get("wb_id"), "来源": text(row["source_ids"]),
                                 "路线内位置": row["membership_index"], "源单元": unit.get("unit_id"), "源锚": text(unit.get("anchor", {})),
                                 "对应op下标": text(unit.get("operation_indexes", [])), "总op数": row["operation_count"], "待发布": result.get("pending", False)})
            return table(rows)
        if envelope["operation"] == "memory.page":
            value = result["value"]
            if result["kind"] == "ops" and isinstance(value, list):
                rows = [{"记录下标": result["offset"] + i, "操作": op.get("op"), "目标": text(op.get("target", op.get("targets", ""))),
                         "内容": op.get("content", ""), "锚": text(op.get("anchor", {})), "来源": op.get("origin", op.get("editor", "")),
                         "不可变": True, "完整摘要": result["full_digest"], "下页起点": result.get("next_offset")} for i, op in enumerate(value)]
            else:
                rows = [{"对象": result["object_id"], "种类": result["kind"], "起点": result["offset"], "计数单位": result["unit"],
                         "完整长度": result["total"], "片段正文（只读）": value, "完整摘要": result["full_digest"], "下页起点": result.get("next_offset")}]
            return table(rows)
        manifest = result["manifest"]
        rows = [{"固定CB": cb["cb_id"], "本次读到的正文": cb["body"], "本次op数": len(cb["ops"]),
                 "档案导航": text(cb.get("archive_navigation", {})), "策略": manifest["policy"], "角色": manifest["role"]} for cb in result["cbs"]]
        rows.append({"固定CB": "未读档案 / 继续导航", "本次读到的正文": "未读不是已读；原始固定档案不改写", "策略": manifest["policy"],
                     "角色": manifest["role"], "档案导航": text({key: manifest[key] for key in ("archive_count", "omitted_count", "fragment_cb_ids", "max_chars", "presented_chars", "navigation")})})
        if manifest.get('legacy_unindexed_count',0):
            rows.append({'固定CB':'旧档案索引状态','旧档案数量':manifest['legacy_unindexed_count'],'WB形成顺序':'unknown',
                         '档案导航':text(manifest.get('legacy_unindexed_navigation',{})),'说明':'未保存层级/时序不补造；已知CB顺序与原WB/source仍可读取'})
        return table(rows)
