# G2→G3 交接：契约、代码覆盖与构建成本

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-28。范围是 [G3 计划 2.4—2.5](G3_IMPLEMENTATION_PLAN.md#24-交接改动) 的前两步：固定契约、修正覆盖、分析成本。业务图格式、TextMap 落盘和按需读取仍待交接。

## 契约与覆盖

- 业务节点、直接关系、UnresolvedRef、TextMap 的字段、身份、写入归属和升级顺序已写入计划。既有八表格式不成为公共查询模型。
- `ReadViewRequest` 显式指定域、关系与 TextMap 需求；`KgReadView` 只能保留满足需求的就绪域能力。Optional 不满足时，由 Provider 提供文本视图及原因。现阶段没有真实 Provider 或图查询接入。
- TextMap 的 entity_id 指 zg 检索实体；每段非空、等长且字节一致。片段范围复用 zg 元数据，版本和摘要归同一来源／构建。
- 图构建结果和 HEAD 增加逐来源状态及实际分母。缺模块根、语言冲突、缺事实、事实不完整和解析未执行分别报告；未解析目标不等于未执行解析。旧结果没有分母时保留 unknown，不补造完整性证据。
- 核验规则加入 `source-coverage-v2`；旧规则产物不能直接复用为本轮完整性证明。Schema 和事实格式未升级。

原 G2 冻结清单的三个表面缺口已定位：两个 `benchmarks/zg-graph/corpus/rust/src/*.rs` 被更宽的 Python 根覆盖，已在基准配置中补上明确 Rust 根；`.github/scripts/readme-demo.py` 则被宿主默认隐藏目录规则排除。原来应分别记录“冻结代码 356、宿主必需来源 355、有事实 353”。没有通过缩小宿主必需来源范围解决缺口。

本轮冻结语料新增了交接与 G3.0 文件。实际代码来源 368 个均有事实，但只有 108 个 `complete=true`；其余 260 个受现有抽取器的整文件保守降级影响，涉及不支持的绑定形式、通配导入等。新判定明确返回 `not_ready(incomplete_coverage)`，不再用“有事实”代替“事实完整”。文档域 161／161 ready。

这 260 个来源是抽取／解析能力限制，不是本轮丢文件；本轮保留其原有解析行为和诊断，没有放宽准入或扩展语言规则。后续构建交接需处理整文件降级粒度，并分别报告处理覆盖、引用解析和正文映射。完整逐来源结果见 [coverage.json](archive/pre-g4-validation/README.md#g2-g3-handoff "包内：knowledge/reports/g2-g3-handoff/coverage.json")。

## 构建耗时

通过 `ZG_GRAPH_PROFILE=1` 显式开启观测，按同步工作线程汇总阶段耗时和调用数；普通构建不输出计时。没有改变批次、SQL、事务、核验、checkpoint 或候选重建策略。

输入为本轮冻结清单，Release、Rust 1.98.0、Apple M4 Pro、CPU 模型 `local/potion-code-16m-v2`，禁止远程 Embedding。每场景新进程、模型文件缓存已热；这是单轮诊断，不替代原 G2.4 三轮阈值测量，也不声称性能改善。输入字节、原生版本和命令见 [analysis.json](archive/pre-g4-validation/README.md#g2-g3-handoff "包内：knowledge/reports/g2-g3-handoff/analysis.json") 与[冻结清单](archive/pre-g4-validation/README.md#g2-g3-handoff "包内：knowledge/reports/g2-g3-handoff/corpus-manifest.json")。

| 阶段（秒） | graph-only | 单 Rust 文件更新 | 无变化 |
| --- | ---: | ---: | ---: |
| 外层完整墙钟 | 178.584 | 192.684 | 8.546 |
| 图工作线程 | 177.791 | 189.800 | 7.107 |
| 旧图读取与提交证明核验 | 0 | 7.230 | 6.859 |
| 事实收集／提取／解析 | 2.064 | 1.206 | 不执行 |
| 候选导入 | 168.436 | 173.921 | 不执行 |
| 根／回执提交及只读确认 | 3.158 | 3.122 | 不执行 |
| checkpoint | 0.748 | 0.899 | 不执行 |
| 图选择 | 0.215 | 0.224 | 复用原构建 |

graph-only 零模型事件、零下载。导入含 1,311 个写批次；212,484 次单边写入累计 95.763 秒，58,279 次引用节点写入 17.712 秒，写批次事务提交累计 9.322 秒。单文件更新仍写入相同数量的边，单边累计 98.058 秒；事实增量没有变成图数据库原地增量。

上述计时有嵌套关系，不能直接相加。逐行写入时间包含原生查询编译、执行和结果处理，未细分这三部分；不能仅凭它断言某个索引或事务内核存在问题。当前证据表明候选导入是首要成本，事实提取与解析不是主导。

无变化场景约 6.9 秒用于打开旧图、核对提交证明并读取事实，其中两次 `read_facts` 分别为 2.641、2.648 秒。它不是零成本复用；该证据支持后续拆开资格核验与按需业务读取，不支持删减正确性核验。

本轮不做性能重构。后续优先评估单行语句数量及辅助事实写入量，其次处理旧图读取和重复核验；批量写入或原地增量方案仍需保留现有提交、并发和恢复语义。

复现：先用 Rust 1.98.0 构建 `zg-engine` 的 Release `graph_build` 示例并启用 `knowledge-graph-graphdb`，再执行 `python3 rust/benchmarks/graph-build/analyze.py`。单轮诊断保存在独立目录，不覆盖 G2.4 历史测量。临时工作区、原生版本、二进制摘要和各场景命令均记录在 analysis.json。

## 验证与边界

| 检查 | 结果 | 记录 |
| --- | --- | --- |
| graphdb＋zvec 原生五包回归，all-targets | 696 passed／0 failed／10 ignored | [日志](archive/pre-g4-validation/README.md#g2-g3-handoff "包内：knowledge/reports/g2-g3-handoff/final-five-package.log") |
| 默认 feature engine 图测试 | 17 passed／0 failed | [日志](archive/pre-g4-validation/README.md#g2-g3-handoff "包内：knowledge/reports/g2-g3-handoff/default-engine.log") |
| 默认 feature KG | 21 passed／0 failed，含 compile-fail doctest；0 项门控目标不计数 | [日志](archive/pre-g4-validation/README.md#g2-g3-handoff "包内：knowledge/reports/g2-g3-handoff/default-knowledge.log") |
| Clippy，图 feature 的 engine／KG，默认 engine／daemon，all-targets | `-D warnings` 通过 | [图 feature](archive/pre-g4-validation/README.md#g2-g3-handoff "包内：knowledge/reports/g2-g3-handoff/clippy-graph.log")、[默认](archive/pre-g4-validation/README.md#g2-g3-handoff "包内：knowledge/reports/g2-g3-handoff/clippy-default.log") |
| 模块根配置回归 | Python unittest 1 项通过 | [记录](archive/pre-g4-validation/README.md#g2-g3-handoff "包内：knowledge/reports/g2-g3-handoff/verification.json") |
| workspace 格式及 diff | 通过 | [记录](archive/pre-g4-validation/README.md#g2-g3-handoff "包内：knowledge/reports/g2-g3-handoff/verification.json") |

新增回归覆盖：错误域和缺少映射／关系能力不得获得图视图；Optional 无图保留租约语义；缺模块根与缺事实分开报告；语法不完整不得 ready；解析已执行但目标未解析可以保留；修复来源与配置后同批来源恢复 ready；Rust 基准根覆盖宽泛 Python 根。TextMap 还检查空段及真实字节一致性。

首次完整回归在沙箱内因本机端口及默认注册表权限失败，保留[原日志](archive/pre-g4-validation/README.md#g2-g3-handoff "包内：knowledge/reports/g2-g3-handoff/five-package-sandbox.log")。上表五包结果来自允许本机测试端口、且注册表和应用目录均隔离于临时目录的完整重跑。

Release 编译退出码为 0；工具链的 `rust-objcopy` 因缺少 libLLVM 无法 strip，产生构建警告，保留[日志](archive/pre-g4-validation/README.md#g2-g3-handoff "包内：knowledge/reports/g2-g3-handoff/release-build.log")。优化构建和实际诊断均成功；不据此作产物体积结论。各检查使用 Rust 1.98.0 和已记录的原生版本。

当前可交接的是契约、覆盖记录与成本证据。代码域仍未 ready；业务图／TextMap 的格式升级、按需读取及 G3 查询尚未实现。G2 的原生事务中途、HEAD fsync 和大语料验证残留未因本轮回归而消除。
