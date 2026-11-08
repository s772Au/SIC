"""Generate actual official domain components and connected candidate workflow."""
import copy
import argparse
import inspect
import json
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from lfx.custom.utils import build_custom_component_template
from components.sic import (SICAction, SICNativeInput, SICWorkspace, SICCandidate, SICDecision,
                            SICMemory, SICConfiguration, SICRoutes, SICBoard, SICRuntime, SICAccounting, SICCoverage, SICEditorTask, SICHierarchy)

ROOT = Path(__file__).resolve().parent
CLASSES = {cls.__name__: cls for cls in (SICAction, SICNativeInput, SICWorkspace, SICCandidate, SICDecision, SICMemory,
                                       SICConfiguration, SICRoutes, SICBoard, SICRuntime, SICAccounting, SICCoverage, SICEditorTask, SICHierarchy)}


def make_workbench_flow(name, steps, connect=False):
    nodes, edges = [], []
    for index, step in enumerate(steps):
        cls = CLASSES[step["component"]]
        component = cls()
        component._code = Path(inspect.getfile(cls)).read_text(encoding="utf-8")
        template, _ = build_custom_component_template(component)
        schema = copy.deepcopy(template)
        schema["display_name"] = step.get("label", cls.display_name)
        for field, value in step.get("values", {}).items():
            schema["template"][field]["value"] = value
        if "action" in schema["template"] and hasattr(component, "update_build_config"):
            schema["template"] = component.update_build_config(schema["template"], schema["template"]["action"]["value"], "action")
        # Source artifacts never ship enabled writes or model grants.
        for field in ("allow_write", "allow_model"):
            if field in schema["template"]:
                schema["template"][field]["value"] = False
        node_id = f"{cls.__name__}-{index + 1}"
        nodes.append({"id": node_id, "type": "genericNode", "position": {"x": (index % 4) * 570, "y": (index // 4) * 850},
                      "data": {"id": node_id, "type": cls.__name__, "node": schema}})
        if connect and index:
            previous = nodes[index - 1]
            output = next(value for value in previous["data"]["node"]["outputs"] if value["name"] == "result")
            sh = {"dataType": previous["data"]["type"], "id": previous["id"], "name": "result", "output_types": output["types"]}
            th = {"fieldName": "previous", "id": node_id, "inputTypes": schema["template"]["previous"]["input_types"], "type": "other"}
            edges.append({"id": f"workbench-edge-{index}", "source": previous["id"], "target": node_id,
                          "sourceHandle": json.dumps(sh), "targetHandle": json.dumps(th),
                          "data": {"sourceHandle": sh, "targetHandle": th}, "animated": False})
    return {"name": name, "description": "官方SIC语义工作台。固定CB/op只读；仅候选与授权配置可改。各输出提供表格与真实版本。",
            "is_component": False, "data": {"nodes": nodes, "edges": edges, "viewport": {"x": 0, "y": 0, "zoom": 0.6}}}


def candidate_steps(context=None):
    context = context or {"chain_id": "SELECT_ROUTE", "source_id": "SELECT_SOURCE", "job_id": "SELECT_DELIVERED_JOB"}
    previous = {"context": context}
    return [
        {"component": "SICWorkspace", "label": "1 原文 / 保护协议 / 固定CB对照", "values": {"chain_id": context["chain_id"], "source_id": context["source_id"]}},
        {"component": "SICCandidate", "label": "2 修订已交付候选（不再调用模型）", "values": {
            "chain_id": context["chain_id"], "job_id": context["job_id"], "action": "修订操作与 P/V/I", "expected_revision": 0,
            "operation_number": 1, "op": "REPLACE", "target": "u1", "content": "在此修改候选正文", "reason": "人工核对来源后的修订",
            "persistent_text": "本次P说明", "volatile_text": "下一WB的V线索", "instant_views": json.dumps({"operator_note": "当前I说明"}, ensure_ascii=False)}},
        {"component": "SICCandidate", "label": "3 校验此候选版本", "values": {"action": "校验候选", "expected_revision": -1}},
        {"component": "SICCandidate", "label": "4 明确发布 — CB/op随后不可改", "values": {"action": "发布此版本", "expected_revision": -1, "expected_cut": 0}},
        {"component": "SICMemory", "label": "5 沿形成时序定位固定结果", "values": {"action": "CB形成时序"}},
        {"component": "SICMemory", "label": "6 从固定CB回到WB与op锚", "values": {"action": "从CB找WB"}},
        {"component": "SICAccounting", "label": "7 读取真实调用账 / unknown", "values": {}},
    ]


def hierarchy_steps(context=None, members=None, level=2, context_id='main'):
    context=context or {'chain_id':'SELECT_ROUTE'}
    return [
        {'component':'SICHierarchy','label':'1 有序下阶CB形成真实高阶WB','values':{'chain_id':context['chain_id'],'context_id':context_id,'level':level,'action':'form','cb_ids':'\n'.join(members or ['SELECT_LOWER_CB_1','SELECT_LOWER_CB_2'])}},
        {'component':'SICEditorTask','label':'2 冻结高阶编辑范围/角色背景','values':{'action':'准备单Editor'}},
        {'component':'SICEditorTask','label':'3 已授权Editor单轮交付（模型默认关）','values':{'action':'并发执行所有Editor'}},
        {'component':'SICCandidate','label':'4 可改候选P/V/I或切换局部op表单','values':{'action':'修改 P/V/I','expected_revision':0,'persistent_text':'本次高阶表示说明','volatile_text':'','instant_views':json.dumps({'hierarchy_note':'当前高阶候选'})}},
        {'component':'SICCandidate','label':'5 校验候选版本','values':{'action':'校验候选','expected_revision':-1}},
        {'component':'SICCandidate','label':'6 固定新高阶CB（下阶不改）','values':{'action':'发布此版本','expected_revision':-1,'expected_cut':-1}},
        {'component':'SICHierarchy','label':'7 Worker及Editor实际采用（形成新cut）','values':{'action':'select','context_id':context_id,'level':level,'roles':'worker\neditor'}},
        {'component':'SICHierarchy','label':'8 本阶WB独立形成链','values':{'action':'wb_timeline','context_id':context_id,'level':level}},
        {'component':'SICHierarchy','label':'9 本阶CB独立发布链及对应','values':{'action':'cb_timeline','context_id':context_id,'level':level}},
        {'component':'SICHierarchy','label':'10 展开同阶WB/有序下阶成员/原单元','values':{'action':'expand_cb'}},
        {'component':'SICHierarchy','label':'11 全op正文回放（不重放外部工具）','values':{'action':'replay','step':-1}},
        {'component':'SICAccounting','label':'12 全角色/阶次真实调用账','values':{}},
    ]


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--only',choices=['accounting','runtime','requirements'],help='Export only this domain; leave all other flows untouched')
    args=parser.parse_args()
    if args.only:
        cls={'accounting':SICAccounting,'runtime':SICRuntime,'requirements':SICCoverage}[args.only]
        flow=make_workbench_flow(f'SIC 工作台 — {args.only}',[{'component':cls.__name__,'values':{} if cls is SICCoverage else {'chain_id':'SELECT_ROUTE'}}])
        (ROOT/'flows'/f'workbench-{args.only}.json').write_text(json.dumps(flow,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
        print(f'workbench-{args.only}.json',len(flow['data']['nodes']))
        return
    flows = {"workbench-candidate-lifecycle.json": make_workbench_flow("SIC 工作台 — 已交付候选修订与固定发布", candidate_steps(), connect=True)}
    flows['workbench-higher-edit-publish.json'] = make_workbench_flow('SIC 真实升阶 — 有序成员到固定高阶CB及角色采用', hierarchy_steps(), connect=True)
    for name, classes in {
        "source": (SICNativeInput, SICWorkspace), "candidate": (SICEditorTask, SICCandidate), "decision": (SICDecision,),
        "memory": (SICMemory,), "routes-board": (SICRoutes, SICBoard), "configuration": (SICConfiguration,),
        "runtime": (SICRuntime,), "accounting": (SICAccounting,), "requirements": (SICCoverage,),
        "hierarchy": (SICHierarchy,),
    }.items():
        steps = [{"component": cls.__name__, "values": {"chain_id": "SELECT_ROUTE"} if cls != SICCoverage else {}} for cls in classes]
        flows[f"workbench-{name}.json"] = make_workbench_flow(f"SIC 工作台 — {name}", steps)
    for name, flow in flows.items():
        (ROOT / "flows" / name).write_text(json.dumps(flow, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(name, len(flow["data"]["nodes"]))


if __name__ == "__main__":
    main()
