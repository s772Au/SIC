# 新核心独立实验入口

这些脚本不重用旧稿实验结果。默认只创建本进程拥有的回环模型、两个独立Core数据库和一个共享fixture endpoint，不读取真实provider配置，也不访问8341。fixture验证运行、记账和不变量，质量始终`not_evaluated`。

```powershell
.local/langflow-ui-venv/Scripts/python.exe scripts/run_v2_experiment.py
.local/langflow-ui-venv/Scripts/python.exe scripts/verify_v2_experiment.py RUN_DIRECTORY
.local/langflow-ui-venv/Scripts/python.exe scripts/score_v2_experiment.py RUN_DIRECTORY
```

`RUN_DIRECTORY`取本次runner实际输出的新目录，默认位于`.local/experiments-v2/`。可用`--task TASK_JSON --output NEW_EMPTY_DIRECTORY`提供任务；已有结果目录不覆盖。任务包含`task_id`及非空`blocks`正文列表。两臂使用相同任务、Worker轮数、模型与回环端点、提示、规则和fixture费率：基线保留积累材料，到末尾一次压缩；SIC逐块编辑并发布，包括末轮Worker尾部。

每臂保存原始HTTP调用、Core操作轨迹、最终CB、资源测量和总账；任务/模型/提示/规则/费率及代码快照冻结，manifest记录每个工件的大小和SHA256。失败保留失败与部分调用工件。独立validator不调用网络/模型，重算哈希、账目和工作流不变量，验证一次/增量编辑次数、固定历史不变与最终无待处理WB。独立scorer先验manifest，fixture不产生质量分。

钱使用`accounting.py`十进制字符串与分币种汇总。`FIXTURE`费率是明确虚构的测试计价，不能当真实价格。测量的进程CPU包含SDK/fixture开销，存储是本次数据库/WAL/SHM快照；未测或缺价部分保持unknown。fixture未提供真实cache-read命中，不强行套F/H成本假设。模型累计时间、区间并集、Editor与工具重叠、任务总墙钟分别保留。

Langflow的SIC调用账组件选择“读取独立核验实验”，填项目相对run目录；Studio的`sic_accounting`选择`verified_experiment`及`run_directory`。两者先运行独立validator再展示对照，不把任意JSON导入当实验完成。不接受任意绝对路径或上溯路径。

真实固定模型正式实验、任务质量rubric及独立scorer尚未实施；不能从fixture的压缩比例/费用/HTTP成功推断质量收益。执行正式实验应另固定实际模型身份、tokenizer、预算、数据版本、提示/规则/费率与评分方法，产出新证据，不复用fixture质量结论。
