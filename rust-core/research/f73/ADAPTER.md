# F73 公开压缩/编辑来源与 SIC 适配包（准备）

状态：**preparation = partial；mechanical_acceptance = not_run；research_evaluation = not_evaluated**。
本文件只记录已锁定的公开来源、提取点与 SIC 适配包，不包含任何新对照结果，也不复用旧 E1/E2 成绩。

## 1. 已锁定来源

| id | 类型 | 仓库/路径 | commit | git blob SHA | 许可 | 取用 |
|---|---|---|---|---|---|---|
| codex_compact_prompt | harness 压缩 prompt | [openai/codex](https://github.com/openai/codex/blob/e58bf8f5233b51e7cd1679d947e3028c1ed91b06/codex-rs/prompts/templates/compact/prompt.md) | `e58bf8f5` | `42fae605` | Apache-2.0 | 全文 426 B |
| codex_summary_prefix | harness 摘要前缀 | [summary_prefix.md](https://github.com/openai/codex/blob/e58bf8f5233b51e7cd1679d947e3028c1ed91b06/codex-rs/prompts/templates/compact/summary_prefix.md) | `e58bf8f5` | `62a7161b` | Apache-2.0 | 全文 399 B |
| swe_history_processors | 非 LLM 历史处理 | [SWE-agent/SWE-agent](https://github.com/SWE-agent/SWE-agent/blob/3ea751c087f32b16e039a2233dd6eefecef325d5/sweagent/agent/history_processors.py) | `3ea751c0` | `2ca8c03d` | MIT | 全文 |
| aider_repomap | 非 LLM 相关性映射 | [Aider-AI/aider](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/repomap.py) | `5dc9490b` | `541bba6e` | Apache-2.0 | 全文 |
| llmlingua_compressor | LLM 压缩引擎 | [microsoft/LLMLingua](https://github.com/microsoft/LLMLingua/blob/01b9e9481d7395e1e83a48df4a062b11f6304325/llmlingua/prompt_compressor.py) | `01b9e948` | `84e390e5` | MIT | 前 100 KB（共 110078 B；blob SHA 为准） |
| llama_index_memory | 公开 memory 引擎 | [types.py](https://github.com/run-llama/llama_index/blob/baeabbfe26241a6916e13e2a8c8156a02e0602cd/llama-index-core/llama_index/core/memory/types.py) | `baeabbfe` | `8a94739b` | MIT | 全文 5034 B |

完整元数据、URL、许可来源与抓取范围见 `sources.json`。以上仅为代表性样本，不是穷尽调研。

## 2. 提取与适配原则

- **默认 KEEP**：Editor 未提及的单元保持原样；所有改动必须落成有序操作。
- **七操作**：KEEP / DELETE / FLAG / REORDER / COMPRESS / REPLACE / ADD；每条操作有目标、理由和来源锚点。
- **P/V/I 分离**：P 写入新 CB；V 作为待处理来源进入下一 WB；I 随发布刷新，原 CB 不被覆写。
- **WB/CB 双链与跨阶**：保留顺序、同阶对应和来源；Worker 通过 CB 导航与层级展开读取，不直接改写历史。
- **角色边界**：Editor 只能调用 `sic_edit_blocks`，不得执行任务；Worker 不得调用 `sic_edit_blocks`，只能使用获授权工具。
- **程序规则**：`deduplicate_exact` 为零模型调用的确定性规则包；程序 KEEP 可闭合冻结快照之后的迟到材料，但不伪称 Editor 已见。

## 3. 来源到 SIC 的适配

### Codex compaction prompt
采纳：checkpoint 交接的四个信息块（进度/约束/剩余步骤/关键引用）与“写给下一个 LLM”的明确受众。
不采纳：把可变摘要当作唯一工件。
SIC 差异：摘要是 COMPRESS/REPLACE 的一种选择；Editor 必须返回 ops/PVI，默认 KEEP，并绑定冻结快照与候选修订。

### Codex summary prefix
采纳：摘要框架与正文分离，便于机器识别。
不采纳：仅靠前缀无法建立可追溯性。
SIC 差异：CB 记录 `editor_source_snapshot_ids`、候选修订和程序 KEEP 来源。

### SWE-agent history processors
采纳：LastNObservations、ClosedWindow、RemoveRegex 作为零模型调用的基线臂。
不采纳：无记录地丢弃旧观测。
SIC 差异：DELETE/FLAG/REPLACE 均目标寻址、可回放；迟到材料有显式 pending/程序 KEEP 语义。

### Aider repo map
采纳：确定性相关性选择（tree-sitter/PageRank）作为非 LLM 基线。
不采纳：把仓库符号图相关性当作对话 WB/CB 状态。
SIC 差异：材料是带来源锚点的 WB/CB 层级，不是仓库符号图。

### LLMLingua
采纳：压缩率/token 成本基线。
不采纳：无目标、无操作、无 PVI 的 token 删除。
SIC 差异：压缩是对单元的有序操作，经过校验；程序 KEEP 负责迟到闭合。

## 4. SIC 适配包快照（SHA256）

| 文件 | SHA256 | 作用 |
|---|---|---|
| `src/prefix.rs` | `9cca9e2b8770dd9f643910fc2c41efc443e238b77b2ac261de2ab49b93e39cd0` | Worker 教学、Editor/Worker 模式 I、CB 前缀 |
| `src/core.rs` | `d7b1fd4d81a8bedd4e8449dc6f0d39cd0e7c616084751867416e51f2cb17af1f` | Editor 请求契约、`sic_edit_blocks` schema、配置冻结 |
| `src/editing.rs` | `05aaaa772619238606675317e8f1a90c3552b7a43bff4a88629c2ad24459750d` | 七操作、PVI、校验与物化 |
| `src/scheduler.rs` | `92ca3be4a7005a7920ce4febd67fe38d87c2dceb2aa0dc49f77e025ca8f06555` | 程序规则、有界调度、逐 WB 预算 |
| `src/experiment.rs` | `8353908dd9e5a5a779a5719784687821a5dfc27a1e654f636098704e3b0cbbaa` | 合成事实保留评分（非真实质量评分器） |

机器可读版本见 `adapter.json`。这些哈希是 2026-11-12 的快照；若相关源码变化，必须重新冻结并更新本表。

## 5. 明确不声称

- 未执行真实模型/工具对照，不声称任何压缩率、成本或质量优势。
- 五个来源不是穷尽调研；未取得的来源不计入准备完成。
- 合成事实保留分数不是语义质量评分；mock 输出按 `not_evaluated` 处理。
- 旧 E1/E2 结果不用于本包。

## 6. 本地复验

```powershell
python rust-core/scripts/verify-f73-pack.py
```

脚本只校验本地文件 SHA256 与来源必填字段，不联网、不证明来源内容未被上游改写。
