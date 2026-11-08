# SIC Core 层级表示：当前实现与验证边界

本文描述 `sic_core/hierarchy.py` 及其与现有编辑、投影、存储的接入，不是全部功能契约已经通过的声明。尤其不把SDK协议夹具作为真实LLM质量、成本收益或论文实验结果。

## 对象、顺序与身份

一条 `chain_id` 是一条Agent工作路线。同一路线可以包含 `main` 和Editor独有的内部 `context_id`；增加内部context、Editor或表示阶次不会创建新Agent。

每个context的每阶保存独立的 `wbs` 形成序列、`cbs` 发布序列、`pairs[wb_id] = cb_id`、待编辑 `pending` 和本阶最新 `instant`。WB对象记录本阶形成序号、来源、单元、形成时间及有序下阶CB成员。原始source片段的装配独立于这两条链。

`hierarchy.form` 完整复制指定连续下阶CB的正文成为真正可编辑的高阶WB，并在单元锚中记录下阶CB。随后复用现有Editor/程序编辑、候选修订和发布机制，生成新的高阶CB及固定op和索引。高阶对象不会覆盖下阶WB、CB或op。阶次没有写死为二阶；专项测试实际运行到五阶。

高阶source保留对下阶source的依赖。读取高阶CB时递归检查这些依赖，因此独立撤销原始source权限后，高阶表示和历史分支不能借该路径恢复被撤销内容。

## 操作

以下操作均使用现有 `Core.operate(operation, context, payload)`。`context` 中的actor、route权限和write/model授权仍由宿主管理。

| 操作 | 主要输入 | 当前行为 |
| --- | --- | --- |
| `hierarchy.get` | 无 | 返回各阶双链、配对、内部context、活动角色选择。此操作供完整检查，不是自动塞入模型的无界目录。 |
| `hierarchy.form` | `level >= 2`, `cb_ids`, 可选 `context_id` | 要求有序、连续、同下阶成员及形成顺序；返回新WB、source和完整编辑单元。 |
| `hierarchy.timeline` | `level`, `kind: wb/cb`, `offset`, `limit`, 可选 `context_id` | 分页展示两条形成链的前后继和同阶对应；不以source列表代替WB顺序。 |
| `hierarchy.expand` | `cb_id` 或 `wb_id` | 返回同阶对应、完整归档单元、有序下阶成员及继续展开/完整分页所需引用。 |
| `hierarchy.select` | `level`, `roles`, 可选 `context_id` | 选择Worker、通用Editor或 `editor:alpha` 等具体Editor的活动表示；建立新历史cut。 |
| `hierarchy.maintain` | `fanout >= 2`, 可选 `rules`, `roles`, `context_id` | 持续把未处理的下阶区间组成高阶WB，执行真实程序编辑并发布；必要时继续升阶，最后采用生成的阶次。 |
| `hierarchy.context.create` | `context_id`, 可选 `shared_contexts` | 在同Agent内部创建持久context；只能共享已存在context，引用关系保持无环。 |
| `hierarchy.context.ingest` | `context_id`, `request.messages` | 保留Editor独有的闭合正文材料并形成其一阶WB；拒绝把受保护指令或未闭合工具协议当作这种正文。 |
| `hierarchy.legacy` | `offset`, `limit` | 分页列出缺少新层级元数据的旧CB；明确旧WB形成时序未知。 |
| `memory.page` | `kind`, `object_id`, `offset`, `limit` | 完整读取source、CB、op、候选/原响应等；高阶和私有context均受同route范围约束。高阶I支持指定 `context_id/level` 或历史角色选择 `role`。 |
| `memory.replay` | `cb_id`, 可选 `step` | 在CB冻结的完整source成员上回放指定op前缀。末步实际比较CB正文；不执行模型、工具或外部动作。 |

形成高阶WB后，调用方使用现有 `edit.prepare` / `edit.run` / `edit.submit` / `edit.candidate.*` / `cut.publish`。`edit.program` 提供程序路径。高阶job继续具有原响应、模型调用记录、单轮scope、候选修订、索引和发布前检查。

`hierarchy.maintain` 默认使用精确去重规则。程序可以原样保留不同正文，因此此动作不保证缩短内容；需要语义压缩时，应使用上述模型编辑路径。实际形成并编辑的新高阶CB，与有预算的正文投影/检索摘录是两个不同对象。

## 实际角色采用与持续编辑

`role_views` 记录角色选择的context和阶次。Worker实际调用的 `_working_view` 从已发布高阶表示覆盖相应连续下阶区间，保留未覆盖材料。Editor按身份冻结自己的 `editor_backgrounds` 与 `editor_instants`，实际 `edit.run` 请求使用该快照。多个Editor仍保留各自原有模型绑定、范围和任务解释。

