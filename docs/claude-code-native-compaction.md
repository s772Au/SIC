# Claude Code 2.1.328 的 SIC 原生压缩

此入口用于可信本机用户的独立文本会话。Claude Code 原生 `/compact` 与自动阈值通过 `PreCompact` 进入 SIC。Gateway 核对原生会话和请求类型，再按 Core 选定切面投影 Worker 的 Anthropic Messages。原生 Worker 工具、权限和流式响应沿用 Claude Code 与 Provider 的配置。

## 安装

1. 安装 Claude Code 2.1.328 和 Python 3.11 或更新版本。Windows 需要 Claude Code 可用的 Git Bash。保留仓库中 `integrations/claude` 的三个脚本在同一目录。
2. 在 Core 为该会话新建独立空链，用 `--harness-grants` 发该链的写入凭据。把凭据通过启动器的私有环境设为 `SIC_CLAUDE_CORE_GRANT`。Core 地址使用 IPv4 回环。
3. 把同一 Anthropic Messages Provider 的 API Key 设为 `SIC_CLAUDE_PROVIDER_KEY`。启动器向 Claude Code 交付每次启动独立的回环凭据。Provider 需要支持所用 Claude 模型、Messages 和客户端需要的原生 beta 头。
4. 给该会话创建一个私有目录，生成一个原生会话 UUID。该目录保存启动器的 SQLite 检查点及 Claude Code 的独立配置与会话历史。

```sh
python /source/SIC/integrations/claude/launch.py \
  --core http://127.0.0.1:18890 --provider https://api.anthropic.com \
  --chain "$SIC_CHAIN" --session "$CLAUDE_SESSION" \
  --state /private/claude-sic -- --model sonnet
```

`--claude` 可指定原生可执行文件。`--batch` 默认 8，可在首次打开新会话时设为 2 至 32。私有目录下 `claude/settings.json` 可配置原生用户工具和权限。启动器为该会话安装自己的 `PreCompact`，保留其配置、原生会话以及 Core 归档供后续恢复。

## 压缩与恢复

手动输入 `/compact`。自动压缩沿用 Claude Code 的阈值。SIC 在原生消息边界冻结范围，编辑授权文本和已结束的文本工具结果。Core 发布后调度递归采用。后续 Worker 请求使用选定 CB 和未覆盖的尾部。原生会话保存压缩检查点。主会话请求按绑定处理，原生子代理和辅助模型请求沿用原始透传路径。

重启时沿用同一工作目录、`--state`、`--chain`、`--session`、Provider 和模型，并在启动命令的 `--` 之前加入 `--resume`。提交或发布回执丢失时，以同一原生材料重试 `/compact`。SIC 会查询已提交候选并复用发布记录。无法确定已发出的 Editor 是否提交时，Gateway 暂停该会话的压缩派发。保留原生历史和 Core 归档供核对。

## 验收范围

Windows 和 Linux 的 x86-64、ARM64 均通过原生手动压缩、自动阈值触发、三阶采用和冷启动续轮。以 `python integrations/tests/claude_native_compaction_gateway_probe.py <Claude Code> --core <Core> --launcher` 运行真实原生 CLI、Gateway 和 Core。加 `--automatic` 检查自动阈值。验收使用本地 Messages Provider 检查实际请求和响应。图片和文档压缩、已有会话迁移、Bedrock、Vertex 和其他登录路由仍待单独验收。[Claude Code Messages 网关规范](https://code.claude.com/docs/en/llm-gateway-protocol)
