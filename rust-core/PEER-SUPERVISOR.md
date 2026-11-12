# Rust 单任务 peer supervisor

`sic-peer-supervisor.exe` 由可信宿主显式启动，支持 `run` 单任务模式、`watch` 有界轮询模式（默认仍为单任务串行）和 `ack` 显式解除本地隔离。它在 Core 中认领已有 spawn task，按有界策略调用 Provider Editor 准备 peer 输入，启动一次隔离 DSH headless 会话，验证最新 Core Worker attempt 后经授权 Socket 回送。它不是生产就绪声明；`watch` 的并发只按宿主显式配置的有界数量执行。

## 前置条件

1. 解压并检查 SIC 包的 `SHA256SUMS.txt`。包内故意不带 `node_modules`。在源包目录中运行 `scripts/setup-dsh-runtime.ps1`，把整包复制到一个**尚不存在**的新绝对目录；脚本只在新副本的 `dsh-plugin` 中按锁文件安装生产依赖。不要在校验过的源包中安装依赖。保留源包只读。
2. 在 runtime 副本中使用独立数据库，并以前台方式启动 Rust Core。Supervisor 的 `db_path` 必须指向 Core 当前正在使用的**同一个绝对数据库文件**；不要指向原 Python 数据库或另一个副本。Core 和 DSH model profile 都必须启用固定的 `sic-worker` alias。Worker 实际 `default_output_tokens`、DSH 插件 `maxTokens` 与 supervisor `max_tokens` 必须一致。
3. 创建一个新的隔离 `DSH_HOME`。在其 headless profile 中先安装 runtime 的 `dsh-plugin`：

   ```powershell
   $RuntimeDirectory = '<runtime 副本绝对路径>'
   $env:DSH_HOME = '<新建的隔离 DSH_HOME 绝对路径>'
   dsh --profile sic-peer-isolated --from-default-profile headless --dump-config
   dsh plugin --profile sic-peer-isolated add (Join-Path $RuntimeDirectory 'dsh-plugin')
   ```

   `dsh_home` 和 `dsh_profile` 配置必须与这里完全一致。不要在普通用户 profile 上运行此验收。
4. 经可信宿主先创建 requester route；让它使用确定的 DSH profile 与 `working_directory` bootstrap 并发布带 protected prefix 的 CB。然后由宿主在 Core 中建立 checkpoint、requester 所有的 Socket，并用 `socket.spawn` 建立 peer route/task。Socket 当前 ACL 必须明确允许 peer route（或授权 actor）向该 Socket 发消息。
5. 宿主先安全准备 Core bearer 环境变量。配置中的 `bearer_env` 仅是**变量名**，例如 `SIC_CORE_TOKEN`，绝不能写 bearer 值。Supervisor 用该 bearer 调 Core API，并将其传给受限的 DSH 子进程环境；不要写入 JSON、命令行、profile、日志或报告。

请求路线和 peer 路线都由 Core 作为独立身份管理。Supervisor 的 `working_directory` 必须与 requester bootstrap 使用的目录相同；DSH 的有效 system instructions 和 protected prefix 也必须相同。前缀不匹配时 Core 应拒绝，不要通过改写既有 CB 绕过。

## 配置与运行

有效字段和有界 fixture 示例见 [`config/peer-supervisor.example.json`](config/peer-supervisor.example.json)。JSON 反序列化拒绝未知字段。将示例中所有 `REPLACE_...` 和 `C:\\PATH...` 占位符替换为宿主从已授权 Core 对象/本机安装位置取得的值；不要臆造 route、task 或 Socket ID。

`core_base_url` 是 loopback 根地址，例如 `http://127.0.0.1:18930`；不得附加 `/v1`、凭据、查询或远程主机。`bridge_base_url` 是 loopback Chat bridge 地址，例如 `http://127.0.0.1:18930/v1`。`dsh_executable` 指向本机 Electron `DeepSeek Harness.exe`；`dsh_cli_js` 指向其 `app.asar` 内 DSH CLI 的逻辑路径，虽然该归档内的文件不一定能作为普通磁盘文件单独检查。

Supervisor 通过 `env_clear` 只给 DSH 子进程传运行所需环境变量，不用 shell 执行命令。可执行文件、argv、工作目录、DSH profile、Core actor、工具 allowlist、timeout 和权限均来自可信宿主配置；模型文本不提供命令、可执行路径、环境变量、chain、Socket、claim、launch 或授权。

宿主通过 Core API 创建 task 后，应固定并持久保存下列身份：

- `actor`、`requester_chain_id`、`peer_chain_id`、`task_id`、`reply_socket_id`：取自宿主创建并验证过的 Core 对象。Socket 必须属于 requester，且当前 `message` ACL 授权 peer 或 actor。
- `claim_id`、`prepare_id`、`launch_id`、`reply_id`：由宿主为该次任务生成稳定且唯一的 ID。网络超时或进程崩溃后，不能换 ID 重新派发；先读取 Core 的 durable receipt 并按下文处理。
- `policy`：显式有界 Provider Editor 策略。示例中的 `flush=true`、`max_model_calls=4` 等值只是 fixture 值；应按本机 Core contract 设置有界策略。

