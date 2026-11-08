# SIC v0.1 演示后端准备情况

核验日期：2026-11-07。源码机制版本仍为 `9.0.0-alpha.3`；“v0.1”是演示里程碑，不重新分配旧实验成绩。本轮未恢复历史实验，没有 8341 推理/实验调用，也没有真实模型质量结论。8341 是非固定测试路由，排除正式实验。

## 已完成与本轮增量

原有 Rust 服务支持不可变 WB/单元、单响应编辑、默认 KEEP、草稿版本、验证、提交、顺序发布、独立动态挂载、手动相邻升阶、归档分页与字面检索。Chat/Responses 支持显式 `tail-only` 的本地投影与流式上游转发；默认原样转发。

本轮补充：

- `scripts/sic_demo.py`：标准库客户端，顺序执行 WB → draft → preflight → 一次模型响应 → validate → commit → publish。任何一步失败立即停止，无隐藏模型重试；数据库保留已创建材料供恢复。需要显式 `--allow-model-request`，且只连接 loopback SIC 服务。
- commit 支持可选 `x-sic-cb-sha256`。客户端传入 validate 返回的 `cb_sha256`；若验证后草稿内容改变，提交返回 409 且事务回滚，不提交错误版本。旧调用方不传此 header 时兼容原行为。此校验锁定输出正文，不声称锁定全部操作元数据。
- `scripts/run_experiment.py`：默认新启自有 loopback Mock 和隔离 SQLite/SIC 子进程，逐案执行 baseline/SIC 两臂。SIC 臂实际走单轮编辑、提交、发布和 Worker `tail-only` 转发。Mock 故意 KEEP 原文并提取合成事实，只验证接线；不模拟模型压缩收益。
- 固定 provider/model 与输出上限；每次运行保存 manifest、二进制/runner/client 哈希、输入文件哈希、案例快照、逐臂记录、预检和编辑/提交/发布证据、Worker 请求/响应、评分和摘要。输出目录必须不存在；不自动续跑或复用旧答案。完成状态独立保存在 completion.json，manifest 的 running 是创建时状态。
- `scripts/score_experiment.py`：离线核对工件摘要、case/arm 唯一性与完整性，重新计算精确匹配分数。失败/缺失与有效错误答案分开。摘要防止意外混用和改动，不是外部签名或可信时间戳。
- `scripts/release_check.ps1`：一键格式检查、Rust 测试、Clippy、构建、隔离烟测、Python 完整性测试、离线两臂 E2E 与重新评分。任一步失败停止。
- `scripts/dsh_chat_bridge.py`：OpenAI Chat 全历史桥。客户端持续发送完整历史并显式提供 `x-sic-bridge-session`；桥保护前导 system/developer，精确检查已接受的消息前缀，按完整工具调用/结果组封 WB，到达可配置 UTF-8 字节阈值后自动单轮编辑、验证、提交、发布，再发送只含活动尾部的 Worker 请求。相同完整请求重试返回缓存结果；修改历史、换宿主指令或不完整工具批次会拒绝，不猜测去重。
- Chat/Responses 的实际投影组装器以同一个 SQLite 读事务获取根、待处理 WB 和挂载，避免并发管理写入造成跨时刻混合快照。

HTTP bootstrap 的 `runner/scorer/experiments=false` 保持不变：新增运行器是 CLI，不是授权 WebUI 自动启动实验的 HTTP 接口。

## 实测结果

- `cargo test`：5 项通过。
- `cargo fmt -- --check`、`cargo clippy -- -D warnings`、`cargo build`：通过。
- `python tests/smoke_backend.py`：通过，含 hash 不匹配 409、草稿保持 open、匹配 hash 正常提交、原有编辑/升阶/分页/并发/Worker SSE 与 400 透传测试。
- `python -m unittest discover -s tests -p "test_*.py"`：14 项通过（安装 benchmarks/requirements.txt 后全执行；未安装时两项格式依赖测试跳过），覆盖失败答案分离、已有目录拒绝、非固定测试路由拒绝、工件改动拒绝、完整历史自动封块、精确重试缓存、前缀保护和工具组闭合。
- `python tests/smoke_dsh_bridge.py`：真实 Rust 服务 + HTTP 全历史桥 + 自有 loopback 模型通过；覆盖自动编辑发布与投影、缓存重试、受保护指令不入 WB、工具组保存为单一源单元、缓冲 SSE、错配 409。
- 最终完整 release check 已通过，含以上两套 HTTP 烟测与 Python 测试。
- 离线证据目录：`OUTPUT_ROOT\backend-release-check-final-20261107`。两条合成案例，baseline/SIC 各 2 条已答，精确匹配各 2 条；这只是确定性 Mock 的连通性结果，不是 SIC 质量成绩。
- 此前 debug exe SHA-256：`f35c30dcd1198644da6e055af78bc5cd2e2fe5e0c0c34f2b34a916515fd6f001`。

