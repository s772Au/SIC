import json
from pathlib import Path

from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.v2_client import CoreClient
from sic_workflow.workbench_support import table


class SICCoverage(Component):
    display_name = "SIC F01–F73验收与缺口"
    description = "逐需求显示真实证据范围；接口/节点/测试数量不替代需求完成，两个官方GUI分别记录。"
    icon = "ClipboardCheck"
    name = "SICCoverage"
    inputs = [StrInput(name="requirement_filter", display_name="需求ID或关键词（空则全部）", value="")]
    outputs = [Output(name="requirements", display_name="逐需求覆盖表", method="requirements", group_outputs=True),
               Output(name="capabilities", display_name="当前核心实际能力与缺失", method="capabilities", group_outputs=True)]

    def requirements(self) -> DataFrame:
        import sic_workflow
        root = Path(sic_workflow.__file__).resolve().parent.parent
        coverage = json.loads((root / "functional-acceptance.json").read_text(encoding="utf-8"))
        query = self.requirement_filter.casefold()
        return table([{"需求":row['id'],"行为":row['title'],"输入":row['inputs'],"操作":row['operation'],"输出":row['outputs'],
                       "机械范围":row['mechanical_acceptance']['scenario_summary'],
                       "GUI":row['gui_acceptance']['status'],"GUI边界":row['gui_acceptance']['boundary'],
                       "研究":row['research_evaluation']['status'],
                       "完整通过":row['mechanical_acceptance']['entire_requirement_declared_passed'],
                       "通过条件":json.dumps(row['pass_conditions'],ensure_ascii=False),
                       "来源":json.dumps(row['sources'],ensure_ascii=False)}
                      for row in coverage['features'] if not query or query in json.dumps(row,ensure_ascii=False).casefold()])

    def capabilities(self) -> Data:
        return Data(data={"capabilities": CoreClient().capabilities(), "claim": "Core declaration; requirement acceptance is listed separately"})
