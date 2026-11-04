# 历史对照输入

g2.4-corpus-manifest.json 是原 G2.4 来源清单的逐字节副本，供 analyze.py 比较历史来源选择；不是当前仓库的覆盖清单。

原始位置：knowledge/reports/g2.4-resources/corpus-manifest.json。完整历史记录与摘要见 [归档索引](../../../../knowledge/archive/pre-g4-validation/README.md)。

## 同语料对照

`runner.py --corpus-root /absolute/path/to/frozen-checkout` 固定语料来源；
`--repo` 继续指定构建源码。两端使用同一 corpus-root，比较输出
`corpus-manifest.json` 的 `manifest_sha256`，并固定模型、原生库及运行选项。
省略该参数时沿用源码仓库语料。输出目录放在语料之外。

六场景对照使用
`--repeats 3 --scenarios text_full,text_and_graph,graph_only,unchanged,update_rust,update_markdown`。
分段诊断设置 `ZG_GRAPH_PROFILE=1`；默认成本复测去掉该变量。

## TP 导入批次对照

2026-11-02，在 Apple M6 上固定 981 文件语料、同一文本索引快照和 GraphDB
`c1a77be7d3327fdd72b1be23be8b2aac1dc2f8ae`。各档使用仅默认导入预算不同的
Release 二进制，分别串行运行三轮插桩和三轮无插桩构图。文本准备、独立审计和
重开检查不计入构图耗时；构图本身包含 TP 导入、核验、checkpoint 和 HEAD 发布。

| 每事务事实预算 | 事实写事务 | 无插桩耗时中位数（秒） | RSS 峰值中位数（MiB） | 新增图目录峰值中位数（MiB） |
| --- | ---: | ---: | ---: | ---: |
| 256 行／256 KiB | 1,021 | 29.085 | 1,216.23 | 180.28 |
| 2,048 行／256 KiB | 360 | 18.562 | 1,263.25 | 179.21 |
| 2,048 行／2 MiB | 127 | 15.128 | 1,217.80 | 178.81 |

首轮选择 2,048 行／2 MiB；后续扩档选择 4,096 行／4 MiB，见下节。首轮该语料耗时下降约 48.0%，事实提交
减少约 87.6%，RSS 基本持平。事务数来自对应插桩运行；另有三个固定适配层写提交、
23 个固定建表自动提交，以及最终 checkpoint。RSS 和磁盘为采样观测峰值，名义
采样间隔分别为 10／50 ms；磁盘按逻辑文件长度计，不代表分配块数或硬上限。

18 次构图事实摘要一致，独立发布证明审计及另进程 TP 重开复用均通过。已有候选
仍按保存的预算恢复，不使用新默认值重分批。单机热缓存结果不作为其他规模的性能保证。

`batch_compare.py` 复用现有 runner 的测量方法；使用 `--binary LABEL=PATH` 指定
已编译档位，`--audit-binary` 指定 engine 的 graph_audit 例程，`--text-seed` 指定
各档共用的临时文本快照，`--corpus-root` 固定语料，`--output` 必须是新目录。
加 `--profile` 采集事务计数，不加时测默认路径。若各档复用同一 Cargo 输出路径，
必须在一档测量结束后再编译下一档。

本机完整数据和源码身份保存在
`knowledge/.artifacts/tp-batch-20261102/summary.json` 与同目录证据包中。

## 更高预算补测

沿用上一轮冻结源码、981 文件语料、文本快照及 GraphDB 提交，增加 4／8 MiB
两档各三轮插桩和三轮无插桩运行，并在同一时段补测三轮无插桩 2 MiB 对照。
基准驱动也固定到冻结版本，避免主工作区的 v2 规则改动混入比较。插桩初段遇到
其他编译负载，其耗时不用于比较；下表均取无插桩中位数。

| 每事务事实预算 | 事实写事务 | 耗时（秒） | RSS 峰值（MiB） | 新增图目录峰值（MiB） |
| --- | ---: | ---: | ---: | ---: |
| 2,048 行／2 MiB（补测） | 127 | 16.261 | 1,155.53 | 178.81 |
| 4,096 行／4 MiB | 64 | 14.012 | 1,224.33 | 178.71 |
| 8,192 行／8 MiB | 32 | 13.605 | 1,244.41 | 178.66 |