## Windows ARM64 二进制包

`cargo build --release` 通过；构建平台为 `aarch64-pc-windows-gnullvm`（MSYS2 Rust 1.99.0）。release exe 为 9,453,568 字节，SHA-256 `7991f246730f9f5dfc786b5c9642b91e4fdbdd67606774b1109b6bb60cd441ed`。

最终发布候选包位于 `OUTPUT_ROOT\release-v0.1.1-catalog2-20261107\sic-v0.1.1-demo-windows-arm64.zip`；具体 ZIP 摘要由发布交接与外层工件清单记录，避免把包自己的摘要写进包内再改变摘要。只适用于 Windows ARM64，不是 x64/Linux/macOS 构建。最终包不含旧 WebUI；包含 bin、libunwind.dll 及许可证、Langflow/LangGraph 集成源和 flow（需独立安装依赖）、启动/桥/文本集 runner/scorer、合成案例和说明；不含 target/debug 或大型源数据。

导入检查显示仅 `libunwind.dll` 为非 Windows 系统运行库，已随包携带。移除 MSYS2 和 Rust 的 PATH 后，包内二进制实际通过两套 HTTP 烟测、包内离线 runner 和复评分；最新便携候选在移除 MSYS2/Rust PATH 后通过两套 HTTP 烟测、合成 runner/scorer，并通过包内 BEAM 全历史 dry plan；证据 `OUTPUT_ROOT\release-ready-portable-mock-20261107` 与 `OUTPUT_ROOT\release-ready-beam-dry-20261107`。运行包不需 Rust/MSYS2，Chat 桥和离线工具需 Python 3.10+；启动脚本需 PowerShell。`scripts/package_windows_arm64.ps1` 可在新输出目录重新打包，集成源和文档以打包时快照为准；后续内容更新后需重新计算 ZIP 摘要。

## 启动和复现

在项目目录中：

```powershell
$env:SIC_ADMIN_TOKEN = '自行设置私有值'
cargo run
```

默认地址 `http://127.0.0.1:8787`。已有环境变量可指定独立 `SIC_DB` 和 `SIC_BIND`。当前只有本地管理 token，不应将演示服务公开到网络。

完全离线验证：

```powershell
./scripts/release_check.ps1
# 或者已构建后只跑新管道：
python scripts/run_experiment.py --out OUTPUT_ROOT/my-new-offline-run
python scripts/score_experiment.py OUTPUT_ROOT/my-new-offline-run
```

真实 provider 入口已实现但本轮没有调用。需要稳定的独立 Chat Completions endpoint、固定 model 和环境变量 API key；显式开启后才会发送请求：

```powershell
$env:SIC_EXPERIMENT_API_KEY = '自行设置私有值'
python scripts/run_experiment.py --provider chat-completions --endpoint https://YOUR-HOST/v1/chat/completions --model PINNED-MODEL --allow-network --out OUTPUT_ROOT/my-new-provider-run
```

CLI 拒绝直接指定非固定测试路由 8341、URL 用户名/密码及 query/fragment；API key 不写入 manifest。两个角色共用固定 provider/model，但压缩器和答题器的请求内容不同，不能将其视作完全相同的计算条件。真实运行会付费且会向配置 provider 发送案例材料；请在取得稳定独立 API 后运行。此小型入口的精确匹配不代表大型语料官方评分；真实文本集的独立适配/计划/执行/评分入口见 real-text-experiments.md。

`sic_demo.py --session SESSION_ID --units units.json --allow-model-request` 可将已有会话中的一块材料自动处理到发布；units.json 是字符串数组。Worker 端继续按既有 `x-sic-session-id` 和 `x-sic-history-mode: tail-only` 契约接入，只传当前活动尾部；不得重复发送已经归档的历史。DSH/Langflow/LangGraph 适配见 integrations 交付。

## DSH 全历史自动演示

配置稳定 API 后，可以一条启动命令同时管理本地 Rust 服务与 Chat 桥：

```powershell
$env:SIC_COMPRESSER_URL = 'https://YOUR-HOST/v1/chat/completions'
$env:SIC_COMPRESSER_MODEL = 'PINNED-MODEL'
$env:SIC_EXPERIMENT_API_KEY = '自行设置私有值'
./scripts/start_dsh_demo.ps1
```

