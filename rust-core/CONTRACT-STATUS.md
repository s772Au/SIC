# Rust Core 契约迁移状态

## 当前工作记录（2026-11-10，开发中）

下表是独立 `rust-core` 的现状与证据。后面的逐条表格保留迁移开始时的
基线；其中“初始状态”不是实时结论。状态按当前精简协议核对：单 Editor、
Worker 等待、共同稳定前缀及独立 I 视图。每项都区分代码存在、测试范围和
真实 Provider/DSH 验收；任何一项都不代表生产可用。

| 契约项 | 状态 | 当前实现与证据 | 未实现或未验收 |
|---|---|---|---|
| F01、F02、F19、F21 | 已接入；有限 live smoke | 单 Editor/Worker 流程、固定角色配置、共享前缀和受信 I 已接入。`bounded_flow.rs` 用本机 mock Provider 验证多阶编辑/等待/flush 与错误不重试。固定 `deepseek-v4.1-flash-eco` 的 Provider smoke 显示 Editor ready、发布 1 个 CB、Worker 返回文本且无 host task（`.local/rust-live-provider-smoke-4-20261110.log`）。 | 这只是受控 live smoke，不代表长上下文/生产稳定；实际 Provider usage 不从 smoke 推算，物理缓存命中仍未知。 |
| F07–F12、F15、F17、F18、F20、F23–F25、F64–F65 | 已接入；有限三阶 live smoke | WB/CB 顺序与引用、编辑候选/回放、P/V/I、显式高阶形成和有界 Provider 调度有 Core 与 mock 测试。固定模型 hierarchy smoke 记录 7 次已观察调用、0 次未知预留，发布序列到 level 3 且有 1 个三阶 CB（`.local/rust-live-hierarchy-smoke-20261110.log`）。 | 一次三阶 run 不证明所有阶次/角色的完整采用、长期自动调度或复杂任务质量；F04 多 Editor 与 F14/F71 Editor 私有递归未实现。 |
| F05、F06、F69 | 部分接入；模块级协议测试 | `protocol.rs` 对 OpenAI Chat、Responses、Anthropic 的保守文本子集做 sidecar roundtrip；静态 inline image 引用保留，未知/有损投影拒绝。Core Chat 路径保留源分组、工具调用关联和迟到结果；见 `tests/protocol.rs`、`tests/core_flow.rs`。 | 协议模块测试不等于三家 Provider 全部端到端；完整多模态、流式和大块拆分尚未验收。 |
| F26、F28 | 已接入；本机 Core/存储测试 | 四视角决策与动态工具 ACL/搜索/展开/回收、执行租约、15 轮回收及 128 工具上限有 Rust 实现和 Core/存储测试。 | 面向真实外部工具的生产宿主执行、跨部署执行器和全部决策工作流仍待验收。 |
| F38–F44、F55 | 基本 IPC 与串行 peer 自动执行部分接入 | 历史 checkpoint、Socket 独立 ACL/read 分页、分身创建、Message P/V、Signal、撤权/故障隔离及持久化恢复有 Core 测试；分身任务可持久记录、原子认领、有界 Editor 准备和校验实际 Worker 来源后幂等回传。显式启用的 Rust watcher 已通过真实 DSH、本机 mock Provider 的自动发现、DSH 自带 read 读取隔离合成文件、回传及请求路线发布采用验收；单任务 supervisor 另在真实 DSH 和固定 OpenDesign 模型下完成一次合成工具往返。 | F45/F46/F49 的多 peer 并发、失败治理、真实模型 watcher 与其他业务工具尚未完成验收。未知外部副作用仍须宿主核对。 |
| F47 Board | 已接入；本机 Core 测试 | SQLite Board、独立 ACL/撤权、CAS、文字/结构化条目、任务认领/进度、模型不能伪造完成，以及每次 CB 发布同事务刷新最新 I 已接入；见 `board.rs`、`board_core.rs`、`board_security_audit.rs`。 | 真实 DSH peer 协作及官方 UI 未验收；已发布 CB 之间的 I 不主动侵入现有上下文，下次 CB 发布取最新值。 |
| F48、F50 | bridge 已接入；真实 DSH + 真实模型限域验收 | 真实 DSH headless、Rust Core 与 OpenDesign `deepseek-v4.1-flash-eco` 在主 Agent 和单任务分身上分别完成只读合成工具往返；分身结果通过 Socket 投递，并被请求路线发布的 CB 采用。模拟模型另验证了分身调用 DSH 内置 `read` 读取合成文件，以及 24 轮、跨重启历史/CB 和已交付结果重放。 | live 只证明合成 fixture，不证明其他业务工具。Provider 曾返回 HTTP 500，失败/未知 turn 的同会话完整恢复仍未实现。 |
| F51–F53 | Rust HTTP API 已接入；官方工作台未验收 | Rust HTTP 控制面支持 loopback、Bearer 和 host 权限上限；API 组件测试已覆盖。 | Langflow/Studio 官方工作台绑定、GUI 页面交互和同一 Core 的实际部署流程不由这些 Rust/API 测试证明；尚未验收。 |
| 迁移隔离 | 窄范围归档/导入与可审计启动已接入；通用转换未实现 | Python DB 只读快照、窄范围 stopped-text 导入和单 Agent 活动库隔离升级有实现/fixture 测试；实际旧库独立副本恢复了经过规则验证的 `task_goal` 与 `editor_feedback`，核验 owner 与 Board ACL 后启动，真实 DSH 与本机模拟 Provider 在临时备份上完成绑定和工具往返。源快照不改写。 | 不是通用转换；5 条旧事件仍隔离且不重放，旧工具授权未自动恢复，复杂历史及活动桥接状态不能整体转成可运行 Rust Agent。源库之后的新变化不自动同步。 |
| F03、F13、F14、F16、F22、F27、F29–F37、F67–F73 | 部分原语已接入；整体未验收 | 多数有局部原语、模块或 mock 测试。 | 持续调度策略、完整 Worker 高阶采用矩阵、全部测量/计费/评分、真实缓存证明、敏感数据全生命周期与研究实验包尚未逐项验收。 |
| F56–F62、F66 | 未完成 | 账务/实验报告目前没有可与真实 Provider 账单完整对账的结果。 | 成本总账、质量评分、可复验实验包和公开 prompt 对照未完成；不得从 token surrogate 推断费用。 |
| F63 | 研究设想 | 保持为研究设想。 | 不是已定运行协议，也不计作已实现功能。 |

