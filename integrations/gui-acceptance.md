# 两个官方 GUI 的独立验收

API/SDK报告不替代本页实际操作。只使用本轮独立实例；写/模型授权分开。已保存的旧四节点未提交提案保留，不执行它。

本轮根代理已实际完成Langflow候选修订→发布→CB/WB/source查看代表路径，独立只读DB核验与截图哈希留私有报告。该专用候选现已发布，不能重新Run来改旧CB；用其已保存输出观察。以下步骤供另一条新候选复验。Studio仍未目视验收，工具因无法确认浏览器URL强制停止，API成功不替代它。

## Langflow

1. 打开 `http://127.0.0.1:17861`。正常登录，私有首次凭据位置 `.local/langflow-17861/admin-auth.json`；不将凭据填入节点。
2. 使用私有 `.local/integration-test-results/workbench-gui-entry.json` 的最新 `url`。它是独立链上由自有fixture实际返回的 revision 0；模型调用只在准备时完成。
3. 展开节点1，查看来源正文与只读协议。节点2选择“修订操作与 P/V/I”，将正文、理由、P、V及 `operator_note` I 改为本次可辨认内容；保持 target=u1 和 expected_revision=0。打开节点2、4的本次写门。模型门保持关闭。
4. 保存后从节点7运行整链一次。节点2应返回revision1；节点3校验成功；节点4固定发布；节点5显示时序；节点6将同一个CB定位回WB、源单元与有序op；节点7显示真实调用账和unknown。
5. 用节点输出查看图标检查已经产生的表格，核对修改正文/P确实进入固定CB，V成为后续WB，I为当前视图。**查看结果时不要再次运行上游链**。需要再次读取时使用独立只读导航节点与明确chain/CB ID。
6. 核验原始Editor交付与候选修订出处仍可分页读取；已发布CB/op无回写入口。更正应通过后续P/V/I。另行记录实际目视结果与持久化检查，未操作到的需求继续列缺口。

## LangGraph Studio

1. 打开官方 `https://smith.langchain.com/studio/?baseUrl=http://127.0.0.1:12026`，按官方要求登录。`12026/docs`不是Studio。
2. 选择 `sic_candidate`，使用私有 `workbench-runtime-acceptance.json` 的 `gui_entry.thread_id`。这是另一条独立链的真实持久暂停，尚未修订或发布，不与Langflow候选共用。
3. 查看暂停中的只读原始响应、当前revision和可编辑候选。按 `resume_form` 字段修改content/P/V/I，将allow_write改为true；不要传chain/job/原始响应或额外控制字段。恢复后应显示revision1及原始响应不变。
4. 同线程用当前chain/job启动 `form.action=validate`、expected_revision=1；检查并恢复。再启动 `publish`，expected_revision=1、expected_cut=0、allow_write=true；检查并恢复。每个写动作单独停留供核对。
5. 切换 `sic_memory`，用同chain查看timeline、CB↔WB映射与ops/proposal分页；`sic_decision`显示四视角，`sic_routes`浏览实际路线、历史点、公示牌任务和分身执行状态。编排thread/checkpoint不是SIC Agent链fork。
6. `sic_routes`的bridge_sessions只读动作显示身份授权的宿主会话→同Core chain映射；选择session_key后从result.context继续导航。目录为空或桥不可达时如实显示，不伪造DSH往返。
7. `sic_accounting`选择verified_experiment，并填本轮实际run项目相对目录；必须先通过独立validator再显示两臂对照。fixture质量not_evaluated，费用为明示虚构测试费率。

Studio表单、暂停、状态与导航是否实际可用，应由真实GUI操作记录。仅图注册、schema出现、API持久线程运行或HTTP200均不足以给GUI通过结论。

最后一批Core档案预取/失败恢复/预算窄补丁需要由服务拥有者协调载入；界面收到缺项或拒绝时保留错误。不能靠放开原文/CB权限或自动模型重试掩盖版本差异。
