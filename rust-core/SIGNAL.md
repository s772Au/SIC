# Signal 控制面：设计、实现与边界

> 本文描述 `rust-core/src/signal.rs` 与 `core.rs` 中的接线。它不是契约验收记录；
> 每一项只说明已经实现了什么、以什么为证据、以及明确没有做什么。

## 0. 一句话

Signal 把「单个 Agent 对自己的状态控制」推广成「一个 Agent 对另一个 Agent 的控制」，
并以同一套机制统一了原先分散的 **Spawn / Fork / Pause / Stop / 唤醒来源 / 控制权授予**。

## 1. 为什么要在 Socket 之外再有一个 Signal

现有 `ipc.rs` 的 Socket 只处理**历史 checkpoint 上的通信**：

- `socket.signal` 的 `SignalAction` 只有 `Pause/Resume/Wake/Stop`，作用域绑定在一个 Socket 上；
- `RouteStatus` 只有 `Running/Waiting/Stopped` 三种，且 `Stopped` 是终态，不能带唤醒白名单；
- `socket.spawn` 只能从 Socket 绑定的 checkpoint 分叉，不能从任意前缀、不能多父、不能自指；
- 停用期间到达的消息**没有任何排队语义** —— `receive_into` 直接写 inbox，没有时间戳与允许/不允许的分类。

Signal 补齐的正是这四点，同时保持 Socket 原样不动：`socket.*` 的行为、测试与 ACL 完全不变。

## 2. 类型

| 类型 | 语义 |
|---|---|
| `ControlRight` | `Pause` `Stop` `Spawn` `Wake` `SourcePolicy` `Grant`。Agent 对自己恒有全部六项。 |
| `ForkOrigin` | `Spawned`（空 CB 前缀）与 `Forked{origin_route_id, prefix_cb_ids, origin_cut_id, self_fork}`。 |
| `SourceKind` | `ChildReply` `AgentMessage` `BackgroundTask` `AdminSignal` `ClockTimeout` `HostEvent`。 |
| `SourcePolicy` | 冻结期允许的 `kinds` + `source_ids`，带 `revision` 与 `updated_at`。 |
| `ApplyMode` | `Now` / `AfterOutput`。 |
| `ControlState` | `Running` `PausePending` `Paused` `StopPending` `Stopped{terminal}`。 |
| `SignalLimits` | `max_child_depth`（0..64）、`max_owned_children`（0..256），默认 8 / 8。 |
| `QueuedItem` | 带 `received_at` 的到达记录，`admitted_at` 为空即仍在排队。 |
| `LogEntry` | 管理指令日志：`seq`、`at`、`action`、actor、target、detail。 |

## 3. 五类 Signal

### 3.1 Signal-Spawn / Signal-Fork

```text
origin_route_id 缺失 + prefix_cb_ids 空   → Spawn（空前缀）
origin_route_id 存在 + prefix_cb_ids 非空 → Fork（该 Agent 的任意 CB 前缀）
origin_route_id == 自己                  → 自分叉（self_fork = true）
```

- **任意 CB 前缀**：`prefix_cb_ids` 原样记录进 `ForkOrigin::Forked`，`origin_cut_id` 可选记录该前缀的最后一次发布。
- **多父**：`co_parents` 里每个仍在运行的 Agent 都获得对该子 Agent的全部六项控制权，并出现在 `children` 列表。
- **初权**：参与创建的每个父 Agent自动持有子 Agent的全部控制权，这是唯一无需显式 Grant 的授权路径。
- **规模控制**：`max_child_depth` 与 `max_owned_children` 同时作用于 Fork 与 Spawn —— 空 Spawn 也消耗请求者自己的持有额度。

### 3.2 Signal-Pause

| 模式 | 结果 | 输入是否还能进 |
|---|---|---|
| `now` | 立刻 `Paused` | 只有 `admits()` 允许的会进并唤醒 |
| `after_output` | `PausePending`，输出边界由 `settle_after_output` 落成 `Paused` | 期间照常接收 |

Pause 是**可唤醒的**：任何被 `SourcePolicy` 允许的到达都会把状态推回 `Running` 并递增 `control_epoch`。

### 3.3 Signal-Stop

```text
wake_sources 为空   → Stopped{terminal: true}  再没有任何来源能唤醒
wake_sources 非空   → Stopped{terminal: false} 仅列表内已注册的来源可唤醒
```

