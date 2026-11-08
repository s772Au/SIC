# v0.1.1 Chat 桥回放兼容补丁

核心 Rust 仍为 `9.0.0-alpha.3`，本补丁只更新 Python Chat 桥、验证与打包。原 v0.1.0 的严格字典比较会把客户端正常重建的 assistant 当成历史分叉；已有 tag 不覆盖，使用新的 v0.1.1 包。

## 实际发现

只读检查本机 DSH `packages/llm/llm-pi-ai/src/adapter.ts`、`stream.ts`、`replay.ts` 以及实际安装的 `@earendil-works/pi-ai` **1.1.0** 后，确认：

- DSH `stream.ts:204` 将 pi-ai 已解析的 tool arguments 再次 `JSON.stringify`；`replay.ts:44–51`、`:163`、`:211` 会再次 parse 回对象。
- pi-ai `dist/api/openai-completions.js:960–962` 重建 assistant 为 `content:null` 或兼容分支要求的空字符串；`:1038` 再次 stringify tool arguments，且不重建 `refusal:null` / `annotations:[]`。
- 同一文件 `:1045–1049` 会为部分 reasoning 模型添加空 `reasoning_content`。非空 reasoning 的丢失不能当成无害格式差异。

因此，上游 `{ "b": 2, "a": 1.0 }` 在正常回放中会变为 `{"b":2,"a":1}`；空 metadata 也会消失。并非每次纯文本响应必然触发，但这些常见形态一旦出现，旧桥的第二轮 prefix 检查就会失败。

本轮没有修改 DSH，也没有控制桌面。实际执行的是安装包内的 `stream()` 和 `convertMessages()`，其 fetch 被替换为内存 SSE；外部网络与模型请求均为 **0**。完整 DSH 宿主仍未端到端验收。

## 窄白名单

只有**上一次由本桥生成的最后一条 assistant**可以按以下规则比较：

- `refusal:null` 与缺失、`annotations:[]` 与缺失、空或 null `reasoning_content` 与缺失等价；没有调用时，`tool_calls:null` / `tool_calls:[]` 与缺失等价。
- 有工具调用的 assistant，空/null/缺失 content 等价；普通文本 assistant 不放宽 null 与空字符串。
- 工具 ID、名称、类型、顺序保持严格。参数必须是合法、无重复键的 JSON **对象**；对象键顺序、空白、Unicode escape 和精确等值数字写法可等价。数组顺序、字符串内容及 JSON 类型不放宽。
- 数字用精确十进制解析和不舍入的结构表示比较，拒绝 `9007199254740993 → 9007199254740992`、重复键、NaN/Infinity、对象变数组、true 变 1；保守保留负零符号。它不是 Python float 或 JavaScript number 的近似比较。

所有已经接受的旧 prefix、保护指令、user、tool 消息继续严格比较。新一轮接受后，以客户端**实际发来的原对象**作为下次 prefix；比较过程只操作副本。WB 源文和实际 Worker 转发请求保持客户端收到/发出的内容，不把它们重写为某种 canonical assistant。请求完全相同时仍返回已有缓存。

非空 refusal/annotations/reasoning、未知字段丢失、正文空白变化、工具 ID/名称变化、真实参数改变仍会拒绝。pi-ai 对长 tool ID 的截短、非空 reasoning 的丢失、provider-specific 内容变换仍属显式不兼容范围；本补丁不做模糊 ID 匹配或忽略实质信息。现有缓冲 SSE 和内存会话/失败停用边界保持不变。

## 可重放证据与测试

`tests/fixtures/pi-ai-1.1.0-replay.json` 保存 8 个实际安装包转换结果：5 个无害转换接受；非空 reasoning 丢失、非空 annotation 丢失、整数精度损失拒绝。fixture 同时保存 pi-ai JS 文件摘要与三份 DSH 源码摘要；`dsh_host_executed=false`。

无网络重新生成方法：

```powershell
node scripts/audit_pi_bridge_roundtrip.mjs PATH_TO_INSTALLED_PI_AI tests/fixtures/pi-ai-1.1.0-replay.json PATH_TO_DSH_SOURCE
```

`PATH_TO_INSTALLED_PI_AI` 指包含 package.json 和 dist 的实际包目录，不会写入该目录。第三个参数只读记录 DSH 源码摘要，可省略。测试 fixture 不要求 CI 安装 DSH 或 pi-ai。

本轮检查通过：

- Python 单元测试 **19 项全部通过**（安装 benchmarks/requirements.txt 的环境）；未装格式依赖时仍有两项可选数据格式测试跳过。
- 原后端 HTTP 烟测通过。
- DSH Chat 桥 HTTP 烟测通过，并实际使用上述 pi-ai 工具回放 fixture 完成下一轮、自动 WB/edit/commit/publish 与 Worker 投影；还验证了归档保留客户端原表示、缓存重试、受保护前缀、完整工具组和真正分叉的 409。

兼容补丁阶段 Rust 源码未改动、未重建；最终软件候选因嵌入 catalogVersion=2026-11-07.2 数据目录元数据另行重建，最新二进制身份见 backend-release-readiness.md。不将这些兼容性测试写成长任务收益、真实 provider 成绩或实际 DSH 宿主验收。
