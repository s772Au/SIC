# Hermes v0.23.2 的 SIC 压缩接入

当前接入使用固定版本源码上的 Context Engine。Windows 和 Linux 的 x86-64、ARM64 已验收自定义 Chat Completions 模型、手动和自动压缩、三阶采用，以及重开原生 SessionDB。模型测试使用本地模拟 Provider。

## 安装

1. 准备固定版本 v0.23.2 的源码和该版本锁定的 Python 依赖。依次应用 `integrations/hermes` 下的 `required-selection`、`required-binding`、`compaction-authority` 和 `native-editor` 补丁。
2. 将 `integrations/hermes/sic_context.py` 安装为检出的 `plugins/context_engine/sic_context/__init__.py`。此入口使用同进程的原生模型调用，按独立 Hermes Profile 部署。
3. 在该 Profile 的配置中设置 `context.engine: sic_context`、`context.require_engine: true`、`compression.enabled: true` 和 `compression.in_place: true`。为新会话准备空 SIC 链，并用 Core 的 `--harness-grants` 发放单链凭据。

将 `SIC_HERMES_CHAIN`、`SIC_HERMES_HARNESS_TOKEN`、`SIC_HERMES_CORE_URL` 和 `SIC_HERMES_JOURNAL` 提供给可信宿主进程。Core 使用本地 IPv4 回环地址。Journal 使用独立可写目录里的绝对路径。新会话可用 `SIC_HERMES_SESSION` 预设原生 Session ID。一份 Journal 在首次使用后绑定该原生 Session、Core 链和模型路由。批大小由 `SIC_HERMES_BATCH` 设置，默认 8。

## 压缩与恢复

沿用原生 `/compress` 和宿主自动阈值。SIC 在保留最新原生用户区段的同时发布旧材料。原生 SessionDB 记录 SIC 选定切面的标记。Worker 重开会话时通过 Core 核对该标记并恢复选定 CB 和未覆盖尾部。

发生回执或原生检查点异常时，用同一 Profile、Journal、SessionDB 和 Core 链重开原会话。保持原生历史供恢复。若 Editor 的结果未知或现场快照不再一致，派发会暂停。该模式尚未验证手动 `/compress here`、多用户共用 Profile、图像、Codex App Server 所有的线程及其他 Provider 协议。

可以从源码运行 `python integrations/tests/hermes_sic_compaction_acceptance.py <Hermes Python> <打补丁后的 Hermes 源码> <Core>`。加入 `--mode automatic` 运行自动阈值路径。
