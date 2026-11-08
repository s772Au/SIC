# SIC 新核心：运行、编辑与验收

本目录对应新的 Python `sic_core`，不是旧 Rust 的改名。设计依据为草稿 `03382521af9390b76cbd30b9dc8a288012fafa0e5840fd28ca38b374f2d5c511` 和功能契约 `495c4247b7748dcc47475c7bc5394f3890f208558a20ed7bb806c28f66ec1cdc`。旧实验、旧测试数字及旧桥接不作为本核心的完成证据。

当前基线为F01–F73，旧65项覆盖表已作废。WB/CB各自时序、同阶配对与真正N阶编辑见[递归核心](hierarchy-core.md)；实际接入和未验收项见[当前验收事实](runtime-acceptance-current.md)。测试总数不替代逐功能证据。

## 启动与共享实例

核心存储、规则、HTTP 使用 Python 标准库；实际模型适配另装固定版本：

```powershell
python -m pip install -r sic_core/requirements.txt
# 私下配置 SIC_CORE_TOKEN 和实际 provider 的凭据环境变量，不写进流程或源码。
python -m sic_core.server --db .local/core-v2.db --config .local/core-v2-config.json --port 18888 --bridge-port 18890
```

`--bridge-port` 可选；启用时控制面与 Chat 桥共享同一个 Core/SQLite/Runtime。桥的具体DSH绑定与宿主验收由其接入文档说明，不能把普通 Core HTTP 成功当成实际 Harness 消费成功。自有 fixture 演示与真实 provider 配置须明确区分。

私有配置以 [config.example.json](../sic_core/config.example.json) 为起点。`tokens` 形状为 `{"SIC_CORE_TOKEN":"host"}`，其键是环境变量名，值是身份；没有明文密钥。Bearer 绑定身份，`context.actor` 必须一致。每次写操作还需 `allow_write:true`；模型动作另需 `allow_model:true`。模型授权、工具执行、Socket投递、历史读取、Signal及共享板权限分别检查。

嵌入使用 `Core(db, config).operate(operation, context, payload)`。传输层最后关闭后调用 `core.close()`，它停止并等待运行时，再关闭数据库；单独桥退出不能关闭仍供控制面使用的 Core。默认 loopback；远程部署由宿主提供 TLS、身份与网络控制。

## 操作接口

`GET /api/v2/capabilities` 与 `POST /api/v2/operations` 均需鉴权。[operations.json](../sic_core/operations.json) 列出当前动作、写/模型门、scope和最小payload，运行时以实际能力目录为准。统一请求及响应：

```json
{"operation":"chain.create","context":{"actor":"host","allow_write":true,"allow_model":false},"payload":{"creators":[]}}
```

返回 `{ok,operation,context,result,evidence}`；新ID直接位于 `result.chain_id/source_id/job_id/cut_id/checkpoint_id/socket_id`。错误包含 HTTP 状态及 `{ok:false,error:{code,message}}`。原生Chat桥另用OpenAI错误外壳。成功、重送、模型是否实际被调用和缓存未知状态分别表达。

## 从来源到可修订候选与固定CB

`source.ingest` 程序decode正文与独立协议sidecar，正文可包含代码、表格和任务JSON。原始文本、角色、原生工具关联与控制信息不交给Editor修改。可带 `idempotency_key`；相同键同内容复送不会重复材料/事件，改内容复用键拒绝。没有键的两次相同文本可以是两个真实输入，不按文本相同擅自去重。

`wb.form` 以连续pending来源组形成WB，记录成员、边界原因及事件；不可重组已冻结job的范围。长文本可拆成有字符锚的单元，KEEP重新组装时保持原字符，原生sidecar仍保留完整原请求。输入单位大小是可配工程参数，不是作者指定阈值。

`edit.program` 主动执行精确去重、空白移除、换行规范化；`edit.prepare` 冻结任务、scope、背景manifest、模型/提示/规则/预算。可用 `program_rules` 将明确机械处理与语义scope组合，`edit.run_all` 真正并发执行不重叠Editor，各自一次完整响应。冲突、越界、过时背景、截断或非法批次拒绝，不隐形发起反馈修复轮。

若需早期原块或完整op，在prepare指定 `readonly_reads:[{kind,object_id,offset,limit}]`。程序在模型调用前真实读取、冻结为独立档案读取记录；Editor的 `readonly_archive_reads` 可见这些材料。它们不成为旧CB可编辑target，也不把单轮任务变成返回后修复循环。

模型原始响应独立存档。`edit.candidate.get {job_id}` 给纯显式 `ops/pvi`、`revision` 和 `original_responses`。`edit.candidate.update {job_id,expected_revision,ops,pvi}` 生成新的immutable修订记录；不改原始模型响应或来源。`cut.publish` 可带 `expected_candidate_revision`，防止UI审阅后候选又变化。人工修订不是又调用一次Editor。

