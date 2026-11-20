# OpenCode 2.3.2 的 SIC 原生压缩

经验收的配置使用 OpenCode 2.3.2、一个独立新 Session、一个指定 Agent 和一条专用 SIC 链。OpenCode 原生管理 Provider、工具、手动压缩和自动阈值。SIC 响应压缩入口，发布并采用共同切面。初次部署先使用临时项目核对配置。

## 装配

1. 从同一版本源码构建 Core。创建空 SIC 链，并按 [宿主凭据配置](portability-and-hosts.md) 为此链发放独立的 `--harness-grants` 凭据。
2. 在 OpenCode 创建一个空 Session，记录原生 Session ID。已有历史另走迁移流程。
3. 在受信任的 OpenCode 配置中引用适配器的绝对路径。将单链凭据通过私有环境变量 `SIC_OPENCODE_HARNESS_TOKEN` 提供给原生进程。用同一工作目录和 OpenCode 数据目录启动或重开。单个原生服务进程处理此会话。

```json
{
  "plugins": [
    {
      "package": "/source/SIC/integrations/opencode",
      "options": {
        "sessionID": "<OpenCode Session ID>",
        "chainID": "<SIC 链 ID>",
        "agent": "build",
        "coreUrl": "http://127.0.0.1:18890",
        "batch": 8
      }
    }
  ]
}
```

用所选 Agent 和同一模型路由运行会话。模型第一次派发时，OpenCode 存储将 Session、链、Agent、模型路由和批大小绑定。`batch` 可在新会话开始时设为 2 至 32，当前递归上限为三阶。归档回查凭据需单独发放。运行具有任意本地命令权限的工具时，使用独立系统账号隔离宿主的私有配置和凭据。

## 运行与恢复

从 OpenCode 执行原生压缩命令，或由 API 客户端调用 `session.compact({sessionID})`。启用 OpenCode 的原生阈值压缩即可自动触发。压缩成功后继续输入，实际 Worker 请求使用已选 CB 和原生尾部。

如果 Core 已提交或发布、OpenCode 在写入原生检查点前退出，重开原 Session，再执行一次原生压缩。适配器读取原事务、复用已完成的 Editor 结果。若 Editor 结果未知，确认没有尚在执行的 Editor 后，可以在该会话运行 `/sic-abandon-compaction`，退役未提交候选，再执行原生压缩。退役保留原始会话和 Core 归档。已提交的候选只能走恢复流程。

## 验收范围

实测 Windows 与 Linux 的 x86-64 和 ARM64。OpenCode 分别使用自身的 Chat Completions、Messages、Responses 适配器，接入相同协议的本地模拟 Provider。每种协议均验证真实 `read` 输出进入 Editor、手动和自动触发、递归到三阶、提交与发布回执丢失、结果未知后的原生退役命令，以及缺少凭据和错误链时停止派发。图片和工具等待期编辑仍需验收。

```sh
python integrations/tests/opencode_scoped_compaction_acceptance.py /path/to/opencode /path/to/sic-core-rust
```

用 `--protocol chat --protocol messages --protocol responses` 运行三种协议。

如果直接使用 Windows 原生可执行文件，用 `--sdk <匹配的 @opencode/client 的 dist/promise/index.js>` 指定同版本 SDK。
