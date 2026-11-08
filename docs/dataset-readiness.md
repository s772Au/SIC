# 测试数据与评分就绪盘点

核验日期：2026-11-07。盘点范围为 `benchmarks/test-catalog.json` 的七个选定配置；不是两个 synthetic demo。所有操作均未请求真实模型或8341测试网关。

## 当前结论

七个选定配置的真实源数据现已全部准备完成。13个锁定主体文件共42,568,507,385 bytes，完整字节SHA-256本轮总验收全部通过；BABILong qa1–qa5共500题完整解析通过；LongMemEval-V2文本与全部轨迹图已通过官方Small/Medium运行时验证。真实文本CLI可用，正式模型、gateway tokenizer和完整官方评测器仍须固定接入；源数据就绪不等于质量成绩就绪。

`python benchmarks/verify_sources.py --hash --all-locked` 已实际运行，退出0；13项均为exists/sizeOk/hashOk=true，原始结果在`dataset-source-verification.json`。脚本仅读取本地清单、文件stat和完整文件字节；无网络、无环境密钥读取、无模型调用。该模式覆盖两份CorpusQA、两个BEAM Parquet、BABILong派生20题及五份完整源、LME正文及两份轨迹图归档；问题、索引及题目图片另做官方checksum和运行时验证。

另实查：两个CorpusQA问题索引各329行；BEAM两个Parquet的实际metadata各5行；LME问题451行。两个控制ZIP已重新生成并通过`zipfile.testzip()`，包含新源验证清单、BABILong500题库存、LME运行时验证及许可/官方metric来源。ZIP是清单、索引和估算控制包，不含大型源数据或完整模型/评分环境。

LME辅助文件新增独立校验：LICENSE、DATA_CARD、SCHEMA、questions、Small/Medium映射和全部29张题目图片，均与官方checksums.sha256一致。固定revision下载的README内容与同仓库checksums.sha256中的README行不一致；这是文档校验差异，须在artifact记录，不把它隐去或误报为所有辅助文件hash全通过。轨迹正文和已列数据辅助文件不受该README差异影响。

## 逐集可用性

| 目录配置 | 实际样本与本地体积 | 本轮证据 | 尚需完成 |
|---|---|---|---|
| CorpusQA-4M | 329问；4份逐字节相同的领域共享上下文；源4,422,636,843 bytes；索引136,228 bytes | Downloads源完整SHA匹配；索引329行 | 官方revision与模型tokenizer冻结；官方judge接入 |
| CorpusQA-10M | 329问；4份领域上下文；源9,787,758,915 bytes；索引136,589 bytes | Downloads源完整SHA匹配；索引329行 | 同上；源`set=128k`不能用来截断或判断长度 |
| BEAM-10M | 10会话、200问；两Parquet合计343,825,539 bytes；库存10会话，每会话20问 | 两文件SHA与官方LFS对象匹配；实际Parquet行数5+5；真实text adapter可用 | 完整官方ability指标、judge与模型tokenizer冻结 |
| BABILong-10M qa3固定子集 | 20个完整记录；803,915,786 bytes；各题独立history | 完整qa3源下载与SHA通过后重新派生，JSONL维持原hash；库存现`sourceIsCompleteOfficialFile=true` | 子集仍只能报告前20题；不能算qa1–qa5正式500题 |
| LME-V2 Small | 451问；2个领域共享history；问题286,186 bytes；轨迹1,195,604,539 bytes；small索引822,632 bytes；29题目图片 | 轨迹、企业/Web图归档完整SHA匹配，图已解压；1,870唯一轨迹、451唯一题 | 官方多模态reader/LAFS接入与冻结，模型tokenizer核验 |
| LME-V2 Medium | 同451问和轨迹；medium索引4,054,244 bytes；按题haystack，不能把全部轨迹无条件提供 | 与Small共享完整本地源；独立medium映射451题ID与domain对应通过 | 同上；逐题token计算与上下文预算，不能沿用small2历史估算 |
| BABILong-10M full | qa1–qa5实际各100题，共500题；20,098,299,256 bytes | 五个文件全部完整SHA通过；500条input/question/target字段校验通过 | 模型tokenizer准入、正式双臂实验；官方prompt设置尚未复现 |

