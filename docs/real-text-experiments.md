# 真实文本数据的可复现实验入口

本轮实现真实数据逐案适配、只读计划、固定 provider 调度和评分接线。已使用 **完整** CorpusQA-4M、BEAM-10M、BABILong-qa3-10M 样本各一条运行离线 Mock，不是把真实材料裁成两条玩具问题。没有真实模型推理或质量提升结论；8341 是非固定测试路由，排除正式实验，本工具不向它发送请求。

## 安装与数据

在独立 Python 环境安装：

```powershell
python -m pip install -r benchmarks/requirements.txt
```

固定依赖为 ijson 3.6.0 和 pyarrow 26.0.1。本机验证环境 `.venv-bench` 使用 CPython 3.12 x64。Parquet 在支持的 Python/平台上需要对应 pyarrow wheel；纯 stdlib 单元测试可跳过两项格式依赖测试，但真实 BEAM/官方 BABILong JSON 数组需要安装依赖。

默认读取 `benchmarks/sources-lock.json` 中的文件路径和 SHA-256。用户已有 CorpusQA 位于 Downloads；BEAM/BABILong 位于 benchmarks/data。新机器可以通过 `--data-root PATH` 指定镜像根目录，子目录为：

```text
PATH/corpusqa/4m_4domains.jsonl
PATH/corpusqa/10m_4domains.jsonl
PATH/beam10m/data/10M-00000-of-00002.parquet
PATH/beam10m/data/10M-00001-of-00002.parquet
PATH/babilong/qa3-10m-first20.jsonl
PATH/babilong/data/qa1/10M.json
... qa2 / qa3 / qa4 / qa5
```

可选 dataset ID：`corpusqa-4m`、`corpusqa-10m`、`beam-10m`、`babilong-qa3-sample`、`babilong-qa1` 至 `babilong-qa5`。完整源缺失或字节数/摘要不符会停止，不自动接受 `.part` 文件，也不自动下载或修复来源。执行前实际逐文件重新算 SHA-256；只读 dry plan 默认仅核对大小，可用 `--verify-sources` 同时核对摘要。锁文件的预期摘要不等于本次已验证摘要，plan 明确记录 `hash_verified`。

## 只读计划

```powershell
python scripts/dataset_runner.py --dataset corpusqa-4m --dataset beam-10m --dataset babilong-qa3-sample --out OUTPUT_ROOT/new-text-plan
```

默认每个数据集取前 1 条完整历史，BEAM 每条取前 2 个 probe；`--limit-cases N` 修改历史数量，`--question-limit 0` 取 BEAM 全部 probe。案例顺序固定为 dataset 参数顺序、源文件顺序、记录顺序；不随机抽样。

计划保存源定位、完整上下文字节数和 SHA-256、问题/reference/rubric、WB 数量、逐角色调用预算、完整固定配置。不会调用模型。真实 token 数保持 unknown，不把字节数或数据集标称 10M 当本机 tokenizer 实测结果。

适配不会加载整个数据集：CorpusQA 按 JSONL 行，BEAM Parquet 每次一行，官方 BABILong 数组通过 ijson 逐条解码。仍须容纳一条完整记录，峰值内存包括 JSON 解码和序列化临时副本；不声称常数小内存或生产资源上界。WB 以 UTF-8 安全、可无损拼回的字节分块，默认最大 524288 字节，不截掉源文。

CorpusQA baseline 保留原始 system prompt，并能将 context + tail 精确拼回原始 user prompt。SIC ingest 只接收题目前的文档上下文，不将参考答案或题目送给压缩器。BEAM 按原数据聊天顺序完整序列化消息及元数据，不只抽取支持句。BABILong 使用完整 input；target 只进入评分。所有 dataset 的问题均在发布历史后单独答题，不把第一题答案加入后续问题上下文。

## 完整材料的离线 E2E

```powershell
python scripts/dataset_runner.py --dataset corpusqa-4m --dataset beam-10m --dataset babilong-qa3-sample --execute --provider mock --max-model-calls 300 --out OUTPUT_ROOT/new-real-text-mock
python scripts/verify_dataset_run.py OUTPUT_ROOT/new-real-text-mock
```

离线模型把每块压成固定的、有明确信息损失标记的假摘要；答题器始终返回固定假答案，judge 固定零分。它不读取 target 当答案，不伪装质量 oracle。这样可验证完整大材料的 ingestion、239 次自动单轮提交发布、CB 累积投影、双臂答题、逐项评分和工件完整性，又不会让 identity KEEP 将完整长源全部积进每个压缩请求。

## 固定真实 provider

使用前先检查 dry plan 的调用预算。真实运行必须同时给出 `--execute --provider chat-completions --allow-network`、明确 endpoint/model/API key；BEAM 和 CorpusQA 还必须固定 judge model：