开发服务仅接受本机监听和宿主认证。这里记录的是有限功能证据，不是生产验收，也不继承 Python 或旧实验的通过状态。

本表将 `paper/review/sic_implementation_contract.md` 的 F01–F73 逐项登记为 Rust 迁移待办。Python 入口和具名测试只用于定位现有参考行为；**Python 实现或测试通过不构成 Rust 实现或验收通过**。

- 基线来源：`docs/contract-implementation-audit-20261110.md`；其记录的 Python Core 源码身份为 `3733a129f6fae10434207a67627829002f13f7ab78e4cf3e110dd046fdeaa472`。
- 每项的 Rust 初始状态均为 **待实现 / 未验收**。旧 `sic-proxy` 原型不继承本表状态，也不能替代新 Rust core 的实现与测试。
- Python 测试名是检索线索，不是 Rust 通过证据；审计指出旧账本中有测试文件 SHA 已变化，引用前应按当前检出复核。缺具名测试的研究项按研究脚本/资料入口列出。
- F63 是可选研究边界，不是已定运行协议；仍登记为研究待办边界，不能把它算成工程协议缺口。

## 双链与编辑交付

| ID | 契约项 | Python 入口（定位） | Python 具名测试（定位） | Rust 初始状态 |
|---|---|---|---|---|
| F01 | 同一Agent的Worker、Editor与主动程序分工 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2_mechanisms.py :: test_real_parallel_editors_and_active_program_rule_combine` | **待实现 / 未验收** |
| F04 | 多个Editor实际分工与合并 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2_mechanisms.py :: test_real_parallel_editors_and_active_program_rule_combine` | **待实现 / 未验收** |
| F05 | 文本与协议分离并回组 | `sic_core/adapters.py :: decode; encode; LiteLLMProvider.call` | `tests/test_core_v2.py :: test_native_text_tool_roundtrips_three_formats<br>tests/test_core_v2_media.py :: test_native_responses_and_anthropic_keep_images_and_reject_lossy_projection` | **待实现 / 未验收** |
| F06 | 原始材料与WB块内来源 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2_mechanisms.py :: test_long_single_text_split_keeps_exact_code_characters` | **待实现 / 未验收** |
| F07 | WB链自身形成时序 | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_hierarchy.py :: test_independent_two_chains_correspondence_and_full_recursive_content` | **待实现 / 未验收** |
| F08 | CB链顺序延伸与永久固定 | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_hierarchy.py :: test_concurrent_upper_candidates_publish_exactly_one_same_level_pair` | **待实现 / 未验收** |
| F09 | 同阶WB_i与CB_i双向对应 | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_hierarchy.py :: test_independent_two_chains_correspondence_and_full_recursive_content` | **待实现 / 未验收** |
| F17 | 默认KEEP与有序组合执行 | `sic_core/editing.py :: validate_response; materialize; replay` | `tests/test_core_v2_mechanisms.py :: test_model_candidate_cas_and_composable_ops_original_response_immutable` | **待实现 / 未验收** |
| F18 | 完整内容操作族 | `sic_core/editing.py :: validate_response; materialize; replay` | `tests/test_core_v2.py :: test_named_add_anchor_and_unknown_anchor_rejection` | **待实现 / 未验收** |
| F19 | 单轮Editor完整交付 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2.py :: test_multi_editor_scope_and_single_round` | **待实现 / 未验收** |
| F20 | 候选版本可修订且不改原交付 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2_mechanisms.py :: test_model_candidate_cas_and_composable_ops_original_response_immutable` | **待实现 / 未验收** |
| F21 | 一致固定发布与Worker采用 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2_mechanisms.py :: test_worker_inflight_publish_defers_without_discarding_valid_reply` | **待实现 / 未验收** |
| F22 | 固定op模型可发现与细粒度追溯 | `sic_core/memory.py :: project; page; bounded_instant` | `tests/test_core_v2_mechanisms.py :: test_worker_reads_original_model_ops_and_manual_revision_via_bounded_proposal_tool` | **待实现 / 未验收** |
| F64 | 共享同阶链和高阶产物而非强制双份 | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_hierarchy.py :: test_internal_editor_context_recurses_and_individual_editor_adopts_shared_references` | **待实现 / 未验收** |
| F65 | 普通编辑的任意中间状态回放 | `sic_core/editing.py :: validate_response; materialize; replay` | `tests/test_core_v2_replay.py :: test_each_intermediate_state_uses_the_same_archived_source` | **待实现 / 未验收** |
| F69 | 后到材料不冒充Editor已见范围 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2.py :: test_post_snapshot_program_keep_precise_index` | **待实现 / 未验收** |
| F70 | 活动表示切换的边界和缓存代价 | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_frontier_hierarchy.py :: test_projection_can_move_additional_slot_and_position_budget_is_not_assumed` | **待实现 / 未验收** |