目录版本已升为`2026-11-07.2`；高资源配置标为`local-source-verified-evaluator-pending`，并保留`gateway-tokenizer-unverified`。低/高控制包均记录本轮源验证日期和CLI覆盖边界。历史包与旧run保持原code snapshot，不把新数据准备状态追写成旧成绩。

## 完整哈希与路径

下表13项均已在本轮完整重算通过。绝对Downloads挂载路径不随Git/ZIP分发，迁机必须重新挂载。

| 文件 | bytes | SHA-256 | 状态 |
|---|---:|---|---|
| `USER_HOME/Downloads/4m_4domains.jsonl` | 4422636843 | `a3e1ce956bfc40f447e582bd2ea2e23730fe725b91cfaf1e529451a96f6efd70` | 本轮通过 |
| `USER_HOME/Downloads/10m_4domains.jsonl` | 9787758915 | `1d6656260ae47ad84058871689f70339d37deef84cfafad5fe33e3c66f06acb4` | 本轮通过 |
| `benchmarks/data/beam10m/data/10M-00000-of-00002.parquet` | 153844664 | `31d96fd47ec56221d202e68792f26c00e49467dd4b36ee105c36ebd19ef78ad5` | 本轮通过 |
| `benchmarks/data/beam10m/data/10M-00001-of-00002.parquet` | 189980875 | `a4f13fe25af51d57405ae41008689c31d1421377f3efde56a024b441deb2ee65` | 本轮通过 |
| `benchmarks/data/babilong/qa3-10m-first20.jsonl` | 803915786 | `87f48469d373c07d3d940a26c39fe3052546ab0a0820ed950a166e4a951f9a92` | 本轮通过 |
| `benchmarks/data/longmemeval-v2/trajectories.jsonl` | 1195604539 | `363cec9a8e87aa8d9101ce4e600aadbf7031d674056ebe4f969e8424abc5f3c6` | 本轮通过 |
| `benchmarks/data/babilong/data/qa1/10M.json` | 4018016805 | `41691cba6b804d5b973bfff0bed92df9165f1716cc3c3ac6433ae9529a881118` | SHA与100条结构校验通过 |
| `benchmarks/data/babilong/data/qa2/10M.json` | 4019960532 | `9ff8c0bb749908e1e2daa4ce624c8aaacbcb3bcc9064f64f2769d7be65e55336` | SHA与100条结构校验通过 |
| `benchmarks/data/babilong/data/qa3/10M.json` | 4025904013 | `6a3d703f32ba18fa8d562b8581f025ddc49699817a3a6633bbeb9cc356c7d434` | 19:48完整下载与整文件SHA通过 |
| `benchmarks/data/babilong/data/qa4/10M.json` | 4016999567 | `0ae3f921d7fa4af250e153c89fa6d38c45946c22c5f6425608978d9ed1a989ab` | SHA与100条结构校验通过 |
| `benchmarks/data/babilong/data/qa5/10M.json` | 4017418339 | `833b3c706bde3c7c74be3b5826a35c3efce1c253b07044af8e0c6cff8534d9ce` | SHA与100条结构校验通过 |
| LME `trajectory_screenshots/enterprise_screenshots_base.tar.gz` | 3354163660 | `5c4a67ae0856aa1ede9b040e7da7c7a2d0b76fdd6344ef87380bcdf9f4b6d7a3` | 19:46下载完成，整文件SHA通过 |
| LME `trajectory_screenshots/web_screenshots.tar.gz` | 2562302847 | `68699c6842412e09a6f89d3c05c5ae8813275918002b52d82dec43ab24dd01fb` | 20:00下载完成，整文件SHA通过 |

源revision实查：BEAM `9b2096193fe74e2837e4713e483351e19817773c`；BABILong `ee0d588794c7ac098062ee0d247c733d62e94fe2`；LME-V2 `f152293e235517d504809563c833d7190b8c713b`。CorpusQA本地文件身份由上表hash固定，尚无这两份ModelScope文件与官方仓库commit的独立对应证据，不编造revision。

## 许可证与官方评分入口