```powershell
$env:SIC_EXPERIMENT_API_KEY = '从私有环境加载'
python scripts/dataset_runner.py --dataset corpusqa-4m --dataset beam-10m --dataset babilong-qa3-sample --execute --provider chat-completions --endpoint https://YOUR-HOST/v1/chat/completions --model PINNED-ANSWER-COMPRESSOR --judge-model PINNED-JUDGE --allow-network --max-model-calls 300 --out OUTPUT_ROOT/new-provider-text-run
python scripts/verify_dataset_run.py OUTPUT_ROOT/new-provider-text-run
```

答题与压缩器固定使用同一 model，judge 使用显式的另一个或同一个固定 model；均在 manifest 记录。默认逐案、逐块、逐问题、逐 arm 顺序执行，并发数为 1；没有自动换模型、降档、重试或续跑。endpoint 不能直接为 8341，不能含 URL 用户名/密码、query 或 fragment。API key 不写入工件。模型别名固定不等于物理模型身份已获见证；provider 可能仍有内部路由和采样不确定性。

`--max-model-calls` 必须至少覆盖计划总调用数，否则在执行前拒绝；失败消耗一次尝试，不自动重试。`--max-output-tokens` 默认 4096，`--max-request-bytes` 默认 64 MiB，是本地限制。上游 token 容量、账单与质量没有预设结论。baseline 是完整源文的直接 Chat 请求，SIC 是完整源文逐块编辑发布后的显式 tail-only 请求；不隐藏截断。baseline 或 SIC 超上下文时保留失败状态，不用裁切把它变成有效答题。两臂预算和材料呈现方式不同，不能把单次结果直接归因成纯记忆质量效应。

输出目录必须不存在，拒绝复用旧答案。每次记录源摘要、完整案例定位/上下文摘要、二进制/adapter/runner/client/scorer/来源锁摘要、模型/端点/输出配置、judge prompt 摘要、单轮作业/commit/publish、实际 Worker relay event 与本地投影、答题和逐项 judge 响应、HTTP 状态/请求响应摘要、顺序调用计数、最终状态及工件摘要。SQLite 保留不可变源文；JSON 工件不再重复复制大段原文，重放需同摘要源数据。执行用代码同时冻结为 code-snapshot.zip 并被 manifest 和 checksums 校验；可用于还原未提交工作树版本。独立 scorer 校验工件和 case/question/arm 唯一性/完整性，重新计算本地分数。摘要是完整性校验，不是签名。

## 评分边界

- **BABILong**：固定官方仓库 commit `7a6efee29f5cac03c3c410e6799c80fd2ffe3610` 的 `babilong/metrics.py`，原文件连 Apache-2.0 许可证与来源摘要存于 `benchmarks/vendor/`。官方函数会对模型输出做首句/标记处理，再匹配任务标签并排除问题中出现的标签；它不是 raw exact-match。工件同时报告 `strict_exact`，并明确没有复现官方 few-shot/prompt 设置，因此只是使用官方 metric 的本地运行。[官方指标源码](https://github.com/booydar/babilong/blob/7a6efee29f5cac03c3c410e6799c80fd2ffe3610/babilong/metrics.py)
- **BEAM**：`beam-rubric-fixed-judge-v1` 按原始每条 rubric 单独调用固定 judge，严格接受 score=0/1 JSON 后求均值。它没有复现完整官方 event-order Kendall tau-b、语义对齐等指标，不是官方总分。来源结构核对官方 commit `b2da22eac88bb0874c64665f13457eb99835774a`。[官方评分实现](https://github.com/mohammadtavakoli78/BEAM/blob/b2da22eac88bb0874c64665f13457eb99835774a/src/evaluation/compute_metrics.py)
- **CorpusQA**：`corpusqa-fixed-judge-v1` 将原 reference（含列表/数值类型）作为整体 criterion，固定 judge 严格二元判分。当前不是 CorpusQA 官方评测器，也没有声称判分准确率或标定完毕。
- 有效答题但判错，与缺答案/请求失败/评分失败分开保存。均值只对有效 scored 项计算，另报缺失项；不把无答案当有效零分。mock 的固定零分不表示模型质量，`quality_claim=false` 和 `official_benchmark_score=false` 保持显式。

## 本轮真实核验

2026-11-07 的 `OUTPUT_ROOT\real-text-mock-frozen-20261107`：CorpusQA-4M 第一例完整上下文 13,160,578 字节、26 WB；BEAM-10M 第一例 53,099,556 字节、104 WB；BABILong-qa3 样本第一例 39,595,283 字节、109 WB。各自源文件 SHA-256 已重新核对。合计 239 压缩、8 答题、6 judge，共 253 次本地 mock 模型步骤；8 个 case/question/arm 全部完成评分接线，独立复评分通过。所有分数为确定性假答案/假judge的零分，不能用于质量或正式实验结论。

正式真实 provider 的兼容性、费用、长上下文容量和模型质量仍待独立运行；LME 多模态、Codex 工程任务评测、生产并发/中断恢复、官方完整 BEAM/CorpusQA 评分仍属后续范围。历史正式实验保持独立，本轮不将这些 Mock 记录分配成旧版本成绩。
