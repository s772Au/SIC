# 多 Agent 下的 Socket 现状，以及前端

时间：2026-11-14。HEAD `2828221`。

## 一、Socket 在多 Agent 下工作正常

### 权限模型是四条独立 ACL

`SocketAcl` 有四个互相独立的授权列表，不是一个大权限开关：

```rust
pub struct SocketAcl {
    pub message: Vec<String>,  // 投递 Message
    pub signal: Vec<String>,   // 发 Signal
    pub spawn: Vec<String>,    // Fork 出新 route
    pub read: Vec<String>,     // 直接读历史 checkpoint
}
```

四个入口各自查各自的那一条（`ipc.rs` L518 / L551 / L618 / L855）。
`spawn_peer` 的注释写得很清楚：spawn 授权的是“恢复快照”，`read` 是另一条独立
许可，**拿到 spawn 不等于能直接读**。

### 实测覆盖

| 测试 | 覆盖 |
|---|---|
| `ipc::message_signal_spawn_and_read_permissions_are_separate` | 四条 ACL 互不代替 |
| `ipc::signals_transition_only_with_signal_acl_and_stop_is_terminal` | signal 单独授权、Stop 终结 |
| `ipc::checkpoint_socket_cannot_bind_another_routes_history` | socket 不能绑别的 route 的历史 |
| `ipc_core::historic_peer_and_communications_are_transactional_and_replayable` | 三 Agent（alice/bob/charlie）真跑：bob 可读、charlie 被拒、消息幂等重放、P/V 进入下一个 CB |
| `worker_ipc_core::worker_message_tool_obeys_socket_acl_and_arrives_at_next_cb` | Agent A 发、Agent B 收、Agent C 被 ACL 拒 |
| `worker_ipc_core::worker_message_pages_preserve_complete_text_and_respect_publication_and_route` | 分页不越权、不跨 route |
| `ipc_catalog_adversarial` | 动态工具目录同步时保留 in-flight lease |

全部逐一跑过，共 34 个测试全绿：

```
ipc                       8 passed
ipc_store                 5 passed
ipc_core                  1 passed
worker_ipc_core           2 passed
ipc_catalog_adversarial   2 passed
peer_tasks                5 passed
peer_task_prepare         8 passed
peer_retry_cancel         2 passed
revocation_peer           1 passed
```

### 一个容易误解的地方：一个进程只能有一个 Agent 身份

`HostAuth` 把 actor 绑在 bearer 上，请求体里写另一个 actor 直接 `actor_mismatch`。
我实测过在同一端口上冒充 alice/bob/charlie 调用，三个都被拒。

我读代码后**差点得出错误结论**：以为“多 Agent 在 HTTP 层不可用”。
实际上这是设计：一个 serve 进程就是一个可信宿主主体，多个 Agent 在同一个
Core 库里以不同 chain 存在，由宿主进程内调用多个 `OperationContext` 区分。
下面这条实测证明了两个 serve 进程不能共享一个库（`another Rust process already
owns this database`），所以**多 Agent 必须在同一进程里**——这正是 `ipc_core`
那条测试的形态，而不是 HTTP 层的形态。

如果前端确实需要“以不同 Agent 身份操作”，那是宿主的职责：宿主自己维护
多身份，对 Core 用不同 `OperationContext` 调用，而不是让浏览器拿多把 bearer。

## 二、前端现状：两代，只有一代能跑

### 能跑的：无依赖静态 webui

`02-source/webui/`，85 KB `app.js` + 13 KB `styles.css`，零构建依赖。
默认是明确标记的本地示例数据，点“连接后端”后才打真实 API。

### 跑不了的：它调的是 `api/v1`，而 `api/v1` 没有服务了

webui 调 22 个 `/api/v1/...` 路由（`ui/bootstrap`、`sessions`、`drafts`、
`test-plans`、`mounts`、`projection`…）。这些路由写在 `02-source/src/main.rs`
（2297 行），但那个文件的 crate 身份是 `sic-proxy`——

