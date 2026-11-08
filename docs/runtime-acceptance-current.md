# SIC v0.1 当前实现与验收事实

2026-11-08。公开v0.1预发布对应当前Python Core、官方工作台源码和[中文预印本v1](../paper/sic_paper_zh_v1.md)。原创软件MIT，论文独立CC BY 4.0；Zenodo pending，不虚构DOI。旧Rust与自制WebUI为legacy，旧实验结果不进入新实现的论证。

## 构建与真实宿主证据

- 发行构建的Core回归273项，0失败/错误/跳过；集成单元38项。报告按源码摘要限定适用范围，数量不折算为73功能全部通过。
- B生产接线单独有115相关测试及实际LiteLLM/OpenAI SDK回环证据：3次请求、3378字节共同前缀、相同公共工具schema、Editor越权工具拒绝、发布回放一致。真实物理缓存命中仍unknown。
- 旧216构建的16类真实DSH故障场景、17次peer尝试以及独立原生/数据库复核保留，但没有自动变成273构建的完整重跑。旧GUI代表验收同理。
- P/V回传政策冻结，首次通信到达和收据可追溯，重送不改原时点。真实收取发生在接收者实际可见的CB发布窗口。Worker保留原生Harness输入及合法工具/媒体序列，追加可见层级的真实续接WB，不把高阶成员快照重复当新材料。
- 当前支持程序、单轮Editor、可修订候选和固定发布。晚于Editor冻结快照的tool结果可用有来源记录的program KEEP补入当前CB；不保证同一阶Editor看过并语义编辑全部晚到材料。

## B布局与官方工作台

默认布局仍为`separate`；`shared_i`是显式可选项。共享序列以Worker SIC教学为主体，附相同CB链与稳定公共工具目录，后置I携带宿主可信模式、边界、范围和激活视图。模式文字不能授予工具或编辑权限；实际能力和当前撤权由宿主检查。不兼容身份、预算、上下文或媒体时显明回退完整A布局，不牺牲载荷来制造相同前缀。

既有Langflow/Studio代表路径已经在其记录构建上真实查看或编辑：候选op/PVI与校验发布、层级和片段导航、决策/公示牌、角色配置、媒体/通信页、selected Editor表及Unicode只读投影。此范围不是所有写动作或所有73组合。

以下B新增域目前只有源码与隔离SDK/API证据，**尚未完成常驻加载及真实GUI验收**：

- Langflow的B配置/预算/逻辑身份及实际layout表，plan/request分页，Editor layout/alias和raw/canonical引用区分。
- Studio的B配置typed表单/审阅恢复、plan/request及预取页类型、Editor prefix/alias安全表。

已通过6项新增隔离专项：真实临时Core API配置与plan冻结、官方LFX构建/运行、Studio SDK interrupt/resume写回、完整分页、fallback、未知/跨路线拒绝、敏感元数据隔离。没有模型调用。准备计划不是实际请求；实际引用来自attempt.request_id。发布前raw正文只提供不可变响应引用，发布后proposal页可完整对照；capability不可从表单修改。

## 真实网关探索与研究准备

真实DSH使用授权测试网关，返回模型别名不证明物理身份。g程序KEEP对照3例机械通过、2例exact；h分离CP/WP的SIC A方案2例机械/exact通过、1例越scope失败未评；i共享布局8次尝试/3CB的独立wire审核已完成：共享消息/工具序列成立，但Editor无效JSON导致任务失败，质量未评。见[逐轮记录](dsh-live-results-20261108.md)。不同完成分母、版本、路由和提示不能推出SIC优势。费用未定，网关内部重试数未知。

七控制、独立答案评分与三类噪音标注可执行。LLMLingua真实CPU权重前向、离线换路径恢复及最终Worker消费已验证。官方LME Memory/processor/media通路已有真实数据机械证据；这不是正式LAFS任务得分或真实模型质量结论。数据和权重独立准备，不随主软件包捆绑。

## 剩余边界

物理模型/tokenizer/价版本的正式受控比较、真实cache收益、质量与费用优势未评。API ingress仅覆盖明确测试的Chat/Responses/Anthropic文本与工具子集及静态inline图像；图像需已知显式计数，未知政策、auto detail、远程抓图、音视频拒绝。Provider出站仍为canonical Chat/LiteLLM，Chat桥SSE缓冲；原生Responses/Anthropic HTTP代理及逐token转发未实现。

安全撤销是访问/依赖读取封锁，不声称物理擦除所有SQLite副本和备份。源包/软件分发不含私有工作稿、原始会话日志、凭据、数据库、视频或大数据权重。新纸稿以公开预印本单独发行，第三方资产保留独立许可。

逐项来源、原证据摘要及新版构建范围见[功能验收账](functional-acceptance.md)。
