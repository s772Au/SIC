# F14/F71：Editor 私有上下文与递归表示

完整版路线的工作计划，不受精简版“单 Editor / 只共享 background”取舍限制。

## 1. 契约要求

- **F14**：长 Editor 背景加新 WB、有限预算 → 为 Editor 上下文形成并使用合法高阶表示，再处理新 WB；Editor 实际采用高阶背景而不是截断；可信 I 标明该阶 CB 只读、新 WB 可编辑范围与七类操作输出；与 Worker 背景不同时明确不共享。
- **F71**：Editor 独有持续材料与共享 CB 背景 → 同一双链机制整理独有材料，形成/采用更高阶 CB；有内部上下文归属且可追溯；不强制混入 Worker 原 WB；共享与独有可共同使用；不产生新 Agent；私有 CB 选择不同时，相应请求不套跨 scope 共享结论。

## 2. 现有机制与缺口

- Agent.contexts 已支持内部上下文与多层 Level（pending/wbs/cbs/instant）；hierarchy.context.create / hierarchy.context.ingest / hierarchy.form / hierarchy.expand / hierarchy.select / hierarchy.timeline 已存在；ingest 已拒绝 protected 指令与未闭合 native calls；shared_contexts DAG 仍返回 501。
- role_views 目前只有 worker 与 editor 两个共享视图；edit.prepare 依据 source 所在 context 建 job；run_editor 用 job.context_id/job.level 的 instant 作为 Editor 背景，没有按 Editor 名区分的私有背景，也没有预算驱动的自动升阶。
- 因此 F14/F71 的缺口是：Editor 私有上下文身份、私有材料双链、私有背景进入 Editor 请求的 scope 标注、预算驱动的选择与测试证据；合并层与多 Editor 编排已具备，不需要另造机制。

## 3. 实现切片

### 切片 1：私有上下文身份与权限（已完成）

- 命名约定 editor:<editor_id>；创建时记录 owner 角色与 editor，Worker 视图不得选择该 context；hierarchy.context.create 增加可选 role_scope/owner（未知值拒绝，默认内部上下文）。
- run_editor 解析该 Editor 的私有 context；没有私有材料时回退当前共享背景，行为与现状一致。

### 切片 2：私有材料与双链

- host 通过 hierarchy.context.ingest 把 Editor 独有材料写入私有 context；按同一 WB/CB 规则注册成员与跨阶引用。
- 私有材料不进入 Worker 的 selected view，也不改写 main context；私有 context 的 ingest 继续拒绝 protected/open native calls。
- 提供显式 hierarchy.form（或等价）在私有 context 升阶；后续切片再加预算触发。

### 切片 3：Editor 请求组合与 scope（level 1 只读背景已完成；高阶与预算触发未完成）

- 共享前缀（共同教学、共享 CB 视图、工具表）保持原样；私有高阶背景只进入可信 I 的只读背景区，并标注 scope=private、read_only_cb_ids、当前私有 WB 的可编辑单元。
- 断言：同一 Editor 的私有背景变化不改变共享前缀 digest；两个 Editor 的共享前缀相同；Worker 请求不含私有材料。

### 切片 4：预算驱动与选择（已形成层级的回退/拒绝已完成；自动形成未完成）

- 以 profile.max_input_tokens 与保留比例计算预算；超限才升阶；选择规则显式、可重放；预算不足时明确拒绝或降阶并记录，不截断。

### 切片 5：测试与证据

- 创建私有 context，连续 ingest 材料，形成 2/3 阶 WB/CB；run_editor 请求包含私有只读背景与 scope 标注；Worker 请求不含；共享前缀 digest 不变。
- 私有选择 level 2/3 时请求标明 scope，不用共享结论；私有 ingest 拒绝 protected/open calls；预算不足的负例；多 Editor 各自私有背景互不混淆。

## 4. 关键负例

- 只给 Worker 升阶；Editor 私有材料只截断；私有背景混入共享前缀或 Worker 视图；用多 Editor 冒充私有递归；把私有 CB 当可编辑；不同 scope 仍宣称共享。

## 5. 当前落地状态（2026-11-12）

- 预算：`edit.run` 先检查无私有背景的基础请求是否能放进 `max_input_tokens`/`context_window`；能放下时，从所选私有层级起按层级升序尝试已形成的表示，选用最低且能放下的层级并标 `fallback`，全部放不下返回 413 `editor_context_over_budget`，不截断、不调用 Provider。自动“形成”高阶表示仍由宿主通过 `hierarchy.form` + 发布驱动，Core 只负责预算内选择与拒绝。

- 已完成：`hierarchy.context.create` 的 `editor` 归属与 `editor:<id>` 命名；共享 role view 拒绝私有上下文；`hierarchy.select` 只接受 owner 的 `editor:<id>`；私有材料 ingest、双层 WB/CB 形成与发布；`edit.run` 将所选私有 CB 作为 `editor_private_background`（scope=private、read_only）放入可信 I，共享前缀 digest 与兄弟 Editor 相同，Worker/其他 Editor 不可见。
- 测试：`tests/editor_private_context.rs`（归属/命名/选择拒绝/两级私有 CB），`tests/multi_editor_concurrency.rs`（e1 私有背景只出现在 e1 请求、e2 不可见、共享前缀 digest 不变）。
- 未完成：预算驱动/自动升阶、私有二阶以上选择的完整矩阵、私有与共享 background 的组合、真实模型证据。
- 取消：`tests/editor_concurrency_abandon.rs` 证明 abandon 一个 Editor 后整单终止、晚响应不采纳、私有 role view/owner 保持；宿主重试流程仍待验收。

## 6. 必需证据

- 私有 context/level/WB/CB 目录与来源；真实 Editor 请求与共享前缀 digest；Worker 请求不含私有材料；预算与选择记录；取消/失败后的 scope 状态。