- 同样支持 `now` / `after_output`；
- `control_epoch` 递增，宿主可据此拒绝在途的模型请求（`signal_accepts_input` 只对 running/pause_pending/stop_pending 为真）；
- **Stop 期间的消息不丢弃**：全部进入 `queue`，每条都带 `received_at`，`admitted_at` 为空、`wake_eligible` 标记是否属于允许集合；
- Stop 会把 `SourcePolicy` 整体替换成 `{kinds: 空, source_ids: wake_sources}`，所以「Stop 允许哪些来源」与「Pause 允许哪些来源」是同一份数据，不会出现两套互相矛盾的名单。

### 3.4 Signal-SourcePolicy

- 所有信息来源必须先用 `signal.source.register` 注册到某个 `SourceKind`；
- **未注册的来源永远无法到达**：`arrive` 对未注册 id 返回 `unregistered_source`（409），`set_source_policy` 对未注册 id 返回 422；
- 这一条是「不会出现某些能唤醒 Agent 的来源无法被控制」的结构保证：白名单是全集的子集，全集本身由注册表定义。

### 3.5 Signal-AdminPolicy (grant / revoke)

- `signal.admin_grant` 只能把自己**已经持有**的权利传出去（`is_subset` 检查），权利不能凭空扩张；
- `signal.admin_revoke` 只能撤销他人对自己的授权，Agent 对自身的六项权利不可撤销；
- 每次 grant/revoke 都进管理日志，`ControlGrant` 带 `revision`、`granted_by_route_id`、`granted_at`。

### 3.6 Sleep / Wait 归入 Signal

`signal.sleep` 的语义是**对自身的 Pause + 一个 ClockTimeout 来源 + 开放唤醒策略**，
不是 Stop。原因就是你指出的那条：Stop 会错过消息，而 Pause 能继续接收并处理。
实现里 `sleep` 会自己注册 `clock:<route>:<timeout>` 这个来源，并把它写进 policy，因此超时唤醒与「同伴消息唤醒」走同一条 `arrive` 路径。

## 4. 管理日志与状态在 I 区广播

`signal.broadcast` 返回：

```json
{
  "signal_state": {"state": "paused"},
  "children": [{"route_id": "kid", "state": {...}, "depth": 1,
                "since": 1794..., "queued": 2, "last_log_seq": 7}],
  "management_log": [{"seq": 3, "at": 1794..., "action": "signal.pause", ...}, ...],
  "queue_depth": 0,
  "control_epoch": 4
}
```

这正好是可以放进可信 I 的形状：当前自身状态、全部子 Agent 的状态与滞留量、以及按时间戳排序的管理指令记录。日志默认保留最近 256 条，广播默认取最近 16 条（`log_limit` 可调，上限 64）。

**它已经在 I 区里了**：`run_worker` 与 `edit.run` 两块在构建 `i_views` 时都调用 `signal_broadcast_i(...)`，
把上面的结构挂到 `views["signal"]` / `i_views["signal"]`。也就是说每次 CB 覆写时，Worker 和 Editor
都能读到最新的自身状态、子 Agent 状态和管理日志 —— 而共享前缀（`common_tools` / CB 链）里什么都没有变。
广播只出现在 CB 之后的 I 段，所以不会把 Signal 的易变状态塞进缓存前缀。

## 5. 持久化与既有状态的关系

- Signal 用**自己的表** `sic_signal_states(namespace, state, digest)`，与 `sic_rust_ipc_states` 并列，不重写 IPC 聚合行；
- 每条记录带 sha256 digest，读时校验，不匹配即 `integrity_error`；
- 所有 Signal 变更都在调用方的事务里执行，和 Agent 状态一起提交或一起回滚；
- `run_state` 镜像：`apply_control_state` 把 Signal 状态写回 `Agent.run_state`，因此既有的 `edit.prepare`、`cut.publish`、`edit.run` 等 `run_state != "running"` 守卫**自动生效**，没有改动那些守卫本身。

## 6. 与本次任务的冲突面

为了让并行修改其他部分的会话不踩踏，改动只落在三处：

| 文件 | 改动性质 |
|---|---|
| `rust-core/src/signal.rs` | **新文件**，1063 行，无既有代码 |
| `rust-core/src/lib.rs` | 加一行 `pub mod signal;` |
| `rust-core/src/core.rs` | 加 12 个 `signal.*` 操作到 `OPERATIONS`；加一个 `if operation.starts_with("signal.")` 分支；新增 `dispatch_signal`、`ensure_signal_node`、`apply_control_state`、`settle_signal_after_output` 与两个小辅助函数 |

**没有改动逻辑**：`ipc.rs`、`ipc_store.rs`、`peer_tasks.rs`、`scheduler.rs`、`dsh*.rs`，以及任何既有的 `run_state` 判断。
`prefix.rs` 只追加 7 条工具 schema 并把 `COMMON_TOOL_VERSION` 升到 `3`（新 schema 进入共享前缀必须有版本号）。

