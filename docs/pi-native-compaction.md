# Pi 1.5.2 的 SIC 原生压缩

此入口适用于 Pi 1.5.2 的独立新会话。Windows 和 Linux x86-64 均已验证原生手动压缩、阈值压缩、Provider 容量溢出重试、三阶自动采用、真实工具等待期间编辑，以及进程被强制终止后的恢复。新增视觉流程在 Windows 和 Linux 的 x86-64、ARM64 四种组合验收。

## 装配

1. 从包含 `--harness-grants` 的源码构建 Core。按现有宿主管理流程新建独立 SIC 链。Core 管理与 Pi 写入各使用一个凭据。启用归档回查时，再提供独立的只读凭据。
2. 保存 `pi-harness-grants.json`。文件填写链 ID 和变量名。该变量的值通过 Core 进程的私有环境设置。归档回查需另配 `--archive-grants` 和独立变量。
3. 首次以固定的 Pi 原生 Session ID 启动。用同一 Session ID、工作目录、模型路由和 SIC 链重开。

```json
[
  { "chain_id": "<该 Pi 新会话的 SIC 链 ID>", "token_env": "SIC_PI_HOST_GRANT" }
]
```

```sh
sic-core-rust serve core.db 127.0.0.1:18890 "$SIC_ACTOR" SIC_CORE_HOST_TOKEN \
  --harness-grants /secure/pi-harness-grants.json
```

向受信 Pi 进程提供环境变量 `SIC_PI_HARNESS_TOKEN`，其值与服务端授权文件指向的变量相同。给 Pi 进程使用此单链写凭据。Worker 回查工具使用单独的只读凭据。可执行任意本地命令的会话继承宿主的系统权限。部署不受信任的执行器时，使用独立的系统身份来保护宿主进程和凭据。

```sh
SIC_PI_CORE_URL=http://127.0.0.1:18890 \
SIC_PI_COMPACTION_SESSION="$PI_SESSION_ID" \
pi --session-id "$PI_SESSION_ID" -e /source/SIC/integrations/pi/sic-compaction.ts
```

Pi 原生配置管理 Provider、模型、原生工具和自动压缩阈值。默认每累计 8 个下阶 CB 后形成高阶候选。`SIC_PI_COMPACTION_BATCH` 可设为 2 至 32，从建立新会话起保持一致。当前路径可以递归到三阶。原始内容保留在 Pi Session 和 Core 归档中。

## 操作与恢复

用原生 `/compact` 或 RPC `compact` 手动压缩。保持 Pi 原生自动压缩开启，即可在阈值和溢出时从同一个入口进入 SIC。已发布压缩块按 Core 当前选定的切面进入后续请求，未覆盖尾部继续保留。

Worker 的已落盘工具请求包含可编辑文本时，SIC 会在原生工具运行期间开始编辑。该批材料在工具启动前冻结。工具结果仍由 Pi 按原工具组归档，随后在 Worker 续轮中保留。冷启动后，在下次派发 Worker 前，扩展会恢复已经提交或发布的工具等待事务。

OpenAI Chat Completions、OpenAI Responses 和 Anthropic Messages 的协议和流式选项由 Pi 原生 Provider 配置控制。具备图片输入能力的模型可接收 Pi 原生图片。SIC 压缩旧文本时会将同期图片带入 Editor，归档图片来源。未选择为视觉压缩材料的图片继续随 Worker 请求传递。

如果 Core 已收到 Editor 提交或已经发布，而 Pi 未完成本地检查点，重开同一个会话并再次运行 `/compact` 可继续事务。Editor 结果未知时，SIC 暂停 Worker 派发。经人工确认后，运行 `/sic-abandon-compaction` 退役未提交候选，随后可用原生 `/compact` 发起新尝试。原有 Pi 消息和 Core 归档保留。该命令只接受处于空闲状态的已绑定会话。

## 验收边界

### 视觉压缩与回查

在单独的 Node 环境安装与 Pi 进程架构一致的 Sharp 0.34.5，然后将变量 `SIC_PI_VISUAL_SHARP` 指向其 `node_modules/sharp/lib/index.js` 的绝对路径。保留 SIC 源码目录结构。扩展会从同一检出读取图像处理实现。已绑定视觉谱系的会话重开前应保留同一 Sharp 配置。

Editor 查看本次授权的原图，输出区域坐标与带编号引用的说明。宿主校验原图、裁切并生成拼图，Core 发布其谱系指针。Worker 可收到分开的裁切组、合成拼图或者文字描述。拼图与裁切组的投影附带编号，避免文字说明重复给出像素内容。视觉 Editor 只得到归档交付工具。Worker 的 Pi 原生工具照常工作。通过原始 Pi Session 条目和图像摘要重新校验已发布谱系。

当 Worker 需要更细证据时，`sic_image_source` 使用 I 区中的 `manifest_id`。依次用 `from=description` 取回拼图、用 `from=mosaic` 和 tile 编号取回裁切、用 `from=crop` 和相同编号取回原图。宿主检查上一级已展示或由原生工具返回。未授权来源、跨级回查和已改变的原始像素均被拒绝。

视觉处理支持单帧、原始方向的 JPEG、PNG、GIF 和 WebP，须符合配置模型的图片输入能力。原图和派生图保存在 Pi Session；应随会话一并做权限隔离和备份。Core 同时保留原图来源与已发布引用。宿主引起的系统段和工具更新由原 Provider 适配器序列化，实际缓存命中由 Provider 决定。

### 已测组合

三种协议都覆盖图片会话的手动压缩、两种工具等待顺序及冷重启恢复。阈值与容量溢出验收使用 Chat Completions。Windows ARM64 的实际 Messages 模型验证了 Editor 选出两个目标区域和冷启动后 Worker 仅从拼图正确作答。逐级回查走真实 Pi 工具和本地模拟 Provider。分叉和树切换在已绑定会话中关闭。已有普通会话的迁移需单独执行。

从源码运行 `python integrations/tests/pi_compaction_acceptance.py --pi <Pi 1.5.2 入口> --core <Core 可执行文件>`。加 `--sharp <绝对路径>` 执行视觉矩阵。Windows 可加 `--node <node.exe>` 调用 Pi 的 JS 入口，并用 `--shell <bash.exe>` 指定 Git Bash 或 MSYS2 Bash。
