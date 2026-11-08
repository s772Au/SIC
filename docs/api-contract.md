# Rust 接口历史笔记

本文件保留早期接口设计与当时状态，部分能力标记和浏览器页面说明已过时。旧 WebUI 已退役；当前接入请以 [后端说明](backend-release-readiness.md)、[Langflow/LangGraph 说明](../integrations/README.md) 和对应实现为准。

以下记录 **2026-11-03 当前 `src/main.rs` 已提供的路由**，以及后续全流程接入所需扩展。当前 Rust 核心为新的最小实现，不继承已删除 V7/V8 源码的能力或成绩。

## 当前可用

请求与响应均为 JSON。写请求需 `x-sic-admin-token`；令牌只留在当前浏览器页面内存，不写入 `localStorage`。前端从同源路径或用户显式输入的服务地址请求 API。

| 方法 | 路由 | 用途与主要字段 |
|---|---|---|
| GET | `/api/v1/ui/bootstrap` | `{version,data_source:"live-local",capabilities,permissions,sessions:[{id,name,created_at}]}`。能力里的 `single_pass:false`、`worker_proxy:false` 如实显示。 |
| POST | `/api/v1/sessions` | `{name}` → `{id}`。 |
| GET/POST | `/api/v1/sessions/{id}/blocks` | GET → `{blocks:[{id,sha256,bytes,created_at}]}`；POST `{source,units?}` → `{id,sha256}`。`units` 若提供，必须是非空字符串数组且拼接后与 `source` 完全相同。`source` 成功写入后不可变。 |
| GET | `/api/v1/blocks/{id}/source` | `{source,sha256,immutable:true}`。前端拒绝将 `immutable` 不是 `true` 的响应当作只读 WB 展示。 |
| GET | `/api/v1/blocks/{id}/units` | `{block_id,units:[{id,ordinal,source,sha256}]}`。未显式切分的旧 WB 返回一个 `wb` 单元；新切分单元 ID 依次为 `u000001` 等。 |
| POST | `/api/v1/drafts` | `{block_id}` → `{id,block_id,revision:0,edits:[]}`。 |
| GET | `/api/v1/blocks/{id}/drafts` | `{drafts:[{id,block_id,revision,status,created_at,updated_at}]}`，最近更新优先，含已提交草稿。块不存在为 404。 |
| GET/PUT | `/api/v1/drafts/{id}` | GET → `{id,block_id,revision,edits,status}`；PUT `{base_revision,edits:[{target_id:"wb",op,risk,reason?,content?}]}`，用修订号检测冲突。 |
| POST | `/api/v1/drafts/{id}/validate` | 返回 `{valid,cb_sha256?,cb_bytes?,error?}`，检验**服务端已保存**的草稿。 |
| POST | `/api/v1/drafts/{id}/single-pass` | 仅在配置 `SIC_COMPRESSER_URL`、`SIC_COMPRESSER_MODEL`、`SIC_COMPRESSER_API_KEY` 后可用。单次 Chat Completions 模型请求，一次 tool call 响应批次；返回 `{draft,model_requests:1,tool_feedback_rounds:0,committed:false}`。整批先校验，再以修订号写草稿；不提交/发布。前端只在用户明确点击并确认后调用，从不自动触发。 |
| GET | `/api/v1/drafts/{id}/single-pass/preflight` | 只读，不请求模型；用与实际单轮编辑共用的请求组装器返回 `request_sha256`、`request_bytes`、`local_request_byte_limit`、`within_local_byte_limit`、WB/CB/挂载字节数与根/单元数。`token_admission` 和 `model_context_limit` 当前均为 `unknown`。未配置模型时使用 `unconfigured-preview` 占位，不能拿此摘要当作未来实际请求摘要。 |
| POST | `/api/v1/drafts/{id}/commit` | 返回 `{id,draft_id,status:"committed",sha256}`。草稿变为只读，CB 独立存在。 |
| POST | `/api/v1/commits/{id}/publish` | 返回 `{id,status:"published",worker_adopted:false,gateway_delivered:false,physical_adoption:"unknown"}`。当前复用 Admin 令牌；未来应定义独立 Publish 授权。 |
| GET | `/api/v1/sessions/{id}/timeline` | `{commits:[{id,status,sha256,created_at,published_at}]}`；CB 正文可另用 `/api/v1/commits/{id}` 读取。 |
| GET | `/api/v1/commits/{id}` | 返回不可变 CB 的 `content`、来源 `draft_id`/`block_id`、SHA-256 和提交/发布时间。 |
| GET | `/api/v1/drafts/{id}/jobs` | 返回单轮编辑作业列表，含 `prepared`/`applied`/`failed`、请求/响应摘要和观测到的上游 HTTP 状态；上游拒绝响应也会保存摘要。明确的上下文容量错误码记录为 `compresser context capacity rejected`，不自动重试；`prepared` 不能当作已送达。 |
| GET | `/api/v1/edit-jobs/{id}` | 读取单个编辑作业证据；不返回密钥或原始上游正文。 |
| GET/PUT | `/api/v1/sessions/{id}/mount` | GET 返回挂载草稿和已发布版本；PUT `{base_revision,content}` 用修订号保存草稿，需 Admin 令牌。 |
| POST | `/api/v1/sessions/{id}/mount/publish` | 独立发布挂载草稿，需 Admin 令牌；不声称 Worker 采用。 |
| GET | `/api/v1/sessions/{id}/mount/revisions` | 返回所有挂载修订及各自 `published_at`，最近修订优先。 |
| GET | `/api/v1/sessions/{id}/projection` | `status:"local-preview"`，`published_cbs` 为当前有序 CB 根（可含高阶节点），再接一份 `dynamic_mount` 和 `pending_wbs`。仅本地预览，未出站。 |
| GET | `/api/v1/sessions/{id}/roots` | 当前稳定 CB 根的有序列表，叶为已发布 CB，高阶为手动升阶节点。 |
| GET | `/api/v1/sessions/{id}/promotion-candidates?limit=20` | 只读列出 2–4 个相邻、同阶、未跨待发布 WB、尚可升阶的根组；偏好 4 个子根，容量尚不明时也列出 2 个子根候选。返回 `child_ids`、层数、来源字节数和 `candidate_count`；不请求模型、不自动升阶，候选在提交前须再次校验。 |
| POST | `/api/v1/sessions/{id}/promotions` | `{child_ids:[...],content,risk,reason}`，2–4 个同级、相邻、按顺序的根，最高升到第 3 阶；需 Admin 令牌。新根立即进入本地投影，仍无 Worker 出站。 |
| POST | `/api/v1/sessions/{id}/promotions/preview` | 同一输入，走与提交共用的结构校验，返回来源/摘要摘要值、升阶前后根数和 CB 上下文 UTF-8 字节变化；`written:false`、`upstream_requested:false`。提交前可反复编辑，实际提交时重新校验；字节变化不等于模型 token 节省。 |
| GET | `/api/v1/nodes/{id}` | 读取高阶节点正文、完整源、children、risk/reason 与顺序范围。 |
| GET | `/api/v1/sessions/{id}/archive/search?q=&limit=` | 会话内字面检索不可变 WB、已发布 CB 和高阶节点，返回最多 50 条摘要；当前为 SQLite 扫描。 |
| GET | `/api/v1/sessions/{id}/archive/{kind}/{entry_id}?offset=&max_bytes=` | `kind` 为 `wb`/`cb`/`node`，按 UTF-8 字节分页，返回实际调整后的边界、`next_offset` 与 SHA-256。 |
| POST | `/v1/chat/completions`、`/v1/responses`、`/v1/messages` | 仅在配置 `SIC_WORKER_UPSTREAM_BASE_URL` 时流式转发。Chat Completions 与无服务端历史引用的 Responses 只有同时带 `x-sic-session-id` 与 `x-sic-history-mode: tail-only` 才组入 SIC 投影；普通请求原样转发。Messages 暂不支持投影。 |
| GET | `/api/v1/relay-events/{id}` | 读取请求摘要、协议、观测到的上游 HTTP 状态及可选 `local_projection`；`cb_adopted:false`、`physical_adoption:"unknown"`。本地组装不等于物理采用。 |
| POST | `/api/v1/sessions/{id}/worker/assemble-chat` | 输入 `{request:{model,messages:[...]}}`；返回仅本地组装的 Chat Completions 请求、根/挂载/WB 来源和摘要。前导 system/developer 原文保留；后段出现新的 system/developer 则拒绝。不会请求模型。 |
| POST | `/api/v1/sessions/{id}/worker/assemble-responses` | 输入 `{request:{model,input:[...]}}`；返回仅本地组装的 Responses 请求与来源。保留顶层 `instructions` 和尾部全部输入项；拒绝 `previous_response_id`、`conversation`、`prompt` 及晚到受保护项。 |

