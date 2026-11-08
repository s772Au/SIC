# 小规模真实DSH探索任务包

本包为新建、无隐私的三项合成任务，用实际DSH headless Harness、内置`read`及SIC工作上下文观察任务完成。模型经非固定网关路由时，结果统一标为 **nonfixed-gateway-exploratory**。它们不是固定模型基准、正式两臂研究或SIC收益证明；旧实验数字不参与本包。

## 任务与独立答案

| case | 阶段 | 观察点 | 工具资料 |
|---|---|---|---|
| retention | intake → answer | 原事实跨用户轮次及一次无关材料读取后保留 | intake.txt、operations.txt |
| late-correction | provisional → corrected | 先真实处理旧规则，再接收迟到更正；保留未改约束 | policy.txt、change.txt |
| multi-hop | resolve | 三次内置read逐级取得索引、路由、清单，并作确定算术 | index.txt → routing/r6.txt → manifests/b804.txt |

[tasks.json](../research/dsh-live-smoke/tasks.json)是**控制侧**清单，含明确ground truth和评分规则。只把准备器生成的stage提示交给Worker，绝不把整份清单或`answer`字段发给模型。七份fixture均为本项目新写的合成资料，按CC0-1.0提供；不使用原工作区、用户历史或旧benchmark结果。

## 准备和执行契约

```text
python scripts/verify_dsh_live_run.py prepare --output .local/dsh-live-smoke-prepared
```

必须使用新空输出目录。生成的`controller-plan.json`列出三个case的工作目录、逐文件SHA和stage提示文件，但不含答案。每case的`materials`只有该case的资料，提示文件在其外；真实运行日志与控制清单同样放在资料目录之外。执行者可进一步按profile约束`read`文件范围，工具allowlist仅内置`read`，不启用自定义成功标记工具。

执行由主控制器完成，本脚本不会启动DSH、访问网关或调用Provider：

1. 三个case分别使用新的隔离DSH Session；材料目录设为该case工作目录。
2. 按plan顺序逐stage传入提示，保存各自原始DSH JSONL。前一stage真实完成后，才发下一stage；同case用DSH公布的`--session-id`续行，同一case不得换Session。
3. `late-correction`不能提前把第二轮提示或change.txt内容加入第一轮请求。`multi-hop`首轮只提供index路径，其余路径从真实工具结果发现。
4. 保留隔离DSH Home下该显式Session的原始记录及SIC provider-projection；保存Core/bridge配置、实际返回模型标识、usage、失败和全任务账。网关显示的请求别名不能当已冻结的实际模型。
5. 不修改或覆盖失败轨迹，也不在原不确定尝试上暗重执行。重试应另有明确记录。

本包的材料短小，主要检验小规模真实接线及确定任务完成；它没有故意制造上下文饱和，不证明长链/N阶容量、噪音随块长变化、协作收益或相对压缩基线的优势。

## 离线验证与评分

```text
python scripts/verify_dsh_live_run.py verify --case retention --prepared-root .local/dsh-live-smoke-prepared --stage-log intake=PATH_TO_INTAKE_JSONL --stage-log answer=PATH_TO_ANSWER_JSONL --run-kind nonfixed-gateway-exploratory --dsh-home PATH_TO_ISOLATED_DSH_HOME --core-db PATH_TO_CORE_DB --output PATH_TO_REPORT_JSON
python scripts/verify_dsh_live_run.py verify --case late-correction --prepared-root .local/dsh-live-smoke-prepared --stage-log provisional=PATH_TO_PROVISIONAL_JSONL --stage-log corrected=PATH_TO_CORRECTED_JSONL --run-kind nonfixed-gateway-exploratory --dsh-home PATH_TO_ISOLATED_DSH_HOME --core-db PATH_TO_CORE_DB --output PATH_TO_REPORT_JSON
python scripts/verify_dsh_live_run.py verify --case multi-hop --prepared-root .local/dsh-live-smoke-prepared --stage-log resolve=PATH_TO_RESOLVE_JSONL --run-kind nonfixed-gateway-exploratory --dsh-home PATH_TO_ISOLATED_DSH_HOME --core-db PATH_TO_CORE_DB --output PATH_TO_REPORT_JSON
```

验证器只读文件，不调用网络、Core服务或模型。它检查冻结清单/资料/提示SHA，stage顺序及同一Session，内置read的真实call/result关联、成功状态、文件范围、完整返回材料和多跳顺序。Core数据库通过SQLite URI `mode=ro`在一个一致读事务中访问；逐个核对相关固定对象摘要与当前撤销记录，不写入数据库。可使用原隔离数据库或该run结束后的SQLite backup，不能在WAL活跃时只复制主db文件而遗漏WAL。

消费证据从原生`provider-projection`的request_id、request_digest和response_id出发，与数据库中的实际Worker请求及响应归属逐一相等核对。每份read的完整工具返回必须保存在该路线原始source中；随后应在实际Worker输入中保持原始pending WB的合法tool/result关系，或沿已发布WB→CB、跨阶有序members找到被该请求实际采用的CB/高阶祖先。CB必须在该请求的固定cut中发布并属于Worker选中的活动表示；请求中的CB片段必须与固定正文/有序op相符。目录中存在一个CB、后来的cut或仅有source归档都不足以证明采用。

合法COMPRESS不需要在活动Worker正文逐字保留原始工具返回；原档完整性与表示采用分别验证。若活动输入仅呈现CB片段，报告其呈现字符数及完整CB长度，不把这种结构采用等同于所有事实已保真。最终语义正确性仍由独立答案评分。CLI、Session和数据库是可信本地输入，不构成防人为伪造整套run的密码学证明。

质量只读最后stage的最终JSON对象，逐字段与独立ground truth比对。items/evidence忽略顺序但拒绝重复、缺项和额外项；其他字段精确比对，数字与布尔类型分开。迟到更正的旧值被明确拒绝。最终严格全对得1，否则得0，同时保留各字段准确率；格式错误属于实际任务失败，不由HTTP成功掩盖。缺失/矛盾/不完整执行证据为`not_evaluated`，不填成0或成功。第一轮provisional及ready是执行阶段，最终答案分数不单独证明第一轮的语义质量。

默认`--run-kind mechanical-fixture`仅检查机械证据，**不输出质量成绩**。显式exploratory模式还必须提供隔离Session和Core数据库证据；该模式是控制器对真实调用来源的声明，验证器本身不能核实网关内部实际路由或固定模型身份。

退出码0表示执行证据有效，**不是答案必然正确**；控制器必须读取`quality.score`与`quality.field_matches`。证据失败退出1，并写出`mechanical_passed=false`和`quality.status=not_evaluated`；真实完整运行的最终JSON答错则保留机械通过、质量0。

## 执行结果（由实际运行控制器填写）

本包制作阶段未调用Provider，未预填真实结果。

| case | 执行/证据状态 | 最终严格得分 | 实际模型/路由与调用账 | 原始证据引用 |
|---|---|---|---|---|
| retention | 待执行 | not_evaluated | 待记录 | 待记录 |
| late-correction | 待执行 | not_evaluated | 待记录 | 待记录 |
| multi-hop | 待执行 | not_evaluated | 待记录 | 待记录 |

记录个案与分母，不将三项合成任务外推成通用质量结论。ΔC/ΔT、资源缺价、真实质量及物理缓存命中分别报告；本包没有预设持平、帕累托优势或缓存命中。
