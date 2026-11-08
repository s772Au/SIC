# 运行器与规划估算的对应关系

当前真实文本CLI已经能读取完整源、封块、调用固定provider、发布、答题与评分。以下规划不能直接当作其已实现调度：

- CLI按固定顺序执行，并发为1；控制包中32并发与自适应吞吐是后续调度规划。
- CorpusQA当前按JSONL问题记录作为独立case，不自动把同一领域的共享上下文聚成4条历史，也不跨case复用已发布CB。控制包按4份共享上下文估算的压缩量不等同于CLI全量遍历的实际调用量。真实调用预算以本次 `dataset_runner.py` 生成的dry plan为准；执行前的显式max-model-calls门槛会拒绝超预算计划。
- 完整历史baseline只在provider实际窗口内可执行。超容量保留为失败/不可执行，不能裁剪后当成完整baseline，也不能把它计成有效质量零分。
- BEAM与CorpusQA当前是明确版本的本地fixed-judge，BABILong使用冻结的官方metric并另报strict exact；官方prompt及完整官方评分复现仍是独立验收项。

上述差异影响成本、时长与实验归因；源数据完成校验不代表所有规划中的调度策略都已实现。先核对dry plan，再配置稳定的固定model/provider运行。