## PVI与决策授权

| ID | 契约项 | Python 入口（定位） | Python 具名测试（定位） | Rust 初始状态 |
|---|---|---|---|---|
| F23 | 持久P进入本次CB | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2.py :: test_pvi_atomic_replacement_and_retry<br>tests/test_core_v2_communication.py :: test_receipt_repeat_notify_and_no_same_cut_prefix_change` | **待实现 / 未验收** |
| F24 | 易失V进入下一WB | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2_mechanisms.py :: test_generated_content_lifecycle_independent_from_persistent_ops<br>tests/test_core_v2_communication.py :: test_receipt_repeat_notify_and_no_same_cut_prefix_change` | **待实现 / 未验收** |
| F25 | 即时I更替且不混入WB | `sic_core/memory.py :: project; page; bounded_instant` | `tests/test_core_v2_frontier_hierarchy.py :: test_instant_refresh_never_becomes_high_order_editable_source` | **待实现 / 未验收** |
| F26 | 四视角决策工作台持续维护 | `sic_core/decision.py :: apply; from_editor` | `tests/test_core_v2_mechanisms.py :: test_four_view_decision_updates_with_sourced_events_and_editor` | **待实现 / 未验收** |
| F27 | 约束、锚点和阶段信息维护 | `sic_core/memory.py :: project; page; bounded_instant` | `tests/test_core_v2_mechanisms.py :: test_four_view_decision_updates_with_sourced_events_and_editor` | **待实现 / 未验收** |
| F28 | Lazy tool发现、扩展与回收 | `sic_core/memory.py :: project; page; bounded_instant` | `tests/test_core_v2.py :: test_board_cas_tools_three_permissions` | **待实现 / 未验收** |
| F29 | 语义理想与规则验证分开 | `sic_core/editing.py :: mechanical response checks; independent quality requires registered rubric` | `tests/test_core_v2.py :: test_protocol_fields_not_editor_targets_and_conflicting_i` | **待实现 / 未验收** |
| F54 | 合法编辑授权与敏感撤销 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2_host_tools.py :: test_current_route_revocation_after_dispatch_blocks_result_and_replay` | **待实现 / 未验收** |
| F67 | 敏感材料专门路径的实际保证 | `sic_core/store.py :: Store.get; Store.revoked; Core._dispatch(security.revoke)` | `tests/test_core_v2_hierarchy.py :: test_recursive_dependency_revocation_blocks_upper_and_old_branch` | **待实现 / 未验收** |
| F68 | 后续PVI更正实际影响工作 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2.py :: test_pvi_atomic_replacement_and_retry<br>tests/test_core_v2_communication.py :: test_receipt_repeat_notify_and_no_same_cut_prefix_change` | **待实现 / 未验收** |
| F72 | 任务和阶段对齐的信息放置 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2_mechanisms.py :: test_historical_peer_executes_edits_and_returns_result_without_blocking_sender<br>tests/test_core_v2_collaboration_policy.py :: test_p_and_v_omitted_run_policy_and_resend_are_stable<br>tests/test_core_v2_communication.py :: test_receipt_repeat_notify_and_no_same_cut_prefix_change` | **待实现 / 未验收** |

## 递归与活动表示

| ID | 契约项 | Python 入口（定位） | Python 具名测试（定位） | Rust 初始状态 |
|---|---|---|---|---|
| F02 | 按角色及阶次固定模型与完整输入预算 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2_sdk.py :: test_frozen_profile_and_budget_rejection_before_network` | **待实现 / 未验收** |
| F10 | 二阶WB有序引用下阶CB | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_hierarchy.py :: test_scope_order_and_route_guards` | **待实现 / 未验收** |
| F11 | 高阶真实编辑与固定产物 | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_hierarchy.py :: test_higher_pvi_candidate_original_and_fixed_ops` | **待实现 / 未验收** |
| F12 | 一般N阶递归而非写死二阶 | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_hierarchy.py :: test_sustained_maintenance_reaches_new_levels_and_continues` | **待实现 / 未验收** |
| F13 | Worker真正采用高阶表示 | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_hierarchy_sdk.py :: test_actual_sdk_third_level_adoption_recursive_tools_paginated_original_and_next_worker` | **待实现 / 未验收** |
| F14 | Editor自身上下文递归管理 | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_hierarchy.py :: test_internal_editor_context_recurses_and_individual_editor_adopts_shared_references` | **待实现 / 未验收** |
| F15 | 跨阶逐层展开直到原WB | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_hierarchy_sdk.py :: test_actual_sdk_third_level_adoption_recursive_tools_paginated_original_and_next_worker` | **待实现 / 未验收** |
| F16 | 高阶表示下继续时间推进与分叉 | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_hierarchy.py :: test_historical_fork_restores_full_layers_and_role_selection` | **待实现 / 未验收** |
| F38 | Agent身份与内容身份分开 | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_hierarchy.py :: test_internal_editor_context_recurses_and_individual_editor_adopts_shared_references` | **待实现 / 未验收** |
| F71 | Editor独有持续材料同样递归 | `sic_core/hierarchy.py :: register_wb; dispatch; context_ids; selected_view` | `tests/test_core_v2_hierarchy_sdk.py :: test_actual_sdk_editor_private_third_order_and_distinct_editor_selections` | **待实现 / 未验收** |

