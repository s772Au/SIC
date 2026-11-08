# SIC v0.1：官方LongMemEval-V2媒体适配

SIC编辑文本；截图按原始source、message/part、角色与工具组保留为不可变只读媒体。Core Worker实际发送原字节的标准image_url data URI，Editor收到已授权文本和只读媒体身份/来源，不收到图片base64作为编辑目标。跨阶媒体引用按原source身份去重；不同时间/part出现的同图仍保留各自观察关系。固定CB和原WB不被图片投影修改。

## 官方接口与来源

`core_lme_memory.py`实现冻结官方`Memory.insert/query/save/load/post_query_hook`接口，注册`memory_type=sic_core_v01`。官方代码版本为`2cc8c540bdb87fe6761629b585e727e1c4704520`，数据版本为`f152293e235517d504809563c833d7190b8c713b`；逐文件SHA见`lme-official-source-lock.json`。插件查询只接收问题文本、可选问题图和官方随机run-local调用ID，不把question ID、gold或evaluator传入Core。

纯carrier入口在`core_lme_carrier.py`，无需导入官方SDK。`trajectory_request`保留完整轨迹顺序及截图；`load_prepared_trajectories`检查haystack顺序、原record与图像SHA；`question_request`只组装问题与受检问题图，不携带答案。

## 安装与准备

在独立Python 3.12环境安装`lme-official-requirements.lock.txt`，不要更新正在运行的Langflow/Studio环境。锁包含兼容Core的OpenAI/Agents及CPU Torch/Torchvision。准备官方源码至上述commit；wrapper的`--official-root`必须显式指向它，逐文件SHA不符即拒绝。

下载固定官方计数processor（约26MB原文件、7MB离线ZIP；不下载9B模型权重）：

```powershell
python benchmarks/prepare_lme_processor.py --output <processor目录>
```

`lme-processor-lock.json`锁定Qwen/Qwen3.5-9B计数processor来源版本、各文件SHA和Apache-2.0许可。离线解压后也可将其中processor目录传给下一步骤。Core图像预算采用已支持模型的真实LiteLLM图像算法、已核静态图片尺寸及明确low/high detail；未知模型、auto detail、动画或计量未知会拒绝派发，不用自编tokens充数。官方reader的记忆预算继续使用其Qwen实际张量计数与原截断算法。

先用官方public_data物化问题（含图片）、haystack及同序轨迹：

```powershell
python benchmarks/run_core_lme.py --official-root <官方源码> prepare-runtime --data-root <公开数据根目录> --domain web --tier small --output-dir <新runtime目录>
```

相同命令支持enterprise、`--question-id`和`--limit`。它不会扁平化问题图，不会改数据文件，也不会覆盖已有runtime目录。

## 执行

`configs/sic-lme-program.json`提供确定程序编辑，`configs/sic-lme-model.json`提供语义Editor路径。复制配置并显式填写可用的data_root；模型编辑另用`--sic-core-config`指向冻结Core roles/profiles配置，凭据只用环境变量，不放进memory_params。插件默认按原state自然边界形成每state一个WB（wb_state_count可显式指定每块state数量），保持同Agent连续两链及完整原序；不是把整条长trajectory塞给一次Editor。Worker/Editor完整输入仍超预算时明确拒绝，不截断原材料。

```powershell
python benchmarks/run_core_lme.py --official-root <官方源码> --processor-path <processor目录> harness --domain web --questions-path <runtime目录>/questions.json --haystack-path <runtime目录>/haystack.json --trajectories-path <runtime目录>/trajectories.jsonl --memory-config-path <memory配置.json> --output-dir <新结果目录> --model <显式reader模型> --base-url <reader地址>
```

Reader/evaluator配置及其它参数原样交给官方harness。仅建库可添加`--save-memory --skip-evaluation`，此时无需reader或processor；保存的Core档包含原截图，不依赖原数据继续挂载。正式结果须分别完成web/enterprise及独立质量评价，不能把建库或fixture通过写成官方分数。

`submission-point`和`submission-package`子命令分别转发官方两个本地提交包脚本的原参数，后者使用原LAFS算法；没有网络发布动作。单个局部研究runner分数不能冒充完整官方LAFS。

## 已运行验证的界限

Core媒体专项覆盖三阶Worker真实SDK图像载荷、Editor只读描述、跨ingest工具组、source分页、只图WB、未知/超限预算及有界省略manifest。官方专项实际运行官方Memory类/注册、保存恢复、public_data、harness建库、模型Editor SDK、Qwen真实计数与截断及LAFS函数。另新读一个真实small任务（100条轨迹且无缺图），首完整轨迹的10张图已在实际Worker SDK请求中逐字节、逐顺序核对。

上述均为软件与协议证据。未调用真实Provider评分；完整任务质量和LAFS研究结果保持not_evaluated，物理KV命中保持unknown。所有常驻服务与用户窗口未被这项工作操作。
