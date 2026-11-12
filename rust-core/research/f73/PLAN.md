# F73 冻结对照计划（尚未执行）

状态：**not_run / research_evaluation = not_evaluated**。本文件是计划与冻结边界，不是结果。

## 1. 任务集（待作者补齐）

| 家族 | 目标 | 最低样本 | 评分入口 |
|---|---|---|---|
| T-COST | 多轮工具任务中的共同成本、增量成本与净差 | 待定 | 独立 rubric + 原始轨迹 |
| T-TEXT | 长文档事实保留、追溯与编辑正确性 | 待定 | 事实/引用逐项核对 |
| T-HIERARCHY | 多阶 WB/CB 选择、跨阶追溯 | 待定 | 层级导航与答案证据 |
| T-COST-SHARED-PREFIX | 共享 Worker 教学/I 模式方案 B 的共同前缀成本 | 待定 | 前缀/缓存/费用账目 |

任务必须新写并冻结；不得复用旧 E1/E2 任务或成绩。

## 2. 对照臂

- **A（SIC）**：本目录 `adapter.json` 的 Worker 教学、Editor 契约、七操作、程序规则包。
- **B（harness 摘要）**：Codex compaction prompt + summary prefix，单次可变摘要。
- **C（LLM 压缩）**：LLMLingua 压缩引擎，固定压缩率参数。
- **D（非 LLM）**：SWE-agent 历史处理器与 aider repo map 的确定性裁剪/选择。

A/B/C/D 使用同一任务、同一工具环境、同一固定模型路由；模型冻结为 `deepseek-v4.1-flash-eco`。当前**未发起任何付费调用**，价格模板未配置。

## 3. 冻结与记录

- 来源：`sources.json` 中的 commit + git blob SHA + 许可。
- SIC：`adapter.json` 中的文件 SHA256。
- 每次运行保存：配置、任务、原始请求/响应、工具结果、CB/op 轨迹、失败与未知、资源计数。
- 两臂/变体分别标记；不得把任务、模型、前缀或工具变化混成单变量效应。
- 独立评分使用新写的 rubric 与 scorer；在真实评分器完成前，研究维度保持 `not_evaluated`。
- 非 LLM 臂零模型调用与程序费用必须如实记录。

## 4. 验收映射（F73）

1. 来源/许可/版本/SHA：部分完成（5 个代表来源；未穷尽）。
2. SIC 适配包：已完成本快照（prompt/schema/教学/规则的文件 SHA 已冻结）。
3. 新任务对照：未执行。
4. Worker 教学与后置可信 I/程序规则：已引用并冻结；A 对照包保留。

## 5. 仍待完成

- 作者新任务集与参考材料；
- 真实独立 rubric/scorer（非合成事实保留）；
- 经授权的固定模型两臂/变体运行；
- 原始轨迹、独立成本报告与质量报告；
- 来源补充与不可得项记录。
