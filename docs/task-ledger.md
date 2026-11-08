# 任务总账与工作台

`ledger.export`可以读取单链，也可以在同一Store事务内读取明确的任务范围。它不调用模型，不修改CB、op或协作状态。

## 范围选择

- `scope: "chain"`：只读取当前路线；省略scope也保留这一旧行为。返回值明确标为单链，不能当作整个多Agent任务的账单。
- `scope: "task"`：读取当前根链及其已记录的collaboration branches，递归纳入分支的协作后代。pending、waiting、failed及cancelled分支已有的调用都保留。
- `additional_chain_ids`：明确补充的其他路线及其协作后代。它不会自动发现所有Agent，也不会自动把普通fork、空白创建、creator关系或所有Socket端点归入任务。

每条纳入路线都检查当前actor的访问权限和source依赖的撤销状态。无权或已撤销的必需分支使整次导出失败，不静默漏账；分支的sender/request来源不一致也会被拒绝。

```json
{
  "operation": "ledger.export",
  "context": {"actor": "host", "chain_id": "YOUR_ROOT_CHAIN", "allow_write": false, "allow_model": false},
  "payload": {
    "scope": "task",
    "additional_chain_ids": [],
    "prices_by_chain": {},
    "task_interval": [100, 160]
  }
}
```

经已认证的Core `/api/v2/operations`调用。示例时间需替换为实际epoch秒；未知时省略task_interval。调用字段和返回结构见[`task_ledger.schema.json`](../sic_core/task_ledger.schema.json)。

## 价格、去重和未知项

任务模式使用各链本次快照的`config.prices`，或显式`prices_by_chain`。其值是“路线ID→费率数组”；费率保留category、model、role、unit、unit_price、per_quantity、currency和price_version。原账项中的显式费率优先。统一`prices`不能覆盖整个任务中不同路线的价格。当前配置/显式声明价会标明来源，不冒充已独立验证的历史Provider报价。

call_id和资源record_id分别标识一项实际调用/测量。同一原记录且相同价绑定只计算一次，引用路线保存在record_locations；身份相同但原记录或价绑定不同则返回`ledger_identity_conflict`。原始usage、模型/route及来源引用保留；Editor编辑阶次和Worker实际请求cut的活动表示可供追溯，缺失信息保持未知。

缺usage、费率或资源覆盖不填零。program/storage测量默认仅覆盖记录所在路线；共同测量可在resource.record中显式声明scope_chain_ids。不同实测量应使用不同record_id。已知在途attempt尚无usage时出现在pending_attempts，也使费用保持未知。多币种分别列总额，不自动换算相加。

返回值为当次观测快照，不预测尚未完成任务的未来费用；unfinished_branches和范围排除关系均显示。priced_calls/priced_resource_records冻结本次价绑定，可与原记录、范围和accounting模块独立核对。

## 时间含义

全任务模型累计时长与模型区间并集分别列出。task_interval是声明的真实任务墙钟区间，已知调用/工具区间超出它会被拒绝；省略时墙钟unknown。

`summary.timing.editor_tool_overlap_seconds`是**全任务**Editor与工具区间的交集，可能来自不同路线。它不能单独证明某个Agent利用了自己的自然工具等待窗口。`timing_by_chain`另列每条路线的时间和同链Editor/tool交集；工作台用task-wide与same-chain两组行区分。

例如Editor A在10–12秒工作、只有Agent B在10–12秒执行工具，全任务交集为2秒，而A、B各自的同链交集均为0。未闭合的工具区间标在timing.unknown，不伪造结束时刻。即便同链区间重叠，也还需结合实际上下文、调度和模型采用证据判断机会利用，不由时间交集推导物理缓存命中或费用持平。

## 官方语义工作台

Langflow的“SIC 调用账与条件费用对照”和Studio的`sic_accounting`提供同名范围字段：

- scope：chain或task。
- additional_chain_ids：明确补充的路线ID数组。
- prices_by_chain：每链版本化费率；chain模式使用原prices数组。
- task_interval：实际起止秒，工作台用空数组表示未知。

输出包括调用/资源明细、范围及各链费率、角色/阶次来源、未知成本，以及分开的全任务和同链时间表。不是任意operation/payload编辑器。未知字段别名、任务模式统一价格覆盖及无效区间会被拒绝，不静默改成其他参数。原“两路线账面对照”仍是两个单链快照的对照；它本身不证明同任务、同配置的研究收益。任务范围应分别读取并保留各自边界。

定向更新源画布：`python integrations/generate_workbench_flows.py --only accounting`。它只写账务flow，不覆盖runtime或其他域。源码、生成画布和服务内已载版本不同；本次修改没有重启现有Langflow/LangGraph。导入/重载后的实际GUI还需另行验收。

## 已验证边界

新核心独立测试覆盖递归peer、实际高阶job/Worker cut、每链价格、冲突/去重、撤权、未知费用、并集及事务一致性。隔离官方SDK验证执行了LFX组件、真实导出画布及Studio编译图，并验证未知别名拒绝、跨链交集负对照和缺测量unknown。验证仅使用自有记录与本地fixture，账务工作台调用模型数为0；没有请求真实Provider，没有据此生成质量成绩，也没有宣称常驻GUI已加载或视觉验收完成。
