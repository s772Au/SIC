# F01-F73 per-feature verification record

What was actually executed, feature by feature, against the tree at `85b7e99`.
A passing named test proves the listed mechanism; it never proves the whole
pass-condition set.

## Method

1. For every F-id, read the live status row in `CONTRACT-STATUS.md`.
2. Take the test binaries that row names, and resolve each against the file
   actually present under `rust-core/tests/` or `experiments/tests/`, so a
   citation to a moved or renamed file cannot masquerade as coverage.
3. Run each named binary individually (`cargo test --test <name>`). A single
   aggregate run truncates at the first binary that cannot start, which is how
   earlier totals under-counted by 22 targets.
4. Record the binary verdict, not the global test count, as the feature result.

## Result

| verdict | count | meaning |
|---|---|---|
| PASS | 72 | two or more cited tests ran green |
| THIN | 0 | the row carries one test only |
| PROBLEM | 0 | a cited binary failed or is missing |
| NO-TEST | 1 | no executable claim exists |

All 73 rows accounted for: 72 + 0 + 0 + 1 = 73.

| F | status (live row) | verdict | cited tests |
|---|---|---|---|
| F01 | 已接入；有限 live smoke | PASS | `bounded_flow` |
| F02 | 已接入；有限 live smoke | PASS | `bounded_flow` |
| F03 | 部分接入；Core 调度与 mock 测试 | PASS | `bounded_flow` |
| F04 | 部分接入；Core 与隔离 mock 并发证据 | PASS | `editing`, `multi_editor`, `multi_editor_concurrency` |
| F05 | 部分接入；Core 与模块级协议测试 | PASS | `f05_f06_provenance`, `f69_late_material`, `f69_program_keep`, `protocol`, `worker_tool_core` |
| F06 | 部分接入；Core 与模块级协议测试 | PASS | `f05_f06_provenance`, `f69_late_material`, `f69_program_keep`, `protocol`, `worker_tool_core` |
| F07 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F08 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F09 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F10 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F11 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F12 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F13 | 部分接入；Worker 高阶采用与表示切换有隔离证据 | PASS | `prefix`, `representation_switch` |
| F14 | 部分接入；私有上下文与只读背景已落地 | PASS | `editor_concurrency_abandon`, `editor_private_budget`, `editor_private_context`, `editor_private_recursion`, `multi_editor_concurrency` |
| F15 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F16 | 部分接入；Worker 高阶采用与表示切换有隔离证据 | PASS | `prefix`, `representation_switch` |
| F17 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F18 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F19 | 已接入；有限 live smoke | PASS | `bounded_flow` |
| F20 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F21 | 已接入；有限 live smoke | PASS | `bounded_flow` |
| F22 | 部分接入；前一版本真实 DSH 和固定模型限域验收 | PASS | `memory_search`, `worker_memory_core` |
| F23 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F24 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F25 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core`, `editing` |
| F26 | 已接入；本机 Core/存储测试；**F28 本轮改为两级程序目录 + `sic_tool_update`** | PASS | `decision`, `toolset` |
| F27 | 部分接入；冻结阶段信息与锚点有隔离证据 | PASS | `editor_config_freeze`, `editor_config_freeze` |
| F28 | 已接入；本机 Core/存储测试；**F28 本轮改为两级程序目录 + `sic_tool_update`** | PASS | `decision`, `toolset` |
| F29 | 部分接入；机械合规与事实保留分开 | PASS | `semantic_vs_rules` |
| F30 | 部分接入；结构化等待声明 + **本轮补上预测/声明/三策略** | PASS | `scheduler_boundary_wait`, `scheduler_budget`, `scheduler_config_compare`, `scheduler_open_calls` |
| F31 | 部分接入；**调度空间已补齐三选一（wait / advance / adaptive）**，真实重叠证据仍缺 | PASS | `scheduler_boundary_wait`, `tool_editor_catchup`, `tool_editor_overlap`, `tool_editor_real_process`, `tool_editor_wait`, `tool_timing`, `tool_tool_overlap` |
| F32 | 部分接入；**调度空间已补齐三选一（wait / advance / adaptive）**，真实重叠证据仍缺 | PASS | `scheduler_boundary_wait`, `tool_editor_catchup`, `tool_editor_overlap`, `tool_editor_real_process`, `tool_editor_wait`, `tool_timing`, `tool_tool_overlap` |
| F33 | 部分接入；**调度空间已补齐三选一（wait / advance / adaptive）**，真实重叠证据仍缺 | PASS | `scheduler_boundary_wait`, `tool_editor_catchup`, `tool_editor_overlap`, `tool_editor_real_process`, `tool_editor_wait`, `tool_timing`, `tool_tool_overlap` |
| F34 | 部分接入；逐 WB 许可与消费收据（调度原语） | PASS | `scheduler_budget`, `scheduler_budget` |
| F35 | 部分接入；同计划成对请求与变量首差矩阵 | PASS | `prefix`, `prefix_request_pair` |
| F36 | 部分接入；provider-reported 缓存观测 | PASS | `cache_levels`, `cache_observation` |
| F37 | 部分接入；块长/阶次配置对照（程序模式） | PASS | `scheduler_config_compare` |
| F38 | 基本 IPC 与有界 peer 自动执行部分接入；**F43 Signal 控制面本轮落地** | PASS | `peer_watch`, `signal_core`, `signal_core_dispatch`, `signal_model_tools` |
| F39 | 基本 IPC 与有界 peer 自动执行部分接入；**F43 Signal 控制面本轮落地** | PASS | `peer_watch`, `signal_core`, `signal_core_dispatch`, `signal_model_tools` |
| F40 | 基本 IPC 与有界 peer 自动执行部分接入；**F43 Signal 控制面本轮落地** | PASS | `peer_watch`, `signal_core`, `signal_core_dispatch`, `signal_model_tools` |
| F41 | 部分接入；多创建者与所有权分离；**本轮补 fork-origin 具名测试** | PASS | `creator_identity`, `creator_identity` |
| F42 | 基本 IPC 与有界 peer 自动执行部分接入；**F43 Signal 控制面本轮落地** | PASS | `peer_watch`, `signal_core`, `signal_core_dispatch`, `signal_model_tools` |
| F43 | 基本 IPC 与有界 peer 自动执行部分接入；**F43 Signal 控制面本轮落地** | PASS | `peer_watch`, `signal_core`, `signal_core_dispatch`, `signal_model_tools` |
| F44 | 基本 IPC 与有界 peer 自动执行部分接入；**F43 Signal 控制面本轮落地** | PASS | `peer_watch`, `signal_core`, `signal_core_dispatch`, `signal_model_tools` |
| F45 | 部分接入；真实 DSH 父 + 顺序/并发/早期 checkpoint peer + watcher 单轮 | PASS | `peer_tasks` |
| F46 | 部分接入；取消/迟到/显式重试有隔离测试 | PASS | `peer_retry_cancel` |
| F47 | 已接入；本机 Core 测试 | PASS | `board_core`, `board_security_audit` |
| F48 | 部分接入；真实 DSH 三轮 + failed/HTTP500/unknown-turn 恢复 | PASS | `discard_unpublished`, `dsh_bridge` |
| F49 | 部分接入；真实 DSH peer read/write 与并发采用 | PASS | `dsh_bridge`, `peer_runner_client` |
| F50 | 部分接入；真实 DSH 三轮 + failed/HTTP500/unknown-turn 恢复 | PASS | `discard_unpublished`, `dsh_bridge` |
| F51 | Rust HTTP API 已接入；官方工作台未验收 | PASS | `server` |
| F52 | Rust HTTP API 已接入；官方工作台未验收 | PASS | `server` |
| F53 | Rust HTTP API 已接入；官方工作台未验收 | PASS | `server` |
| F54 | 部分接入；Core/存储与隔离测试 | PASS | `board_security_audit`, `revocation_branches`, `revocation_entry_points`, `revocation_redaction_audit`, `revocation_scope`, `security_tool_core`, `tool_claim_core` |
| F55 | 部分接入；并发发布 CAS 与真实 Core 重启恢复 | PASS | `ipc_store`, `multitool_recovery`, `peer_retry_cancel`, `publish_cas`, `tool_restart_interrupt` |
| F56 | 部分接入；host 测量账本骨架 | PASS | `accounting_ledger`, `accounting_overlap`, `accounting_program_tools`, `prefix`, `tool_timing`, `cost_algebra`, `cost_control` |
| F57 | 部分接入；host 测量账本骨架 | PASS | `accounting_ledger`, `accounting_overlap`, `accounting_program_tools`, `prefix`, `tool_timing`, `cost_algebra`, `cost_control` |
| F58 | 部分接入；host 测量账本骨架 | PASS | `accounting_ledger`, `accounting_overlap`, `accounting_program_tools`, `prefix`, `tool_timing`, `cost_algebra`, `cost_control` |
| F59 | 部分接入；host 测量账本骨架 | PASS | `accounting_ledger`, `accounting_overlap`, `accounting_program_tools`, `prefix`, `tool_timing`, `cost_algebra`, `cost_control` |
| F60 | 部分接入；合成两工作流 + LongMemEval 单题两臂 pilot | PASS | `experiment_two_workflows`, `experiment_two_workflows` |
| F61 | 部分接入；官方 evaluator pilot + 独立 rubric 门槛 | PASS | `rubric_gate`, `rubric_gate` |
| F62 | 部分接入；可复验数据包骨架 | PASS | `experiment_pack`, `experiment_verify` |
| F63 | 研究设想 | NO-TEST | _none_ |
| F64 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F65 | 已接入；有限三阶 live smoke | PASS | `bounded_flow`, `core_flow`, `dsh_volatile`, `editing`, `memory_search`, `pvi_correction`, `worker_memory_core` |
| F66 | 部分接入；三类 host 测量 | PASS | `accounting_measurements`, `accounting_measurements` |
| F67 | 部分接入；显式敏感性政策与访问撤回 | PASS | `revocation_branches`, `revocation_levels`, `revocation_peer`, `sensitive_policy` |
| F68 | 部分接入；后续 PVI 更正已有隔离证据 | PASS | `pvi_correction`, `pvi_correction_levels` |
| F69 | 部分接入；Core 与模块级协议测试 | PASS | `f05_f06_provenance`, `f69_late_material`, `f69_program_keep`, `protocol`, `worker_tool_core` |
| F70 | 部分接入；Worker 高阶采用与表示切换有隔离证据 | PASS | `prefix`, `representation_switch` |
| F71 | 部分接入；私有上下文与只读背景已落地 | PASS | `editor_concurrency_abandon`, `editor_private_budget`, `editor_private_context`, `editor_private_recursion`, `multi_editor_concurrency` |
| F72 | 部分接入；冻结阶段信息与锚点有隔离证据 | PASS | `editor_config_freeze` |
| F73 | 部分接入；公开来源与 SIC 适配包（preparation 部分） | PASS | `f73_preparation` |

## The one NO-TEST row

**F63** is a research proposal, not a runtime protocol. There is no executable
claim to test, so having no test binary is the correct state rather than a gap.
Its two negative cases ("claiming a semantic space is done without running
embeddings or propagation", "treating an internal training idea as a shipped
capability") are claims about prose, and the row's status is `研究设想` rather
than `部分接入`.

## Tests added this round

| F | test | what it now covers that was unasserted |
|---|---|---|
| F25 | `editing::instant_never_becomes_working_source_and_old_instant_is_not_reapplied` | a later round does not inherit the previous I; I is not an edit target; ordinary I text cannot carry a capability field |
| F27 | `editor_config_freeze::a_later_job_freezes_the_new_stage_while_the_old_job_keeps_its_own` | a stage change freezes the new goal for new work without rewriting an earlier prepared job |
| F27 | `editor_config_freeze::constraint_text_cannot_grant_a_mode_or_capability` | constraint prose declaring its own authority stays data; the assignment set still bounds editing |
| F29 | `semantic_vs_rules::a_fixed_compression_ratio_never_stands_in_for_task_utility` | a shrink ratio cannot stand in for which facts survived |
| F34 | `scheduler_budget::a_permit_spent_on_one_wb_does_not_authorize_another` | two ledgers, not one chain total |
| F36 | `cache_observation::a_provider_that_reports_no_cache_is_unknown_not_a_zero_hit` | an unreported cache field stays null and counts as an unknown attempt, never a measured miss |
| F41 | `creator_identity::fork_records_its_origin_and_is_not_a_creator_child` | a fork records its origin route plus inherited CB prefix and is not a creator child |
| F60 | `experiment_two_workflows::a_paid_arm_without_explicit_authorization_is_refused` | a paid arm without `allow_model` halts the pass and publishes nothing |
| F61 | `rubric_gate` (+3) | an anonymous rubric cannot produce a score; two arms from different tasks cannot look comparable; one out-of-range dimension fails the whole score |
| F66 | `accounting_measurements::measurements_stay_three_separate_classes_and_never_merge_into_a_score` | the three classes stay separate; no merged score or inferred price |
| F73 | `f73_preparation` (3 tests) | every locked source names a full commit and blob SHA; the metadata stays metadata-sized; the package claims no runtime result |
| — | `invariant_stress::a_random_operation_sequence_keeps_the_append_only_invariants` | 24-step deterministic sequence: cut grows by one, earlier CBs stay replayable, read-only steps never mutate, decision view carries no capability |

## Known limits

- No real Provider or real DSH run was made in this pass. Rows requiring one
  remain partial in `CONTRACT-STATUS.md` and say so.
- `signal_core_dispatch` cannot execute from its own path on this machine
  (Windows image-load policy, not a code defect). Copying the built binary
  elsewhere runs all 8 tests; see `known-issues-20261114.md`.
- A green suite proves the code does what its tests say. It does not prove the
  paper's claims about model behaviour, cache economics or task quality.
