# Editor身份与profile映射

官方Langflow的“SIC 角色、教学与预算”支持两个简便命名控件及任意数量的JSON映射；Editor填写自身名称（如alpha），角色表示选择才使用editor:alpha。

- 保留现有映射：不提交editor_profiles。
- 合并指定映射（默认）：从刚读取的当前映射合并命名控件和高级映射，空profile控件不删除任何身份。
- 完整替换映射：只使用高级JSON对象，未列身份被明确移除；忽略便捷控件。
- 清空映射：明确写入空对象；新任务采用宿主默认，旧任务冻结绑定保持。

非法名称、重复/冲突输入、非对象JSON或非允许profile会在配置写入前拒绝，亦不会派发模型。profile来源仍是宿主allowlist，密钥与端点不放画布。显示表列出真实身份→profile映射，便于与新job的model_bindings比较。

本次测试通过实际Core HTTP与官方LFX单节点flow构建/执行：alpha/beta不同profile、第三身份、保留未编辑映射、完整替换、清空回默认、错误负例、旧job绑定不变，0模型调用。这里没有给Core配置新增CAS；多个操作者同时更新全映射时仍应串行操作，不能将客户端合并描述为服务端并发合并。
