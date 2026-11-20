# Windows、Linux 与其他宿主接入

本文适用于 Core 平台包的构建验收及新宿主接入。完整宿主生产接入以单独的验收范围为准。Pi 和 MCP 归档扩展需要支持 `--archive-grants` 的 Core 构建，首次部署在独立测试实例验证会话范围。

旧交付以源码快照 `c7dfcbd70bdf2cb54d7eb1b091df297a5636fb4f` 和原 ZIP 的哈希识别。新的构建路径沿用 Core 的实际 API 与数据格式。这里的 x86 指 x86-64。

## 构建、打包和启动

在干净的源码检出中为对应 target 准备 Rust 标准库、C 编译器和 Cargo.lock 所列依赖。构建和打包使用 Python 3.11 或更新版本，压缩包解包验收使用 Python 3.12。Windows ARM64 继续使用 `scripts/build_private_release.ps1`。另三个目标可用以下命令，`--platform` 使用 `linux-arm64`、`linux-x86_64` 或 `windows-x86_64`。

```sh
python3 scripts/build_private_release.py --platform linux-arm64 --toolchain 1.97.1 \
  --target-dir /srv/sic-build/target
python3 scripts/package_rust_v2.py --platform linux-arm64 \
  --binary /srv/sic-build/target/aarch64-unknown-linux-gnu/release/sic-core-rust \
  --output /srv/sic-build/linux-arm64
```

- Linux ARM64 使用 `aarch64-unknown-linux-gnu`。用该发行环境核对 ELF 的 `GLIBC_*` 版本需求。
- Linux x86-64 使用 `x86_64-unknown-linux-musl`。将 `CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER` 设为 Rust 自带的 `rust-lld`。原生 x86-64 Linux 可将 `CC_x86_64_unknown_linux_musl` 设为 `musl-gcc`，将 `AR_x86_64_unknown_linux_musl` 设为 `ar`。跨编译时使用目标 musl sysroot。打包时用 `--runtime-licenses <目录>` 随包附出 musl 声明。
- Windows x86-64 使用 `x86_64-pc-windows-gnu`，构建脚本启用静态 CRT。跨编译使用 MinGW-w64 x86-64 的 GCC/AR，并设置 `CC_x86_64_pc_windows_gnu`、`AR_x86_64_pc_windows_gnu` 和 `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER`。打包用 `--runtime-licenses <目录>` 随包附出 GCC Runtime Library 与 MinGW-w64 的声明。用 PE 导入表和实际加载确认运行时依赖。

发布包中的 `release.json` 记录源码和目标，`checksums.json` 记录每个文件的摘要。Windows 产物为 ZIP。Linux 产物为保留可执行权限的 `.tar.gz`。核对整包及文件摘要后，在独立目录首次运行 `bin/sic-core-rust init <database>`，随后用 `bin/sic-core-rust serve <database> 127.0.0.1:18890 <actor> SIC_CORE_HOST_TOKEN` 启动。Windows 使用对应的 `.exe`。变量值由 Core 服务进程的私有环境提供。

平台验收应使用解包后的 Core，运行认证、真实发布、同身份重放、硬终止后恢复及在线备份恢复。宿主适配也要单独验证完整会话循环。[生产运维](rust-v2-production-operations.md)

## 配置只读单链归档凭据

用已有 Core 管理流程取得要授权的链 ID。为该链生成独立的随机凭据，通过服务进程私有环境传入。下面的 JSON 只保存环境变量名，可用于多条链，各凭据必须不同。

```json
[
  { "chain_id": "<已授权链 ID>", "token_env": "SIC_ARCHIVE_PROJECT_A" }
]
```

在 Core 启动参数中加入 `--archive-grants /path/to/archive-grants.json`。由宿主仅把 `SIC_ARCHIVE_PROJECT_A` 的值交给对应 Pi 或 MCP 客户端。Core 从凭据解析固定的链，并只允许归档搜索和原文分页。权限修改随服务重启生效。授权文件和宿主管理环境由受信运维账号持有。

## Pi 归档来源回查

`integrations/pi/sic-archive.ts` 使用 Pi 原生扩展工具注册和 `before_agent_start` 接点，可挂载在已有 Pi 1.5.2 配置中。用 Pi 启动参数 `-e <检出路径>/integrations/pi/sic-archive.ts` 加载。用原生 `--session-id <id>` 打开已选会话。

由宿主为该进程设置 `SIC_PI_SESSION_ID`、`SIC_PI_CORE_URL` 和 `SIC_PI_ARCHIVE_TOKEN`。Core URL 使用 `http://127.0.0.1:<端口>`。扩展按原生 Session ID 启用工具。用已有授权链做回查，查询结果沿 Pi 原生工具续轮。未选择会话保留原工具表。