`model_alias` 固定为 `sic-worker`。Supervisor 本身不选择供应商或读取 Provider key；Core model config 负责 Editor/Worker Provider。Provider 凭据只由宿主按其变量名提供。`prices` 是 DSH 必需的数值字段；示例中的四个 `0` **只供无计价的本机 mock fixture**，不表示真实服务免费。真实运行必须按使用者的 DSH 账户配置实际费率，不要照抄 fixture 价格。

`allowed_tools` 是显式授权的 DSH 原生工具表；`allow_all_tools` 默认为 false。它只限制工具名称，不限制工具参数，也不把文件工具限制在 `working_directory` 内；文件、网络和副作用权限仍需由 DSH 宿主或操作系统约束。示例只启用固定的 `sic_fixture_read`，它返回合成标记，不读取文件或访问网络。`fixture_read_tool=true` 只注册该本地 fixture 工具。

配置完成后，从 runtime 目录运行：

```powershell
& '.\release\sic-peer-supervisor.exe' run --config '.\config\peer-supervisor.example.json'
```

将命令中的配置路径替换为填好的宿主配置。进程成功时 stdout 返回 JSON 摘要，包括 `state`、`task_id`、`peer_chain_id`、`attempt_id`、`reply_message_id`、`replayed` 和 `requester_adopted_cb`。失败时进程非零退出并输出静态错误码；不要收集或转发模型正文。Supervisor 不把凭据或 prompt 写入其结果摘要。

## 可选：有界轮询（默认单任务）

在完成上述配置和授权后，可信宿主可以使用 [`config/peer-watch.example.json`](config/peer-watch.example.json) 建立独立配置。示例默认 `enabled=false`，不会启动；明确检查授权、隔离目录、Provider 模型和费用后，设置为 `true`。配置中的 `supervisor` 保留与单任务模式相同的受信配置；它的 `peer_chain_id`、`task_id` 和四个控制 ID 仅为通过预检的非空占位，轮询时分别由 Core task 与稳定派生 ID 替换。`actor`、`requester_chain_id` 和 `reply_socket_id` 始终使用宿主配置，模型和 task 不能修改。Socket 必须对各 peer 当前授权，实际回复时会再次检查。

`namespace` 是宿主固定、持久的身份，应在重启后保持一致；不要通过更换 namespace 对结果不明的任务重新派发。`poll_interval_ms` 为 500 至 3600000，`page_size` 为 1 至 100。`max_tasks_per_cycle` 取 1 至 8，表示每轮最多选取多少个 Core task；`max_concurrent_runs` 取 1 至 4 且不得超过 `max_tasks_per_cycle`，省略时为 1。默认仍是每轮一个 task、一次一个 supervisor，与旧配置兼容；只有宿主显式提高这两个值，才会在单个 watcher 进程内并行执行多个 supervisor。该服务仍不跨 requester 自动分配，也不管理全局 worker 池。

```powershell
& '.\release\sic-peer-supervisor.exe' watch --config '<可信宿主准备的配置路径>'
```

标准输出是逐结果一行 JSON：`watching`、`reply_delivered`、`reply_present` 或 `blocked`；一轮并发处理多个 task 时会输出多行。状态可以包含宿主的 task/message ID，但不包含消息正文或凭据。`reply_delivered` 仅表示 Socket 投递，仍须验证 requester 下一次 CB 发布时是否消费。Core 或持久存储不可用时进程停止并返回静态错误码，不会对不明结果自动重试。正常 Ctrl-C 会等待正在执行的单项任务结束并记录结果后退出；强制终止、系统故障或异常网络中断后应依据 durable receipts 对账，不能推断外部工具已停止或已回滚。

并发只表示 watcher 会并行调用同一个受信 supervisor 配置。多个真实 DSH 子进程若共用同一个 `dsh_home`／profile，其会话库和写盘行为是否互相隔离**尚未验收**；在没有独立 profile 或额外隔离时，应保持 `max_concurrent_runs=1`。本机自动化测试只使用合成 runner，验证并发上限、顺序与失败治理，不代表真实 DSH 并发可用。

终态失败会写入本地持久治理表：同一 task/revision 的失败先按 `poll_interval` 指数冷却（上限 15 分钟），累计 3 次后进入隔离，输出 `blocked` 且 reason 为 `quarantined_awaiting_host_ack`，之后不再自动检查该 task。仍在 `intent`／`running` 的子进程不算失败，只继续核对 receipt。宿主确认结果后，可以显式解除隔离：

```powershell
& '.\release\sic-peer-supervisor.exe' ack --config '<同一受信 watch 配置>' --task-id '<Core task ID>'
```