默认值提高到 4,096 行／4 MiB：比补测 2 MiB 快约 13.8%，采样 RSS 增加约
68.8 MiB；与上一轮 2 MiB 的 15.128 秒比较，收益约 7.4%。8 MiB 相比 4 MiB
只再省约 0.41 秒（2.9%），却将单事务回滚／重做的预算单位再翻倍，暂不选为默认。
采样与单机波动口径沿用上节；取消响应未单独计时，不把批次平均耗时当成取消上限。

本轮 15 次构图的事实摘要一致，发布证明审计和另进程 TP 重开通过。导入恢复回归
同时覆盖旧的 256 行／256 KiB、2,048 行／2 MiB 候选，按保存预算续建后核验根与回执。
补测数据位于同目录 `higher-budgets-summary.json` 与 `higher-budgets-evidence.tar.gz`。

## v6 重复读回合并

2026-11-02，重新冻结当前 relations-v2、schema/facts v6 源码和 1,002 文件语料，
保持 TP、4,096 行／4 MiB 预算和上述 GraphDB 提交。先测未合并读回的 Release
基线，再测合并版本；每个场景各三轮无插桩运行，另各一轮插桩核对工作量。
文本快照、语料、原生库和选项固定，测量包含构图、checkpoint 和 HEAD 发布，
文本准备、独立审计及另进程重开不计时。

| 场景 | 耗时中位数，前→后（秒） | 耗时下降 | RSS 峰值中位数，前→后（MiB） | 图目录总峰值，前→后（MiB） |
| --- | ---: | ---: | ---: | ---: |
| 首次构图 | 14.445 → 12.234 | 15.3% | 1,398.97 → 1,242.92 | 184.01 → 184.01 |
| 已有图重建 | 19.535 → 15.136 | 22.5% | 1,965.62 → 1,785.56 | 244.38 → 244.38 |
| 带语义数据重建 | 20.807 → 16.283 | 21.7% | 2,087.61 → 1,836.73 | 312.89 → 312.89 |

重建从已选中的 Code 图切换为 Both，触发真实全量构建，文本版本保持不变。
语义场景含 1,000 个实体、1,000 个 Episode、2,000 个 Fact、3,000 条来源关联、
1,000 条 Mention，重建后语义摘要及数量一致。图目录总峰值包括保留的旧候选；
新增图目录峰值分别约为 184.01、184.01、190.39 MiB，前后基本不变。工作区总
磁盘峰值也基本不变，分别为 323.80、384.22、452.73 MiB。本次没有磁盘收益。

| 场景 | 全表读回，前→后 | 解码，前→后 | 读事务，前→后 | 写事务，前→后 |
| --- | ---: | ---: | ---: | ---: |
| 首次构图 | 6 → 5 | 3 → 2 | 71 → 70 | 68 → 68 |
| 已有图重建 | 8 → 6 | 5 → 3 | 75 → 72 | 68 → 68 |
| 带语义数据重建 | 8 → 6 | 5 → 3 | 77 → 74 | 70 → 70 |

事务数来自适配层显式事务计数，不含建表自动提交；事实批写分别为 65／65／67 次，
前后相同，建表及 checkpoint 流程也相同。

导入末次新事务确认同时比较完整预期 Catalog，engine 复用该确认，省去随后的
重复 `read_facts`；读取旧图时，在同一个新只读事务中核验事实和 Root/Receipt。
Root 提交前仍独立完整读回，提交后仍用新事务确认；逐批 prefix 核验、保存预算
恢复、checkpoint、HEAD CAS 和取消边界保留，不缓存校验证明，不修改持久化格式。