此接点交付 `sic_memory_search` 和 `sic_source_text_page`。独立新会话的压缩配置、协议与恢复见 [Pi 原生压缩](pi-native-compaction.md)。[Pi 扩展契约](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md)

### Pi 编辑时序验收

Core 的 `pi-messages` 载体保留 Pi SDK 消息原貌。受信宿主用 `host_message_origins` 把可编辑文本锚定到持久化的 Pi Session 条目。工具挂起时，`native.begin` 校验实际的路由、助手消息及调用顺序，并把来源暂存到发布。发布后的 CB 仅包含选中的文本。随后到达的工具结果由 Pi 保持并在下一次调用时传入。原始会话、工具参数及不透明协议字段保留供回查。

可在源码检出的隔离环境运行 `python integrations/tests/pi_compaction_acceptance.py --pi <Pi 1.5.2 的入口> --core <Core 可执行文件>`。Windows 下可用 `--node <node.exe>` 调用 Pi 的 JS 入口，并用 `--shell <bash.exe>` 指定 Git Bash 或 MSYS2 Bash。测试调用真实 Pi、Bash 和 Core，Provider 使用本地模拟。用例检查两种工具完成顺序、进程强杀恢复、连续采用，以及实际请求中的共享前缀、原生工具和后到结果。

## OpenCode 原生压缩

独立新会话的原生压缩通过 `integrations/opencode` 加载。单会话、单链、单 Agent 的配置与恢复步骤见 [OpenCode 原生压缩](opencode-native-compaction.md)。

## Hermes 与 Claude Code 原生压缩

Hermes 的自定义 Chat Completions 路由通过原生 Context Engine 接入。Claude Code 的独立文本会话通过本机 Messages Gateway 接入。会话授权、安装和恢复见 [Hermes 原生压缩](hermes-native-compaction.md)、[Claude Code 原生压缩](claude-code-native-compaction.md)。

## Codex App Server 原生压缩

Codex 使用本机单线程 Responses Gateway。启动器为独立配置启动原生 App Server。安装和复用原生线程见 [Codex 原生压缩](codex-native-compaction.md)。

## MCP 归档来源回查

`integrations/mcp/archive_server.py` 是可由宿主启动的本地 stdio 服务，验证用 Python MCP SDK `1.30.0`。在专用 Python 环境安装 `mcp==1.30.0`，将 stdio 命令设为该环境的 `python`，参数为此脚本的绝对路径。传入 `SIC_MCP_CORE_URL` 和 `SIC_MCP_ARCHIVE_TOKEN`。每个 MCP 进程使用一条链的凭据，部署配置负责选择可以启动它的会话。托管多用户会话的原生 Session 绑定需逐个宿主接入验收。

模型可通过原生 MCP 工具调用这两个归档接口。工具名称可能带宿主自己的服务器命名空间。当前用途是归档回查，原生工具、任务、权限、Provider 与压缩控制仍由各宿主处理。使用按需启用的专用宿主配置，便于锁定回查范围。[Python MCP SDK](https://github.com/modelcontextprotocol/python-sdk)

Codex 可在隔离配置中加入以下 stdio 服务器。使用测试实例地址和专用的归档凭据，并填入实际脚本路径。这是隔离测试配置示例。

```toml
[mcp_servers.sic_archive]
command = "C:/sic-test/venv/Scripts/python.exe"
args = ["C:/sic-test/source/integrations/mcp/archive_server.py"]

[mcp_servers.sic_archive.env]
SIC_MCP_CORE_URL = "http://127.0.0.1:18890"
SIC_MCP_ARCHIVE_TOKEN = "<专用的测试归档凭据>"
```

Claude Code 可在隔离的测试配置中调用原生 CLI 注册，并用 `claude mcp list` 检查连接。生产凭据通过受信宿主的私有配置交给子进程。

```powershell
claude mcp add --scope user sic_archive `
  -e 'SIC_MCP_CORE_URL=http://127.0.0.1:18890' `
  -e 'SIC_MCP_ARCHIVE_TOKEN=<专用的测试归档凭据>' `
  -- 'C:/sic-test/venv/Scripts/python.exe' 'C:/sic-test/source/integrations/mcp/archive_server.py'
claude mcp list
```

归档工具通过 `/api/v2/archive/operations` 使用单链凭据。该凭据在 Core 的宿主管理入口无法认证。此说明覆盖两个只读归档工具的接入，Editor 与递归发布另有验收范围。