`ack` 只读取同一配置中的 `db_path` 与 `namespace`，不读取 bearer，也不 claim、launch 或投递任何工作；输出 `{"state":"acknowledged","task_id":...,"cleared":true|false}`。Core 对该 task 产生新的 revision 时，本地失败历史自动失效并重新开始计数。

宿主也可在不带 bearer 的情况下查看当前治理状态，最多返回 512 条，按 task ID 排序；内容不含任务文字、prompt 或模型输出：

```powershell
& '.\release\sic-peer-supervisor.exe' status --config '<同一受信 watch 配置>'
```

输出形如 `{"state":"watch_status","namespace":...,"governance":[{"task_id":...,"revision":2,"failures":3,"state":"quarantined","until_ms":0,"reason":"..."}]}`。`state` 为 `cooldown` 时表示尚未到期，`quarantined` 必须用 `ack` 解除。该命令只读本地库，不向 Core 发请求。

在已安装 DSH 和本包依赖的隔离 runtime 中，可用 PowerShell 7 运行 `pwsh -NoProfile -File .\dsh-plugin\test\run-peer-native-read-watch-headless.ps1 -WatchBinaryPath .\release\sic-peer-supervisor.exe`，复验内置 `read` 路径。测试只使用本地模拟模型和临时生成的合成文件，报告写入系统临时目录。

给同一命令追加 `-VerifySignalWait`，可复验分身等待指定 Signal、另一 Agent 经授权 Socket 发送唤醒、分身再读取合成文件并回传的路径。这个打包命令使用本地模拟模型；另有一次固定 OpenDesign 模型的隔离联测完成了相同路径，请求方启动使用模拟模型，分身任务明确指定了等待及读文件步骤。

也可追加 `-VerifySignalCursor`，让分身使用序号游标等待下一条 Signal，
无需预先知道其 ID；该打包检查使用本地模拟模型。另一次隔离联测中，固定
OpenDesign 模型按明确提示使用游标完成等待、读取合成文件和 Socket 回传，
请求方启动使用模拟模型，CB 采用使用程序 fixture。游标由调用方保存，读取
本身不会消费 Signal 收据。

给同一隔离命令追加 `-VerifyMessageWait`，可检查宿主通过 `message.wait`
发现分身回传时消息尚未被 CB 消费，随后再检查请求方发布 CB 并采用消息。
该检查使用模拟模型；当前 watcher 仍使用轮询。

## 状态与恢复边界

成功结果 `state="reply_delivered"` 表示 Core 验证了 peer 的最新成功 Worker attempt、task 的 prepare/inbox receipt、当前权限与 Socket ACL，并将该 Worker 的实际纯文本结果通过 Socket 投递。`requester_adopted_cb` 会是 `false`：消息进入 requester inbox **不等于** requester 已发布或消费该消息。宿主还要在 requester route 上按授权流程读取 inbox、提供必要的后续输入并发布 CB，验证对应消息 receipt 已被该 CB 消费。

再次使用完全相同的稳定配置调用时，只有在 durable launch receipt 显示此前子进程确已成功退出后，Supervisor 才能跳过启动并幂等获取 reply receipt；`replayed=true` 表示这次是恢复已完成操作。它不应再次运行 DSH/Worker 或重复工具动作。

若 claim、prepare、进程启动、进程退出或 reply 的结果不确定，Supervisor 不自动重试。launch intent 一旦 durable 提交后，启动失败、超时或宿主崩溃都会使结果保持 `unknown` 或不能安全确认。**不要换 `claim_id`、`launch_id` 或 `reply_id` 重启同一 task**；保留 DB 和 `state_directory`，按 Core receipts 与实际运行证据人工对账。尚未证明可以安全继续时，应让任务保持隔离状态。

进程 timeout 会尝试终止 Supervisor 直接启动的 DSH 进程；当前 Windows 实现没有通过 Job Object 保证整个进程树都已结束。子进程若另外启动了后代，不能据直接子进程退出推断所有后代和外部动作都已停止或回滚。不要把该工具用于未经验证的不可逆外部动作。

单次 supervisor 只处理一个已授权 task：它会 claim、prepare、启动一次 DSH、校验 attempt 并投递 reply。`watch` 只发现同一 requester 的任务；默认每轮一个、串行执行，显式提高 `max_concurrent_runs` 后才有界并行。它不自动恢复 unknown work：终态失败先冷却、再隔离，必须由宿主 `ack` 后才能再次检查。隔离测试已使用真实 DSH 和固定 OpenDesign 模型驱动分身 Editor/Worker，展开并调用 DSH 自带 `read` 读取合成文件，回传结果并由 requester 的 CB 消费；请求方启动使用模拟模型，CB 消费用程序 fixture。该测试另加拦截器，只允许读取其合成文件；这项测试限制不是 SIC 的生产文件沙箱。其他业务工具、并发负载、长时间运行和生产运行手册仍未验收。
