# 上下文编辑 prompt 与引擎调研（新机制基线）

本目录是F73的新研究准备，不含旧E1/E2结果，也不宣称真实质量已测。`sources.lock.json`固定2026-11-08读取的公开上游commit、文件、链接、字节数与SHA256；`profiles-v0.1.json`冻结独立编写的SIC Editor prompt、Worker教学和对照计划。现已补可执行控制包与独立评分/人工标注接口；LLMLingua已完成真实本地权重执行；官方LME媒体接入另行跟踪，不把fixture当研究质量。

## 已读来源与对本机制的启发

- [Pi compaction](https://github.com/badlogic/pi-mono/blob/6e2222a8dda06e3d69fad46d630f152f5873567a/packages/coding-agent/src/core/compaction/compaction.ts)：首次与更新摘要分开；目标、约束、进度、决策、下一步与精确标识有明确位置。适合普通摘要控制。SIC需把这些信息变成按来源定位的编辑，明确失败/提议/已执行的区别，而不是反复整体重写旧CB。更新时保留信息的倾向也不能直接解释成所有事实永不变。
- [OpenCode compaction](https://github.com/anomalyco/opencode/blob/4ada59972a9f4788222422fc7db38f86f6a6f1e7/packages/core/src/session/compaction.ts)：区分当前工作状态与后续动作；其提示把可重新获取的当前环境与需传递的对话事实分开，并存在后台摘要及保留尾部机制。SIC可采用恢复来源而非重复全文的思路，但不能照搬其策略宣称所有harness均只有阻塞压缩；比较时需冻结后台/尾部配置。
- [LLMLingua PromptCompressor](https://github.com/microsoft/LLMLingua/blob/01b9e9481d7395e1e83a48df4a062b11f6304325/llmlingua/prompt_compressor.py)：公开模型辅助的prompt压缩实现。它属于模型辅助/token选择一类，不是纯程序基线；代码MIT许可不等于所选权重、训练数据、运行数据均可任意使用。需要另锁模型与tokenizer，再测对Worker任务的影响。
- [LangChain trim_messages](https://github.com/langchain-ai/langchain/blob/d9caf083577c43a91c5bf6bd1f7dfc289581184d/libs/core/langchain_core/messages/utils.py)：非生成式预算裁剪，可配置保留首尾、system、部分消息及消息类型边界。比较应禁用随意拆开工具配对，明确token计数来源；它不提供语义改写和递归WB/CB结构。
- 本项目现有`editing.program_ops`提供精确去重、空文本去除和换行正规化。确定性不等于语义无损：重复文字若承担不同时间或状态证据，规则适用性必须另核；协议sidecar与来源不能跟随删除。

这些仓库代码许可证均读到MIT；完整上游源码仅保存在本地研究缓存，不复制整段上游prompt进入本文件。本文为独立分析，没有将上游报告的收益当作SIC实验结果。

## 新prompt为什么与普通摘要不同

Editor拿到的是已解析文本单元、只读协议/来源、任务和已有上下文，而不是任意修改原始LLM请求。产出是有序七操作与P/V/I，未提及默认KEEP。编辑的语义目标以Worker完成实际任务为准：事实与猜测、失败与未尝试、工具提议与执行均需区分；精确细节可沿源锚回取。规则也适用于高阶WB，创建新CB而不改写下阶对象。Worker教学同时说明如何使用这些表示和何时回取细节。

该prompt目前是冻结的研究候选，尚未宣称优于当前默认prompt。接入时可通过现有`config.update`的editor_prompt与worker_prompt字段显式配置；不同条件须各自建链，不在同一条既有实验链中途替换prompt后声称变量受控。

## 本轮可执行包

`pack/control-pack.json`冻结六个条件：同Editor一次末尾压缩batch、SIC逐块、独立编写的harness结构摘要、程序精确规则、固定`langchain-core==1.7.6`的整文本单元末尾裁剪、不编辑。裁剪使用Unicode码点预算，**不是计费token**，不拆原生tool协议；它不是宣称重现上游harness完整调度。程序/裁剪/不编辑条件不调用Editor模型，仍保留各条件实际Worker模型调用和程序资源。`editor-response.schema.json`与Worker教学/PVI/默认KEEP/双链递归导航一起冻结；`dependencies.lock.json`记录实际导入版本、模块SHA及许可元数据。

```powershell
# 默认自有loopback fixture，不连接任何既有Core/网关/真实provider
.local/langflow-ui-venv/Scripts/python.exe scripts/run_compression_research.py --output .local/research-v2/NEW_RUN
.local/langflow-ui-venv/Scripts/python.exe scripts/score_compression_research.py .local/research-v2/NEW_RUN --output .local/research-v2/NEW_SCORE.json
# 为各阶候选导出独立人工span/任务rubric标注表；填写后另存并评分
.local/langflow-ui-venv/Scripts/python.exe scripts/score_compression_research.py .local/research-v2/NEW_RUN --annotation-template .local/research-v2/ANNOTATIONS.json
.local/langflow-ui-venv/Scripts/python.exe scripts/score_compression_research.py .local/research-v2/NEW_RUN --annotations .local/research-v2/REVIEWED.json --output .local/research-v2/REVIEWED_SCORE.json
```

每个条件最终编辑/递归维护后，统一提交同一冻结final-question并实际调用Worker作最终回答；评分只使用该`final_answer`，账务包含此调用。`final-request.json`保留不可变请求引用、实际可见CB及活动阶次，验证编辑结果确被消费。最终问答留下的新pending尾明确保留不再编辑，不为凑pending0增加无意义压缩。中间回答仅作轨迹，不作为终态编辑质量证据。

每个输出目录必须新建；schema为`sic-compression-research-run-v2`，不读取旧E1/E2/旧runner结果。manifest锁任务、prompt包、源锁、代码快照、角色配置和实际轨迹；独立validator重算每臂账务和对照差额，检查旧CB前缀不改写和非LLM编辑条件的Editor调用确为0。缺价/缺程序或存储价格时总费用unknown，不补0。FIXTURE输出即使填写标注仍不生成任务质量分。

固定provider仅由**显式**`--provider-config PRIVATE_CONFIG --allow-provider`接入；示例在`pack/provider.example.json`，须填写两个角色固定model/revision、api_base、tokenizer/容量预算、temperature/top_p（可另锁seed）及密钥环境变量名。请求无自动重试或静默fallback，URL内凭据始终拒绝，8341默认拒绝；作者显式授权的测试网关须同时提供`--allow-provider --provider-config PRIVATE_CONFIG --allow-test-gateway`。此窄开关仅允许该端口，不放宽sampling、环境凭据或容量约束。`public_config`、configuration及manifest将该路线标为`nonfixed-exploratory`、`quality_generalization:false`和`model_identity_attested:false`；配置中的别名/声明revision不是实际固定模型证明。原schema的`mode: fixed_provider`仅表示显式配置分支，实际身份边界以这些字段为准。真实值不进包。通过Core公开provider注入接口冻结sampling，实际SDK请求带该参数。revision为作者声明，未冒充provider已证明固定快照。可另给`--prices PRIVATE_RATE_JSON`，采用Core accounting原类别与版本字段；未给价仍可执行但成本不完整。**本轮没有调用真实provider。**

## 数据与块长/阶次

`benchmarks/core_adapters.py`复用现来源锁和私有source resolver，支持CorpusQA、BEAM、BABILong qa1–qa5及固定qa3子集，将完整历史无损分块成新Core任务。Worker只拿原材料、保护指令和问题tail，参考答案/rubric只入独立评分材料。运行时用`--dataset babilong-qa1 --chunk-bytes 32768`选择一条完整源记录，**不会以头几块代替整条历史**；真长任务可能产生大量调用，先检查私有准备信息和预算。默认只选首题；全量任务应逐任务新目录运行。固定provider输入预算超限明确失败，不偷偷截断。

用不同`--chunk-bytes`生成同源块长条件，输出`level`供分层噪音统计。可加`--hierarchy-fanout 2`在每次发布后实际程序递归维护高阶，导出各阶真实编辑正文和op；该开关改变工作流，不能再声称只变化缓存。默认task JSON也可直接给作者自己的固定块边界。

`prepare_compression_research.py`复用2026-11-07全13文件SHA核验，**本轮仅重查存在与尺寸**；该区别写入`pack/data-readiness.json`，没有宣称重新hash42GB。公共包仅相对路径，不复制本机数据、私有override或运行ID。`SIC_SOURCE_OVERRIDES`显式JSON优先，其后DATA_ROOT、原工程`.local/source-overrides.json`、公共相对路径；干净展开无私有配置不会回落原工程。既有高/低控制ZIP已独立CRC/SHA校验。新`benchmarks/packages/sic-research-controls-v2.zip`带可执行源码与冻结包，不带数据或旧运行结果。

## 独立评分与三类噪音

`pack/rubric.json`与`scripts/research_scoring.py`分别处理无依据事实新增、来源失真、编辑格式漂移。人工标签绑定源与候选SHA，定位候选span、来源证据及理由；明确标为建议的新想法单列，不自动当幻觉。语义两类分母是已判断事实claims，格式分母是已判断规则检查；未知/未覆盖单列，不补0或冒充穷尽。

事实短答案可做本地normalized exact（明确非官方得分）；其它任务使用独立人工criteria：事实找回、决策续做、跨阶真实追溯、真实协作、资源取舍。每个pass/fail须定位已校验运行artifact和证据；缺评/无答案保持unknown/not_evaluated。rubric格式通过不意味着语义正确，人工标签也不由代码自动证明。

## 明确缺口与待作者新实验

- LLMLingua现已实际准备和执行，详下节；真实任务质量仍未评价。
- 官方LongMemEval-V2准备器`benchmarks/prepare_core_lme.py`现在逐项锁原record和截图SHA；`core_lme_carrier`把其原顺序文本+图片送新Core。研究runner已接完整媒体任务，官方Memory/harness/LAFS则由`benchmarks/run_core_lme.py`及其冻结上游接口执行；正式模型/任务得分仍未评价。
- 真实provider任务质量、公开方案优劣、安全阶数/最优块长尚未评价。后续作者固定provider、价格版本、任务和独立标注，产生新结果；preparation、mechanical_acceptance、research_evaluation分列。

## 官方LLMLingua-2真实本地控制

本机另建`.local/llmlingua-venv`（Python3.12.16 x86_64，Windows ARM64仿真可运行），没有向Core/LFX环境安装模型依赖。LLMLingua0.2.2、Transformers4.44.2、CPU PyTorch2.5.1、Accelerate0.34.2及全部34项依赖固定于`requirements-llmlingua.txt`与带PyPI SHA的`llmlingua-requirements.lock.txt`；实际包元数据与模块身份在`pack/llmlingua-runtime.lock.json`。

官方Microsoft mBERT较小权重固定revision `5f0c82792b7ea14c6484e015b6a072009496b7f2`，709,388,104字节safetensors的SHA为`22b9ecde52fec5c97e8c54a293be768727df95a81c6c8dccb03f262a50c58324`。官方卡声明权重Apache-2.0，LLMLingua代码MIT，完整条文和来源分别保存在`pack/llmlingua-licenses/`与对应JSON。模型tokenizer/config/vocab、独立cl100k_base计数器资源也逐文件锁SHA；后者仅是上游压缩前后计数，不是provider计费。

```powershell
# 独立环境；明确x86_64解释器，保持现Core/LFX依赖不变
uv venv --python cpython-3.12.16-windows-x86_64-none .local/llmlingua-venv
uv pip install --python .local/llmlingua-venv/Scripts/python.exe --require-hashes -r research/compression/llmlingua-requirements.lock.txt
# 公开官方固定revision下载，断点保留、Content-Range与最终SHA核验，无需HF token
.local/langflow-ui-venv/Scripts/python.exe scripts/prepare_llmlingua.py
# 新Core控制条件；真实本地权重压缩，Worker仍默认自有fixture
.local/langflow-ui-venv/Scripts/python.exe scripts/run_compression_research.py --engines llmlingua --output .local/research-v2/NEW_LOCAL_MODEL_RUN
.local/langflow-ui-venv/Scripts/python.exe scripts/score_compression_research.py .local/research-v2/NEW_LOCAL_MODEL_RUN --output .local/research-v2/NEW_LOCAL_MODEL_SCORE.json
```

LLMLingua使用2个CPU线程、单批、float32、seed42、rate0.5、保留数字和声明标点，不是按测试答案选force token。严格离线加载已校验safetensors，禁用remote code；设置真实官方逻辑model ID供其原生word-boundary算法分派，未修改第三方引擎或替换模型。分类器张量与文件逐值核对，模型forward hook记录实际logits/shape/次数；缺环境/权重、SHA错误或模型异常均拒绝，无规则/fixture替代。

真实输出合并表示映射为第一个单元COMPRESS、其余单元DELETE，原始存档与协议sidecar仍只读。证据`compressor-evidence/llmlingua-*.json`包含源、原生结果/word labels、CPU/内存与版本；本地模型CPU作为独立程序资源入账，不假冒免费，也不伪造provider token usage。Core最终Worker请求必须实际带发布后的同一压缩文本；fixture回答仅证明后续消费机制，语义质量仍not_evaluated。

可移植模型资产单独在`benchmarks/packages/sic-llmlingua2-mbert-model.zip`（权重、tokenizer缓存、许可、依赖锁，不含私有配置）。它未塞进轻量控制源码ZIP。解压到任意目录后可用`SIC_LLMLINGUA_MODEL_DIR`、`SIC_LLMLINGUA_TOKENIZER_CACHE`、`SIC_LLMLINGUA_PYTHON`显式指定；所有检查保持fail-closed。包SHA与恢复布局见`pack/llmlingua-bundle.json`。

## 新Core媒体研究入口

```powershell
.local/langflow-ui-venv/Scripts/python.exe benchmarks/prepare_core_lme.py --tier small --output .local/research-v2/LME_TASK.json
.local/langflow-ui-venv/Scripts/python.exe scripts/run_compression_research.py --task .local/research-v2/LME_TASK.json --media-data-root DATA_ROOT/longmemeval-v2 --engines program_exact llmlingua --output .local/research-v2/NEW_MEDIA_RUN
```

第二条是完整所选haystack的真实工作量，不能把极小机械fixture当它的成绩；执行前固定预算，超限拒绝，不丢图或改小常量。研究循环在编辑期间只用通用记忆目标，不让下游问题提前影响材料；同一最终问题及其截图在最终编辑之后才送Worker。参考答案/评估函数不进任何模型请求。LLMLingua只压可编辑文本，图片原始字节/role/tool骨架由Core不可变sidecar保留；独立validator重读最终request核图片SHA。fixture媒体配置只在拥有的loopback端点声明已知gpt4o计数策略，不是调用真实GPT-4o，也不作真实usage/质量证据。

本入口是可复验的研究控制工作流，不冒充完整官方Memory/LAFS评估；正式接口用`benchmarks/run_core_lme.py --official-root OFFICIAL_CHECKOUT ...`并通过`lme-official-source-lock.json`验证原文。论文得分必须来自作者以后固定provider和独立评估。


## 显式测试网关（新增授权入口）

```powershell
# 仅在已授权网关、私有profile与环境凭据准备后执行；此说明不是已执行证据。
.local/langflow-ui-venv/Scripts/python.exe scripts/run_compression_research.py --task .local/research-tasks/diagnostic.json --engines unchanged batch sic --provider-config .local/research-provider.json --allow-provider --allow-test-gateway --output .local/research-v2/NEW_EXPLORATORY_RUN
.local/langflow-ui-venv/Scripts/python.exe scripts/score_compression_research.py .local/research-v2/NEW_EXPLORATORY_RUN --output .local/research-v2/NEW_EXPLORATORY_SCORE.json
```

示例任务/profile路径须先实际准备并冻结。当前默认任务是机械示例，其goal与短答案标签不足以作为正式质量任务；首个真实微型诊断应显式固定可回答的问题、严格答案格式和独立reference，再选择该task文件。N个输入块每臂为N+1次Worker；batch为1次Editor、sic为N+1次Editor（含终态压缩），因此unchanged+batch+sic合计4N+5次模型请求，不计模型主动工具续轮。输出预算须在私有profile逐角色固定。

此研究运行器通过新Core调用模型，不是DSH Harness。现有`run-dsh-harness.ps1`是隔离fixture接入检查，硬限制自有三端口同进程及fixture工具/标记，不能冒充已授权真实Provider实验。真实DSH应另用相同冻结任务和SIC插件接同Core桥，保存原生session、最终回答/请求引用/CB与收据后独立评分；不能把两个运行器的账混为同一实验。
