# Rust 精简版逐项验收（2026-11-10，进行中）

本文只记录**精简版**（为 CTFy 项目特化的交付线）的证据。真实 Provider 的验证限定为
`deepseek-v4.1-flash-eco`；模拟测试不能替代真实模型或生产验收。精简版的取舍——单
Editor、固定 DSH 别名、受限协议/多模态与受控调度——只约束这条线；完整版以
`CONTRACT-STATUS.md` 的 F01–F73 为准，不受这些取舍限制，两线证据不能互相替代。

| 要求 | 现状与证据 | 仍需完成或验证 |
|---|---|---|
| 单 Editor；Worker 等待 | 同一 Agent 同时只运行一个 Editor；已调度的任务在 open、inflight、ready 或 failed 状态下，Worker 均不能越过发布继续。Editor 可按固定模型配置，对明确收到的 500/502/503/504 在既定总尝试额度内延迟重发；调度预算、每次调用账务和重发前的 claim／权限核对有专项测试。另在真实 DSH 分身与固定模型联测中注入一次合成 503，后续真实模型调用、原生 `read` 和 Socket 回传成功，失败调用用量保持未知。`tests/editor_runtime.rs`、`tests/editor_http_retry.rs`；工具任务 created/claimed/completed 时间与 Editor 重叠/先后顺序由 `tests/tool_timing.rs`、`tests/tool_editor_overlap.rs`、`tests/tool_editor_wait.rs`、`tests/tool_editor_catchup.rs` 覆盖（工具延迟由测试宿主模拟）。 | 未验证长期运行、异常恢复后的所有调用路径。默认不启用该重发；429、超时和响应不明不自动重试。额度耗尽后需显式处理，不能静默跳过。 |
| Worker/Editor 共同前缀、不同 I | 相同的保护指令、CB 表示和固定工具表；宿主在其后的 I 中激活不同模式、作用域与权限；Editor 非法调用会进入有界纠错。逻辑摘要和计划范围摘要分开记录，路由、控制或缓存命名空间不同时不混称同一范围。`tests/prefix.rs`、`tests/editor_runtime.rs`。 | 宿主声明的缓存命名空间不等于 Provider 证据；逻辑前缀相同不证明物理缓存命中。不同模型或配置下的缓存效果未知。 |
| WB/CB 双链和高阶 | 保存顺序、同阶对应和来源；支持高阶 WB 引用低阶 CB、逐层展开、选择及有界自动调度。真实模型一次形成四个一阶、两个二阶、一个三阶 CB。`tests/representation_switch.rs` 另证明 Worker 请求实际使用所选层级，切回低阶时逻辑前缀 digest 改变、再切回时恢复。`tests/core_flow.rs`、`tests/bounded_flow.rs`。 | 长期运行、复杂来源和所有高阶选择组合还未充分验证。 |
| 记忆检索 | 宿主 `memory.search` 与 Worker `sic_memory_search` 可按阶分别精确搜索待处理来源、已发布 WB 来源和 CB 正文；待处理命中区分 WB 是否形成并明确尚无 CB，来源命中返回锚点、CB/WB、物化单元、操作序号和迟到来源标记，CB 正文命中不会虚构来源单元。隔离测试覆盖二阶来源对低阶 CB 的引用、CB 中的 P、待处理到发布及游标失效。`tests/memory_search.rs`。前一工具表版本曾完成真实 DSH 两轮合成任务并核对一阶来源检索。 | 不是语义搜索；长链性能未验收。待处理、高阶、CB 正文和后续工具表变化尚未做真实模型验收；工具表变化可能造成上游前缀缓存重建。 |
| Worker 内部上下文边界 | Worker 的来源分页、操作回放、层级展开和时间线限制在当前选中的内部上下文；宿主仍可检查该 Agent 的其他上下文。隔离模拟测试覆盖对另一上下文的拒绝和当前上下文读取：`tests/worker_memory_core.rs`。 | 共享内部上下文 DAG 尚未实现；Editor 私有递归可由一次 `runtime.schedule` 自动形成 L2/L3（`tests/editor_private_recursion.rs`），但真实模型/DSH 与取消恢复未验收；这一边界测试不等于所有跨角色权限场景验收。 |
| 模型工具长输出 | Worker 层级展开按单元分页并限制每条文本预览，操作回放按 Unicode 字符分页，保留完整正文摘要；宿主接口仍可读取完整结果。消息列表限制数量和预览长度，已投递消息可按正文分页，其他路线和未投递消息不可读取。24 万字符合成 WB 和长中文消息分别验证完整数据可回读：`tests/worker_memory_core.rs`、`tests/worker_ipc_core.rs`。 | 其他 SIC 工具、DSH 外部工具结果与所有工具调用合计尚无统一字节预算；长上下文性能及真实模型分页行为未验收。 |
| 原生工具大结果投影 | Worker 请求中的纯文本工具结果默认单条预览最多 32,768 字符、合计最多 131,072 字符，优先最近结果；被缩减内容有明确标记，原始消息、来源及后续 Editor 输入保持完整，Worker 可调用 `sic_source_text_page` 分页读取。宿主可配置阈值或关闭；隔离 mock 覆盖多结果预算、新读取页的优先级、完整摘要和 DSH bridge 工具结果续接：`tests/tool_result_projection.rs`、`tests/dsh_volatile.rs`。 | 这是工具结果的字符预算，不是整个请求的字节上限；标记、非纯文本或其他上下文仍可能超限。真实 DSH／Provider 与模型是否按需读全、任务质量尚未验收。 |
| 编辑操作和审计 | 支持 KEEP、DELETE、FLAG、REORDER、COMPRESS、REPLACE、ADD；保留候选、操作和回放，迟到材料不伪称 Editor 已见；冻结后晚到的 tool 结果以显式 program KEEP 记录，Editor 快照仍是冻结范围。`tests/editing.rs`、`tests/core_flow.rs`、`tests/f69_program_keep.rs`。 | 实验尚未衡量复杂任务的信息保留质量。 |
| 持久、易失、即时信息 | P 写入新 CB，V 先作为待处理来源供下一 Worker 使用，在下次编辑边界与后续材料一起形成 WB；I 随发布刷新，原 CB 不被覆写。隔离 DSH bridge + 模拟 Provider 测试覆盖 V 被 Worker 看到、随新输入进入下一 WB、被后续 Editor 删除，以及外部工具往返中重启且不重复派发。模拟旧 Rust 状态中，旧的未编辑 V WB 被保留并标记为被新 WB 取代。`tests/dsh_volatile.rs`、`tests/core_flow.rs`、`tests/board_core.rs`。 | V 连续流尚未用真实 DSH／真实 Provider 验收；旧库兼容仅覆盖模拟的未编辑 V WB，用户现有数据库未改动或验证。需继续扩大长链、跨阶及异常条件验证。 |
| 动态工具搜索和扩展 | 宿主维护目录与独立权限；模型可分页搜索、请求展开，或在获授权的调用时自动展开；目录可超过 128 项，I 区只列前 20 个工具名。`tests/dynamic_runtime.rs`、`tests/worker_tool_core.rs`、`tests/dsh_bridge.rs`。 | 工具 schema 的完整语义校验仍依赖执行宿主；DSH 目录总大小受 1 MiB 上限约束。 |
| 动态工具回收 | 15 个已完成 Worker 轮次未使用，或固定核心工具与展开动态工具的完整 schema 总数将超过 128 时，回收合适的旧工具；当前 25 个固定核心工具占用总额度，动态 schema 最多 103 个。模型可请求回收指定工具，在途和固定工具受保护。`tests/toolset.rs`、`tests/tool_store.rs`。 | 长时间真实 DSH 会话中的回收行为尚未做生产负载验收。 |
| 决策树 | 四种视角、宿主/模型权限与修订冲突检查已接入；模型不能自行证明任务完成。`tests/decision.rs`、`tests/worker_ipc_core.rs`。 | 完整的多 Agent 任务编排与真实任务质量未验收。 |
| Socket 和历史分身 | 可基于已发布 CB 检查点创建 Socket 和分身，历史读取按权限分页；分身任务有持久记录、宿主原子认领和有界 Editor 准备。真实 DSH 的 watcher（默认串行）已在隔离测试中发现任务，由固定 OpenDesign 模型驱动分身的 Editor/Worker 展开并调用 DSH 原生 `read`，读取合成文件，结果经 Socket 回传并被请求路线的 CB 消费；请求方启动使用模拟模型，最终 CB 采用使用程序 fixture。watcher 另有有界并发与持久失败治理（每轮候选 1–8、同时运行 1–4，默认均为 1；终态失败冷却后隔离，需宿主 `ack`），目前只有合成 runner 测试，真实 DSH 并发未验收。另有全模拟及真实模型单任务合成工具验收。`tests/ipc_core.rs`、`tests/peer_watch.rs`。 | **创建分身不等于分身已执行任务**；真实模型 watcher 仅覆盖一次受限只读文件测试。其他业务工具、并发协作及长期运行尚未验收；生产工具的参数和文件权限仍由 DSH 宿主负责。 |
| Signal / Message | 权限校验、Signal 状态切换和 P/V 消息在 CB 发布时收取已有本机测试；旧 Signal 重放不回退新状态。宿主 `signal.wait` 和 Worker `sic_signal_wait` 支持按 ID 或序号游标等待 Signal，最多 30 秒；等待时释放数据库事务；同一 Core 实例的写入主动唤醒，其他实例通过有界轮询观察持久收据，每实例最多 32 个并发等待。取消 Worker 时不会继续采用旧结果，但保留已返回用量。真实 DSH 分身在固定 OpenDesign 模型与模拟模型下均完成指定 Signal 等待、接收另一 Agent 的唤醒、调用内置 `read` 和回传；真实模型测试的请求方启动使用模拟模型，流程由测试任务明确指引。`tests/ipc_core.rs`、`tests/signal_wait.rs`。 | 非零 Worker 等待须为本轮唯一工具调用；混合调用会得到明确的工具错误。Signal 等待不自动修改 Agent 的 `run_state`；宿主可另用 `message.wait` 等待消息到达，不改变消息的 CB 投递规则。当前不等待公示牌变更。完整进程间协议和长期并发负载仍待验收。 |
| 公示牌 | 独立 ACL、CAS、认领与进度、宿主确认完成；每次 CB 发布时把最新公示牌放入 I。`tests/board_core.rs`、`tests/board_security_audit.rs`。 | 跨真实 DSH 分身的协作场景未验收。 |
| DSH 主 Agent 工具往返 | 真实 DSH 0.2.1-rc.5、Rust Core 和 OpenDesign 固定模型完成 Editor → CB → Worker → DSH 执行只读合成工具 → Worker 采用结果；WB 根来源及工具输出状态修复后再次通过真实模型验收。2026-11-12 隔离 DSH 0.2.2-rc.1 用固定 `deepseek-v4.1-flash-eco` 完成原生 `read` 两轮与 `write`+`read` 一轮（同一 session、Core 重启）以及两个顺序 peer、两个并发真实 DSH 子链（`read`/`write`）、四个、六个以及八个并发真实 DSH peer（read/write/glob/grep/read/write/read/glob）和真实 `glob`/`grep` 子链、一个 mid-run cancel 子链、一个 provider 500 child 失败样本的 `peer.task.reply` 均被父链后续 cut 采用；父链同一 DSH session 继续输出 `CONCURRENT-A`/`CONCURRENT-B`，并复用 cut1 早期 checkpoint 完成 `EARLY-READY` peer，另验证 failed turn 后的 session 恢复；HTTP500 经 indeterminate → retirement → `runtime.discard_unpublished` → 新 session 同链恢复（cut0 `HTTP500-RECOVERED`、cut1 `LATER-RECOVERED`）；bridge_restarted 未知 turn 经同样路径输出 `UNKNOWN-RECOVERED`；第二次留下 open job 时显式 `allow_open=true` 输出 `UNKNOWN2-RECOVERED`；见 `research/real-evidence-20261112.md`。另用本机模拟 Provider 验证同一 Bridge 会话连续 24 轮、其中一次中途重启、历史和 CB 不变与交付重放。 | live 验收仍只证明 `read`/一个 peer 的限域路径，未证明其他应用工具、多 peer 或长任务；24 轮属于模拟模型测试，未证明任意真实应用工具或长任务。此前一次收到 Provider HTTP 500 时没有自动重试；现可按上面的 Editor 策略选择有界重发。 |
| DSH headless 启动入口 | `scripts/run-dsh-headless.ps1` 为单次任务启动独立 Rust Core 和隔离 DSH profile，可显式引用现有 DSH 凭据、按工具名授权并输出含会话 ID 的 JSON 事件。本机模拟模型下完成工具往返、重启后同一会话续用，并验证缺失凭据、并发启动和未托管目录会被拒绝；扫描确认未把合成凭据复制到状态目录。固定 OpenDesign 模型与真实 DSH 的合成工具往返也已通过，真实凭据只在进程内传给 Core。 | 此入口仅覆盖 Windows 上已验证的 DSH 版本及 headless 路径，不自动接管旧会话或现有 DSH profile；工具名授权不是参数沙箱。中断后的外部动作仍须对账，界面和生产负载尚未完整验收。 |
| DSH 会话状态与界面 | 状态摘要区分最近桥接请求与该会话最近一次有记录的 WB/CB 发布。真实 DSH 0.2.1-rc.5 Web + 本机模拟模型中，已打开 SIC 标签、分页读取主上下文 WB/CB 时间线、展开对应关系、读取 CB，并新增决策条目后重新读取；同时显示原生工具事件及分模型用量。宿主代理不把 bearer 给浏览器。 | 不是 Langflow/Studio 验收；界面仅支持主上下文追溯和三类决策新增，其他可编辑操作、内部上下文、完整分叉/Socket 和公示牌界面未实现。工具续接是另一条桥接请求，其 WB 引用可以为空；CB 发布与桥接记录之间的中断仍可能缺引用。 |
| DSH 原生工具事件 | 同一 actor/session/call 的 `tool/result` 元数据持久化，按序号去重并拒绝冲突；DSH 合成的结果未知与未启动事件分别处理。真实 DSH headless 在模拟模型及固定 OpenDesign 模型的分身调用内置 `read` 时，原生结果在 Worker 续接前被匹配，Core 记录工具调用返回正常。另在隔离数据库中模拟桥接事件丢失，重启并恢复同一真实 DSH 会话后，通过异步查询补回完全相同的调用 ID、序号与状态；迟到观察可进入后续 Editor/Worker 的 I。 | 已返回正常只表示工具调用未报告错误，后台任务与外部副作用最终状态未知。查询服务缺失、超时、超出最近 512 个事件或队列／批次限制时不保证补齐，未匹配结果保持未知；长期离线和真实模型下的恢复尚未验收。迟到事件不改写已交付内容，I 只保留最近 20 项并标注截断。 |
| Editor / Worker 分模型用量 | 冻结每次调用的请求模型及路由身份，按角色、模型与 Provider 路由汇总；保留输入、输出、缓存输入的已知值及缺失次数。未决调用在返回前标未知，放弃后迟到的用量仍记录但旧结果不采纳；Editor 各次反馈重试分别记账。一次真实 Provider 分身测试中，分别核对 3 次 Editor 和 3 次 Worker 的输入、输出及 Provider 报告的缓存输入，与 Core 逐次记录和分组汇总一致；请求方的模拟调用未算入该核对。本机测试覆盖中断、重试与重启；DSH 页面用模拟模型验证过分角色显示。 | 新增的未决提示尚需重新做实际页面验收；保留旧库的无法恢复记录，无法推断未返回用量的失败请求费用。价格、账单核对、跨 Agent 总账及长期真实 Provider 计量尚未验收。缓存输入是 Provider 的报告，不据此证明缓存命中原因。DSH 自带用量栏可能显示 0。 |
| 迁移与 DSH 绑定 | 原 Python 库快照与隔离导入保留原始记录；实际旧库的一个空历史 Agent、Board、工具目录与 5 个事件已迁入副本。另一个独立副本在核验 owner、来源摘要、停止状态和字段兼容性后恢复了 `task_goal`、`editor_feedback`，验证幂等回执，启用原所有者的 Board 权限并启动；真实 DSH 绑定该副本的临时备份后，与本机模拟 Provider 完成只读工具往返。`legacy-config-audit` / `legacy-config-restore` 及原 Board 激活命令提供明确的离线宿主操作。 | 5 个旧事件仍隔离不重放，15 个旧工具未自动授予执行权限；复杂历史和通用 Python 数据迁移未实现。源库之后发生的新变化不会自动同步；迁移副本并未替换正在使用的 Python 服务。 |
| 失败和重启 | DSH 单库实例独占；外部工具通过原子 `tool.claim` 认领并在完成时校验 claim；已交付文本可重放，未知外部执行不会自动重派；Worker/Editor 有宿主显式 abandon。已收到工具文本但未证实工具成功时标记 `delivered_unattested`，与确无结果的 `unknown` 区分，两者都不授予重复执行权。`tests/publish_cas.rs` 用同一 file DB 的两个独立 Core/Store（各 4 连接）覆盖同一 cut 的并发发布：只有一个成功，另一个转 superseded；同 idempotency key 的重放在确认丢失后不追加 CB。真实 Core 在 Editor 阶段被强杀后，`bridge_restarted` 经 retire/discard 由新 session 恢复（`UNKNOWN-RECOVERED`/`UNKNOWN2-RECOVERED`）。宿主可封存失败会话，仅当 Agent 无待处理 WB、工具或租约时再绑定新会话。`tests/tool_claim_core.rs`、`tests/dsh_multiturn.rs`、`tests/dsh_retirement.rs`。 | 不保证外部副作用 exactly-once；多任务逐项认领，部分失败保留已认领项。新会话不恢复旧 DSH 原生历史；确无结果的未决工具仍需核对；工具执行/发布确认点的真实中断与断电/跨进程矩阵未完成。 |
| Provider 协议与多模态 | Chat 文本和原生工具往返可用。完整非流式响应中重复的工具 `index` 按数组位置规范化，调用 ID、参数和原始响应保留；模拟回归验证同轮搜索、展开和后续工具结果。助手的 `reasoning_content` 在工具续接期间作为不可编辑的协议元数据原样保留，已有模拟验收。协议模块另有 Responses/Anthropic 文本 sidecar 测试。 | 其他供应商的推理字段及官方 DeepSeek 直连尚未逐项验收；OpenDesign 返回的 `reasoning_details` 目前保留在原始响应中，未作为续接字段转发。Core 尚未接好 Responses/Anthropic；图片目前在入口明确拒绝，不能宣称已支持图像；上游模型流在 Core 内缓冲。 |
| 账务与生产部署 | 能记录 Provider 返回的部分使用量、未知值和不可变操作记录；`accounting.get` 另提供 host-measured `resources`（storage objects/bytes、blocks、timing 区间、by_role 与 overlap）和 `measurements`（block 长度、edit noise、cost），并明确 host_surrogate / provider_billed=false / non_claims，`priced_cost` 保持 null。`resources` 还分 program（jobs/status/assignments）与 tools（dispatch/succeeded/bytes/tasks/execution_seconds/by_tool_duration）计数；Provider 报告的 cache tokens 只作 provider_reported 观察，physical_cache_hit 仍为 unknown。`experiment.pack` 只导出 ids、存储字节 sha256、manifest_digest、accounting/measurements 与 privacy 标记，不含 source text 或 Provider 账单；`experiment.verify` 可重算对象/manifest digest 并单列 cut 是否过期。本机服务限制 loopback、bearer、宿主权限，并有独占锁；只读 GET `/api/v2/chains/{chain_id}/ledger` 以宿主 actor 返回同一账务，撤销后 measurements 被抑制。 | 实际缓存命中、计价/账单对账、逐项工具与适配器成本、跨 Agent 总账、并发容量、长期稳定性、观测与运维均未验收，当前不能称为生产就绪。 |

