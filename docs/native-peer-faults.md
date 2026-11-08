# 真实DSH宿主故障与生命周期验收

这组验收通过已安装DSH的公开CLI、隔离profile/Loader和正常原生工具运行时执行。替身仅用于确定性模型及本地故障传输；分身创建、原生工具体、持久Session、Core请求、收据和恢复均实际运行。使用新的隔离Home、动态端口和自有token，不接触日常桌面资料或现有Core/UI服务，不需要窗口、鼠标或键盘。

## 已验证的16个场景

1. 工具体运行后取消：原生工具收到AbortSignal并记录aborted，结果不采纳。
2. dispatch前撤权：有reservation，没有actual dispatch或工具体。
3. 工具体运行后撤权：实际中止，不能补交成成功。
4. Core已提交完成但响应丢失：只重送收据，原生工具仍一次。
5. 完成数据未到Core：持久indeterminate；Core重启、原生Session恢复后不重执行，原样迟到收据被拒绝。
6. 两个历史peer真正并发：各有独立原生身份，工具体区间实际重叠，回信由请求者消费。
7. 在途lease到期：状态归unknown，原生工具中止，不伪造tool_end或成功结果。
8. 并发中仅stop一支：该支中止，另一支完成，状态分别保留。
9. 宿主传输断连：watcher中止、lease归unknown；Core及原生Session重开后无旧工具重执行。
10. Editor在途wait/wake：单轮响应保留，暂停期间没有新Worker，唤醒复用候选。
11. reservation之后、dispatch之前wait/wake：只对明确未准入的暂停码等待，唤醒后执行一次。
12. 实际工具体中wait/wake：已运行的工具安全settle；暂停期间不新增Worker，唤醒消费原收据。
13. Editor在途stop：有限终态，不继续发Worker，也不永远挂在waiting。
14. 真实工具失败及显式重试：失败request重送不产生新调用；无重复风险确认的重试被拒绝；明确确认后新attempt完成并进入原生请求者后续输入。原失败工具仍只执行一次。
15. 仅Message权限：第二认证角色可P/V投递，不能读取目标历史、spawn、Signal或访问目标公示牌；真实目标请求消费两类通信及最新公示牌I，P固定入CB，V进入独立的下一WB，I不成为可编辑源。同一场景显式派发P回传，自动执行沿冻结政策，唯一回信携固定接收时间/收据进入父CB并被父Worker消费。
16. DSH自带的只读`read`：custom wait关闭，allowlist仅read；读取隔离Home刚创建的随机文本，原文件hash不变，真实内容首次进入peer下一Worker输入。

16场景包含17个真实原生peer尝试；停止于Editor阶段的场景没有伪造原生工具尝试。内置read由实际`@deepseek-ai/dsh-tool-fs`注册提供，参数file_path/offset/limit先从注册源码、组合profile核实，再逐次核对实际请求schema。它不是插件自定义wait改名，也不读取用户文件。具体计数、源码快照、独立关联结果和私有原始证据SHA见[验收摘要](../integrations/native-peer-faults-acceptance.json)。不把场景数当作73项全部通过。

当前摘要中的16场景在同一最终源码快照上完整重跑，包含接收收据与通信投影修复；运行期间全部Core Python文件、操作目录及宿主插件均保持原hash。独立复核只读备份该批数据库并核对原生会话绑定，不拼接早期版本的通过结果。

## 修复的实际缺口

原协作运行假定每次run之后都有Worker步骤；在途Editor被暂停时，返回candidate-held而没有Worker，结果被误记失败。现在保留等待状态和候选，wake沿同一请求恢复。Worker每轮遵守实际run_state，暂停时不新发工作或回传未获准结果；stop形成可观察终态。

暂停与撤权分开：已dispatch的工具可在wait期间安全结束并保存收据；新的begin/dispatch收到明确`host_tool_paused`。插件只对此**确定未准入**状态等待后重试握手，不重新执行原生工具；网络未知、reservation/dispatch结果不明、lease过期、取消或撤权仍保持不确定/终止事实。当前工具权限每次核验，等待和wake不会恢复已撤销权限。

测试用`fixtureWaitTool`合作处理exec.signal并记录真实aborted；正常成功仍为started/ended。`fixturePeerCount`和`fixtureWaitFailure`只扩展默认关闭的自有验收工具，用于并发与确定性失败，不增加生产权限或任意执行接口。

协作回传生命周期在dispatch时固定于父request、branch及child任务。后续显式run、自动事件、child timer与确认后的失败重试沿用该政策；不匹配的显式覆盖在执行前被拒绝，已完成请求重送不重复投递。旧版请求从原child任务恢复政策，不把历史P默认为V。`fixturePeerReturnLifecycle`只用于默认关闭的验收工具选择P/V。

## 重现

在装有官方DSH CLI、已安装固定插件依赖和完整本地SDK的环境中，以后台命令运行：

```text
python integrations/tests/native_peer_faults.py --dsh-command PATH_TO_SUPPORTED_DSH_CMD --output .local/new-native-fault-run
python integrations/tests/verify_native_peer_faults.py --batches .local/new-native-fault-run --output .local/new-native-fault-verification.json
```

输出必须是新空目录。runner只通过公开headless profile启动，恢复使用实际CLI公布的`--session-id`，不通过自造SDK应用绕过宿主。所有子进程后台创建；异常时仅结束本fixture拥有的进程树。原始合成请求、Session日志及数据库保存在私有目录，不在公共摘要中复制运行ID、凭据或原请求。

独立verifier不请求模型、不启动服务，不修改原数据库；以只读SQLite backup构造临时副本，逐字段核对原生binding与Core provider_request/response、工具体开始/结束/取消、完成收据和请求者实际后续输入。未知结果恢复及明确新attempt与原尝试分开核验。

## 边界

合作wait fixture证明取消信号真正到达工具，不代表能撤销任意外部副作用。未知结果保持unknown；本组不证明真实模型质量、经济持平、Provider物理缓存命中或所有GUI域。第二Message角色通过真实认证Core API发送；这里验证了原生目标消费和权限分离，没有把该发送者冒充另一个实际推理模型。
