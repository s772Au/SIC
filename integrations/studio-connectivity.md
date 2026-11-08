# 官方 Studio 连接诊断

入口是 `https://smith.langchain.com/studio/?baseUrl=http://127.0.0.1:12026`，本地服务是官方 Agent Server；`/docs`并非Studio。官方当前[快速入门](https://docs.langchain.com/langsmith/quick-start-studio)采用此云端Studio连接本地Agent Server方式；本轮未找到可直接替换它的独立离线Studio安装方案。没有建立外网tunnel或自制WebUI。

## 已确认并修复的服务端问题

实际版本为LangGraph1.2.20、CLI0.4.42、API0.17.1、SDK0.4.14、Starlette1.10.0。本地普通CORS预检200，但加入浏览器使用的`Access-Control-Request-Private-Network:true`后返回400，正文为`Disallowed CORS private-network`。同时官方另一个中间件给拒绝响应附上allow-private-network:true，单看响应头会误判。

Starlette支持`allow_private_network`，但API0.17.1的http.cors配置schema过滤这个字段。仅把它写到JSON不生效。没有修改安装库或降级依赖。

现通过[官方http.app扩展](https://docs.langchain.com/langsmith/cli)挂载`studio_http.py`：空Starlette app仅含标准CORSMiddleware，允许官方smith origin的私网预检。它没有页面、代理、请求头改写或认证替换。普通API请求仍进入官方服务；其他origin被拒绝。配置和依赖版本已固定在本项目启动入口。

复验结果：官方origin的私网预检200且许可头正确；不受信任origin400；官网HTML及两个入口JS均200、数秒内完成。报告只存`.local/integration-test-results/studio-connectivity-{before,after}.json`。

```powershell
.local/langgraph-venv/Scripts/python.exe integrations/tests/studio_connectivity.py --phase after --remote-resources
```

诊断不操作浏览器、不改系统代理、不关闭浏览器安全、不调用模型。HTTPS资源测试使用正常网络请求，只覆盖入口资源，不证明浏览器所有动态chunk或登录状态正常。

## 最小 GUI 重试

1. 在Chrome打开上述官方入口；必要时正常登录LangSmith。
2. 若站点提示本地网络访问权限，按[官方故障指南](https://docs.langchain.com/langsmith/troubleshooting-studio)在该站点的Local network access中允许，然后刷新。不是全局关闭安全功能。
3. 选择`sic_candidate`。新表单`edit_mode`支持replace、append、remove、pvi_only；空ops默认KEEP也能只改P/V/I。原始响应、已发布CB/op仍只读。
4. 页面是否显示、能否在Studio实际编辑/恢复必须单独目视验证。根代理此前getTab/AX45秒超时与URL确认失败，不能单凭已修PNA断言同一根因或GUI已通过。

本轮仅修独立连接和候选缺陷，不以旧需求编号覆盖率宣称整个SIC设计完整。CB/WB关系的需求基线正在由作者重新核对。