## 测试计划接口

Rust 规划接口不运行测试，也不请求网关。当前本地 8341 维护中，当前不请求；正式实验等专用网关。`bootstrap.capabilities` 中 `test_catalog`、`test_plans` 为 `true`；`runner`、`scorer`、`gateway_telemetry`、`version_evidence` 为 `false`。目录顶层状态为 `prepared-no-experiment`，估算并非测量值。

| 方法 | 路由 | 用途 |
|---|---|---|
| GET | `/api/v1/test-catalog` | `{catalogVersion,status,estimatesAreMeasured,tokenGate,datasets:[...]}`，只读目录。数据集提供 `tier`、`status`、`historyCount`、`questionCount`、`questionsPerHistoryMax`、资源和请求估算。前端不把目录条目直接算作已运行任务。 |
| GET | `/api/v1/test-plans/default` | `{id,revision,plan,updatedAt}`。未保存时 `revision=0, plan=null`；前端保留浏览器草稿。 |
| PUT | `/api/v1/test-plans/default` | `{baseRevision,plan}`，需 Admin 令牌；服务端返回新 revision，修订冲突拒绝覆盖。 |
| POST | `/api/v1/test-plans/default/validate` | `{plan}` → `{valid,errors,resolved,estimates}`；可验证未保存草稿。后端拒绝未知数据集、资源层级不匹配、histories/questions 越界、非 HTTP(S) 网关 URL、URL 内嵌凭据/查询/片段和不合法调度参数。前端将服务端估算标为估算，不当作实测网关资源。 |