动态工具的空闲轮次按已采纳的 Worker 响应推进，包含调用工具的响应；同一
响应多次调用只计一轮。新展开的工具从该响应之后开始计算空闲轮次，重复
展开已展开的工具不刷新使用时间；执行中的工具受保护。Core 集成测试验证
连续工具调用时第 14 个空闲响应保留、第 15 个响应回收，以及后续 I 中的
完整工具目录同步移除。真实 DSH headless 配合模拟 Provider 连续启动三次
并恢复同一会话，也核对了这一边界；尚未将该长会话测试扩展到真实 Provider。

Signal 的 `after_sequence` 会返回当前 Agent 路线上序号最小的后续 Signal，
并给出 `next_after_sequence`；读取不会消费收据，调用者应保存游标避免
重复读取。真实 DSH 分身配合模拟 Provider 已验证按游标等待未知 ID 的
Signal、唤醒后读取合成文件并回传。固定 OpenDesign 模型也按明确提示使用
游标等待并完成同一链路；请求方启动使用模拟模型，CB 采用使用程序 fixture。

宿主 `message.wait` 使用同一接收序号空间，等待并读取下一条已投递消息；
读取不消费消息，Worker 仍只能在后续 CB 发布后收到消息。Signal 与 Message
等待共用每个 Core 实例最多 32 个非零等待名额。真实 DSH 分身配合模拟
Provider 的测试已验证宿主先发现未消费的回传、随后由请求方 CB 采用；现有
peer watcher 仍使用轮询，尚未改成由此接口驱动。

