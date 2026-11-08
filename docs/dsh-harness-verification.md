# 真实 DSH Harness 只读验收

`integrations/tests/verify_dsh_harness.py` 只读取显式指定的 CLI JSONL、隔离 DSH_HOME 内对应 SessionId 的压缩会话记录，以及已认证的同核控制接口。它不调用推理接口，不发送工具事件，不推进 Runtime，也不修改会话或服务。

使用拥有 `zstandard` 的 Python 环境执行。控制令牌默认从 `SIC_CORE_TOKEN` 环境读取；也可使用私有 `--token-file`，不要将令牌写进命令行或报告。

```powershell
python integrations/tests/verify_dsh_harness.py `
  --jsonl $env:SIC_DSH_RUN_A_LOG --jsonl $env:SIC_DSH_RUN_B_LOG `
  --dsh-home $env:SIC_ISOLATED_DSH_HOME `
  --output .local/dsh-verification.json
```

默认控制接口为本机 18888 与桥 18890，可通过 `--core-url` / `--bridge-url` 指定独立回环端口。验证器拒绝非本机地址、8341 和 URL 凭据。`--actor` 默认 `host`，不会授予权限。

验收必须同时具备：

- CLI 的真实 `sic_fixture_wait` 调用、成功结果及最终 `SIC_DSH_ROUNDTRIP_OK`，且顺序正确。
- 原生 `session.v4.jsonl.zstd` 中至少两次 `plugin:sic/provider-projection`，最后一次与桥当前证据逐字段相等；请求正文摘要、请求/响应引用及 Core 调用账引用一致，最终 Worker 请求实际包含工具结果。
- `plugin:sic/tool-event` 的真实 around-dispatch 起止，以及 `plugin:sic/fixture-body` 的函数体起止；函数体必须处于宿主执行窗口内，Core 必须记录对应宿主事件。
- 实际 Editor 调用区间与实际工具函数体有正长度交集，且 Core 账目报告正长度 Editor/tool 重叠。缺少任一证据均退出非零，不能由模型提出 tool_call 代替实际执行。
- 两个独立 SessionId 对应不同 Core 链，调用账 ID 与新建 CB ID 不交叉。

报告只写引用、摘要、时段及断言结果，不复制完整提示词、工具正文或凭据。输出应放在 `.local/` 等私有目录。成功是自有合成 fixture 的接入证据，不是实际 provider 兼容性、模型质量或节约收益的证明。

完成协调重启后，用完全相同输入及新增参数复核：

```powershell
python integrations/tests/verify_dsh_harness.py `
  --jsonl $env:SIC_DSH_RUN_A_LOG --jsonl $env:SIC_DSH_RUN_B_LOG `
  --dsh-home $env:SIC_ISOLATED_DSH_HOME `
  --baseline .local/dsh-verification.json `
  --output .local/dsh-verification-after-restart.json
```

这会额外要求 SessionId→chain、请求/响应引用、CB、当前 cut、调用账 ID 及原生记录摘要均保持不变，因此能发现恢复后丢失绑定、重复调用或证据漂移。它本身不执行重启，也不声称覆盖正在运行时故障、未知远端结果或所有宿主工具的分身执行。

从全新隔离 Home 运行真实 Harness

先按 `sic_core/dsh_plugin/README.md` 安装锁定的插件依赖，并由宿主启动本项目的 `integrations/run_core_demo.py`，同时提供 Core18888、模型fixture18889、桥18890。随后执行：

```powershell
./integrations/tests/run-dsh-harness.ps1 `
  -DshCommand $env:SIC_TEST_DSH_COMMAND `
  -DshHome .local/dsh-harness-fresh-home `
  -Runs 2 -OutputDirectory .local/dsh-harness-fresh-run
```

`SIC_TEST_DSH_COMMAND` 指向本机 DSH CLI 入口。该脚本只接受全新空 Home 或带本测试专属标记的 Home；只创建独立 `sic-*` profile。它核对三个端口属于同一进程，且进程命令明确为本checkout的离线fixture runner，否则拒绝。它不会启动、停止或切换已有服务，不使用桌面默认profile。专用overlay关闭宿主重复压缩、远程provider、辅助标题模型与上传插件，临时凭据仅从本地私有文件注入进程环境并在结束后恢复。

运行完成后对新产生的两个 JSONL 使用上面的验证命令，`--dsh-home` 指向同一个新 Home。不要把 CLI 退出0本身当成验收通过，必须继续核对持久会话与 Core 证据。

2026-11-08 本地证据：4条真实 DSH CLI 工具往返经过只读交叉验收，各自产生5次自有fixture模型调用、3个CB；其中全新Home的2条会话又在协调重启后通过baseline复核，链绑定、调用引用、CB、cut与原生记录摘要均不变，验证器没有触发新推理。另用只读内存负对照移除Editor/工具重叠，验证器明确失败。上述调用均为自有合成fixture，真实provider兼容性与模型质量没有据此得到验证。

## 历史分身的真实宿主工具往返

运行上述脚本时添加 `-Peer`，再执行：

```powershell
python integrations/tests/verify_dsh_peer.py `
  --jsonl .local/peer-run/roundtrip-1.jsonl `
  --dsh-home .local/dsh-harness-home `
  --db .local/core-ui-demo.db --token-file .local/core-demo-token.txt `
  --output .local/peer-verification.json
```

请将路径指向实际隔离运行。验证器只读指定父会话及对应分身的原生记录，并核对独立会话/CB链身份、真实工具dispatch和函数体时间、完成收据、同链Editor重叠、分身下一次Worker确实收到工具结果，以及原Agent收到分身回传。它读取本地控制令牌但不输出它，不请求推理。

2026-11-08新增两次独立真实DSH分身往返，均通过该交叉核验。此结果证明获准的合成wait工具通过原生宿主执行通路完成，不代表所有外部工具、权限弹窗、故障恢复和Studio界面均已验收。取消/租约过期/重复完成等Core边界另有单元回归；不可用这些回归代替原生宿主的全部故障场景。
