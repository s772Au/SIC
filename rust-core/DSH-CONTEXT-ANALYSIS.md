# dsh-context 对 SIC 的参考与适配边界

本记录依据 `bowenliang123/dsh-context` 的 [0.83.1 源码快照](https://github.com/bowenliang123/dsh-context/tree/a37a72cb22481cfd101fb00e390e291cb03ded8b)，不是对其他版本的兼容性保证。该项目采用 Apache-2.0；本次 SIC 改动独立实现，没有复制其源码。

## 值得借鉴的设计

| dsh-context 的做法 | 对 SIC 的意义 |
|---|---|
| 在 DSH 宿主注册会话投影，并从持久事件推导时间线、统计和界面数据；见其 [宿主入口](https://github.com/bowenliang123/dsh-context/blob/a37a72cb22481cfd101fb00e390e291cb03ded8b/src/host/index.ts) 和 [时间线](https://github.com/bowenliang123/dsh-context/blob/a37a72cb22481cfd101fb00e390e291cb03ded8b/src/host/timeline.ts)。 | SIC 应在 DSH 会话视图之外，呈现真实的 WB、CB、编辑任务、分支与模型调用之间的引用，而不是仅展示 Harness 侧消息。 |
| [facts 接口](https://github.com/bowenliang123/dsh-context/blob/a37a72cb22481cfd101fb00e390e291cb03ded8b/src/host/facts.ts) 按需请求部分数据并带修订信息；缺失或未对齐的部分明确报告，前端无需每次加载完整历史。 | 对长会话使用有界摘要、按需分页和一致性标识；详情与摘要来自不同版本时，不把它们拼成看似完整的同一时刻。 |
| [工具事件处理](https://github.com/bowenliang123/dsh-context/blob/a37a72cb22481cfd101fb00e390e291cb03ded8b/src/host/fold.ts) 使用 DSH `tool/result` 消息的 `isError` 和调用标识；后台任务还有单独的生命周期事件。 | SIC 宿主插件已按会话、调用 ID 和事件序号记录原生结果；只有文本而没有原生事件时仍为 `delivered_unattested`。DSH 返回正常只是该工具调用未报告错误，不能证明后台任务或外部副作用最终成功。 |
| [会话事件](https://github.com/bowenliang123/dsh-context/blob/a37a72cb22481cfd101fb00e390e291cb03ded8b/src/host/fold.ts) 中的 `assistant/message` 可携带用量，统计还需区分模型、缓存类别和计价依据。 | SIC 一次 DSH 请求可能产生多个 Editor/Worker 调用，且二者可用不同模型。当前 bridge 没有将这些调用用量汇总回 DSH；不能把多模型用量直接作为一个 Worker 请求费用，或把缺失用量当成零。 |
| 通过 `systemPrompt` 的有序 section 配置前后缀，见 [promptInjection.ts](https://github.com/bowenliang123/dsh-context/blob/a37a72cb22481cfd101fb00e390e291cb03ded8b/src/host/promptInjection.ts)；项目也维护 [DSH 版本兼容证据](https://github.com/bowenliang123/dsh-context/blob/a37a72cb22481cfd101fb00e390e291cb03ded8b/docs/compatibility.md)。 | SIC 要明确区分需稳定的共同前缀和每次更新的 I 区；宿主提示或工具表改变可能破坏共同前缀，必须检测并显式处理。不能仅凭版本范围声称其他 DSH 版本已验收。 |

DSH 原生会话投影主要反映 DSH 看到的消息和事件。当前 SIC 通过模型桥接层进行编辑，DSH 会话历史并不自动等于 Provider 实际收到的 SIC 工作视图；并排展示时必须明确标明各自来源。这里的共同前缀是逻辑设计，不证明 Provider 物理缓存命中。

## 本次实现：DSH 会话的 SIC 状态摘要

Rust Core 的 `POST /v1/sic/status` 保留已有的活跃状态字段，并新增：

- `last_turn`：该 DSH 会话最近一条桥接请求的状态、阶段、创建时间，以及已记录的来源、WB、Editor 任务、发布 CB、cut 和最新 Worker attempt 引用。
- `last_publication`：该 DSH 会话最后一次有记录的 WB/CB 发布，与工具续接这样的最近请求分开标注；它不代表其他宿主对同一 Agent 的后续发布。
- `worker_rounds_recorded`、`tool_completions_recorded` 和 `issued_host_tasks`：只描述对应桥接记录，不代表所有 Provider 调用或外部动作的完整总账。
- `revision`：对本次摘要计算的内容指纹。下次请求可带 `if_revision`；若摘要不变，响应只返回 `unchanged` 和指纹。

该摘要在同一 SQLite 事务中读取，按已认证宿主 actor 和 DSH session 过滤，不返回消息正文、工具参数、工具结果或密钥。`null` 表示该请求没有记录该字段，不能推断整个 Agent 从未产生对应对象；尤其 DSH 工具结果续接是另一条桥接请求，不一定重建 WB。新增的 CB 引用只在更新后的发布流程中记录，旧请求不会自动补填。CB 发布和桥接阶段记录是两次写入，若恰好在两者之间中断，摘要也可能缺少引用，不能据此判定发布失败。`revision` 不是单调事件序号，也不涵盖 bridge 之外的 Board 或 Agent 状态变化。

```powershell
$headers = @{ Authorization = "Bearer $env:SIC_RUST_BEARER" }
$body = @{ session_id = '<DSH session ID>' } | ConvertTo-Json
$status = Invoke-RestMethod -Method Post -Uri 'http://127.0.0.1:18930/v1/sic/status' -Headers $headers -ContentType 'application/json' -Body $body
$nextBody = @{ session_id = '<DSH session ID>'; if_revision = $status.revision } | ConvertTo-Json
```

真实 DSH 0.2.1-rc.5 headless 加本机模拟模型的验收中，请求方状态中的 CB/WB 对应关系与 Core 的 `hierarchy.expand` 一致；分身调用 DSH 内置 `read` 后，续接状态正确记录工具结果已收取，条件请求与无授权拒绝也通过。之后在隔离目录中，以模拟模型启动请求方、以固定 OpenDesign 模型驱动分身 Editor/Worker，验证了 DSH 内置 `read`、原生事件匹配、Socket 回传和程序 fixture 采用回信。工具读取范围由测试专用拦截器限定，不代表任意工具或生产沙箱已验收。

## 原生事件、模型用量与界面

DSH 插件订阅 `session/event`，只发送已启用 SIC 的会话中 `tool/result` 的会话 ID、调用 ID、序号与状态，不发送参数、输出或错误正文。Rust 只接受已发出的同一 actor/session/call 的任务，记录去重并拒绝冲突。事件先于续接到达时，其工具调用结果进入该次 `tool.complete`；迟到时保留原始续接状态，并将后到观察写入后续 Editor 和 Worker 的 I 区，不改写旧 CB。后到观察在 I 区有 20 项上限及截断标识；已持久化但尚未交给 Core 的观察会在同一会话下次模型请求前重试交付，不重放外部工具。`TOOL_OUTCOME_UNKNOWN` 与 `TOOL_NOT_STARTED` 分开处理；即使工具返回正常，也不推断后台任务已经完成。宿主插件进程内待发送队列有界；恢复使用 DSH `sessionQuery.readEvent` 异步分窗，最多尝试最近 512 个事件，不再调用已弃用的同步历史读取。隔离测试中，真实 DSH 首次进程退出后人为移除测试库中的一条桥接事件，再恢复同一会话，原调用 ID、序号和状态经查询补回并与 Core 观察核对。查询服务不可用、超时或队列溢出时，未匹配的结果仍为未知；长时间离线与更大规模恢复尚未验收。

`/v1/sic/accounting` 按角色、固定请求模型、Provider 报告的模型和 Provider 路由分组。输入、输出、缓存输入分别保留已知数与缺失次数；缓存输入包含在输入中，不能重复相加。一次固定 OpenDesign 模型分身测试中，3 次 Editor 与 3 次 Worker 的这些值与 Provider 返回值逐次核对一致，请求方模拟调用不计入。旧记录只从原任务或原运行时配置尝试恢复模型身份，不能恢复时标为未知。价格、缺失用量及缓存命中原因均不推测；DSH 自带单模型用量栏可能把未返回用量显示成 0，不代表 SIC 没有消耗。

独立的 DSH 插件会话标签通过 DSH 宿主代理读取 SIC 摘要、工具事件和账务，并可按层级分页查看主上下文 WB/CB 链、展开对应关系与来源、分页读取 WB/CB 和编辑操作。界面可读取四视角决策树，并以版本检查新增进行中、尚未尝试或可能忽略的条目；写入会校验浏览器同源信息，模型调用中不允许修改，结果不确定时不会自动重试。真实 DSH 0.2.1-rc.5 Web、隔离数据库和本机模拟模型中，已实际打开标签、展开 WB/CB、读取 CB 并保存后重新读取一条合成决策。现有 bridge 的 bearer 同时具备宿主写入和模型调用权限，始终留在宿主进程，不能直接暴露给浏览器。当前界面不是完整可编辑工作台：决策条目的修改、删除和完成确认，其他内部上下文、Socket/公示牌操作和 Editor 候选编辑等尚未接入；Langflow 与 Studio 也未因本界面而得到验收。

后续应优先验证：长时间断网及重启后的事件补齐、后台任务最终生命周期、更多 Provider 的用量字段、跨分叉关联以及只在新 CB 或 I 层生效的受权编辑。对于已覆写 CB，界面不能提供原地改写操作。
