# SIC v2 官方 Langflow / LangGraph 控制面

当前逐功能状态见[F01–F73实施验收账](../docs/functional-acceptance.md)，新增[任务总账](../docs/task-ledger.md)与[真实角色前沿对照](../docs/frontier-comparison.md)。源表单、隔离SDK、常驻API、浏览器GUI分别记录，不按节点数量判完成。当前专用Studio最终组件已加载：selected Editor各表及media/communication完整分页在官方GUI实测通过，无这些控件的重载待办；证据`GUI_STUDIO_FINAL`及SHA见[验收账](functional-acceptance.json)，不声称73项全部GUI通过。

双链、N阶真实编辑及Editor私有持续上下文的新语义入口见[递归工作台](hierarchy-workbench.md)。旧R65覆盖表不是新73项完备证明；新域GUI验收单独记录。

新流程绑定Python `sic_core`，不是旧Rust alpha API或自制网页。核心可嵌入harness/provider，本机HTTP仅是控制面传输方式。机制与服务配置见[核心指南](../docs/core-v2.md)。

## 编辑范围

可编辑的是新工作材料、未提交op/PVI提案及宿主允许的profile、教学、规则和预算。**已发布CB、随CB固定的有序op、原始存档及协议sidecar只读。** 更正通过新P/V/I影响后续工作。安全撤销须独立宿主授权，阻断材料及依赖视图访问，不改不可变字节。

SIC Core Action真实访问`GET /api/v2/capabilities`和`POST /api/v2/operations`。目录缺项显示`capability_missing`，连接失败显示`core_unavailable`，写/模型开关独立。前步失败默认阻断下游。结果包含核心事实与editable_scope；HTTP成功不证明LLM质量或物理缓存命中。

## 安装和启动

从`PROJECT_ROOT`执行。独立Python3.12 x64环境避免Windows ARM64依赖缺wheel，不替换系统Python：

```powershell
uv venv --python cpython-3.12-windows-x86_64-none .local/langflow-ui-venv
uv pip install --python .local/langflow-ui-venv/Scripts/python.exe -r integrations/requirements.lock --link-mode copy
$env:SIC_CORE_URL = 'http://127.0.0.1:18888'
$env:SIC_CORE_ACTOR = 'host'
# SIC_CORE_TOKEN由私有环境加载，须与核心绑定身份一致。
./integrations/start-langflow.ps1
```

官方Langflow默认`http://127.0.0.1:17860`，脚本保留身份验证默认值；已有回环demo的显式auto-login与正常登录分开。可传`-Port`。令牌只在运行时环境，不能填进节点、context、payload或导出图。

完全离线演示另开终端运行`./integrations/start-core-demo.ps1`。它使用真实核心、进程内LiteLLM和自己的回环HTTP fixture，0真实provider，不代表模型质量。私有token位于git忽略的`.local/core-demo-token.txt`；启动两种UI的终端须加载同一token：

```powershell
$env:SIC_CORE_TOKEN = [IO.File]::ReadAllText((Join-Path $PWD '.local/core-demo-token.txt')).Trim()
$env:SIC_CORE_URL = 'http://127.0.0.1:18888'
$env:SIC_CORE_ACTOR = 'host'
./integrations/start-langflow.ps1
```

demo tokenizer明确标注为fixture口径。正式role profile须声明tokenizer、输入/输出预算与容量；UI只能选择宿主预授权profile，不能提交任意provider key或URL。

## Langflow流程

Import导入`flows/core-*.json`，写入和模型开关默认关闭。字段改变只修改提案，启用动作才执行。分组工作台中每个动作均绑定后端，独立节点需填写目标ID，不可把“运行全部”当安全任务。

