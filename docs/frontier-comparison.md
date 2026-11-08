# 同次编辑前沿比较

`frontier.compare` 在同一个cut比较必要KEEP发布与实际候选发布后的请求。两臂复用真实发布和角色组装路径，在数据库保存点内演算并完整回滚；不会调用模型、发布真实CB或消费待处理WB。默认比较Worker，保留原有调用行为。

## 运行表单

Langflow的“SIC 持续工作与编辑窗口”选择“比较同次编辑窗口”，填写任务、已核对cut、位置预算及长度字节预算，再选择下一请求的角色。选择Editor后填写其身份和下一WB的source_id。身份沿用既有模型配置、活动表示和私有背景；此表单不会切换配置或活动阶次。

“下一WB完整编辑范围”默认开启。关闭后，每行填写一个单元ID；空白明确表示空编辑scope。完整范围与空scope有不同含义，顺序保留，重复或空单元ID会被拒绝。

Studio的RuntimeForm对应字段为comparison_role（worker/editor）、frontier_editor、next_source_id和next_unit_ids。next_unit_ids为null时采用整个下一WB，空列表表示空scope；列表表示指定单元。Editor身份或下一source缺失会在表单校验中拒绝。运行域表单不接受通用operation、payload或原生请求JSON字段。

## 只读结果

两界面展示当前角色布局、必要KEEP请求、候选附加请求，以及编辑阶次/内部context、候选是否真正作用到该角色请求、共同前缀字节、摘要、位置和长度预算、控制及提示变化。previous_request用于定位当前布局的重建起点，不是从相邻历史请求拼出的另一对照臂。高阶或私有候选未被所选角色采用时，结果如实显示其请求未受候选影响。

same-pending-slot、same-frontier-cut和same-volatile-source只是同次演算的受控身份；它们不是已经发布的UUID。physical_cache_hit和provider_internal_layout保持unknown。局部窗口成立不证明切换表示免费，也不证明物理缓存命中、整个任务费用或语义质量。

## 验证和界面边界

`integrations/tests/test_frontier_forms.py`独立验证默认Worker兼容、Editor完整/指定/空scope、表单负例和动态字段可见性；通过官方LFX输出构建器与LangGraph SDK，向自有临时Core HTTP服务运行Worker及私有三阶Editor比较，核对两界面到同一Core结果的映射和只读输出。原始结果保存在本地`.local/contract-rebuild/frontier-forms-sdk.json`。

`integrations/flows/workbench-runtime.json`已定向重新生成。当前常驻Langflow/Studio服务没有重启，也未加载新表单或重新完成浏览器目视验收；上述SDK和表单通过不能代替GUI重新载入后的实际验收。
