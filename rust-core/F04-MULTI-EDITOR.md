# F04 多 Editor：现状、约束与实现切片

本文件是**完整版**路线的工作计划，不受精简版“单 Editor”取舍约束。精简版仍以
LEAN-ACCEPTANCE.md 为准，本文件不改变它。

## 1. 现状：合并层已经存在，Core 仍按单 Editor 收口

src/editing.rs 已按多 Editor 设计：

- assignments(value, units, allow_overlapping)：校验 {editor, unit_ids}，要求 editor
  唯一、unit_id 在冻结 WB 内、默认禁止重叠。
- validate_response(scope, response, primary_decision)：每个响应只能操作自己的
  scope；ADD 只能用自己 scope 内的锚；P/V/I 与决策树校验独立于 scope。
- materialize_inner：要求**每个已分配 editor 都有响应**，逐 editor 校验后按
  assignment 顺序合并；未分配单元保持 KEEP。

Core 的三处单 Editor 收口：

1. core.rs::prepare 的 assignments.len() != 1 直接拒绝（“lean profile supports
   exactly one Editor”），并且只为 assignments[0] 解析 model_profile。
2. core.rs::submit 只接受 assignments[0] 的 editor，且用单响应**整体替换**
   job.responses；job.candidate 只保存一份。
3. job_material / combined_material / publish / update_candidate 都取
   assignments[0] + job.candidate；edit.run 也只有一个 claim_id 与一份
   model_profile。

因此 F04 不是从零实现合并，而是把 Core 的编排从单 Editor 改成 N Editor 的
“分别提交、全部到齐后一次合并发布”。

## 2. 契约必须保持的不变量（F04）

- 同一 WB、同一背景版本（frozen source / base_cut / decision revision），两个
  Editor 的 unit_ids 互不重叠；未分配单元 KEEP。
- 两次真实模型调用必须有可核对的并发区间；顺序调用不算并行。
- 任一响应只能操作自己的 scope；跨 scope、重复 editor、未知 unit 一律拒绝。
- 响应不全、冲突、候选过期时**不半发布**；合并与采用时宿主重新校验分工范围与
  当前授权。
- job、分工、时间不得进入 Worker 与 Editor 的公共前缀；各自的可信 I 携带本 job
  与 assigned units。
- 合并只产生一份 CB；CB/op/回放仍按现有固定与回放规则处理。
- 两个 Editor 仍是同一 Agent，不产生 peer/新链。

## 3. 实现切片

### 切片 1：准备与校验（已完成）

- editing.rs 暴露 pub fn validate_editor_response(source, assignments, editor, response)，
  复用现有 scope 校验；materialize 保持“全部到齐”语义。
- prepare 接受 1..=N 个 assignment：editor 唯一、unit 存在、默认不重叠、至少一个；
  N>1 时不再返回 “lean profile supports exactly one Editor”。
- Job 增加 editor_profiles（按 level/editor 解析）与 pending_editors（由 assignments
  推导）；单 assignment 的现有结果字段保持不变。
- 测试：两组不重叠 assignment 被接受；重叠/重复/未知单元被拒绝；单 editor 行为与
  现有测试逐字节一致。

### 切片 2：分别提交与合并候选（已完成，host 提交路径）

- submit({job_id, editor, ops, pvi})：校验 editor 在 assignment 中，校验该响应
  scope 后写入 job.responses[editor]；pending_editors 移除该 editor。
- pending_editors 非空时 job 仍为 open，拒绝发布；为空时用全部 responses
  materialize 得到唯一候选，状态转 ready。
- edit.candidate.get 返回 per-editor 响应与合并候选；edit.candidate.update
  先只允许单 editor job（多 editor 的修订语义单列，见 §4）。
- 测试：两响应到齐才 ready；缺一不发布；重复提交同 editor 拒绝；跨 scope 响应拒绝；
  合并正文/op 与 editing::materialize 直接调用一致。

### 切片 3：并发派发与账务（已完成 mock 并发；真实 Provider 未验收）

- Job 增加 per-editor claim（editor_claims: editor -> claim/attempt 引用）；
  edit.run 增加必填 editor，只派发该 editor 的冻结 profile。