可组合KEEP、DELETE、FLAG、REORDER、COMPRESS、REPLACE、ADD，顺序有意义。ADD可带 `anchor:{target,position:before|after}`。ADD/COMPRESS/REPLACE/FLAG生成内容可选 `lifecycle:P|V|I`，I须给 `view_key`；操作账本本身始终P。`risk` 可为none/low/medium/high/unknown，风险声明不自动授予权限或证明语义无损。

发布事务共同固定CB正文、有序有效op、原始提案/候选修订引用、WB映射及时序，接入P、将V放入下一WB、替换I并确认消息。旧CB不可UPDATE/DELETE，长期维护不做摘要升阶或根替换。CB中的proposal引用可由模型用 `sic_memory_page(kind:proposal,object_id:cb_id)` 取得原始模型有序op与人工修订链；旧I只在显式档案读取时出现，不自动积入CB。

Worker正有在途请求时，发布返回 `publication_deferred`，候选保留，先采用合法旧basis回复再发布。Signal暂停后，网络响应可保留成candidate，但发布和Worker继续均停止；可审阅、CAS修订、唤醒后续做。

## P/V/I、决策、有限工作投影

P进入本次稳定CB，V进入下一WB，I在稳定CB后独立呈现且排除于下一WB载荷；I不是强制整个HTTP末尾。原档案和逻辑链保持完整，单次有限工作投影单独记录。

`decision.get/update/maintain` 维护Working on、Failed Before、Not Tried Yet、Overlooked四视角。条目有状态与证据，完成事件指向目标，遗漏视角保留“What have I missed?”提示。用户材料和真实工具失败可驱动程序维护；Editor可在I中更新有来源的四视角。当前视图与其历史依据分开；软件fixture不证明真实模型已达到语义理想。

`memory.timeline` 按指定链成员顺序返回前驱/后继与原形成路线，`memory.locate` 双向解析CB/WB/source及块内op锚。`memory.page` 分页读取source、cb、ops、proposal、view、tool_result；ops/proposal等按JSON字符页拼接后可还原完整对象，单位与 `next_offset` 明示。

Worker与Editor使用显式bounded projection：保留所选背景/片段，给archive数量、选入及未读范围、内容/操作页范围、导航路径和策略版本，不静默slice档案。更换早期活动投影会改变请求前缀，必须入前沿/费用记录。超大的原块仍可分页取回；Editor先按需预取，再在有限窗口单轮处理。

默认模型工具 `sic_source_read/search/operation_directory` 返回有界页、带定位的短匹配或简明目录，完整正文/op/提案可继续分页。其他可能过大的内置工具返回持久 `tool_result` 引用，不能用一次工具结果再次撑爆有限输入。host的完整source.read/cut.get仍可供审阅，不等于模型每轮全读。

## 运行时、工具窗口与协作

共享 `core.runtime` 提供 `notify/step/run/start/stop/drain`。`runtime.configure` 开启事件驱动；模型编辑或自动Worker需要明确standing模型授权。`worker_mode:host` 只自动编辑，由DSH宿主拥有Worker调用；automatic才主动续做Worker。不同路线由独立worker槽并行，同一路线不重入，存储锁不跨网络调用。事件队列与处理进度持久化。自治Worker每项任务有可配worker_turns预算（默认64），协作分支受branch_budget约束（默认4）；达到预算明确停止并可由宿主延展，新外部用户任务才重置，模型不能靠自发回传无界循环。

`harness.event` 接真实tool_start/end、等待声明与完成/取消；调用提议不冒充实际执行。等待剩余时间、Editor估计、工具状态和预算进入调度，改变等待、重叠或交错选择。每WB额外推进预算冻结，幂等调度返回单次grant；先调度再调用Worker不重复扣减。逻辑预算不是provider miss数。

`Runtime.register_tool(name, host_callback)` 是嵌入宿主提供外部工具执行的方法；schema和prompt不能注入可执行代码。实际开始/结束由执行器记录。Worker工具运行时可真正dispatch Editor，结果后闭合原生关联、发布就绪CB并继续Worker。无宿主执行器时明确等待工具结果；不能返回成功占位。未知外部副作用不静默重发。

`frontier.compare` 在同cut/任务下对照必要KEEP替换与附加编辑投影，报告前缀、长度预算、顶层schema/教学变化。字节序列、逻辑稳定区和provider内部KV布局分开；控制字段可能在messages前，不能统一声称I/schema更新免费。

Agent身份取逻辑链。`chain.checkpoint` 可选历史cut；`chain.list/get` 提供授权路线/历史点目录。peerfork共享起点但有本链cut/ack，继承当时V/I/配置，原路线不被改写。空白创建才记录一个或多个creator。