一次 DSH 工作轮默认最多运行 8 次 Worker 模型调用，包含内部工具调用和
跨桥接请求的外部工具续接；可由宿主设置为
1 至 64 次（CLI 或模型配置的 dsh_bridge.max_worker_calls_per_turn，CLI 优先）。真实 DSH headless 与模拟 Provider 在上限设为 16 时，已完成
两次各含 12 次 Worker 调用的桥接请求。达到上限会报错，已执行动作不会
自动重放，失败请求的完整同会话恢复仍未实现。

推理配置允许 `none`、`minimal`、`low`、`medium`、`high`、`xhigh`、
`ultra`、`max` 并原样透传；这些是协议可接受值或兼容别名，不代表模型
一定有八种独立推理强度，也不保证所有 Provider 都支持所有值。一次固定
OpenDesign 模型的 `none` 测试完成了真实 DSH 合成工具往返，另一次 Worker
响应在 `n=1` 请求下返回两个 choice，Core 拒绝采用并保存原始响应和用量；
真实上游的推理关闭效果与异常出现频率仍未充分验证。默认配置不设置该项。

对已完整收到且 choice 数量不符的 Worker 响应，Core 不采用任何候选或派发
其中的工具，并保留原始响应与已知用量。DSH headless 入口默认最多重新请求
一次，宿主可设为 0 至 2 次；每次计入 Worker 调用上限和账务。真实 DSH 配合
模拟 Provider 注入异常后已完成续接并验证没有派发异常工具；另在固定
OpenDesign 模型与真实 DSH 联测中注入同类合成异常，随后真实模型完成了
工具往返。修复后尚未自然再次观察到原生异常。网络超时、429、HTTP 失败和
外部工具不走此重试。Editor 的相同异常进入其原有有界反馈预算。

