# 可用性核查：除三件待议事项外，SIC 是否已可用

时间：2026-11-14 09:30 左右。针对 HEAD `9e9a0c4`，用真实 release 二进制做实测，
不靠测试数量推断。

## 结论

**三件待议事项都不是阻塞项，其余部分可用。** 下面按"实测过"和"未实测"分列。

## 一、实测可用（真实进程 + HTTP）

起了一个真实 `sic-core-rust` release 进程（23.7 MB，sha256 `9CB22B37…`），
`init` 新建库、`serve` 监听 loopback、Bearer 鉴权、96 个操作可用，逐项打了一遍：

| 能力 | 实测结果 |
|---|---|
| `chain.create/list/get/checkpoint` | OK |
| `source.ingest/read/text` | OK（`source.text` 需要 `unit_id`，见下） |
| `wb.form` → `edit.program` → `cut.publish` | OK，CB 正常产生 |
| `hierarchy.get/timeline/expand` | OK |
| `memory.page/replay/search` | OK（`memory.page` 要 `kind`+`object_id`） |
| `decision.get`、`config.get` | OK |
| `board.create/get/history` | OK |
| `socket.create` | OK |
| `signal.status/spawn` | OK |
| `tools.configure/status` | OK（要 `{acl:{discover,expand,execute},tools:[]}`） |
| `accounting.get` → `measurements` + `stats` | OK |
| `experiment.pack` | OK |
| `security.classify/revoke` | OK，撤销后 `suppressed=revocation` |
| HTTP `/api/v2/chains/{id}/ledger` | OK |

**真实模型路径（Provider + DSH bridge）也跑通了**：用一个本地 OpenAI 兼容 fixture
作为模型端点，走的是 Core 真正的 `provider.rs` 路径（不是 mock）：

- `edit.prepare` → `edit.run` → **status=ready**，attempts=1，prompt_tokens=100，
  cache_hit_rate=0.4，steps=1。**统计链路的六个字段全部有值。**
- DSH bridge 一整轮：用户消息 → `source.ingest` → `wb.form` → Editor 出 edit →
  `cut.publish` → Worker 出文本 → SSE 返回 **HTTP 200**，turn receipt 落到
  `phase=delivered`。
- 第二轮同一 session **不会重放**（`turn_indeterminate`），符合"不自动重试"的契约。

## 二、三件待议事项的真实影响

| 事项 | 现在的影响 | 是否阻塞 |
|---|---|---|
| Signal-Stop 不主动终止 | 只有宿主读 `control_epoch` 才生效；不读就是纯记录 | **不阻塞**。Core 本来就不拥有子进程 |
| 前两块窗口不接 1/4 长度换算 | 用块数（`protected_prefix_blocks`，默认 0=关） | **不阻塞**。默认关闭，开了也只按块数 |
| Advance/Adaptive 只改错误标签 | 行为等同 `wait` | **不阻塞**。默认就是 `wait`，两种策略都是显式 opt-in |

三个默认值都是"保守且关闭"，所以不选也不会有错误行为。

## 三、实测中发现的三处接口约定不直观（文档问题，非 bug）

调用时我先按常理想当然，被 422 挡了三次。这些都不是逻辑错误，但**没有任何
文档说明**，属于"能跑但不好用"：

| 操作 | 我猜的 | 实际要求 |
|---|---|---|
| `source.text` | `source_id`+`offset` | 还需要 `unit_id`（先 `source.read` 拿） |
| `memory.page` | `cb_id` | `kind` (source/cb/ops/wb) + `object_id` |
| `hierarchy.expand` | `cb_id`+`wb_id` 都给 | 只能给一个，二选一 |
| `tools.configure` | `{tools:[]}` | `{acl:{discover,expand,execute},tools:[]}` |
| `signal.spawn` | `origin_route_id` 够 | 必须给 `child_route_id`，且 fork 要非空 CB 前缀 |
| `worker.run` | 可带 `source_id` | 不接受任何参数，只认 `retry_after_attempt` |

`/api/v2/capabilities` 只返回 `{operation, write, model}`，**不含参数 schema**，
所以宿主无法自查。要补的是能力表里的 schema，不是改行为。

## 四、还没实测的

- **真实外部 Provider**（DeepSeek/OpenAI 真实端点、真实计费）。本轮用的是本地
  fixture，证明的是"链路通"，不是"真实模型质量"。
- **真实 DSH 客户端**（真的 DeepSeek Harness 打进来）。本轮用 curl 形态的
  请求打到 `/v1/chat/completions`。
- **官方 Langflow / LangGraph 工作台**（F53 的运行时侧）。
- **长上下文 / 大并发**。全部测试都在几百 KB 量级。

## 五、建议的三件事

1. **补 `/api/v2/capabilities` 的参数 schema**——这是"可用"与"好用"之间最大的
   一条，宿主现在只能靠文档或试错。
2. **把 `worker.run` 的静默参数拒绝写进文档**——它不接受任何参数，但错误信息
   `unknown Worker run argument` 不告诉调用者"这里就是不该传参数"。
3. 三件待议事项**保持现状**即可；它们是 opt-in 且默认关闭，不需要在"能否用"
   这个问题上做决定。
