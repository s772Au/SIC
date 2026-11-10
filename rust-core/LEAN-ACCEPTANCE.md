# Rust 精简版逐项验收（2026-11-10，进行中）

本文只记录独立 Rust Core 的证据。真实 Provider 的验证限定为
`deepseek-v4.1-flash-eco`；模拟测试不能替代真实模型或生产验收。

| 要求 | 现状与证据 | 仍需完成或验证 |
|---|---|---|
| 单 Editor；Worker 等待 | 同一 Agent 同时只运行一个 Editor；已调度的任务在 open、inflight、ready 或 failed 状态下，Worker 均不能越过发布继续。`tests/editor_runtime.rs`。 | 未验证长期运行、异常恢复后的所有调用路径。失败后需显式建立并发布替代编辑，不能静默跳过。 |
| Worker/Editor 共同前缀、不同 I | 相同的保护指令、CB 表示和固定工具表；宿主在其后的 I 中激活不同模式、作用域与权限；Editor 非法调用会进入有界纠错。`tests/prefix.rs`、`tests/editor_runtime.rs`。 | 逻辑前缀相同不证明 Provider 发生了物理缓存命中。不同模型或配置下的缓存效果未知。 |
| WB/CB 双链和高阶 | 保存顺序、同阶对应和来源；支持高阶 WB 引用低阶 CB、逐层展开、选择及有界自动调度。真实模型一次形成四个一阶、两个二阶、一个三阶 CB。`tests/core_flow.rs`、`tests/bounded_flow.rs`。 | 长期运行、复杂来源和所有高阶选择组合还未充分验证。 |
| 编辑操作和审计 | 支持 KEEP、DELETE、FLAG、REORDER、COMPRESS、REPLACE、ADD；保留候选、操作和回放，迟到材料不伪称 Editor 已见。`tests/editing.rs`、`tests/core_flow.rs`。 | 实验尚未衡量复杂任务的信息保留质量。 |
| 持久、易失、即时信息 | P 写入新 CB，V 进入之后的 WB，I 随发布刷新；原 CB 不被覆写。`tests/core_flow.rs`、`tests/board_core.rs`。 | 需继续扩大长链、跨阶及异常条件验证。 |
| 动态工具搜索和扩展 | 宿主维护目录与独立权限；模型可分页搜索、请求展开，或在获授权的调用时自动展开；目录可超过 128 项，I 区只列前 20 个工具名。`tests/dynamic_runtime.rs`、`tests/worker_tool_core.rs`、`tests/dsh_bridge.rs`。 | 工具 schema 的完整语义校验仍依赖执行宿主；DSH 目录总大小受 1 MiB 上限约束。 |
| 动态工具回收 | 15 个已完成 Worker 轮次未使用，或固定核心工具与展开动态工具的完整 schema 总数将超过 128 时，回收合适的旧工具；当前 22 个固定核心工具占用总额度。模型可请求回收指定工具，在途和固定工具受保护。`tests/toolset.rs`、`tests/tool_store.rs`。 | 长时间真实 DSH 会话中的回收行为尚未做生产负载验收。 |
| 决策树 | 四种视角、宿主/模型权限与修订冲突检查已接入；模型不能自行证明任务完成。`tests/decision.rs`、`tests/worker_ipc_core.rs`。 | 完整的多 Agent 任务编排与真实任务质量未验收。 |
| Socket 和历史分身 | 可基于已发布 CB 检查点创建 Socket 和分身，历史读取按权限分页；分身任务有持久记录、宿主原子认领和有界 Editor 准备。显式启用的串行 watcher 已在真实 DSH 和本机模拟模型下自动发现 task、调用只读合成工具、回传并由请求路线采用；另一次使用 DSH 自带 `read` 读取隔离目录内的合成文件并完成同一往返。单任务 supervisor 另用真实 DSH 与固定 OpenDesign 模型完成只读合成工具往返：工具结果进入后续 Worker，实际回复经 Socket 投递并被请求路线 CB 采用。重复执行与进程不明状态有受限恢复规则。`tests/ipc_core.rs`、`tests/peer_watch.rs`、`dsh-plugin/test/run-peer-supervisor-watch-headless.ps1`。 | **创建分身不等于分身已执行任务**；真实模型验收仅覆盖一次单任务 supervisor 和合成工具，DSH 自带 read 只用模拟模型验证。watcher 使用真实模型、其他业务工具、并发协作及长期运行尚未验收。 |
| Signal / Message | 权限校验、Signal 状态切换和 P/V 消息在 CB 发布时收取已有本机测试；旧 Signal 重放不回退新状态。`tests/ipc_core.rs`、`tests/recovery_core.rs`。 | 非零超时的阻塞等待、完整进程间协议和真实 peer 往返未完成。 |
| 公示牌 | 独立 ACL、CAS、认领与进度、宿主确认完成；每次 CB 发布时把最新公示牌放入 I。`tests/board_core.rs`、`tests/board_security_audit.rs`。 | 跨真实 DSH 分身的协作场景未验收。 |
| DSH 主 Agent 工具往返 | 真实 DSH 0.2.1-rc.5、Rust Core 和 OpenDesign 固定模型完成 Editor → CB → Worker → DSH 执行只读合成工具 → Worker 采用结果；WB 根来源及工具输出状态修复后再次通过真实模型验收。另用本机模拟 Provider 验证同一 Bridge 会话连续 24 轮、其中一次中途重启、历史和 CB 不变与交付重放。 | live 验收只证明一个只读 fixture；24 轮属于模拟模型测试，未证明任意真实应用工具或长任务；一次额外联测曾收到 Provider HTTP 500，未自动重试。 |
| 迁移与 DSH 绑定 | 原 Python 库快照与隔离导入保留原始记录；实际旧库的一个空历史 Agent、Board、工具目录与 5 个事件已迁入副本。另一个独立副本在核验 owner、来源摘要、停止状态和字段兼容性后恢复了 `task_goal`、`editor_feedback`，验证幂等回执，启用原所有者的 Board 权限并启动；真实 DSH 绑定该副本的临时备份后，与本机模拟 Provider 完成只读工具往返。`legacy-config-audit` / `legacy-config-restore` 及原 Board 激活命令提供明确的离线宿主操作。 | 5 个旧事件仍隔离不重放，15 个旧工具未自动授予执行权限；复杂历史和通用 Python 数据迁移未实现。源库之后发生的新变化不会自动同步；迁移副本并未替换正在使用的 Python 服务。 |
| 失败和重启 | DSH 单库实例独占；外部工具通过原子 `tool.claim` 认领并在完成时校验 claim；已交付文本可重放，未知外部执行不会自动重派；Worker/Editor 有宿主显式 abandon。已收到工具文本但未证实工具成功时标记 `delivered_unattested`，与确无结果的 `unknown` 区分，两者都不授予重复执行权。宿主可封存失败会话，仅当 Agent 无待处理 WB、工具或租约时再绑定新会话。`tests/tool_claim_core.rs`、`tests/dsh_multiturn.rs`、`tests/dsh_retirement.rs`。 | 不保证外部副作用 exactly-once；多任务逐项认领，部分失败保留已认领项。新会话不恢复旧 DSH 原生历史；确无结果的未决工具仍需核对，同会话完整恢复未实现。 |
| Provider 协议与多模态 | Chat 文本和原生工具往返可用。协议模块有 Responses/Anthropic 文本 sidecar 测试。 | Core 尚未接好 Responses/Anthropic；图片目前在入口明确拒绝，不能宣称已支持图像；上游模型流在 Core 内缓冲。 |
| 账务与生产部署 | 能记录 Provider 返回的部分使用量、未知值和不可变操作记录；本机服务限制 loopback、bearer、宿主权限，并有独占锁。 | 实际缓存命中、计价、完整资源总账、并发容量、长期稳定性、观测与运维均未验收，当前不能称为生产就绪。 |

真实数据目录及测试报告属于本机数据，不应随代码自动上传。旧 Python 服务和
数据库未替换；迁移的副本与已运行的服务之间没有自动同步。
