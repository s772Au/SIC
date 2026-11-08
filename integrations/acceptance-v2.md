# SIC v2 官方交互验收

> 旧R01–R65基线已失配，下文旧覆盖数只保留历史证据，不能证明新73项契约完备。新双链/N阶/Editor私有上下文工作台及本轮分层证据见[hierarchy-workbench.md](hierarchy-workbench.md)；新域Langflow/Studio GUI尚未验收，既有候选GUI成功不能外推。

本轮依据 `paper/review/sic_implementation_contract.md` 的 R01–R65，区分核心动作、官方组件、官方服务 API 与实际 GUI。旧 Rust 成绩、旧稿实验不作为本设计证据。逐需求状态来自 `requirement-coverage.json`，界面中可用 SIC R01–R65 验收与缺口查看；动作数量不等于需求全部完成。

## 当前真实运行

本轮隔离服务：Core `http://127.0.0.1:18888`，同实例 DSH bridge `http://127.0.0.1:18890`，官方 Langflow `http://127.0.0.1:17861`，官方 LangGraph API `http://127.0.0.1:12026`。旧服务端口保留。fixture 使用本进程拥有的回环模型，不接真实 provider。

`manage-runtime.ps1 -Action Status` 核查端口、项目进程归属和健康；`-Action Start -Mode Fixture` 以 Hidden 启动本轮服务，遇到非本项目占用即拒绝，不停止旧进程、不改系统开机设置。身份配置留 `.local`，不放画布或公开报告。

## 已执行的证据

| 层次 | 真实结果 | 私有报告名 |
|---|---|---|
| 官方 SICAction → Core HTTP | 当前 58/58 动作全部执行，7 项拒绝边界；包括真实分身工作/编辑/发布/回传、候选 CAS、双索引和恢复 | v2-acceptance.json |
| 官方 LFX 导出整图 | 原生程序链 11 节点、双 Editor 链 12 节点真实执行；7 次自有模型 HTTP，真实 provider 0 | v2-acceptance.json |
| 语义组件 | 12 种组件真实调用：正文/协议、候选版本、四视角、档案页、配置、路线、公示牌、Editor 预取、运行、账目和缺口表 | workbench-acceptance.json |
| 候选工作台整图 | 实际 7 节点导出加载执行；已交付候选修改后进入固定 CB，P 生效；整图不再调用模型 | workbench-flow-acceptance.json |
| 官方 LangGraph SDK | 新候选图实际持久 checkpoint、interrupt 修改与 resume 生效；原始响应不变，固定 CB 不变；7 个导航方向、11 个域事务和账目含真实协作 | workbench-graph-acceptance.json |
| 官方常驻 Langflow API | 10 份语义flow真实导入读回；保存typed字段后实际7节点整图执行，候选正文与P进入固定CB；无重发模型 | workbench-langflow-api.json |
| 官方常驻 LangGraph API | 9个语义图注册；真实持久线程三次interrupt/resume、15条history、候选/PVI修改生效；三项历史改写拒绝 | workbench-runtime-acceptance.json |
| 同Core可重放绑定 | 两次独立重放：Langflow发布→LangGraph同链读取/修改/发布→Langflow读回两CB与来源/协议/同调用账；旧CB与原响应不变 | runtime-replay/replay-*/result.json |
| 首次真实DSH链只读观察 | 根代理提供的真实工具往返链：session绑定匹配，两个官方runtime与18888的3个固定CB、全部正文/op及4条调用账一致；观察0Core写/0模型请求 | 同上私有真实chain观察报告 |
| 最终DSH两会话及重启 | 两独立真实会话均有3CB/5调用账并跨runtime一致；Core重启后未继续会话与旧报告的chain/cut/CB全归档SHA/调用数量一致；同session续轮由根代理CLI完成后，5CB/8调用账也同时到达两个runtime | runtime-replay私有基线与重启后报告 |
| 官方Langflow GUI代表路径 | 根代理实际在七节点画布修改typed content/reason/P/V/I、打开节点2/4写门、保存后只Run一次；Inspect Output看到revision1固定发布及CB↔WB/source对应。独立只读DB核对全部修改与不可变原响应通过 | langflow-gui-publication.json及根代理截图 |
| 新设计实验基础 | baseline 一次压缩、SIC 增量，两臂自有真实 HTTP；冻结 17 工件，独立哈希/账目/历史不变量校验通过；独立 scorer 输出 not_evaluated | 独立 experiment run 目录 |

报告只存 `.local/integration-test-results/`，不发布链/线程/flow ID。模型 fixture 仅验证协议与执行路径，不能证明质量、真实费用、物理缓存命中或生产速度。

## 官方 GUI 与当前缺口

Langflow 已建立新的七节点专用 GUI 入口：模型已交付 revision 0，候选节点可用正文、理由、风险、生命周期和 P/V/I 字段修订；校验与发布另设节点，写门默认关闭。该入口与 DSH 会话及旧四节点未提交提案分开。具体本机 URL/上下文只保留私有报告。

新 Studio 图包括 `sic_candidate`、`sic_memory`、`sic_decision`、`sic_routes`、`sic_runtime`、`sic_configuration`、`sic_editor`、`sic_source`、`sic_accounting`。候选/协作/配置有明确表单、只读事实与写前 interrupt；resume 只接受域表单，不可替换 chain/job、原始响应或固定 CB。`sic_core` 保留高级动作编排。

新语义图已在本轮常驻 LangGraph 中注册，官方 API 持久运行通过。**Langflow候选修订/固定发布/来源映射代表路径已实际GUI通过；其他域的GUI操作不能据此全部划过。Studio GUI仍未验收。** 根代理打开Studio时遇getTab超时，随后Computer Use因无法可靠确认浏览器URL强制停止，没有换技术绕过，也没有以API测试代替Studio。官方入口是 `https://smith.langchain.com/studio/?baseUrl=http://127.0.0.1:12026`；可能要求账户登录。`12026/docs`只是API文档。最终Core由根代理核对owned进程后重启，重启前后绑定及固定档案已跨runtime实际比对通过。

真实API验收发现并修正两项部署问题：Langflow缓存早期客户端导致candidate revision未继承，已仅重载本轮实例；LFX1.13.1按status错误分类Table展示artifact，现由自有组件基类把展示artifact转JSON records，保留官方Table边与输出，不修改第三方库。已用常驻整图复验。

R57–R59 的真实 DSH 插件/宿主事件与同链观察由独立联调覆盖；前端不能仅凭 bridge 监听就宣称 DSH 全通。R64 的候选 CAS、幂等与恢复入口已有机械证据，进程中断/迟到/失败重试需保留对应专项证据。R65 的本地 fixture runner 已可用，真实模型质量 scorer 与正式实验尚未实施。

首条真实DSH正常工具往返已由根代理确认，同链只读API观察已通过；该轮toolcall文本为空，没有可编辑WB进展文字，**不能据此声称工具期发生Editor模型重叠**。之后带实际进展文字的两条真实fixture会话及重启/续轮的跨runtime观察已完成；Editor与真实工具窗口重叠须依独立严格时序核验，不能由调用数量增加推定。可重放入口见[常驻绑定说明](runtime-replay.md)。

上一轮自动启动曾被 `blocked by policy` 拒绝，本轮经新授权在独立端口成功启动；该历史拒绝不是当前服务断连原因。本轮没有停止用户旧服务，也没有以测试隐藏部署动作。
