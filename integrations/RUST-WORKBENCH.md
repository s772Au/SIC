# SIC Rust 与官方 Langflow / LangGraph

workbench-rust.json 是 Langflow 的 Rust 工作台；sic_rust 是 LangGraph Studio 图。
二者通过同一宿主进程的 /api/v2 控制面访问同一个 Rust Core。它们每次读取运行中
Core 的能力目录、写入/模型标志与 payload 参数描述；连接的是 Python Core 或
后端不可用时会明确报错。已有的 Python 专用语义流程仍保留，不把 Rust 不支持的
操作伪装成成功。

## 本机启动

先从当前源码构建 Rust release，并将二进制放在 03-backend-builds 下的稳定位置。
下面是整理后的本机示例；按实际构建路径替换 -RustBinary。它为新版工作台创建独立的
Rust 数据库和 Langflow 数据库，不替换已在其他端口运行的服务。

    cd .
    .\integrations\start-rust-workbench.ps1 -RuntimeDir ./05-frontend\runtime-local\rust-workbench -RustBinary ./03-backend-builds\rust-workbench-386325a\sic-core-rust.exe -LangflowPython ./.local\langflow-venv\Scripts\python.exe -LanggraphCli ./.local\langgraph-venv\Scripts\langgraph.exe -LangflowExtraPythonPath ./evidence/real-run-20261112/langflow-overlay

脚本只绑定 127.0.0.1，若端口已被占用则停止而不替换服务。默认端口：Rust Core
18898、Langflow 17862、LangGraph 12028；可分别用 -CorePort、-LangflowPort、
-LanggraphPort 指定。Langflow 登录信息保存在运行目录下的 private/langflow-admin.json，
Rust bearer 在同一受限目录的 core-token.txt；不要把凭据填入节点、图或提交到版本库。
安装器会自动导入 Rust 工作台；重复启动时保留用户已经编辑的画布。

默认不授权模型或安全撤销。需要模型时由宿主明确提供私有 Rust 模型配置，并同时传
-ModelConfig <文件> -AllowModel；需要安全撤销时单独传 -AllowRevoke。这只是宿主
权限上限；前端每次操作仍需要分别勾选写入、模型或撤销。启动本身不会调用模型。
请自行提供配置所引用的私有环境变量，不要将其放进导出图。

查看启动状态：

    .\integrations\start-rust-workbench.ps1 -Action Status -RuntimeDir ./05-frontend\runtime-local\rust-workbench

## 界面操作

- Langflow：打开 http://127.0.0.1:17862，登录后打开已导入的 **SIC 工作台 — rust**。
  切换操作分组以刷新实时下拉目录；选中操作后，参数说明取自 Rust Core 的 payload。
  输入路线 ID 和 JSON 参数，按需开启独立的授权开关。capabilities 只读取目录，
  目录输出不会重复执行写入。同一画布还提供新工作材料、来源对照和 Rust 单路线账务节点。
- LangGraph：在官方 Studio 使用本机 Agent Server http://127.0.0.1:12028，选择
  sic_rust 图。默认动作 capabilities 返回实时目录；填写操作、路线与参数后运行。
  写入或模型操作会先暂停供复核，恢复时不能更改已选择的操作或路线。
- 两个界面可以使用同一 chain_id，但 Studio 的线程/checkpoint 不等同于 SIC 的
  CB 或 checkpoint。所有操作以 Core 返回的结果和权限为准。

SIC Rust 实时工作台是覆盖 Rust 已公布操作的通用入口，复杂操作使用 Core 公布的
JSON 参数；这不表示旧 Python 工作台的每个专用表单都已改写。模型质量、真实服务商
计费和长时间并发不由本次对接验证。

## 测试

tests/test_rust_workbench.py 在 SIC_RUST_TEST_BINARY 指定时启动临时 Rust 数据库，
跨 Langflow 组件和 LangGraph SDK 完成创建、导入、编辑、发布及读取。
tests/rust_official_runtime.py 检查正在运行的官方服务，用测试链验证从 Studio 创建、
从 Langflow 读取；它不会发模型请求。官方服务 API 测试和浏览器图形界面验收是不同的证据。
