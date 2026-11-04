# S2 设计与实现验收矩阵

2026-11-03 校准。设计依据为[知识图谱 FY27 S2 规划](https://yuque.alibaba-inc.com/proxima-se/cg5gr6/fygih1gn6wy02z7h)（2026-11-02 07:48:25 UTC）及 [KG 模块封装与接口设计](https://yuque.alibaba-inc.com/proxima-se/cg5gr6/khyqauxnxe0be129)（2026-11-02 09:36:05 UTC）。阶段编号是实施批次，六语言本地功能通过不代表完整八语言或完整 S2 交付。

当前 M0—M4、M6 的公共模型和六语言功能已交付；M5 按用户决定跳过。M7 功能及本机安装查询通过，六项成本门禁未过，构建成本继续暂缓。物理 Schema／事实格式 v7、daemon 协议 15，静态抽取规则 static-code-v8；旧规则图需显式重建，不复用旧 ready 产物。

查询效果 Q0—Q4、四语言 R0—R5 和后续两项 Review 正确性修复完成。v9 空行评分修正与产品改善分开；短预览正文去重修复使 requests Augment 必需证据完整覆盖均值 39.6% → 83.9%（9 题改善）。C++ 跨头文件继承及调用正文修复使 Constrain 同口径均值 37.5% → 62.5%；后续根限定和 Go range 遮蔽修复在冻结题集上持平。上述收益属于各自固定基线，不能相加为本轮收益。

四语言仍有 52 组／79 个缺失区间，Rust/Python v9 仍有 17 组／33 个缺失区间；四语言 20 次及 v9 五次既有预算退化保留。四语言退化为四题各五次的 output_bytes 原子申请超 65,536 B，未使用输出计数为 0，申请 100,679—120,628 B；无正文结构仍超限，不能归类为错误计费。默认预算、输出契约、题集及模型保持不变，未宣称全图召回率。

已完成的本轮实施报告和逐次制品归档按用户要求删除。活跃冻结输入、标准答案、协议及源码原样包保存在 [benchmarks](../benchmarks/zg-retrieval/README.md)，历史 v1 与修订 v2 并存；旧的迁移基线保留。核心设计、当前边界及未解决问题在本矩阵维护，不再追加重复阶段日志。

| 设计要求 | 当前实现及入口 | 已有证据 | 剩余工作 | 阶段 |
| --- | --- | --- | --- | --- |
| graphdb 持久化、候选提交、版本选择与恢复 | `zg-knowledge/src/graph`、`zg-engine/src/pipelines/indexing/graph` | G2 总验收；P0 typed batch 回归 | 原生事务中途故障、HEAD fsync 窗口与资源门禁 | 准备／持续验收 |
| 八语言代码构图 | `zg-engine/src/graph_facts/code` 支持 Rust/Python 及 M4／M6 的 Go／Java／C／C++ 静态范围 | G3、M3、M4／M6 逐语言证据及 M7 六语言混合工作区 220 次四入口对照 | JavaScript、TypeScript、八语言总验收；已记录的不支持语义和 C++ grammar 限制 | 代码图必交 |
| 符号、调用、导入、有界路径及来源证据 | `zg-knowledge/src/query`、engine explore pipeline | G3.1 入口对照；本轮相对导入及真实发布重开两跳回归；G4 v8 43/43 正常与 5/5 负向契约 | 本轮缺陷已闭环，后续语言扩展需补充相应来源证据 | 本轮完成／代码图持续验收 |
| explore 关系类型操作与结果分页 | 尚未实现。`ExploreOperation` 只有 `Semantic / Locate / Neighbors / Expand / ShortestPath / References`；关系类型只能作为 `Neighbors.relations` 与 `TraversalSpec.relations` 的入参过滤，没有独立的可用关系类型查询。结果截断只由 `Completion::{Complete,Partial}`、`stop_reason` 与 `BudgetUsage` 表达，没有 `cursor`／`page_size`，`Partial` 不可续读 | 语雀 11-02 [KG 模块封装与接口设计](https://yuque.alibaba-inc.com/proxima-se/cg5gr6/khyqauxnxe0be129)第 3、4.2 章 | 新增关系类型操作与游标分页，并保持四入口一致、预算与既有约束语义不回退 | explore 契约增强后续项（本轮登记，不实现） |
| 显式继承／实现关系 | Rust supertrait／trait impl 与 Python 显式基类已贯通 AST、解析、存储及公开遍历 | M3：真实发布重开、跨文件声明位置、失效／歧义、四入口对照 | M4／M6 已补 Java 显式继承／实现与 C++ 具名基类；JS／TS 及八语言总验收待完成，动态／隐式实现不推断 | 六语言当前范围／代码图必交 |
| Off／Augment／Constrain | engine context 与 KG augment／resolve_scope | G3.2／3.3；本轮冻结 v2 输入两端各 458 次调用，普通排序与范围约束通过 | 后续语言和文档接入继续沿用相同约束 | 本轮完成／持续验收 |
| 可控构建与更新成本 | 完整候选重建；typed batch 及核验内读回优化已接入 | GRAPH_BUILD_OPTIMIZATION；代码图收敛轮同语料三轮 graph-only 耗时下降 36.2%、RSS 下降 17.5%；M7 同 M3 960 文件对照已完成，未通过六项总体成本门禁；初构 resolver 整理轮同 1,002 文件语料三标签各三轮，fresh 耗时中位下降 17.89%、RSS 峰值中位下降 1.44%、图目录峰值增量不变 | 500 ms 单文件维护、低成本增量及总体性能门禁；M7 候选单文件更新约 15.5 秒；engine `--lib` 全量套件新暴露的 12 项只读改写 proxima 文件与 8 项图读取预算超时待分别定级 | 总体成本未过／按用户决定暂缓 |
| 公共 SourceNode、SemanticEntity、Episode、Fact 与业务关系 | 固定当前目录及必要更正记录以 v7 落盘；现有 explore 支持语义对象、来源和有限历史、MENTIONS 与实体事实选择；Provider 授权先于支持累计 | [M1](M1_TYPE_OWNERSHIP_AND_CONTRACT.md)、M2.2及 M3 C01—C07 验收；来源继承、更正、重开及固定输入恢复通过 | 原生故障窗口持续验收；产品文档／语义 Context 查询及第二宿主后置，不提供完整多版本图 | 公共模型本地功能完成／后续集成 |
| 可选业务 Schema | 尚未实现 | 新版设计第 2、5 章 | 构建输入、类别／端点／属性校验及版本依赖 | 文档阶段必交 |
| Markdown 结构、链接与统一查询 | `zg-markdown` 与 KG Markdown facts 已有基础；公开文档结构查询待接入，M3 仅贯通语义 MENTIONS 最小夹具 | Markdown 整合 P0—P2 | P4／P5、TextMap 跨侧验证、文档 explore／Augment／Constrain | 文档阶段必交 |
| 普通文档自动实体／关系抽取 | 尚未实现；外部导入不能替代自动抽取 | 新版 S2 规划明确为必交 | 确定抽取方案，交付来源证据、分项就绪及最小增量闭环 | 文档阶段必交 |
| 文档与代码双向关联 | 尚无公开完整闭环 | G3.4 之外的后续范围 | 文件链接、锚点、明确符号引用及跨来源更新 | 关联阶段 |
| Agent memory | 尚未形成原型 | S2 原型范围 | 用户／项目任务／会话事件、归并、更正及退出检索 | 原型探索 |
| 第二宿主与独立复用 | KG 类型不依赖 engine；独立接入未验证 | Provider 与模块收敛记录 | 第二宿主验证来源维护、版本与查询；据此评估拆分 | 能力探索 |
| 公开版本可用 | 图 feature 为可选；M7 实际本机 npm 包启动、索引及查询验证通过 | M7 6 次启动、53 组／159 次安装包查询，包内摘要核验通过 | 初次 macOS SIGKILL 原因、其他平台与正式发布验收 | 本机通过／发布待验收 |

## 配置、读取与资料维护

PackageLayout 和模块根的纯校验收敛为一份，语言转换复用同一入口，真实路径与重复配置检查保留。四入口验收 helper 归位为 `zg-engine/examples/m7_api.rs`，通过 Cargo example 构建，无需手动复制源码。

Constrain 复用同次范围核验读到的实体；最终正文读取借用这些实体，来源记录仅在同次读取内缓存。预检保留范围／版本核验，不生成最终调用正文；不跨请求或读取视图缓存。接口、冻结输入、模型、默认预算和输出契约未修改。

2026-11-03 本轮隔离快照验证：158 项 Rust 图相关测试、179 项评测框架及 8 项标注检查通过；最终范围／配置子集复验、默认 feature 编译、严格 Clippy 和格式检查通过。四语言 800 次与 Rust/Python v9 458 次同口径复测均与基线持平，无普通排序或证据回退、无新增退化；8 组边界 32 次、六语言／公共模型 55 组 220 次四入口及 8 组检索通过。其他任务源码及性能记录按摘要单列保留，未混入本轮验证制品；这不是当前并发改动组合的完整回归。

六份活跃冻结材料原字节迁入评测 fixtures，删除已完成阶段新增报告及归档副本 169 个文件、981,785,168 B。原迁移基线、核心设计和未解决事项保留。验证日志与删除清单留在仓库外，本轮不再生成重复归档包；本机运行目录可由 `/private/tmp/zg-consolidation-current` 读取。构建成本未复测，既有预算退化与召回缺口继续保留。

## 其他任务的独立记录与已知问题

2026-11-03 初构 resolver 整理（详见[对照输入记录](../rust/benchmarks/graph-build/fixtures/README.md#初构-resolver-冗余整理2026-11-03)）：按上一条暂缓构建成本优化的决定，本轮只整理新鲜全量构图的解析开销，不改六项成本门禁判定。知识层 relations 去掉 `FactCatalog::validate` 克隆并深比整份文件事实清单，以及每条引用重复物化的一份 G1 读集；持久化格式、批次预算与核验语义不变。同一 1,002 文件冻结语料、三标签同轮交替各三轮：初构耗时中位 15.862 → 13.024 秒（−17.89%），RSS 峰值中位 1,253.31 → 1,235.25 MiB（−1.44%），图目录峰值增量三方逐字节相同。插桩轮按 self 时间归因，只有 `fact_rows.resolve`、`import.validate`、`host.resolve` 三处 `FactCatalog` 解析调用点下降（合计 −2.184 秒，占总下降 94%），其余阶段在 ±0.04 秒内。两会话共 18 次构图，来源投影摘要与事实摘要各自唯一，关系计数、发布证明审计、另进程 TP 重开与 `build_record` 字段一致；TP、4,096 行／4 MiB 预算、Root／Receipt、checkpoint、HEAD CAS 及完整 FactCatalog／FactRows 读回核验全部保留。六项总体成本门禁仍未通过，本轮不据此改判。

本轮回归把 engine `--lib` 图相关子集从此前各轮的 88—90 项扩到全量 546 项，暴露两类此前未覆盖的失败，均与本轮 relations 整理无关：

- 预算超时类（8 项）：`ZG.ENGINE.DEADLINE_EXCEEDED`，出自 `storage/read_session/graph.rs:46` 的图读取截止。同一子集默认并发为 74 通过／20 失败，改 `--test-threads=1` 后为 82 通过／12 失败且日志中 `DEADLINE_EXCEEDED` 归零，即这 8 项只在并发争用下出现；HEAD 提交说明已记录同类 AdmissionTimeout 先例。属环境类失败，非产品缺陷。
- 只读改写类（12 项）：断言 `text_hashes(...) == before` 失败，每次差异只有一个文件——`storage/fragments_<digest>/0/embedding.index.3.proxima`（并发轮其中两项另含 `.5.proxima`），其余 119 项逐字节一致，内容哈希变化而 mtime 不变。串行单线程下这 12 项仍全部失败，与并发无关。

只读改写类的归因证据：专用的 `storage::store::tests::readonly_host_queries_close_and_drop_preserve_storage_files` 单线程隔离运行 6 次全部失败，其中 2 次为默认 feature（不编译 graphdb 与知识图谱），删除 8.3 GB 构建产物释放磁盘后仍失败；`storage/store` 实现与该测试文件均未修改；测试链接的 `libzvec_c_api.dylib` 与上午通过时逐字节相同（`06f183bf…`，原生制品时间 09-20）；把 relations 四个文件逐字节回退到 M4／M6 已接受基线后同样失败；A1—A3 只改关系解析的内部形态，无法改变哪些存储被打开。同一批用例在 10:31（88 项）与 10:57（89 项）两次通过，触发窗口的变化未完全定位，怀疑与内存压力下索引写回时序有关，本轮不下结论。按只读查询不得改动存储文件的既有不变量，这是待单独定级与修复的存储／原生层缺陷，不在本轮范围；图事实、摘要与发布证明不受影响，18 次构图摘要唯一、审计与另进程 TP 重开全部通过。