计划 JSON 包含 `version`、`low`、`high`、`scheduler`、`scoring`。默认 low ID 是 `beam-10m`、`corpusqa-4m`、`corpusqa-10m`、`babilong-10m-sample`；high ID 是 `longmemeval-v2-small`、`longmemeval-v2-medium`、`babilong-10m-full`。每项 `histories` 和 `questions` 默认 1；连接目录后，前端使用 `historyCount`、`questionCount` 和 `questionsPerHistoryMax` 限定编辑上限。CorpusQA 的 329 问可共用 4 份文档历史；不能把问题数直接当作历史份数。用户自定义 ID 保留在本地草稿，但后端会拒绝未绑定目录的数据集。

低资源集用固定 seed、包数、每包用例数、标称内存/token 预算生成稳定组合，优先覆盖不同类别；高资源任务单列。CorpusQA 多问可共享文档历史，前端分别编辑 histories/questions。每个组合包可按目录 `requestEstimate` 显示 SIC/native 两臂的压缩、答题和评分调用估算。预览包之间可能重复同一数据集，不能跨包累加为独立历史或真实总请求。

`scheduler.gatewayUrl` 接受有效 HTTP(S) URL，可含路径，但不可含 username、password、query 或 fragment；与 Rust 校验一致。默认 `http://127.0.0.1:8341` 仅为当前占位；它维护中，当前不请求；正式实验等专用网关分配后再替换地址。前端默认 256 在途和 1000 RPS 是未来探索上限，**不是测得网关容量**；还记录突发容量、爬坡、冷却、429/5xx 停止阈值、本机内存占用与 p95 延迟保护。未来运行器需受实测内存、429 和 p95 反馈约束。当前前端组合是规划预览，真正“压满”需要后端实测吞吐、延迟、在途请求和网关利用率证明。

Va.b.c：`a` 是机制代际，`b` 是同思路的优化设计，`c` 是工程修订。机制、设计、工程证据分别记录；正式成绩及质量证据独立存放，不作为增加 `b` 的前置条件。计划中的证据引用是待核验文本；填写引用不等于已经发布版本或确认成绩，也不继承旧二进制身份。