Socket的message/signal/spawn ACL分离。投递权不升级成历史读取权。`collaboration.dispatch` 向选定历史端点安排问题，`collaboration.run` 让分身实际执行Worker和Editor并发布，再把有来源结果送回P/V；原路线可独立推进。请求、分身、结果及消费cut关联。失败有明确failed终态，同ID复送不隐形重试；同scope失败也不会因队列重复事件另开Editor任务，须显式恢复或fresh prepare；只有显式retry_failed及不确定重复确认才新开attempt。`runtime.recover(action:cancel_collaboration)` 取消未完成分支，迟到结果不覆盖取消。

Signal影响真实调度，公示牌采用共享board_id/CAS，每次cut取最新获准状态。`board.claim` 提供任务、认领者、进度的原子认领；不同分支不能静默抢占。默认工具可实际调用记忆、决策、路线、Socket及Signal动作，仍通过宿主ACL。

## 计量、恢复与支持范围

`ledger.export` 导出版本化原始/归一usage、调用身份、request引用、tokenizer准入信息和时间事件。`resource.record` 记录真实测得的程序/存储/其他服务用量与计量方法；费率含单位、币种与价格版本。缺测量/usage/类别拆分/费率保持未知，不报告零成本成功。累计模型时间、任务墙钟、真实工具与Editor重叠分开。新实现比较runner和scorer另行验收，不移用旧Rust数据。

`runtime.recover` 仅作显式确认的失败/不确定状态处置，不调用模型；它关闭自动运行供审阅，再通过明确run或重新授予standing策略继续。敏感源撤销是独立宿主授权tombstone，封锁原文、派生CB、旧cut、job副本、工具和分叉，不重写CB，不等于物理擦除或追回已外发字节。

| 路径 | 真实支持边界 |
|---|---|
| 输入decode与请求preview | Chat文本/文本块/function tools；Responses文本/function call/output含同批多call；Anthropic文本/tool_use/result的明确子集。未知载体/控制拒绝，不冒称完整原生协议。 |
| Worker/Editor调用 | 固定角色配置→进程内LiteLLM completion，canonical Chat响应；不重造外置必选gateway。原生多格式透明响应另看桥矩阵。 |
| 预算 | 按宿主选定tokenizer完整计数，不截断；缺预算或未知选型拒绝。LiteLLM表收录仍不证明与真实模型tokenizer完全一致。 |
| 供应商/缓存/质量 | 未验证真实provider、真实物理KV命中或任务收益；这些须用户新实验。自有mock仅说明机制/接口行为。 |

适配库固定LiteLLM1.103.10，实测OpenAI SDK2.54.0。官方依据：[Python SDK](https://docs.litellm.ai/docs/python_sdk)、[token usage及fallback说明](https://docs.litellm.ai/docs/completion/token_usage)、[上游代码与许可证](https://github.com/BerriAI/litellm)（本轮使用的常规SDK适用MIT；enterprise目录另有条款）。

## 新测试与契约覆盖证据

```powershell
python scripts/check_core_v2.py --json-output .local/core-check.json
python scripts/check_core_v2.py --require-sdk --json-output .local/core-sdk-check.json
```

用上述命令在SDK环境执行完整新Core回归；无SDK环境明确跳过相关测试，require-sdk模式不允许靠skip报完成。当前运行结果见本地JSON报告及当前验收事实；旧次数不随源码自动有效。计数不是F01–F73逐项覆盖证明。关键证据分别为：

- `test_core_v2_mechanisms.py`：candidate CAS及原response不变、组合op/内容PVI、并发Editor和程序规则、四视角、长链有界投影/完整分页、原始提案模型读取、同cut窗口对照、真实工具重叠、Signal中途暂停/修订/恢复、独立路线并行、分身实际续做/回传/失败取消、输入幂等与费用未知。
- `test_core_v2_atomicity.py`：逐持久写点故障回滚、同key复送、分支自身cut/ack。
- `test_core_v2_sdk.py`：真实SDK Editor→CB→Worker、并行与人工候选修订、冻结profile、异Worker/Editor固定模型、容量/未知tokenizer拒绝；12块大档案→源读取工具→下一Worker在4000输入token/8192context预算内实际走通。
- `test_core_accounting.py`：互斥类别、provider差异、价格/资源未知、并行时间与条件成本计算。
- 桥的连续会话、真实DSH宿主、官方Langflow/LangGraph表单及常驻浏览器验收由对应集成工件记录；本指南不以核心fixture代替那些证据。

当前真实DSH往返、进程重启及两个常驻工作台的综合结果与剩余缺口见[运行验收](runtime-acceptance-current.md)。

本轮无真实provider推理、无8341推理/实验调用，不生成Mock质量分。语义空间被动邻近交流、模型推理/训练内化仍是设计稿中的研究设想；显式协作不以它们充数。
