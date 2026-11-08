# 常驻官方运行时的同Core重放

这项验收通过实际发布与交叉读取证明常驻绑定，不只比较capabilities或端口健康。它不操作浏览器，不重启服务，不执行已有GUI提案。GUI目视仍单独未验收。

从 `PROJECT_ROOT` 执行：

```powershell
./integrations/manage-runtime.ps1 -Action Status
. ./integrations/use-core-demo-environment.ps1
.local/langflow-ui-venv/Scripts/python.exe integrations/tests/replay_runtime_binding.py
```

脚本固定本轮回环端口：Core18888、Langflow17861、LangGraph12026。Core角色必须明确为自有offline fixture，否则默认重放拒绝，不回落其他provider。凭据只由私有环境及正常Langflow登录读取；临时官方API key在finally中删除，不写trace或导出flow。

每次重放新建独立nonce与Core链，实际执行：

1. 在18888保存新源并让自有fixture Editor单轮交付。
2. 由17861保存typed候选编辑并执行完整七节点流程，发布第一个CB。
3. 由12026读取同一CB正文，核对第一个编辑确实生效。
4. 同链新WB由fixture交付，12026持久线程暂停、修改op/PVI、恢复、校验并发布第二个CB。
5. 返回17861读取同链时序、源正文/协议和调用账；与18888和12026逐项比较。Studio分页读完全部CB正文与op，检查第一个CB完全不变、模型原交付仍保留。

重放只有准备阶段两次自有fixture调用；观察阶段不调用模型，真实provider调用为0。重复执行会创建另一个独立链，不能再次发布已有job来冒充可重放。

## 真实 DSH 链的只读观察

根代理完成真实DSH往返后提供实际 `CHAIN_ID`，若有精确桥绑定则同时提供 `SESSION_KEY`：

```powershell
. ./integrations/use-core-demo-environment.ps1
.local/langflow-ui-venv/Scripts/python.exe integrations/tests/replay_runtime_binding.py --observe-chain CHAIN_ID --session-key SESSION_KEY
```

`--session-key`可省略；提供时必须与当前身份可见的桥会话→chain绑定完全一致。观察模式不保存源、不编辑候选、不发布、不发模型请求、不控制路线；只在两个官方runtime中创建本地只读flow/thread。它也不要求真实provider配置变成fixture。

观察要求该链已有至少一个真实发布CB。Langflow读取实际路线、时序、来源正文/协议和调用账；Studio读取同一时序及全部CB/op分页，均与18888完全匹配。前后cut与CB必须不变；若DSH仍在推进导致快照漂移，脚本失败并要求在宿主安静时重读，不暂停或修改DSH来制造通过。

结果及逐次官方API返回只保存 `.local/integration-test-results/runtime-replay/replay-*/`。其中result提供实际Langflow只读入口、Studio持久thread与相同chain/cut证据，供后续GUI目视使用；不将内部运行ID写入公开说明。`gui_visual_verified`始终false。

服务拥有者完成安全重启后，可在同一只读命令追加`--compare-report PRIOR_PRIVATE_RESULT_JSON`，自动核对前后chain/cut、CB数量与完整归档摘要、调用账数量；新报告包含调用账摘要时也逐摘要比较。该选项不自行重启、不重放DSH/provider请求、不发模型，只验证已持久化事实和原session绑定。正常工具往返、工具期Editor重叠、收据重送是否调用模型仍需宿主各自证据。

正式DSH接入是否成立仍须其真实CLI/插件、历史接续、工具start/end及返回证据支持。对任意人工链读取成功，或者会话目录存在，不是DSH往返完成。