当前草稿核心接受显式 `target_id:"wb"` 整块编辑，或仅编辑已切分 WB 的一个或多个 `u000001` 等源单元；同一批不能混用。未提及单元按原字节 KEEP。Op 为 `KEEP`、`FLAG`、`COMPRESS`、`REPLACE`、`DELETE`，risk 为小写 `low`、`medium`、`high`。`COMPRESS`/`REPLACE` 要求非空 `content`；`KEEP`、`DELETE`、`FLAG` 不带 `content`；`FLAG` 必须带 reason。零编辑与空 CB 会被拒绝。单轮模型只收到实际单元 ID 的工具枚举。前端示例中的 `REORDER`、`ADD` 仍是**设计演示**。

## 状态与证据

| 状态 | 充分证据 | 不能由此推断 |
|---|---|---|
| 草稿保存 | PUT 成功并读回新 revision | 已提交 CB |
| CB 提交 | commit 成功和 timeline 中 `committed` | 已发布或 Worker 采用 |
| 发布 | publish 成功和 timeline 中 `published` | Worker 请求实际含 CB |
| Worker 请求采用 | 未来出站请求审计显示目标 CB | 网关后物理模型接收 |
| 网关送达 | 未来网关回执/抓包 | 物理模型有效利用 |
| 物理接收 | 有效上游见证 | 保留了每条事实 |

当前 `publish` 是新 Rust 核心的状态更新，不执行 Worker 投影或出站调用。前端实验状态始终显示“暂停”。

## 后续扩展建议（当前未实现）

要把示例中的全部环节变成真实数据，推荐增加下列资源。所有写接口仍需服务端 Authorizer，不能只靠前端按钮可见性。

| 资源 | 建议接口 | 要求 |
|---|---|---|
| 配置与教学 | `GET/PUT /api/v1/config` | 有版本号/If-Match；区分 Worker 和多轮编辑教学 profile。V9 单轮教学不加入“原始上下文属于 WB 而非模型自己上下文”的补充。 |
| 封块策略 | `GET/PUT /api/v1/segmentation-rules` | 记录受保护前言、正式边界、工具批次闭合、目标/浮动区间；原生 system/developer 原文只读。 |
| V9 单轮驱动的作业审计 | `GET /api/v1/edit-jobs/{id}` 等 | 当前已有一次请求的基本路由；后续仍需作业 ID、调用见证、取消/恢复、容量反馈、完整 SSE/原生响应证据。未知/冲突/截断/断流不发布半块。具体契约由主线 V9 设计决定。 |
| 动态挂载历史 | 现有 `/api/v1/sessions/{id}/mount/revisions` 后续扩展 | 当前保留旧修订与发布时间；仍需挂载结构化字段及权限细化。 |
| 投影审计 | 现有 `/api/v1/sessions/{id}/projection` 后续扩展 | 当前为本地材料预览；仍需 CB 选阶、请求摘要和真实出站见证。 |
| SIC 代理/送达证据 | 现有 Chat/Responses tail-only 投影与 `/api/v1/relay-events/{id}` 后续扩展 | 两协议可显式组入 CB；仍需 Codex CLI 自动会话绑定、Messages 投影、内部召回、网关送达与物理模型接收分层证据。 |
| Worker 内部归档工具 | 现有 archive search/read API 后续接入 | 本地只读 API 已有；仍需 Worker 工具续推、权限分层与 glob。DELETE 仅移出工作视图，拒绝不可逆擦除。 |
| N 阶自动调度与 E12 | 现有 `/api/v1/nodes/{id}` 后续扩展、`GET /api/v1/e12/events` | 手动相邻升阶与 children/source 追溯已实现；容量触发自动升阶、因果 cut、租约、预算、peer 尚未实现。 |
| 权限 | `GET /api/v1/principal`、独立 `Publish` 动作 | 现存旧 Admin 动作只有 Read/Search/List/WriteDraft/Commit；Publish 必须明确新增，模型不能提权。 |

建议将 `bootstrap` 保持简洁，附上明确的 capability flags；前端按能力显示真实页面或“待接入”。不要把示例结构填入 live 响应。