| 集合 | 已核对许可 | 评分/接入入口 |
|---|---|---|
| CorpusQA | 官方代码仓库标MIT；源文档可能有各自权利，保留原数据来源，不把代码许可直接当作所有第三方文档授权 | [官方仓库](https://github.com/Tongyi-Zhiwen/CorpusQA)：`src/infer.py`答题、`src/eval.py`语义judge。需冻结代码commit、judge配置和空答案语义 |
| BEAM-10M | 官方HF卡`cc-by-sa-4.0`，派生分发应保留归属及相同方式共享义务 | [官方数据卡](https://huggingface.co/datasets/Mohammadta/BEAM-10M)：10类能力；当前真实text adapter与本地固定rubric judge可用，完整官方Kendall/语义对齐等指标仍待接入 |
| BABILong | 官方代码Apache-2.0；混合源PG-19为Apache-2.0、bAbI为BSD；HF卡未提供机器可读单一license字段，应保留混合源说明 | [官方metric固定源码](https://github.com/booydar/babilong/blob/7a6efee29f5cac03c3c410e6799c80fd2ffe3610/babilong/metrics.py)已连许可与摘要收录vendor；使用官方label-match并附strict_exact，没有复现官方prompt设置 |
| LongMemEval-V2 | 本地官方DATA_CARD及HF卡为Apache-2.0 | [官方仓库](https://github.com/xiaowu0162/LongMemEval-V2)：`data/prepare_data.py`、`data/validate_data.py`、`evaluation/harness.py`和`leaderboard/` LAFS；按官方Memory接口实现`insert`/`query`，不向query暴露gold/question metadata |

## 正式实验接入顺序

1. 本轮源准备已完成：13个主体SHA、500条BABILong记录、LME全部运行时截图引用通过。保留README单一文档checksum差异记录；`.part`与`.incomplete`不被当作数据源。
2. 冻结每集官方代码commit、数据revision、subset IDs、license说明与输出schema。当前BEAM/CorpusQA/BABILong已有真实逐案text adapter、只读计划和本地评分接线，见`real-text-experiments.md`；LME仍需官方Memory适配器。继续离线校验顺序、共享上下文、gold不泄漏及全部选题覆盖，不能只用两个synthetic示例代替。
3. 以专用实验模型tokenizer实测每条独立历史，保留源token数；入选门槛为>2M，不能把累计API input、文件名10M或反复拼接前缀计为源规模。128Ki WB估算是规划，不是token容量准入。
4. 冻结baseline/SIC两臂、模型、prompt/hash、输出预算、重试和人工promotion政策；先一题每臂配对验证完整管道，再正式调度。长源单请求改为顺序SIC导入时标注SIC-adapted。
5. BABILong已冻结官方metric；BEAM/CorpusQA当前为本地固定judge，完整官方评分与标定仍待对齐。用正确/错误/空答离线验证语义，模型judge仅在明确的正式实验配置下执行。LME需要多模态reader及独立LAFS工件，text-only不得冒充官方多模态成绩。
6. 记录实际出站payload digest及可回放引用、来源revision、WB/draft/commit/publication/roots/mount IDs、usage/cache字段、cost、wall-clock和失败分类；完整答案与评分收齐才生成成绩。官方UI测试和hash通过不等同于模型质量实验。

本轮下载使用固定官方revision；Hub尝试显式`token=False`，最终使用有实际进度的断点HTTP下载。下载器逐段严格核对Content-Range起止/总长，完成后整文件SHA匹配才改为正式文件名；同尺寸但hash错误会被拒绝。数据下载与文件hash不触发付费模型。

完整BABILong qa1–qa5共500条严格JSON流式解析与字段校验通过；各任务input字符范围见`benchmarks/prepared/babilong/full-inventory.json`，字符数不是token数。企业截图完整归档安全流式解压35,391文件、3,548,206,703未压缩bytes到`screenshots/`。

Web完整归档解压14,317文件、2,657,251,967未压缩bytes。重新扫描完整1,870轨迹：企业34,292与Web14,317个截图引用全部对应本地文件，缺失为0；两档haystack各451题的轨迹ID、重复检查和domain匹配均通过。完整官方多模态数据本地可读，正式reader/judge/LAFS仍须按前述冻结接入。

官方验证也已实际运行：LongMemEval-V2代码仓库commit `2cc8c540bdb87fe6761629b585e727e1c4704520` 的原始`data/validate_data.py`与`data/public_data.py`，Small及Medium两次默认`--check-screenshots`均退出0，各报告451 questions、1,870 trajectories、451 haystack_questions。代码SHA与结构检查结果保存在`dataset-validation.json`；这证明本地数据布局通过官方验证，未执行reader、judge或LAFS评分。
