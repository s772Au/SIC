import json
from pathlib import Path
import sys

from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import BoolInput, CodeInput, DataInput, DropdownInput, StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.workbench_support import cached, context_of, error_rows, object_value, perform, reset, table, text
from sic_workflow.accounting_scope import ledger_payload,ledger_tables


class SICAccounting(Component):
    display_name = "SIC 调用账与条件费用对照"
    description = "单链或明确任务范围的真实总账。task只递归已记录协作分支及补充路线，不自动抓所有Agent；全任务交集可能跨路线，不代表同Agent免费窗口。"
    icon = "ChartNoAxesCombined"
    name = "SICAccounting"
    inputs = [DataInput(name="previous", display_name="前一步", required=False),
              StrInput(name="chain_id", display_name="当前路线", value=""),
              DropdownInput(name="scope", display_name="账务范围", options=["chain","task"], value="chain", info="task包含根链已记录协作分支及明确补充路线，不自动选择所有Agent。"),
              CodeInput(name="additional_chain_ids", display_name="明确补充的路线ID数组", value="[]"),
              CodeInput(name="prices_by_chain", display_name="任务各链版本化费率（路线ID→费率数组）", value="{}"),
              CodeInput(name="task_interval", display_name="实际任务区间 [开始秒,结束秒]；未知 []", value="[]"),
              DropdownInput(name="action", display_name="账目动作", options=["读取完整调用账", "两路线账面对照", "读取独立核验实验", "记录资源测量"], value="读取完整调用账"),
              StrInput(name="baseline_chain", display_name="基线路线", value=""),
              StrInput(name="sic_chain", display_name="SIC路线", value=""),
              StrInput(name="run_directory", display_name="已完成实验的项目相对目录", value="", info="先由独立validator核SHA和两臂不变量，再读对照；不执行模型。"),
              CodeInput(name="prices", display_name="版本化费率（数组，可空）", value="[]", advanced=True),
              StrInput(name="record_id", display_name="测量记录ID（重送沿用）", value="measurement-1"),
              DropdownInput(name="resource", display_name="实测资源", options=["program", "storage", "other-service"], value="program"),
              StrInput(name="quantity", display_name="实际测量用量（十进制）", value="0"),
              StrInput(name="unit", display_name="测量单位", value="cpu-second"),
              StrInput(name="measurement_method", display_name="测量方法与范围", value=""),
              StrInput(name="unit_price", display_name="单价（未知留空）", value=""),
              StrInput(name="currency", display_name="币种（未知留空）", value=""),
              StrInput(name="price_version", display_name="价格版本/来源", value=""),
              BoolInput(name="allow_write", display_name="允许记录本次实测资源", value=False)]
    outputs = [Output(name="result", display_name="核心账 / 独立核验对照", method="run_accounting", group_outputs=True),
               Output(name="charges", display_name="调用 / 资源 / 费率明细", method="charges", group_outputs=True),
               Output(name="unknowns", display_name="总账、时间与未定项", method="unknowns", group_outputs=True)]
    outputs += [Output(name="scope_details",display_name="范围 / 链价 / 角色阶次",method="scope_details",group_outputs=True),
                Output(name="timing_details",display_name="全任务与同链区间（分别展示）",method="timing_details",group_outputs=True)]

    def _pre_run_setup(self):
        reset(self)

    def set(self,**kwargs):
        unknown=set(kwargs)-{item.name for item in self.inputs}
        if unknown:raise KeyError('Unknown accounting input field: '+', '.join(sorted(unknown)))
        return super().set(**kwargs)

    def _act(self):
        context = context_of(self)
        if self.action == "读取独立核验实验":
            import sic_workflow
            project = Path(sic_workflow.__file__).resolve().parents[2]
            relative = Path(self.run_directory)
            if not self.run_directory or relative.is_absolute() or ".." in relative.parts:
                return {"ok": False, "context": context, "error": {"code": "relative_run_required", "message": "请选择项目内相对run目录，不接受任意绝对路径或上溯。"}}
            directory = (project / relative).resolve()
            directory.relative_to(project)
            scripts = str(project / "scripts")
            if scripts not in sys.path:
                sys.path.insert(0, scripts)
            from verify_v2_experiment import validate_run
            verified = validate_run(directory)
            if verified["status"] != "passed":
                return {"ok": False, "context": context, "error": {"code": "experiment_incomplete", "message": "仅有失败工件；不能显示为完整比较。"}}
            comparison = json.loads((directory / "comparison.json").read_text(encoding="utf-8"))
            return {"ok": True, "operation": "accounting.verified_artifact", "context": context, "result": comparison,
                    "verification": verified, "evidence": {"source": "independent-run-validator", "quality": "not_evaluated"}}
        payload=ledger_payload({'scope':self.scope,'prices':object_value(self.prices,list),'additional_chain_ids':object_value(self.additional_chain_ids,list),'prices_by_chain':object_value(self.prices_by_chain,dict),'task_interval':object_value(self.task_interval,list)})
        if self.action == "两路线账面对照":
            if self.scope!='chain':raise ValueError('两路线账面对照分别取单链；任务总账请按各根路线分别读取，不能暗中合并补充范围。')
            from sic_core.accounting import compare_workflows
            baseline = perform(self, "ledger.export", payload, context={**context, "chain_id": self.baseline_chain})
            if not baseline.get("ok"):
                return baseline
            sic = perform(self, "ledger.export", payload, context={**context, "chain_id": self.sic_chain})
            if not sic.get("ok"):
                return sic
            comparison = compare_workflows(baseline["result"]["summary"], sic["result"]["summary"])
            return {"ok": True, "operation": "accounting.route_comparison", "context": context, "result": comparison,
                    "evidence": {"source": "core-ledger-and-accounting", "paired_task_verified": False,
                                 "boundary": "两条路线的账面对照本身不证明同任务/同配置研究收益"}}
        if self.action == "记录资源测量":
            record = {"record_id": self.record_id, "resource": self.resource, "quantity": self.quantity,
                      "unit": self.unit, "measurement_method": self.measurement_method}
            if self.unit_price:
                record.update(unit_price=self.unit_price, currency=self.currency, price_version=self.price_version)
            saved = perform(self, "resource.record", record, context=context)
            if not saved.get("ok"):
                return saved
        return perform(self, "ledger.export", payload, context=context)

    def run_accounting(self) -> Data:
        return Data(data=cached(self, self._act))

    @staticmethod
    def _summaries(result):
        if "baseline" in result and "sic" in result:
            return [("baseline", result["baseline"]), ("sic", result["sic"])]
        return [("current", result.get("summary", result))]

    def charges(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        rows = []
        for arm, summary in self._summaries(envelope["result"]):
            for item in summary.get("rows", []) + summary.get("resource_rows", []):
                rows.append({"工作流": arm, "调用/测量": item.get("call_id") or item.get("record_id"), "角色/资源": item.get("role") or item.get("resource"),
                             "模型": item.get("model"), "互斥类别": item.get("category"), "用量": item.get("quantity"), "单位": item.get("unit"),
                             "单价": item.get("unit_price", "unknown"), "价格版本": item.get("price_version", "unknown"), "币种": item.get("currency", "unknown"),
                             "金额": item.get("cost") if item.get("cost") is not None else "unknown"})
        return table(rows)

    def scope_details(self) -> DataFrame:
        envelope=cached(self,self._act)
        if not envelope.get('ok'):return table(error_rows(envelope))
        detail=ledger_tables(envelope['result']);rows=[]
        for kind in ('scope_rows','price_rows','context_rows'):rows.extend({'分组':kind,**item} for item in detail[kind])
        return table(rows)

    def timing_details(self) -> DataFrame:
        envelope=cached(self,self._act)
        return table(ledger_tables(envelope['result'])['timing_rows']) if envelope.get('ok') else table(error_rows(envelope))

    def unknowns(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        result = envelope["result"]
        rows = []
        for arm, summary in self._summaries(result):
            rows.append({"工作流": arm, "项目": "总账", "内容": text({key: summary.get(key) for key in ("complete_cost", "total_cost", "currency", "totals_by_currency", "call_count")})})
            rows.append({"工作流": arm, "项目": "时间口径", "内容": text(summary.get("timing", {}))})
            for unknown in summary.get("unknown", []):
                rows.append({"工作流": arm, "项目": "未定项（不是0）", "内容": text(unknown)})
        if "baseline" in result:
            rows.append({"工作流": "comparison", "项目": "差额 / 条件 / 质量", "内容": text({key: result.get(key) for key in ("delta_cost", "delta_cost_reason", "delta_wall_seconds", "controlled_comparison", "controlled_terms", "quality")})})
        return table(rows)