## 调度与交错

| ID | 契约项 | Python 入口（定位） | Python 具名测试（定位） | Rust 初始状态 |
|---|---|---|---|---|
| F03 | 新材料驱动的持续编辑 | `sic_core/runtime.py :: Runtime.run; Runtime._worker_loop; Runtime.collaborate; Runtime._refresh_deadlines` | `tests/test_core_v2_pending_flush.py :: test_terminal_short_block_flushes_without_more_input_or_provider<br>tests/test_core_runtime_restart.py :: test_explicit_reenable_rearms_old_pending_deadline_after_recovery` | **待实现 / 未验收** |
| F30 | 自然WB边界与等待声明 | `sic_core/runtime.py :: Runtime.run; Runtime._worker_loop; Runtime.collaborate; Runtime._refresh_deadlines` | `tests/test_core_v2_mechanisms.py :: test_wait_statement_changes_actual_schedule_input_and_cancellation` | **待实现 / 未验收** |
| F31 | 工具运行期即开始编辑 | `sic_core/runtime.py :: Runtime.run; Runtime._worker_loop; Runtime.collaborate; Runtime._refresh_deadlines` | `tests/test_core_v2_mechanisms.py :: test_host_tool_execution_overlaps_real_editor_dispatch_and_legal_continuation` | **待实现 / 未验收** |
| F32 | 赶上时无额外等待推进 | `sic_core/runtime.py :: Runtime.run; Runtime._worker_loop; Runtime.collaborate; Runtime._refresh_deadlines` | `tests/test_core_v2_bridge_runtime_contract.py :: test_host_wait_complement_excludes_overlapping_tools_and_cancelled_wait` | **待实现 / 未验收** |
| F33 | 工具先结束时串行等待 | `sic_core/runtime.py :: Runtime.run; Runtime._worker_loop; Runtime.collaborate; Runtime._refresh_deadlines` | `tests/test_core_v2_mechanisms.py :: test_measured_serial_wait_and_explicit_interleave_intervals_are_separate` | **待实现 / 未验收** |
| F34 | 有代价交错与逐WB预算 | `sic_core/runtime.py :: Runtime.run; Runtime._worker_loop; Runtime.collaborate; Runtime._refresh_deadlines` | `tests/test_core_v2.py :: test_interleave_concurrency_one_budget` | **待实现 / 未验收** |
| F35 | 同次必要替换与附加编辑对照 | `sic_core/frontier.py :: compare; canonical; actual role-specific simulated request adoption` | `tests/test_core_v2_frontier_hierarchy.py :: test_same_cut_high_worker_comparison_matches_actual_adopted_prefix` | **待实现 / 未验收** |

