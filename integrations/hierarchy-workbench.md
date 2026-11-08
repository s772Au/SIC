# 双链与递归官方工作台

本入口绑定新Core；内部context不创建新Agent。每阶WB形成链与CB发布链分别查询，以真实同阶配对和有序跨阶成员导航；不把Source目录当作WB链。

## Langflow

官方本机服务默认 http://127.0.0.1:17861 ，保持正常认证；私有登录信息在`.local/langflow-17861/admin-auth.json`，不要将其复制到flow。

导入`flows/workbench-higher-edit-publish.json`。节点1选真实chain、context、目标阶次及**连续有序下阶CB ID**。12节点依次形成真实高阶WB、冻结Editor范围、执行单轮Editor、修订候选P/V/I、校验、发布、采用、查询两条链、展开、正文op回放、计量。导出写入门和模型门全部关闭；明确授权对应节点才执行。节点4可以切换typed操作编辑；模型原响应始终只读。完整链包含写操作；成功后只查看输出，继续工作须形成新WB/新job，勿把整图再次Run当只读刷新。发布沿已核对job/candidate/cut版本检查，冲突时刷新后重审，不盲目取最新版本。

`workbench-hierarchy.json`提供单动作语义表单：上下文总览、WB/CB两链、形成、展开、角色采用、递归程序维护、私有材料、旧档案、正文回放。动态显示对应字段，表输出实际配对、成员顺序、读取定位与角色选择。`workbench-candidate.json`的Editor节点可指定alpha/beta分工，beta允许显式空编辑scope；仍可读取其冻结背景。`SICEditorTask`只读表分别呈现每位Editor的实际背景/I以及原响应。

## LangGraph Studio

服务 http://127.0.0.1:12026 ；官方Studio入口 https://smith.langchain.com/studio/?baseUrl=http://127.0.0.1:12026 。常驻API健康不等于Studio GUI已可用或验收。

选择`sic_hierarchy`，typed `form.action=form`提交chain、context、level和cb_ids，在interrupt核对只读事实后resume授权表单。用`sic_editor` prepare/run_all，随后`sic_candidate` revise（支持pvi_only、append、remove）、validate、publish；expected_cut=-1引用已冻结job基线，expected_revision仍须核对。回`sic_hierarchy` select，分别查询wb_timeline/cb_timeline，expand_cb逐级读取，replay step=0或-1对比。持久thread保留每步输入、interrupt、修改和结果；Studio checkpoint本身不是Agent fork。

私有Editor路线：create_context（如editor-notes、共享main）→ingest_context→正常编辑发布→maintain或手动form编辑发布→select roles=[editor:alpha]。另选main低阶roles=[editor:beta]，再用`sic_editor` two_editors=true、两个名字及各自单位范围准备并执行，比较真实角色背景。共享或私有内部层级不改变Agent身份。

## 旧档案与只读边界

`legacy`仅展示已保存CB顺序；未索引记录显示level=null、legacy-unindexed、WB形成顺序unknown，不补造高阶/原始WB时序。有限背景的manifest展示legacy_unindexed_count及legacy_unindexed_navigation，使用该导航分页读取。原始Editor已见单元与发布时补充来源分列。

固定CB/op/原响应无写入入口；纠正走新候选/PVI。`replay`只回放普通编辑正文前缀，不重新执行工具，不声称恢复完整cut或I。敏感撤销/不可逆处理仍是独立授权路径。

## 持续短块与终轮

Runtime配置中`max_pending_seconds`默认1秒，可设有限正数至86400。它约束真实pending短块的持久最长等待，不是每次轮询重置；实际宿主work_end终轮刷新由Core处理。前端不伪造宿主终轮事实。运行状态表保留实际政策和事件供检查。

## 可重放证据

加载私有环境后，在项目根运行：

```powershell
. integrations/use-core-demo-environment.ps1
.local/langflow-ui-venv/Scripts/python.exe integrations/tests/hierarchy_runtime_acceptance.py
```

它仅允许自有loopback fixture，创建新测试路线，不重启服务，不改既有GUI提案；实际导入/执行12节点官方LF流程及Studio持久interrupt/resume，并检查同Core存储、真实角色请求、三级展开、原WB读取与原响应不变。详细ID/trace保留`.local/integration-test-results/hierarchy-runtime-acceptance.json`供GUI查看，不加入公共导出。

隔离Core+真实HTTP+官方LFX/Studio SDK测试：`integrations/tests/hierarchy_workbench_acceptance.py`。其实际构造旧存储fixture验证null/unknown导航，不伪称所有生产旧档均已迁移。两类测试均明确GUI=false；新层级域GUI由另行实际目视验收确认。既有一次候选GUI成功不能替代新双链/高阶/私有上下文GUI。

## 本轮载入边界

本轮两端已重载层级主图、候选job/cut绑定、alpha/beta空scope分工与max_pending_seconds。随后新增的Studio `sic_memory` 历史I分页context/level/role选择字段仅落源码；第二次ownedLG重载被自动审查以`blocked by policy`拒绝，未执行，不换方式绕过。这一小域尚未载入常驻/未实测；独立`sic_hierarchy`中的context/level双链、选择和展开主路径已载入。新LF导出包含显式宿主work_end选项，但不将表单选择当实际终轮证据。

## 本轮最终实际结果

最终同Core常驻回放通过：12节点官方LF实际执行，两个连续有序下阶成员→真实二阶WB/Editor/候选PVI/发布/采用；Studio API持久thread的interrupt/resume完成三阶同生命周期及逐级3→2→1原WB读取、正文回放。固定下阶CB和原Editor响应保持不变。私有三阶Editor上下文形成并被alpha实际模型请求采用，beta实际请求不含它；Worker实际请求含main三阶表示。max_pending_seconds=0.5实际配置读回通过。5次自有回环fixture模型调用，零真实provider；质量不由该机械回放评定。

上述是官方服务API实际执行证据，新层级LF GUI与Studio GUI均未验收。Studio I历史页新scope字段未载入；LF宿主work_end新选项存在于最新导出组件代码，但注册表/GUI尚未单独核验。隔离测试通过11次自有fixture调用及真实旧存储导航；17项集成单元测试通过。来源SHA与分域状态见`hierarchy-acceptance.json`。它只证明所列路径，不证明新73项全部完成。