| 流程 | 真实绑定路线 |
|---|---|
| core-native-to-cut | 原生输入→文本/sidecar→程序规则→校验→不可变CB/op/cut→双索引→encode→状态/计量 |
| core-editor-to-cut | 同一Agent、两个Editor scope→单轮op/PVI提案→组合校验/发布→可发现op目录→encode |
| core-editor-pvi | prepare/submit/run/program/validate及cut；模型调用是独立显式动作 |
| core-source-inspection | 来源只读查看和搜索 |
| core-scheduler-provider | 工具事件、等待/交错、encode与显式Worker调用 |
| core-chain-socket | checkpoint、平级fork、多creator、历史spawn、live信箱、Signal及board |
| core-config-index-cost | 角色/教学/预算、Lazy权限、内置读取、索引、费用与独立宿主撤销 |

chain/source/job/cut/checkpoint/socket ID自动进入context。`$context.source_id`等精确占位符保留类型，不执行表达式；高级bindings如`{"expected_cut":"sequence"}`把结果路径存入context。新建链样例的expected_cut为0，继续旧链须使用当前序号。

重跑下游可能重跑上游。继续已有任务请使用明确ID的独立动作；发布/Worker使用稳定idempotency key，丢确认先查结果。创建链/来源不是刷新仪表板的动作。旧CB/op查看结果没有回写入口。

## LangGraph / Studio

```powershell
uv venv --python cpython-3.12-windows-x86_64-none .local/langgraph-venv
uv pip install --python .local/langgraph-venv/Scripts/python.exe -e 'integrations[studio]' --link-mode copy
# 同样加载SIC_CORE_URL、SIC_CORE_ACTOR与私有SIC_CORE_TOKEN。
./integrations/start-langgraph.ps1
```