Worker 可另行通过固定模型配置 `retry_worker_http_5xx` 选择有限重试：默认
关闭；启用后 DSH bridge 对明确的 500、502、503 或 504 重试，次数和首次等待由
`worker_retry_max_attempts`（1–5，省略为单次重试）与 `worker_retry_backoff_ms`
（0–5000，省略为 250 ms，逐次翻倍）决定，且受当轮 Worker 调用上限约束。Core 在重试前记录失败，未采用该响应或派发其
工具；缺失用量仍为未知。`tests/dsh_core_integration.rs` 用隔离模拟服务
核对了选择开关、相同请求、账务、上限和非重试状态；真实 Provider、断电
恢复及真实外部工具尚未验收。
两类 Worker 重试都绑定具体失败记录，并在重试前核对重建的请求、模型配置及
输入／控制状态；并发材料改变上下文时不会发出第二次 Provider 请求，相关
边界有隔离模拟测试。

本地 DSH 入口改为让自己启动的 Core 直接绑定系统分配的端口，读取该子进程
的监听信息后再生成 DSH 配置，避免探测并释放端口导致的竞争；
`dsh-plugin/test/run-port-isolation-fixture.ps1` 使用合成 DSH 命令验证了
端口、健康检查和独立子进程清理。修改后的真实 DSH 路径尚未重新验收。