```toml
# Cargo.toml at b66d278 (v0.1)
name = "sic-proxy"
```

现在的根 `Cargo.toml` 只有两行 members：

```toml
[workspace]
members = ["rust-core", "experiments"]
```

所以 `src/main.rs` **根本不会被编译**。`tests/smoke_backend.py` 找的是
`target/debug/sic-proxy.exe`，这个文件不存在，脚本会直接 `raise SystemExit
("Build first: cargo build")`。

`webui/README.md` 第 7 行还写着“Rust 服务已运行时，在浏览器打开其地址（默认
`http://127.0.0.1:8787`）。Rust 的静态文件服务会提供本目录”——这句话现在不成立：
现役的 `sic-core-rust` 只有 4 条路由（`/health`、`/api/v2/capabilities`、
`/api/v2/operations`、`/api/v2/chains/{id}/ledger`），**没有任何静态文件服务**，
也没有 8787。

### Python 侧也只剩 v2

`sic_core/server.py` 的 `do_GET`/`do_POST` 只认 `/api/v2/capabilities` 和
`/api/v2/operations`。`integrations/run_core_host.py` 用的就是它。
`api/v1` 的 327 处引用集中在 `docs/api-contract.md`、`webui/`、各类 smoke 脚本
和 `integrations/tests/`——都是历史文档或测试，没有活的服务端。

### Integrations 目录

`02-source/integrations/` 有完整的 Langflow / LangGraph 适配：
`sic_workflow/client.py`（走 `api/v1`）、`run_core_host.py`（走 v2 Core）、
`generate_*_flows.py`、各类 acceptance JSON。这些都在 `02-source/tests` 里跑，
293 个 Python 测试全绿。但它们接的是 Python Core（`sic_core`），
不是 Rust 的 `sic-core-rust`。

## 三、结论

| 问题 | 答案 |
|---|---|
| 多 Agent 时 Socket 还工作吗 | **工作**。四条 ACL 独立、三 Agent 实测通过 |
| 前端能用吗 | **能起，但连不上**。webui 调 `api/v1`，而 `api/v1` 的服务端
  `sic-proxy` 已不在工作区编译 |
| 前端静态服务 | 现役 `sic-core-rust` 不提供，`webui/README.md` 的说法已失效 |

### 若要恢复前端，有两条路

**A. 把 `src/main.rs` 恢复成 workspace 成员。** 它的 crate 身份是
`sic-proxy`（`Cargo.toml at b66d278`），加回 `members` 并补 `tower-http` 依赖
即可编译。但两者**存储 schema 完全不同**：

| | v1 `sic-proxy` | 现役 `sic-core-rust` |
|---|---|---|
| 表 | `sessions` / `blocks` / `block_units` / `drafts` / `commits` / `test_plans` / `edit_jobs` / `mount_states` / `nest_nodes` / `worker_relay_events` | `objects` / `boards` / `chains` / `revocations` / `sensitivity` / `sic_rust_meta` |
| 模型 | 会话-块-草稿-提交，每张表一个实体 | 不可变 object + digest，一种表存万物 |

两套表没有任何重叠。A 不是“加回一个成员”，是把一个独立的旧系统搬回来，
且它不会读到现役 `sic-core-rust` 写的任何一个 CB。

**B. 让 webui 改调 `api/v2`。** 96 个操作已经是干净的统一信封，
`/api/v2/capabilities` 现在也带 payload schema。要改的是 `app.js` 里 22 处
`fetch` 的目标路径和响应解包，以及补一个静态文件服务（tower-http 或独立静态
服务器）。这条路线不会碰到 schema 兼容问题，但 webui 里那些 v1 专属概念
（drafts、test-plans、mounts、projection）需要映射到 v2 的
`edit.candidate.*`、`hierarchy.select`、`config.update` 等操作。

我的建议是 **B**，理由是 v1 那套 schema 已经和现役 Rust Core 脱节，
A 会把一个已废弃的存储格式重新拉回主线。