默认`http://127.0.0.1:12025`；官方[Studio](https://smith.langchain.com/studio/?baseUrl=http://127.0.0.1:12025)可能要求LangSmith账户，SDK无需账户。新图`sic_core`有8个机制执行阶段：解码、程序/Editor、CB/PVI/cut、encode/Worker、调度、peer链/Socket、当前视图/双索引、配置/计量，并保存每步trace。

输入`actions`数组，每项为`operation/context/payload/allow_write/allow_model/bindings`。写前interrupt允许检查和改授权材料，resume必须使用完整action且保持operation；写门须true，模型门另外true。actor文本不授予权限。长计划设置`recursion_limit`如300。

```json
{"actions":[
 {"operation":"chain.create","payload":{"creators":[]},"allow_write":true},
 {"operation":"source.ingest","payload":{"format":"openai-chat","request":{"messages":[{"role":"user","content":"Working material"}]}},"allow_write":true},
 {"operation":"edit.program","payload":{"source_id":"$context.source_id","rules":["deduplicate_exact"]},"allow_write":true},
 {"operation":"edit.validate","payload":{"job_id":"$context.job_id"}},
 {"operation":"cut.publish","payload":{"job_id":"$context.job_id","expected_cut":0,"idempotency_key":"$context.job_id"},"allow_write":true},
 {"operation":"cut.get"},
 {"operation":"provider.preview","payload":{"format":"openai-chat"}}
]}
```

失败停止，不自动重试模型。编排checkpoint不是SIC CB checkpoint；只有核心chain.fork/socket.spawn产生平级路线。旧图明确命名`sic_legacy_control`。

## 验收和当前限制

```powershell
$env:PYTHONPATH = 'integrations'
.local/langflow-ui-venv/Scripts/python.exe -m unittest discover -s integrations/tests -p 'test_*.py' -v
.local/langflow-ui-venv/Scripts/python.exe integrations/tests/v2_acceptance.py
.local/langflow-ui-venv/Scripts/python.exe integrations/tests/v2_official_api.py
```

`v2_acceptance.py`使用隔离临时DB、真实核心HTTP及自有模型Mock，动态校对整个catalog，并实际执行官方LFX/LangGraph。`v2_official_api.py`不启动服务，仅向已有官方实例导入、保存、读取和探测绑定。报告只写`.local/integration-test-results/`；不公开runtime ID。

本轮独立端口为Core18888、同实例bridge18890、Langflow17861、LangGraph12026，保留已有数据库与旧端口。两个官方界面均已通过独立浏览器实际操作；候选版本修改、固定发布、角色前沿与任务总账由界面执行并独立读回。Langflow还完成二阶、三阶各12节点的整条依赖流程，以及命名Editor映射、决策与公示牌编辑。所选Editor任务等后续修复的常驻加载和逐项GUI状态，以[当前验收事实](../docs/runtime-acceptance-current.md)为准；代表路径不等于穷尽全部功能组合。

## 语义工作台与一键状态

```powershell
./integrations/manage-runtime.ps1 -Action Start -Mode Fixture
./integrations/manage-runtime.ps1 -Action Status
```

启动器只管理本项目本轮独立端口与私有DB，后台窗口隐藏；不停止已存在服务、不改变系统启动项。Langflow17861启用正常身份验证，首次凭据保存在`.local/langflow-17861/admin-auth.json`，通过登录框使用，不写进flow。Core凭据由私有环境加载。真实固定profile模式必须显式指定`-Mode Configured -ConfigFile PRIVATE_CONFIG`，不会回落fixture；该模式启动不发模型请求，实际模型工作另需授权。

导入`flows/workbench-*.json`可用专门组件查看正文/只读协议对照、编辑已交付候选版本的op和P/V/I、检查四视角决策、沿时序与CB↔WB定位片段、浏览历史路线及实际分身协作、公示牌任务、运行政策和费用账目。`SIC Editor任务与只读档案`在单轮模型前程序预取并冻结选中档案页；旧CB绝不成为编辑target。候选CAS使用已核对revision，原始模型响应与每次修订出处保留。

`workbench-candidate-lifecycle.json`是完整七节点已交付候选链；源模板中的身份占位符需替换为当前授权链、source和已返回job，不会在导入时创建或调用模型。节点2修订，3校验，4发布，5/6导航固定结果，7读账。所有写/模型门默认关闭。后续节点遇上游失败停止。观察已发布结果应使用独立只读节点，避免重跑上游。

Studio提供`sic_candidate`候选表单及持久interrupt、`sic_memory`只读分页、`sic_decision`四视角、`sic_routes`真实路线/协作/任务、`sic_runtime`调度/恢复、`sic_configuration`宿主profile、`sic_editor`单轮范围/只读预取、`sic_source`新材料解码、`sic_accounting`账目/独立实验核验。Studio表单输入只有授权工作材料；暂停信息中的原始响应/CB只读，resume不能改route/job。官方Studio可要求登录，Swagger不是Studio。

新实验入口和证据边界见[独立实验说明](experiments-v2.md)。当前F01–F73逐需求状态可在SIC验收组件查看，未以动作下拉有名字代替功能通过。

常驻17861/12026是否确实连接同一18888，可使用[跨运行时重放](runtime-replay.md)：同链先由Langflow发布，再由LangGraph读/改/发布，最后返回Langflow核对固定历史。另有真实DSH chain的只读观察模式，不发模型、不改Core。

后端catalog稳定后再生成：

```powershell
$env:PYTHONPATH = 'integrations'
.local/langflow-ui-venv/Scripts/python.exe integrations/generate_core_flows.py
```

`core-operation-catalog.json`是生成快照，运行时仍取核心capabilities。旧v1组件、三份旧流程和测试仅保留兼容，不计新设计证据。

官方参考：[Langflow自定义组件](https://docs.langflow.org/components-custom-components)、[LangGraph interrupt/resume](https://docs.langchain.com/oss/python/langgraph/interrupts)。

Studio补充证据`GUI_STUDIO_DEEP`已展开核验u1正文与alpha分配、alpha冻结模型绑定、beta profile、两Editor原交付ID/来源及冻结背景来源。背景material仍为截断预览，original_response子对象仍折叠，不声称全部正文逐字核验；独立证据`GUI_STUDIO_UNICODE`已验证`editor:δ`只读投影表单的typed→Raw→Submit及返回manifest角色原文可见；不证明δ模型、私有Editor身份或任务执行，Provider未调用、物理缓存仍unknown。
