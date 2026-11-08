# 共享I前缀：官方工作台源码入口

本页记录B布局的前端源码与隔离SDK/API验收，不表示常驻服务已加载，也不表示浏览器GUI或物理缓存收益已验收。Core适用字段以`prefix_layout.validate_config`为准。

Langflow“角色、教学与预算”新增“本次更新前缀布局/B设置”开关；不勾选时不发送这组设置，普通profile/教学修改不会隐式切换布局。可选择`separate`/`shared_i`、>=512的统一投影字符预算，以及保留、统一或按角色整体替换的非敏感逻辑身份声明。身份只允许`model/provider_id/cache_namespace`，或worker/editor对应的同形对象；不能填API密钥、环境变量字段或带凭据/查询参数的URL。这是宿主声明的逻辑身份，不是物理模型身份认证。

Studio `sic_configuration`使用同名typed字段和真实review→interrupt→resume写门。`replace_prefix_layout=false`保持原设置，`prefix_identity_mode=keep`保持原声明；预算留空时不修改已有预算。两个角色及各已开始job的冻结绑定由Core决定，配置保存成功不代表已共享；不兼容时必须查看实际fallback。

Editor任务新增只读`prefix_rows`/`alias_rows`：冻结计划ID/digest、请求布局/应用布局、fallback原因、宿主模式、别名到canonical ID。准备时显示的shared计划不等于模型实际调用；实际请求引用来自`job.attempts[editor].request_id`，实际布局来自该attempt的元数据。没有实际引用时明确为空。snapshot_payload、suffix及完整capability不作为表单字段，也不随安全task_view展开。

B交付表区分canonical转换后的op/PVI、原模型response引用和canonical_response引用；不能把转换后的ops标成原模型逐字响应。原响应正文未嵌入任务表时明确显示这一事实，发布后可用已有proposal档案分页查看；人工candidate仍使用canonical单位ID。任何UI提案都不能改写原响应、已发布CB/op或宿主capability。

Langflow `SICMemory`新增“读取冻结前缀计划”和“读取实际模型请求”，分别使用`memory.page(kind=prefix_plan|provider_request, object_id, offset, limit)`。Studio `sic_memory`同样新增这两个typed action；只读预取`ArchiveRead`也允许这两类。它们返回真实完整JSON的受限页、next_offset和full_digest，不把未知对象替换成另一个对象。对象不存在或属于其他路线时显示Core拒绝。

受影响的通用官方导入文件已重生成：`workbench-configuration.json`、`workbench-memory.json`、`workbench-candidate.json`、`workbench-candidate-lifecycle.json`和`workbench-higher-edit-publish.json`。默认写门/模型门保持关闭，原生成器的初始action可见字段初始化保留；导出不包含凭据或运行中任务ID。

验收入口：

```powershell
$env:PYTHONPATH='integrations;.'
.local/langflow-ui-venv/Scripts/python.exe -m unittest integrations.tests.test_shared_prefix_workbench -v
```

六项专项使用自有临时Core HTTP、官方LFX构建/运行和LangGraph SDK。配置、计划冻结、跨路线拒绝及分页均实际调用Core API；模型调用为0。实际请求分页的载体测试使用显式标注的合成不可变归档，只证明读取/权限，不伪称实际模型调用。常驻/浏览器GUI由root在安全加载后另行验收。
