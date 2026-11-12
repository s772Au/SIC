# 真实 DSH/Provider 证据（2026-11-12）

这不是完整契约验收记录；它只记录一次隔离的真实运行，供 F48/F50/F36/F56/F58 等行引用。原始日志、SQLite 与账务导出位于 `D:\SIC-real-evidence-20261112`，未提交到仓库；本文件不含凭据。

## 环境与隔离

- DSH `0.2.2-rc.1`，独立 `DSH_HOME` 与 headless profile `sic-rust-local`；独立临时工作目录与 SQLite。
- Provider：`https://amr-link.open-design.ai/v1`（Chat），固定模型 `deepseek-v4.1-flash-eco`；凭据只经进程环境变量传给 Core，未写入日志或仓库。
- Core：当前 debug 构建，`--allow-model`，model config 的 context_window=1,000,000、max_output_tokens=393,216。
- DSH 每轮由 launcher 启动一个 Core，结束后停止；第二轮复用同一 DSH session 与 SQLite，因此覆盖正常的跨 Core 重启续做，不是故障注入。

## 运行

1. 第一轮：`Read note.txt ... reply with exactly the file contents.`；DSH 原生 `read` 调用成功，返回 `SIC-REAL-DSH-42`。
2. 第二轮：同一 session ID，`Read note.txt again ... followed by SECOND`；再次原生 `read`，返回 `SIC-REAL-DSH-42\nSECOND`。
3. 第三轮：同一 session ID，要求用 `write` 创建 `result.txt`（`REAL-WRITE-OK`）再用 `read` 读回；DSH 原生 write 产生实际文件副作用，随后 read 返回 `REAL-WRITE-OK`。
4. 三次 DSH 均正常退出；Core DB 记录 1 个 chain、4 个 cut/CB（第三轮后程序 schedule 又发布 1 个）、4 个 completed host tool tasks（read/read/write/read）、4 个 DSH tool events（outcome=ok、core_observed=1）。

## Provider 用量（来自 Core 保存的 raw response）

- 12 次 model attempt（3 Editor + 9 Worker），全部 HTTP 200；model identity requested=`openai/deepseek-v4.1-flash-eco`，returned=`deepseek-v4.1-flash-eco`。
- 合计：prompt 66,859；completion 2,000；reasoning 1,405；provider-reported cached 48,896（单次 2,048–6,016）。
- 这是 provider-reported 缓存值，不是物理 cache key/命中或账单证据；`physical_cache_hit` 仍为 unknown。
- 无真实单价，`priced_cost=null`；不得据 token 数伪造费用。

## 账务/时间

- `accounting.get`（两轮时）：model class=host_surrogate、provider_billed=false；timing total_model_seconds=37（editor 13、worker 24、tool 0）；tools read 2 次、312 bytes、2 completed；measurements 2 blocks、edit noise KEEP 2、storage 35 objects/177,939 bytes。
- 第三轮后：4 个 completed host task（write 也落账）；程序 schedule 返回 completed=true/published=1，cut 4 的 CB 正文包含 `REAL-WRITE-OK`，证明组级 open-call 修复后真实 DSH 的后续发布可用。
- tool 区间为秒级分辨率，读工具在 1 秒内完成，tool seconds=0 是分辨率结果，不是“没有工具”。

## 非声明

- 主链覆盖 `read`/`write` 与正常三轮续做；peer 覆盖 `read`/`write` 与两个顺序 peer。仍不证明其他业务工具、HTTP 500/未知 turn 恢复、peer 并发、物理缓存命中、真实账单、长上下文或任务质量。
- 不能据此把 F48/F50 标成完整通过；也不能用这些 token 数推断费用或压缩收益。

- 真实 DSH watcher 单轮通过：父 DSH bootstrap → checkpoint/Socket/spawn → supervisor watch → DSH native read 合成文件 → Socket reply → 父 ProgramFixture CB 采用；`peerReplyDelivered`/`requesterAdoptedReply`/`nativeToolEventVerified` 均为 true；报告 `research/watch-native-20261112.json`（本地 mock 模型）。

