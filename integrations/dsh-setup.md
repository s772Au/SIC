# DSH 接入示例

此模板已对照本机 DSH 源码的 `llm-pi-ai` 配置和提供方指南核对：`api: openai-completions`、`headers`、`retryPolicy` 均为实际配置字段。HTTP 闭环已通过模拟上游；未修改用户现有 profile，也未宣称完成真实 DSH 桌面/真实 provider 验收。

1. 先按 `docs/backend-release-readiness.md` 启动 Rust 与完整历史桥。
2. 参考 `dsh-example.patch.yml`，将 `sic-local` 合并进当前 profile 的 `llm-pi-ai` 提供方字典。模型 ID 和容量/输出上限改成实际固定模型的值，保留已有其他提供方。
3. 在 DSH 启动环境设置 `SIC_DSH_LOCAL_API_KEY=local-demo`。这是本机桥的占位认证值；真实 provider key 由 SIC 服务进程自己的环境配置加载，不填进模板。
4. 选择 SIC local demo 的模型。每个新对话使用独立 `x-sic-bridge-session` 值；同一个对话后续请求保持该值。桥重启、历史编辑、分支或回滚后建立新绑定。
5. 此演示让 SIC 桥管理历史变更，因此关闭 DSH 自身的摘要/工具结果修剪，避免已确认的完整历史前缀被宿主改写。插件 ID 以实际 profile 为准。发生 409 时查看历史和版本，不绕过前缀检查。

DSH 继续发送完整历史，由桥去重并在封块后发送 SIC 投影。当前桥只支持文本 Chat、单 choice 和现代 `tool_calls`，SSE 在完整上游响应后输出；它不是 Responses 或 Anthropic Messages 的全历史适配器。静态 header 需要用户为新对话换值，尚未集成 DSH 自动会话 ID 注入。

可先在 Langflow 实例中观察对应 Rust session 的 WB/CB 和发布版本；响应 header 的 `x-sic-session-id` 是后端会话 ID，与 DSH 的桥绑定键有不同用途。