Editor独有context可以共享main或其他已存在context的表示引用；无需复制另一条Agent路线。调用方可将Editor持续材料送入该context，经同样的WB→CB循环递归处理。当前不自动把所有Editor内部推理提取为私有材料。

有限投影明确报告省略/片段和完整阅读入口。`sic_hierarchy_expand`、`sic_hierarchy_timeline`、`sic_memory_page`、`sic_memory_replay` 与 `sic_hierarchy_legacy` 是新route上的相应模型工具。Editor维持一次完整响应的模式，需要的补充原文可在prepare时通过 `readonly_reads` 备齐。

## 发布、历史与生命周期

各阶共用同一Agent的cut版本和费用账本。过期背景、候选版本冲突、顺序冲突及Worker在途时的发布/切层受到检查。发布在一次存储事务中写入CB、WB对应、V、I、cut和幂等收据；失败回滚，重试不会产生第二个CB。

P固定到本阶CB；V成为本阶下一WB；I更新本阶最新视图，并可从历史cut分页恢复。主路线Mailbox的P/V仍在主一阶发布边界收取，不能被高阶后台发布提前消费。回放返回的是正文操作结果；它明确不声称仅靠op恢复全部PVI-only视图或整个cut。

切换活动表示本身生成新的cut，因此checkpoint/fork继承当时完整层级和角色选择。新route随后独立推进，不读取原route现在的头。

WB在明确形成边界保留成员快照。若已有冻结Editor输入后又收到原生工具闭合材料，发布记录额外保留这些来源及独立程序KEEP来源。展开时原WB成员与publication补充来源分开显示，避免把后到材料伪称Editor已经读过。

## 旧档案兼容

旧CB继续保留在主路线CB目录及实际Worker/Editor投影中；升级后追加新CB不会丢弃旧CB。旧CB仍可经原目录、定位、完整source/CB/op分页和正文回放读取。

旧数据没有保存的WB形成序号或时间不会补造。缺少新阶次契约的记录显示 `hierarchy_index_status: legacy-unindexed`、`level: null` 和 `wb_formation_order: unknown`。`hierarchy.expand` 仍返回已保存的CB顺序、CB↔WB引用、完整单元及source分页入口；不会因旧WB缺level字段向前端抛出未处理异常。

模型投影只增加有界的 `legacy_unindexed_count` 和分页导航，不输出全部旧ID目录。`hierarchy.legacy` 每页最多100行。旧route不会被静默增加已撤回的工具权限；缺少新工具时，导航可使用其既有 `sic_operation_directory` 和source阅读工具，宿主/UI可直接调用新只读展开API。若这些工具也没有执行授权，导航如实标为无可用模型工具。

新显式阶次对象仍强制WB元数据、形成链成员与同阶配对；不能用legacy fallback掩盖新数据不完整。

## 已有验证和证据范围

- `tests/test_core_v2_hierarchy.py`：独立两链及配对、多source原文、真实高阶编辑/续链、各阶PVI和候选、发布回滚/幂等/并发/过期、传递撤销、历史fork、磁盘恢复、Editor私有context和逐op公共回放。
- `tests/test_core_v2_hierarchy_sdk.py`：实际LiteLLM SDK连接本机自有临时HTTP夹具。主测试实际完成三阶编辑、Worker/Editor采用、逐阶工具展开、两页原文与下一Worker请求；第二测试实际完成私有Editor三阶和alpha/beta各自选择。
- `tests/test_core_v2_hierarchy_legacy.py`：人工按旧持久格式构造无层级快照、旧WB缺阶次信息的档案，验证旧+新两角色实际视图、未知标记、分页、回放、范围拒绝及有界目录。
- 原始SDK证据：本地 `.local/contract-rebuild/hierarchy-sdk-evidence.json` 保存两次执行的完整请求/响应、WB/CB/source/cut、同Agent账本和当时源码hash。两次分别有10与6次SDK请求，均与账本逐调用对应。该证据保留其生成时的旧freeze hash；后续兼容补丁的hash另行冻结。
- 兼容补丁后的证据：本地 `.local/contract-rebuild/hierarchy-compatibility-evidence.json` 另外保存4项旧态验证、重新执行的上述2项SDK原始数据以及当前源码hash。兼容补丁后的全 `test_core_v2*` 回归为114项通过、16项子测试通过；只有SDK依赖自身的event-loop弃用警告。

这些SDK调用没有访问外部模型服务。夹具证明协议、来源、实际请求采用和生命周期闭环，不证明真实LLM能否正确总结或自主找到答案，也不证明全部功能契约、两官方GUI或DSH宿主已经在该版本上通过；这些由各自独立验收负责。