## 审计修复（2026-11-12）

- F62：`experiment.verify` 读取 `objects.data` 原始字节重算 sha256，并同时比对存储 digest 与 pack 期望；新增原始 CB 数据/存储 digest 篡改测试。
- F55：`tests/publish_cas.rs` 用 file-backed 两个独立 Core/Store（各 4 连接）+ multi-thread runtime，两个 publish 事务真实重叠，只有其一 CAS 成功；另测发布确认丢失后的同 key 重放不 append，同 key 不同 payload 返回 conflict；关闭并重开 file DB（两次）后同 key 仍返回原 cut/cb；`tests/tool_restart_interrupt.rs` 在真实 pwsh 1200ms 工具运行期重启 Core，claimed task 持久化、不自动重派，重启后第二次 claim 返回 conflict，错误 claim 返回 conflict，同一 claim 完成且只保留一个 task，同 claim/result 重放仍 completed 不新增 task，同 claim 不同 result 返回 conflict 且 task 不变，连续两次重开 DB 后 task 仍唯一且 completed；真实 Core 在 Editor 阶段强杀后的 bridge_restarted→retire/discard→新 session 恢复也属于故障恢复证据。
- F30：新增 `tests/scheduler_open_calls.rs`，真实 open native call 下 `runtime.schedule` 报 `unresolved_tool_calls` 且不形成 WB。
- F48：`tests/discard_unpublished.rs` 覆盖 open 与 inflight current job 默认拒绝、显式 `allow_open=true` 清理；真实 unknown2/unknown3 走同一路径。
- F54/F67：`tests/revocation_scope.rs` 对 `edit.submit`/`edit.run`/`edit.validate`/`edit.candidate.get`/`edit.candidate.update`/`cut.publish` 逐一断言 revoked job forbidden；`tests/revocation_levels.rs` 从 checkpoint 创建 socket，撤销前可读、撤销后 socket.read forbidden。
- F31：`tests/tool_editor_real_process.rs` 用 `spawn_blocking` 运行真实 pwsh 进程，实测 editor 与 tool 区间 overlap=3 秒。
- F33：`tests/tool_editor_wait.rs` 用真实 pwsh 300ms 进程写 `REAL-WAIT-TOOL-DONE`，工具先完成、Editor 在途时 Worker 返回 `editor_pending`，发布后继续。
- F32：`tests/tool_editor_catchup.rs` 用真实 pwsh 2500ms 进程写 `REAL-CATCHUP-TOOL-DONE`，Editor 600ms 先就绪，工具仍打开；工具完成后采用且只有 1 次 Editor 调用。
- F14/F71：私有递归测试断言 L3 成员的 cb_id/level 与 level-2 cbs、source_ids，不只比较数量。
- F62：`experiment_verify` 另覆盖 pack 后 `config.update`，verify 报 `config_changed`；原始 CB data/digest 篡改测试仍在；pack 在 Store 关闭/重开后仍能重验。
- F68：旧 L2 与 `pvi_correction` 旧 CB 的 stored data/digest 前后一致，而非只比较 replay/body。

## 真实外部进程工具区间

- `tests/tool_editor_real_process.rs`：测试宿主用真实 `pwsh` 进程执行 1.6s，并写出 `REAL-TOOL-DONE` 日志；实测 editor=1794464469..1794464473、tool=1794464470..1794464473、overlap=3 秒。这是外部进程级 fixture，不是 DSH native 工具，也不是真实 Editor Provider 延迟。

## 真实 peer 任务（同日，顺序与并发）