## DSH协作与工作台

| ID | 契约项 | Python 入口（定位） | Python 具名测试（定位） | Rust 初始状态 |
|---|---|---|---|---|
| F41 | 从空白新建与多创建者 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2.py :: test_fork_peers_and_multi_creator` | **待实现 / 未验收** |
| F42 | Socket历史接收点与独立权限 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2.py :: test_socket_spawn_separate_history_acl_and_first_message_both_lifetimes` | **待实现 / 未验收** |
| F43 | Signal实际控制与唤醒 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2_mechanisms.py :: test_signal_pauses_inflight_editor_preserving_candidate_for_cas_and_resume` | **待实现 / 未验收** |
| F44 | Message信箱在覆写收取 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2.py :: test_socket_spawn_separate_history_acl_and_first_message_both_lifetimes<br>tests/test_core_v2_collaboration_policy.py :: test_p_and_v_omitted_run_policy_and_resend_are_stable<br>tests/test_core_v2_communication.py :: test_receipt_repeat_notify_and_no_same_cut_prefix_change` | **待实现 / 未验收** |
| F45 | 选择一批历史peer实际解题回传 | `sic_core/runtime.py :: Runtime.run; Runtime._worker_loop; Runtime.collaborate; Runtime._refresh_deadlines` | `tests/test_core_v2_mechanisms.py :: test_worker_socket_dispatch_autonomously_executes_peer_and_consumes_return<br>tests/test_core_v2_collaboration_policy.py :: test_p_and_v_omitted_run_policy_and_resend_are_stable` | **待实现 / 未验收** |
| F46 | 协作失败、取消与有限重试 | `sic_core/runtime.py :: Runtime.run; Runtime._worker_loop; Runtime.collaborate; Runtime._refresh_deadlines` | `tests/test_core_v2_host_tools.py :: test_child_pending_timer_observes_inflight_collaboration_without_duplicate_execution<br>tests/test_core_v2_collaboration_policy.py :: test_p_and_v_omitted_run_policy_and_resend_are_stable` | **待实现 / 未验收** |
| F47 | 共享公示牌、任务认领与每cut最新视图 | `sic_core/mechanisms.py :: Mechanisms._features` | `tests/test_core_v2.py :: test_shared_board_latest_at_branch_cut_and_cas` | **待实现 / 未验收** |
| F48 | DSH真实主Agent原生往返 | `sic_core/bridge.py :: Bridge request/history/receipt paths; sic_core/dsh_plugin` | `tests/test_core_v2_bridge.py :: test_binding_and_reply_survive_new_core_and_bridge_instances` | **待实现 / 未验收** |
| F49 | DSH peer真实工具与结果采用 | `sic_core/host_tools.py :: HostToolBroker.execute; HostToolBroker.action` | `tests/test_core_v2_host_tools.py :: test_actual_begin_events_and_idempotent_completion_no_reexecution<br>tests/test_core_v2_bridge_runtime_contract.py :: test_unclaimed_collaboration_survives_recovery_and_runs_only_after_reenable` | **待实现 / 未验收** |
| F51 | Langflow官方全链可见可编辑 | `integrations/components/sic :: domain components; integrations/generate_workbench_flows.py` | `integrations/tests/test_runtime_error_visibility.py :: test_both_workbench_paths_expose_and_resolve_interrupted_tool` | **待实现 / 未验收** |
| F52 | Studio官方全链独立验收 | `integrations/sic_workflow/workbench_graph.py :: graph domain callbacks; integrations/sic_workflow/domain_graphs.py` | `integrations/tests/test_runtime_error_visibility.py :: test_both_workbench_paths_expose_and_resolve_interrupted_tool` | **待实现 / 未验收** |
| F53 | 常驻同Core绑定与一键状态 | `integrations/run_core_host.py :: shared Core host; bridge binding; persistent UI clients` | `integrations/tests/test_runtime_error_visibility.py :: test_both_workbench_paths_expose_and_resolve_interrupted_tool<br>tests/test_core_v2_bridge_runtime_contract.py :: test_unclaimed_collaboration_survives_recovery_and_runs_only_after_reenable` | **待实现 / 未验收** |