DSH 使用 OpenAI Chat 提供方，base URL 为 `http://127.0.0.1:8788/v1`，模型为配置模型，API key 可填本地占位值（真实 key 由本地服务配置）。为每条对话设定唯一的自定义 header `x-sic-bridge-session: YOUR-CONVERSATION-KEY`；一直发送完整历史，不能自行去重。若客户端无法发送自定义 header，需先增加其适配，不要把不同对话静默并入同一个会话。DSH 具体配置选项仍需在实际宿主验证，当前验收使用相同 HTTP 协议的模拟客户端。

默认阈值为 65536 UTF-8 字节，可通过启动脚本 `-SealBytes` 修改；这是演示用字节策略，不是历史 64–192 Ki token 的质量策略，也不代表模型容量准入。保持最后一个完整消息组为活动尾部。桥将完整消息组 JSON 作为 WB 源单元，不拆工具批次。早到的宿主指令不进 WB；晚到的 system/developer 明确拒绝。启动脚本保留 runtime/dsh-demo.db 和本地日志；不打印 key。Ctrl+C 后只清理本次拥有的 Rust 子进程。

桥默认缓冲上游非流式完成后返回 JSON 或兼容 SSE；这不是实时 token 流，暂不支持取消后精细续推。桥的会话游标、返回缓存和锁保存在内存，重启后使用新会话键并重放全历史；不自动恢复旧键。遇到可能已提交的失败，当前键停止接收请求，需检查数据库并新建会话，不盲目重试。外部 Admin 仍可修改同一 SIC 会话，客户端应让桥独占它创建的会话。

## 未完成与发布边界

真实 CorpusQA/BEAM/BABILong 的逐案适配、dry plan、固定 provider 顺序执行与评分接线本轮已补充，见 real-text-experiments.md；仍无真实模型质量结论。

尚未实现：Rust 原生自动封块调度、准确模型 token 容量准入、V7 自动提前升阶和原 WB 重试、Worker 内部归档工具续推、Messages SIC 投影、REORDER/ADD、E12、物理模型采用证据、官方完整大数据集评分复现、自动化质量提升论证。独立 Chat 桥已有保守的字节阈值自动封块，不能等同历史正式策略。当前 release 内嵌 catalogVersion 为 2026-11-07.2，默认 provider URL 为空，8341 标记为 testing-only-unstable-route；重建后两套 HTTP 烟测（含 catalog 断言）及 14 项 Python 测试全部通过。

Rust 单轮入口和 sic_demo 客户端从明确 WB 开始；Chat 桥额外提供上述保守自动封块，但不是通用长期任务调度器。手动升阶和候选预览不等于自动模型汇总。single-pass 草稿更新与 edit-job 最终状态目前分两次数据库写入：若后者失败，HTTP 可能报错而草稿已应用；必须检查草稿和作业，不能仅凭 HTTP 失败断言无修改。CLI/桥没有通用中断恢复调度或自动重试策略。真实 provider 的兼容性、速率限制、长上下文容量、费用与任务质量仍待独立验收。当前可发布为本地可运行演示，不能宣称全部 SIC 历史机制已恢复或长工程质量获胜。

## v0.1.1 桥回放兼容补丁

兼容补丁保持核心 Rust 机制和源码不变；最终候选因嵌入 catalog.2 数据元数据重新构建，二进制身份见上文。Python 桥仅对上次本桥生成的最后一条 assistant 接受已验证的空 metadata 和无损 JSON 参数表示差异；既有 prefix、宿主/user/tool 内容保持严格。实际安装 pi-ai 1.1.0 的无网络 stream/convertMessages 生成 8 个 fixture；19 项 Python 测试与两套 HTTP 烟测通过。精度损失、重复键、NaN、非空信息丢失及工具身份变化仍拒绝。详情见 dsh-bridge-replay-compatibility.md。原 v0.1.0 tag 不覆盖，使用独立 v0.1.1 包。

## 最终 catalog.2 软件候选

最终构建仅更新内嵌数据目录元数据：catalogVersion=2026-11-07.2，global source-prepared-cli-runner-available-evaluation-pending；高资源三项均为 local-source-verified-evaluator-pending，official-evaluator-pending / gateway-tokenizer-unverified 继续显式保留。隔离 GET bootstrap/catalog 检查与两套 HTTP 烟测均通过；核验记录 OUTPUT_ROOT\v0.1.1-final-validation-20261107.json。没有重新运行全真实大材料 Mock，也没有真实 provider 推理。软件包不附论文。