- 移除 “the single Editor is already running” 对多 assignment job 的全局限制，改为
  同一 editor 不可重复在途；两个 editor 可同时在途。
- 每次模型调用的 usage 继续按角色/模型/路由记账；per-editor 尝试独立。
- 可信 I 只含本 editor 的 job/assigned units；公共前缀（Worker/Editor 共同教学与
  CB 背景）不得因分工变化。
- 测试：barrier 让两次 mock 调用真正重叠（记录起止时刻）；公共前缀 digest 相同；
  一个 editor 失败/放弃不影响另一个的在途状态；放弃后不采用旧结果。

### 切片 4：合并发布与宿主复核（host 提交路径已完成；多 Editor 修订语义待切片 3 之后）

- publish 使用全部 responses 的合并候选；CB 记录 editors 元数据
  （editor、scope、candidate revision、attempt 引用），正文仍只来自合并 material。
- 发布前重校验：assignments 未变、scope 与当前授权一致、candidate revision 匹配、
  pending_editors 为空；否则 409，不写半成品。
- 回放/重放走现有 cut/op 机制；未分配单元逐字 KEEP。
- 测试：合并发布后正文与 op 顺序可回放；晚响应/过期候选被拒；跨分工提交被拒；
  撤权后合并发布被拒。

## 4. 待定与风险

- **多 Editor 的候选修订语义**：是 per-editor revision（推荐，保留各响应修订史）还是
  合并后可整体修订？推荐前者；edit.candidate.update 增加 editor，单 editor 兼容。
- **编辑预算**：一次 edit.run 只跑一个 editor；N 个 editor 意味着 N 次调用与独立
  预算/重试，需在宿主文档里写清，不能用“单次编辑预算”覆盖。
- **并发上限**：需要显式上限（建议 2–4）与超时/放弃语义，不能无界并发。
- **公共前缀**：assignment/job 必须只在可信 I；要用 prefix digest 测试证明不进入
  公共段。
- **F14/F71**：Editor 私有递归是另一条工作线，本计划不包含；多 Editor 与递归可以
  独立落地。

## 5. 当前落地状态（2026-11-12 00:05）

- 已完成：`editing::validate_assignments` / `validate_editor_response`；`edit.prepare` 接受
  1..=N 个不重叠 assignment 并为每个 Editor 冻结 profile；`edit.submit` 逐 Editor 累积、
  全部到齐才 ready；`publish`/`job_material`/`combined_material` 使用全部响应合并，CB
  记录 `editors` 元数据；`tests/multi_editor.rs` 覆盖合并、KEEP、跨 scope、重复提交、
  不完整发布与显式拒绝多 Editor 模型派发。
- 已完成（2026-11-12）：per-editor claim，`edit.run` 的 `editor` 参数（多 Editor 必填）、
  按 editor 过滤可编辑单元、job/editor/分工只进可信 I、两个 Editor 可同时调用 Provider，
  全部到齐才 ready；`runtime.abandon` 支持多 Editor 显式取消；`tests/multi_editor_concurrency.rs`
  用 barrier 证明并发、公共前缀 digest 相同、各自 I 带 editor/分工，并核对合并与 KEEP。
- 已完成（2026-11-12 第二轮）：`edit.candidate.update` 支持按 editor 修订（CAS、其余响应
  不变、跨 scope 拒绝、重放记录带 editor）；每 Agent 同时在途 Editor 上限 4，超出返回
  409 `editor_concurrency_limit`。
- 已完成（第三轮）：`tests/editor_concurrency_abandon.rs` 用阻塞 mock 证明第 5 个 Editor 在
  Provider 前被拒、放行后 4 个响应落盘且第 5 个随后可跑；同文件证明 abandon 一个 Editor
  后 job 终止、晚响应不采纳、私有 role view 与 owner 保持不变。
- 未完成：真实 Provider/DSH 并发与质量/成本证据、取消后的宿主重试流程、多 job 并发。
  不能当 F04 通过。

## 6. 完成判定（按契约“完成维度”）

- preparation：N assignment 的 schema、校验、冻结与文档。
- mechanical_acceptance：上述隔离测试全部通过（mock Provider 即可，不冒充真实质量）。
- research_evaluation：真实模型下的并行区间、合并质量与成本另记，未做则明确
  not_evaluated。
