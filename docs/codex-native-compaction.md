# Codex App Server 的 SIC 原生压缩

此入口面向可信本机用户的独立文本会话。启动器把原生 App Server 的模型路由绑定到一个 Responses Provider、一条 Core 链和一个原生线程。使用 App Server 原生的手动压缩与自动阈值。Worker 继续使用原生工具和权限，Provider 的流式响应原样返回。

## 安装

1. 安装 Codex CLI 和 Python 3.11 或更新版本。保留包里的 `integrations` 目录。
2. 新建独立 Core 空链，用 `--harness-grants` 授予该链写入凭据。Core 使用 IPv4 回环。将单链凭据与 Provider API Key 分别放入启动器的 `SIC_CODEX_CORE_GRANT` 和 `SIC_CODEX_PROVIDER_KEY` 环境变量。
3. 可选安装 `mcp==1.30.0`，另发该链只读归档凭据，并设 `SIC_CODEX_ARCHIVE_GRANT`。启动器将原生 MCP `sic_archive` 加入此独立配置。
4. 把包里的 `integrations` 目录加入 `PYTHONPATH`，为此会话设置新的私有 `--state` 目录。

```sh
PYTHONPATH=/opt/sic/integrations python -m codex.native_launch \
  --core http://127.0.0.1:18890 --provider https://api.openai.com/v1 \
  --model "$CODEX_MODEL" --chain "$SIC_CHAIN" --state /private/codex-sic \
  -- app-server --listen stdio://
```

`--codex` 可指定原生可执行文件。客户端按原生 App Server 协议初始化，调用 `thread/start` 建立新会话，模型名用 `--model` 的值，模型 Provider 用 `sic_local`。`--batch` 默认 8，此设置从首次建立会话起固定。原生容量阈值按模型和配置确定，也可由部署方显式指定 `--auto-threshold`。

## 手动压缩和重开

调用 `thread/compact/start` 手动压缩。自动阈值使用同一接管入口。SIC 按原生消息 ID 识别旧的用户材料、相连的助手回答与已闭合的文本工具结果。Core 保存原文、发布 CB 并自动递归。Worker 的新问题和工具续轮仍留在尾部。

复用同一 Core 数据库、链、Provider、模型与 `--state`，重启启动器后调用原生 `thread/resume`。原生线程 ID 来自首次 `thread/start` 的回执。再次尝试手动压缩时会检查已提交或已发布的候选。结果未知时保留原本的线程和 Core 数据用于复核。

## 适用范围

验证范围是原生 App Server、显式 Responses Provider 和独立的新文本线程。强制重开、原生工具结果入档和同文重复发生均有离线实际宿主测试。原生桌面内置账号路由、会话迁移、多会话共享实例、多模态压缩和其他 Responses 工具种类需单独验收。[Codex App Server 协议](https://developers.openai.com/codex/app-server/)