真实数据目录及测试报告属于本机数据，不应随代码自动上传。旧 Python 服务和
数据库未替换；迁移的副本与已运行的服务之间没有自动同步。

Worker 的 Provider 响应已经返回、但非零 Signal 等待尚未完成时，Core 会先
持久记录模型身份与报告的用量，并在账务视图标记待完成操作。模拟测试中，
取消等待任务后关闭并重新打开磁盘数据库，已知用量仍可读取；正常完成后的
最终记录取代这一观察值，避免重复计数。另在模拟 Provider 下强制终止等待中的
Rust 服务，重新启动后核验用量及未决状态，再显式放弃旧执行并运行新 Worker，
确认启动时没有自动重派。断电和等待自动恢复尚未验收；等待期间不复制完整
原始响应，观察写入前中断
仍只能标记未知。账务 `complete` 只表示每次调用的输入与输出 token 均
已知，缓存字段仍可能未知；也不能据此判断 Agent 工作或待处理动作完成。
宿主显式放弃旧 Worker 后，
该观察会从待处理改为执行权已作废，并保留用量；这并不声称旧等待已经
实际退出。旧执行迟到并保存最终记录后，以最终记录为准。真实 DSH 分身与
固定 OpenDesign 模型的隔离测试还在 Signal 等待期间核对了当次输入/输出
用量与 Provider 返回一致，唤醒后完成原生 `read`、Socket 回传和请求方 CB
采用；请求方启动使用模拟模型，CB 采用使用程序 fixture。