## 恢复与分叉

| ID | 契约项 | Python 入口（定位） | Python 具名测试（定位） | Rust 初始状态 |
|---|---|---|---|---|
| F39 | 历史checkpoint整体恢复 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2_current_authority.py :: test_later_revocation_reaches_existing_child_and_grandchild` | **待实现 / 未验收** |
| F40 | 平级分叉与原路线继续 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_v2.py :: test_fork_peers_and_multi_creator` | **待实现 / 未验收** |
| F50 | 会话历史、收据与重启续做 | `sic_core/bridge.py :: Bridge request/history/receipt paths; sic_core/dsh_plugin` | `tests/test_core_v2_host_tools.py :: test_completed_and_indeterminate_receipts_survive_reopened_database` | **待实现 / 未验收** |
| F55 | 可检查的故障恢复和并发发布 | `sic_core/service.py :: Core._dispatch; Core._model; Core._publish; Core._new` | `tests/test_core_runtime_restart.py :: test_interrupted_queue_is_scoped_durable_and_requires_acknowledged_recovery<br>tests/test_core_runtime_restart.py :: test_killed_bridge_worker_returns_unknown_before_waiting_and_requires_recovery<br>tests/test_core_v2_bridge_runtime_contract.py :: test_recovery_refuses_a_live_request_even_after_lease_expiry<br>tests/test_core_v2_bridge_runtime_contract.py :: test_recovery_refuses_direct_live_editor_and_preserves_its_result<br>tests/test_core_v2_bridge_runtime_contract.py :: test_unclaimed_collaboration_survives_recovery_and_runs_only_after_reenable` | **待实现 / 未验收** |