## 7. 明确没有做的

1. **不执行进程控制**：Signal 只记录状态，不 kill 子进程。`Stop` 之后是否还在接受输出，由宿主读 `control_epoch`/`signal_accepts_input` 决定；本模块不自己发终止信号。
2. **Fork 不复制 CB 对象**：`prefix_cb_ids` 只记录继承的前缀引用；实际快照复用仍由既有的 `checkpoint_snapshot` 与 `Storage` 负责。
3. **未接 GUI**：Langflow/Studio 侧没有任何 Signal 页面。
4. **未覆盖持久化队列的淘汰策略**：队列上限 4096 条，超出时丢最旧 —— 这是明确的容量上限，不是「静默丢弃规则内消息」。

## 7.1 已经接上的

- **`after_output` 落点**：`run_worker` 记录 worker_attempt 之后、commit 之前调用 `settle_signal_after_output`。因此 `pause_pending`/`stop_pending` 会在**本次输出已经落地之后**变成 `paused`/`stopped`，在途完成不会被丢弃。这是 Signal 在 Worker 路径里唯一的写点。
- **`after_output` 不抢控制权**：`pause`/`stop` 只在 `now` 时递增 `control_epoch`；`apply_control_state` 也只在 `paused`/`stopped` 落定时递增。于是 `run_worker` 里的 `interrupted = run_state != running || control_epoch != control_epoch` 对 `pause_pending`/`stop_pending` 保持为假 —— 本次 attemp 的正文与工具调用照常采用，只有下一次输入被闸住。否则 `after_output` 会退化成 `now`。
- **模型可见工具**：`prefix::common_tool_registry()` 里新增 7 个 `sic_signal_*` 工具（status / broadcast / pause / sleep / drain / register / arrive），并在 `run_worker` 的 activated 列表里放行。`COMMON_TOOL_VERSION` 从 `2` 升到 `3`，因为有新 schema 进入共享前缀。

## 8. 测试

| 文件 | 数量 | 覆盖 |
|---|---|---|
| `rust-core/tests/signal_core.rs` | 13 | 纯状态机：空前缀/前缀 Fork、多父、深度与持有数限制、Pause 两模式、唤醒与排队、Stop 白名单与终态、Sleep 不是 Stop、未注册来源、grant 不扩张、无权限不能控制他人、I 广播形状、status 字段 |
| `rust-core/tests/signal_core_dispatch.rs` | 8 | 经 `Core::operate` 的完整链路：spawn/fork/自指、run_state 镜像与既有守卫、排队跨 Stop 存活、I 广播进 Agent 区、跨 Agent 需要 Wake 权、Sleep 非终态、Core 重开后状态仍在、深度与持有数拒绝 |
| `rust-core/tests/signal_model_tools.rs` | 5 | `after_output` 的 pending 语义、共享工具表里的 7 个新工具、delegations 暴露、空 children 广播 |

## 9. 操作清单

| 操作 | 写 | 说明 |
|---|---|---|
| `signal.spawn` | ✓ | Spawn 或 Fork，多父、自指、空前缀 |
| `signal.pause` | ✓ | `mode` = `now` / `after_output` |
| `signal.stop` | ✓ | `wake_sources` 为空即终态 |
| `signal.sleep` | ✓ | 自身 Pause + ClockTimeout 来源 |
| `signal.source.register` | ✓ | 注册 `source_id` 到某个 `SourceKind` |
| `signal.source_policy` | ✓ | 发布允许集合（kinds + ids） |
| `signal.admin_grant` | ✓ | 只能传已持有的权利 |
| `signal.admin_revoke` | ✓ | 撤销他人授权，不动自身权利 |
| `signal.arrive` | ✓ | 送入已注册来源；受策略约束 |
| `signal.drain` | ✓ | 取走已准入条目，按到达顺序 |
| `signal.status` | — | 单 Agent 视图 |
| `signal.broadcast` | — | I 区广播视图 |

## 10. 模型可见的工具名

共享工具表（`prefix::common_tool_registry()`，`COMMON_TOOL_VERSION = "3"`）新增 7 个：
`sic_signal_status`、`sic_signal_broadcast`、`sic_signal_pause`、`sic_signal_sleep`、
`sic_signal_drain`、`sic_signal_register`、`sic_signal_arrive`。

加上既有的 `sic_signal_send`（经 Socket 发 pause/resume/wake/stop）与
`sic_signal_wait`（等一个 Signal），模型侧一共 9 个 Signal 工具。

**模型不能在工具参数里授予自己权限**：`sic_signal_pause` 只接受 `target`/`mode`/`wake_reason`，
每一项权利检查都在 `dispatch_signal` 里按当前 ACL 重算。