18 次无插桩构图及 6 次诊断构图均通过独立事实／发布证明审计和另进程 TP 重开复用；
同场景前后事实摘要一致。RSS 是观测值，单次波动较大，例如语义基线三轮为
2,087.61／2,094.22／1,652.72 MiB；中位数下降不代表所有运行的硬峰值下降。
沿用名义 10／50 ms 采样和逻辑文件长度口径，仅代表本机热缓存结果。早期重叠运行
的语义耗时及一次被终止的重复运行已排除，正式语义基线采用串行重测。

知识层 all-targets 122 项、engine 图相关单元测试 70 项、图事实集成测试 25 项均通过。
Clippy 收尾的等价整理后，知识层 122 项再次通过，语义夹具摘要复核一致；最终
Clippy（两个 crate、GraphDB feature、all-targets、`-D warnings`）、全工作区格式
检查通过。上表采用整理后的最终二进制重新运行的三组对比，保留首轮结果供复核。

`batch_compare.py --scenario fresh|rebuild|semantic` 运行上述场景，语义场景另传
`--semantic-seed-binary`（engine 的 `graph_semantic_seed` 例程，只操作临时工作区）。
驱动按共用文本快照加互斥锁；复用 Cargo 输出目录时，编译也必须串行安排。
完整数组、源码和二进制身份、审计结果、回归日志及冻结源码／语料位于
`knowledge/.artifacts/v6-readback-20261102/summary.json` 和同目录证据包。

## M7 对照校准（2026-11-03）

默认 Rust／Python 生成策略不再固定 raw contract，由各端自己的标准化入口选择并记录契约。六语言使用预先冻结的 `--graph-options FILE`，避免按目录名猜测语言；来源清单保留 `.C`／`.H` 与 go.mod。`runner.py --binary PATH --digest-test-binary PATH` 可固定已编译的 graph_build 例程和摘要探针，配合 `--corpus-root` 在同一源码清单上重跑六场景。

`batch_compare.py --reference-audit-binary PATH` 指定第一个基线二进制对应的审计器，其余使用 `--audit-binary`。各端仍独立核验规范 FactCatalog／FactRows 和 Root／Receipt；跨端比较 `source-projection-v1`，保留来源摘要、结构、位置、引用及目标，仅排除规则指纹与 generation 栅栏。不同契约不再强制整份物理摘要相同。历史报告和已冻结旧 runner 不回改。

## 初构 resolver 冗余整理（2026-11-03）

六语言 `zg-relations-v3` 源码，冻结同一 1,002 文件语料（manifest SHA-256
`0c6eceb2c7f6a778a9322566b238aa3047877928039c14fcfe205956b203ff7f`）、文本种子、原生库
和 `--graph-options`。三个二进制用 `batch_compare.py --scenario fresh` 加三个 `--binary`
同轮交替测量，每标签三轮；测量含构图、checkpoint 与 HEAD 发布，文本准备、独立审计与
另进程 TP 重开不计时。二进制 SHA-256：base `91b940eb…`、opt1 `e92dbceb…`、opt2 `b1011965…`。

改动只在知识层 relations 的内部形态，不改持久化格式、批次预算或核验语义：

- `FactCatalog::resolve` 拆为薄壳加借用式 `derive`；`validate` 改为重跑 `derive` 再与自身
  结果比较，不再克隆并深比整份文件事实清单。
- `visit_code`／`visit_documents` 向调用方交出本次解析所用的命名空间成员／文档集借用切片，
  由调用方决定是否物化 `dependencies`。`FactCatalog` 路径不再为每条引用克隆一份 G1 读集
  （约 67,033 条引用按命名空间规模展开，Σm² 为 104,343，最大命名空间 210 个文件）；公开
  `resolve_code`／`resolve_documents` 仍逐条物化，输出逐字节不变。
- `output_cost` 口径不变：把 `dependencies.len()` 换成等长的 `catalog.len()`／
  `documents.len()`，预算判定与改动前完全一致。

| 标签 | 耗时中位数（秒） | RSS 峰值中位数（MiB） | 图目录峰值增量（MiB） |
| --- | ---: | ---: | ---: |
| base（改动前） | 15.862 | 1,253.31 | 184.75 |
| opt1（Catalog 校验去克隆） | 16.265 | 1,196.25 | 184.75 |
| opt2（opt1 加去每引用读集克隆） | 13.024 | 1,235.25 | 184.75 |