## 账务与研究边界

| ID | 契约项 | Python 入口（定位） | Python 具名测试（定位） | Rust 初始状态 |
|---|---|---|---|---|
| F36 | 真实缓存证据与逻辑checkpoint分开 | `sic_core/service.py :: Core._frontier; Core._normalized_usage` | `tests/test_core_v2.py :: test_memory_tool_execution_and_cache_frontier_evidence` | **待实现 / 未验收** |
| F37 | 块长、输出形式和阶次可配置比较 | `sic_core/service.py :: Core._dispatch(config.update); Core._model` | `tests/test_core_v2_sdk.py :: test_frozen_profile_and_budget_rejection_before_network` | **待实现 / 未验收** |
| F56 | 完整计费类别与全角色/全阶次总账 | `sic_core/task_ledger.py :: export_task; bind_prices; insert_unique` | `tests/test_core_task_ledger.py :: test_recursive_peer_higher_editor_prices_routes_and_union_time` | **待实现 / 未验收** |
| F57 | 程序、存储、适配和工具成本 | `sic_core/accounting.py :: normalize_usage; summarize_records; timing_summary; controlled_cost; compare_workflows` | `tests/test_core_v2_mechanisms.py :: test_ledger_resource_unknown_and_explicit_measurements` | **待实现 / 未验收** |
| F58 | 时间与重叠分别计量 | `sic_core/accounting.py :: normalize_usage; summarize_records; timing_summary; controlled_cost; compare_workflows` | `tests/test_core_task_ledger.py :: test_cross_route_intersection_is_not_a_same_agent_natural_window` | **待实现 / 未验收** |
| F59 | 成本条件与真实账目分开 | `sic_core/accounting.py :: normalize_usage; summarize_records; timing_summary; controlled_cost; compare_workflows` | `tests/test_core_accounting.py :: test_controlled_cost_and_ledger_reconciliation` | **待实现 / 未验收** |
| F60 | 新实现两工作流可执行实验 | `scripts/run_v2_experiment.py :: run_arm; snapshot; paired task fixture; independent verify_v2_experiment.py` | `research scripts / rubric / pack; no named test in current acceptance record` | **待实现 / 未验收** |
| F61 | 独立质量评分与作者新实验 | `scripts/research_scoring.py :: score_run; verify_v2_experiment.validate_run` | `research scripts / rubric / pack; no named test in current acceptance record` | **待实现 / 未验收** |
| F62 | 实验数据包、来源及隐私可复验 | `benchmarks/core_adapters.py :: run_arm; snapshot; paired task fixture; independent verify_v2_experiment.py` | `research scripts / rubric / pack; no named test in current acceptance record` | **待实现 / 未验收** |
| F63 | 探索设想不冒充已定协议 | `paper/draft/sic_draft_working.md :: exploration section; no mandatory runtime protocol` | `paper/draft/sic_draft_working.md :: exploration section` | **待实现 / 未验收（研究边界）** |
| F66 | 块长与编辑噪音三类独立测量 | `research/compression/pack/rubric.json :: score_run; verify_v2_experiment.validate_run` | `research scripts / rubric / pack; no named test in current acceptance record` | **待实现 / 未验收** |
| F73 | 压缩/编辑提示与引擎调研和新对照包 | `research/compression/pack/control-pack.json :: sources.lock.json; profiles-v0.1.json` | `research scripts / rubric / pack; no named test in current acceptance record` | **待实现 / 未验收** |