- 父链先由真实 DSH + 固定模型完成原生 `read`，回复 `PARENT-READY`；父 cut=1。
- 顺序两个 peer：peer1 子链 `read`→`CHILD-READY`（cut 3），peer2 子链 `write`→`CHILD-TWO-WRITE`（cut 4，文件副作用核验）；两次 reply 均 `reply_delivered`，父链 cut 3 的 CB 分别含两个结果。
- 随后并发两个真实 DSH 子进程（Start-Job 同时启动）：A 用 `read` 返回 `CONCURRENT-A`（cut 5，2 次 Worker attempt），B 用 `write` 创建 `b.txt` 并返回 `CONCURRENT-B`（cut 5，3 次 attempt）；A/B 子链时间区间 1794455478–536 与 1794455479–542 实际重叠，两次 reply 均投递，父链 cut 4 的 CB 同时含 A/B。
- 并发运行使父链累计 4 个 peer task、24 次 Provider attempt（全部 HTTP 200），prompt 138,267、completion 7,429、provider-reported cached 105,984、reasoning 5,959；6 个已完成 host task（4 read + 2 write）。
- 顺序运行曾暴露并促成修复一个真实的组级 open-call 问题：同一 Worker 响应中的并行 search/expand 结果被拆成后续 source part 后，scheduler 从根 source 读取 open_calls 会误判未关闭；现在 `source.read` 支持 `group:true`，scheduler 使用组级累计状态，回归测试在 `tests/worker_tool_core.rs`。并发运行验证了该修复。
- 仍不是 F45/F49 完整验收：已扩展到 2/4/6/8 并发 peer，并有 cancel/child failure 注入；仍缺更复杂业务工具与 watcher 长跑。reply 记录在投递时 `requester_adopted_cb=false`，采用是在后续 cut 发生，必须看 cut 正文而不能只看 reply 字段。
- 父链真实 DSH 续做：先修复 bridge 的两个会话状态问题——failed turn 只回收显式 `failed` 的 active fingerprint；恢复时只保留当前 pending group 的 `pending_sources`——并清理诊断产生的未配对 WB 后，同一父 session 在父链 cut 5 成功输出 `CONCURRENT-A`、`CONCURRENT-B` 和 `PARENT-CONTINUE-DONE`，证明 peer 采用结果进入实际下一 Worker 输入。该轮后累计 26 次 Provider 200、prompt 150,013/completion 7,890/cached 114,176/reasoning 6,344。
- 早期 checkpoint 复用：父链已推进到 cut5 后，复用第一 peer 的 cut1 socket/checkpoint 再派发真实 child；child 用 `read` 返回 `EARLY-CHECKPOINT`/`EARLY-READY`（child cut3、3 次 attempt、reply delivered），父链 cut6 采用。该父链累计 31 次 Provider 200、prompt 177,884/completion 10,026/cached 136,192/reasoning 8,206。
- 另一真实 peer 使用 `glob`：子链 cut3、4 次 Worker attempt，列出 alpha.txt/beta.txt，回复 `GLOB-READY`；`reply_delivered`，父链 cut8 采用。父链累计 6 peer task/37 次 Provider 200，prompt 211,442/completion 11,952/cached 163,712/reasoning 9,770。
- 真实 mid-run 取消：Provider guard 延迟 4s 期间启动真实 child，task 已在 `prepared` 状态运行；host 用 `peer.task.unknown` 置 `unknown`（revision 4），late advance 被拒；父链 cut9 继续。为此修复了 peer task 状态机，允许 `prepared -> unknown`，并加隔离测试。
- 八个真实 DSH peer 从同一 checkpoint 并发（read/write/glob/grep/read/write/read/glob）：8/8 reply delivered，父链 cut14 采用，child cut15；父链累计 26 peer task、154 次 model/worker attempt（145 次 HTTP 200）、prompt 923,612/completion 71,701/cached 663,808/reasoning 60,896。
- 六个真实 DSH peer 从同一 checkpoint 并发：read/write/glob/grep/read/write 全部 reply delivered，6/6 父链 cut13 采用，child cut14；父链累计 18 peer task、114 次 model/worker attempt（105 次 HTTP 200）、prompt 645,295/completion 46,299/cached 472,704/reasoning 39,210。
- 四个真实 DSH peer 从同一 checkpoint 并发：`read` FOUR-A、`write` FOUR-B、`glob` FOUR-C、`grep` FOUR-D；4/4 reply delivered、4/4 父链 cut12 采用，child cut13；父链累计 12 peer task、85 次 model/worker attempt（76 次 HTTP 200）、prompt 449,333/completion 32,614/cached 340,608/reasoning 27,555。
- 真实 `grep` peer：child cut3、6 次 Worker attempt，在 data.txt 命中 NEEDLE，回复 `GREP-READY`，`reply_delivered`，父链 cut10 采用；父链累计 7 peer task、56 次 model/worker attempt（53 次 HTTP 200）、prompt 301,214/completion 17,252/cached 238,592/reasoning 14,001。
- 真实 unknown turn 恢复：DSH turn 在 Editor 阶段被强杀 Core；重启后 bridge turn 为 `indeterminate`/`bridge_restarted`（`editor_model_started`）；retire + `runtime.discard_unpublished` 清理 current-cut running/failed 未发布工件，新 DSH session 同链输出 `UNKNOWN-RECOVERED`，已有 CB 保留。第二次同链强杀留下 current-cut `open` job（无 candidate）；discard 显式 `allow_open=true` 清理后新 session 同链输出 `UNKNOWN2-RECOVERED`，前序 4 个 CB 保留；默认仍拒绝 open job。第三次强杀留下 current-cut `inflight` job（无 candidate/CB）；`allow_open=true` 同时允许 open/inflight，discard 后新 session 同链输出 `UNKNOWN3-RECOVERED`，前序 5 个 CB 保留。
- 真实 child provider failure：guard 放行首次 Editor 后，child 的 Provider 请求返回 500，child exit1；host 将 prepared task 置 `unknown`（revision4），late advance 拒绝，父链 cut11 继续。父链累计 8 peer task、64 次 model/worker attempt（55 次 HTTP 200）、prompt 311,588/completion 17,853/cached 246,784/reasoning 14,445。
- 另一次真实 provider 500 发生在 peer prepare 阶段：Editor 3 次失败后 task 自动进入 `unknown`（revision 3），child 未执行，父链未推进；该样本只证明失败隔离，不冒充 child 运行中失败或父链续做。
- 真实取消注入：从早期 socket 派发的新 task 在 child 执行前被 `peer.task.unknown` 置为 `unknown`（claim revision 2），随后的 late `peer.task.advance` 被拒绝；父链继续 ingest/schedule 到 cut7。
- 失败 turn 恢复也已验证：一次真实 `bash` 工具尝试留下 failed turn 后，bridge 自动回收 active fingerprint 并过滤 stale pending_sources，下一 turn 成功返回 `MAIN-AFTER-FIX`。
- HTTP 500 注入：本机 loopback guard 对 Editor 请求返回 500，真实 DSH turn 记录为 `indeterminate` / `editor_model_started`；同一 session 直接重试仍返回 409 `session_busy`。显式 retirement 后，`runtime.discard_unpublished` 清除当前 cut 的 pending source、未配对 WB 和 failed job；本次真实证据在 cut0，清理后新 DSH session 绑定同一 chain 成功并输出 `HTTP500-RECOVERED`；隔离测试 `tests/discard_unpublished.rs` 固定清理/边界；其中 cut1 用例证明已发布 CB、配对 WB 与 published job 保留，只清 failed current job。
- 后续 cut HTTP500 恢复：先正常发布 `BASE-CUT`（cut1），再对同一 chain 注入 HTTP500；Editor `indeterminate` 后 retire + `runtime.discard_unpublished` 清除当前 cut 未发布工件，新 DSH session 绑定同链输出 `LATER-RECOVERED`，`BASE-CUT` 的已发布 CB 保留。
- 回归测试：`tests/dsh_bridge.rs::stale_pending_source_is_filtered_before_wb_form` 用 mock chain context 验证 stale pending 被过滤；`failed_turn_fingerprint_is_reclaimed_on_next_session_turn` 验证 definite failed 恢复。
- 语义边界：`edit.run` 后的 provider failure 是不确定结果，bridge 不自动重放；直接重试保持 409 `session_busy`，必须显式 retirement。`tests/dsh_bridge.rs::indeterminate_edit_run_failure_requires_session_retirement` 固定该行为。