base 到 opt2 耗时中位下降 17.89%，RSS 峰值中位下降 1.44%，图目录与工作区峰值增量三方
逐字节相同，本轮没有磁盘变化。opt1 单独的耗时与 RSS 都落在噪声内（同轮 base 有一次
21.881 秒离群），保留它的理由是 `validate` 不再复制整份事实清单。

插桩轮（同三标签、每标签三轮交替、`--profile`）按 self 时间中位归因，只有三处
`FactCatalog` 解析调用点下降，其余阶段全部在 ±0.04 秒内：

| 阶段（秒） | base | opt1 | opt2 | opt2 减 base |
| --- | ---: | ---: | ---: | ---: |
| `fact_rows.resolve` | 2.782 | 2.637 | 1.679 | −1.104 |
| `import.validate` | 1.472 | 1.397 | 0.931 | −0.541 |
| `host.resolve` | 1.471 | 1.407 | 0.932 | −0.539 |
| `build` 总计 | 14.604 | 13.881 | 12.288 | −2.316 |

三处合计 −2.184 秒，占插桩总下降的 94%。`host.resolve` 包的就是
`pipelines/indexing/graph.rs` 的 `FactCatalog::resolve`，`fact_rows.resolve` 每次解码各调
一次，`import.validate` 再调一次，正好是每轮四次解析。插桩轮 base 到 opt2 耗时中位
14.891 → 12.576 秒（−15.55%），RSS 峰值中位 1,284.47 → 1,215.83 MiB。

两个会话共 18 次构图，`source-projection-v1` 摘要唯一
（`e65bd01b5cbbeac497d52a1d6db2a3b94317e1ddae182bf71fbe28050369d6ca`），事实摘要唯一
（`1a0714e4def7ee27cb2dae8127dcd4ffae778c2382a20fb4bdf30c9e93a2934f`），关系计数逐项相同，
发布证明审计与另进程 TP 重开全部通过，`build_record` 除 `graphBuildId`、`operationId`、
`createdEpochMs` 外逐字段一致。TP、4,096 行／4 MiB 预算、Root／Receipt、checkpoint、
HEAD CAS、完整 FactCatalog／FactRows 读回核验与 StoreCounts／digest 全部保留。

回归：知识层 all-targets 132 项通过；engine 图相关 `--lib` 子集默认并发 74 通过／20 失败，改
`--test-threads=1` 为 82 通过／12 失败。20 项里 8 项是并发争用下的 `DEADLINE_EXCEEDED`，串行归零；
其余 12 项串行仍失败，每次只差 `storage/fragments_<digest>/0/embedding.index.3.proxima` 一个文件的
内容哈希，与本轮 relations 整理无关：`storage/store` 实现与该测试文件均未修改，默认 feature（不编译
graphdb 与知识图谱）下专用的只读存储用例同样失败，relations 四个文件逐字节回退到 M4／M6 已接受基线后
仍失败，测试链接的 `libzvec_c_api.dylib` 与上午通过时逐字节相同。归因与定级见
[S2 验收矩阵](../../../../knowledge/S2_ACCEPTANCE_MATRIX.md)，本轮不修复。静态检查：
`cargo fmt --all --check`、两 crate GraphDB feature all-targets 严格 Clippy 与 `git diff --check` 全部通过。

本轮只优化新鲜全量构图的解析开销，六项总体成本门禁仍未通过，不据此改判。RSS 是观测值，
单次波动大：opt2 三轮非插桩为 1,235.25／1,050.05／1,237.14 MiB，中位数下降不代表所有
运行的硬峰值下降。沿用名义 10／50 ms 采样和逻辑文件长度口径，仅代表本机热缓存结果。
完整数组位于本机 `/private/tmp/zg-perf-20261103/{fresh-3way,profile-3way}/results.json`，
属临时证据，不随源码提交；上表数字即为本轮的持久记录。
