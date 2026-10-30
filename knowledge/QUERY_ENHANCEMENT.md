# zg 知识图谱查询增强：代码与文档分别推进

日期：2026-10-23。当前主线为 zg 查询增强，独立通用 KnowledgeBase SDK 完整重写后置。通用设计与 Python 迁移记录保留作参考，不再作为 G0/G1 的实现前置条件。

## 范围与阶段

Rust/Python 首轮提供规则抽取的静态调用、导入和引用位置；Markdown 首轮提供章节、显式文档链接及其来源位置。没有代码与文档关联，文档指向已索引代码文件的链接标记 `unsupported_graph_target`。其他语言继续走已有普通检索。

| 阶段 | 目标 | 当前边界 |
| --- | --- | --- |
| G0 | 事实契约、两组问题、来源和证据标签、失败语义与 binding 接入门 | 已落地候选事实契约、16 题和文件摘要；初始标签待人工复核。普通搜索/图增强对照协议已写明，尚无质量测量 |
| G1 | Rust/Python 规则抽取与消歧、Markdown 章节和显式链接、可检查事实 | 已提供默认关闭 feature、来源版本/范围/实体映射、解析状态、示例与测试。不是生产图后端 |
| G2 | 实际 graphdb 适配、联合发布、增量失效和重开核对 | G2.0–G2.4 已接通真实构建、维护、选择、恢复与 L1 测量，并带残留收尾；代码域覆盖不足、候选重建成本、HEAD fsync 和 R1/R2 原生窗口见 [总验收](G2_ACCEPTANCE_REPORT.md)。没有公开图查询或质量收益结论 |
| G3 | explore、Augment／Constrain、CLI/MCP／daemon、覆盖与就绪状态 | G3.0—G3.3 已完成本地验证；当前接口与预算以 [G3 实施计划](G3_IMPLEMENTATION_PLAN.md) 为准，文档图后置 |
| G4 | 普通 zg 与图增强 zg 固定对照 | 依赖 G3；代码、文档独立报告收益和代价。既有 SWE-QA20 不变 |

G2 不使用替身通过代替真实 graphdb＋zvec 验收，不新增宿主 FFI、Python 子进程或备用生产数据库。上游最小能力见 [binding 接入要求](GRAPHDB_RUST_BINDING_REQUIREMENTS.md)。

2026-10-27 模块调整：zg-knowledge 现只承担图事实、消歧和存储；独立 SDK 组件已退役，摄入/分段/Embedding/调度复用 engine。见 [收敛报告](MODULE_CONSOLIDATION_REPORT.md)。A1/A2 的固定身份重放、提交后停止和 manifest 切换后响应问题已修复；直接证据与剩余原生窗口见 [故障窗口报告](G2_3_FAILURE_WINDOWS_REPORT.md)。

## 当前代码入口

- `zg-engine` 的 `knowledge-graph` feature 默认关闭。`graph_facts::inspect_sources` 接收调用方已选定、不可变的字节输入和模块映射，不遍历磁盘、不加载模型、不打开索引、不写图存储。
- Rust/Python 使用宿主同一次 Tree-sitter 解析，向 KG 输出自有结构；不会传递 AST 或新增 grammar。宿主原有实体提取仍使用该树。Markdown 使用 KG 的固定 CommonMark 解析库，关系范围与原有检索实体按 decoded UTF-8 区间映射，不改宿主分段。
- `zg-knowledge::relations` 提供文件/模块/符号/章节、词法绑定、引用、目标候选、状态与解析依赖。它是候选事实和纯规则处理组件，不是可查询的生产图数据库。
- [固定问题与运行协议](../benchmarks/zg-graph/README.md) 和 [集成测试](../rust/crates/zg-engine/tests/graph_facts.rs) 分别记录 G0 输入与 G1 验证。`questions.json` 不放在 `corpus/` 内。

```bash
cd rust
cargo test --locked -p zg-engine --features knowledge-graph --test graph_facts -j 14
cargo test --locked -p zg-knowledge --test relations -j 14
cargo run --locked -p zg-engine --features knowledge-graph --example graph_facts -j 14
```

底层 `graph_build` 示例输出 `published=false`、`search_ready=false`，明确候选 committed 不等于宿主 selected。engine 的 `graph_build` 示例驱动真实联合构建、graph-only、unchanged 和维护路径；G2.4 已完成本地 L1 三轮测量，文档域 ready、代码域 incomplete_coverage，候选重建成本与全部残留见 [G2 总验收](G2_ACCEPTANCE_REPORT.md)。relations 查询、遍历与 CLI/MCP 仍未接入；普通索引/搜索行为不启用此 feature，也不增加第二套正文或向量。历史 `ProjectionIndex` 已从活动源码移除；正式图增强使用 engine 已有 collection。

## 候选事实契约

`SourceVersion` 保存宿主 generation、规范化相对路径、原始字节摘要和 decoded UTF-8 摘要。`Span` 使用解码后 UTF-8 的 `[start,end)`；与原 KG 快照的 raw-byte `SourceSpan` 明确区分。G1 事实路径使用 UTF-8、正斜线的规范相对路径；不能表示原生非 UTF-8 文件名，G2 应逐文件诊断并保留普通检索能力。UTF-8 BOM 被宿主去除，CRLF 保留；Python encoding cookie 继续使用宿主解码规则，不能把 decoded 偏移直接用于原始字节。

节点逻辑 ID 由路径、语言、节点种类、限定名和重复声明序号生成，排除正文、摘要和行号。函数正文或前置空行改变不改变唯一声明的 ID；同名重复声明间插入/删除、改名和文件移动不承诺自动追踪。引用点身份绑定来源版本和范围，不能充当跨修订的符号身份。

`entity_indices` 指向同次宿主提取结果中的实体序号；G2.1 已消费这些序号绑定真实 EntityId，持久化层不保存序号。完整 [事实格式](G2_1_FACT_SCHEMA.md) 包含八张 typed 事实表、结构关系、目录修订与版本化 JSON。定义与调用点分别保存映射，未覆盖的导入/顶层位置允许空映射。G2/G3 复用已有正文；只有核对原始文件摘要和同一解码摘要后才允许从原文件复取未覆盖范围，无法复取必须诊断。G1 不额外保存原文快照。

代码的 `CodeModule` 明确 namespace、模块路径和是否为 Python 包；宿主负责按选定源码根给出，G1 不猜测工作区内不同 crate/package 的归属。解析器输出词法作用域、定义绑定、导入绑定和动态绑定，KG 只解析显式且唯一的目标。代码与文档分别解析，不跨域匹配同名节点。

| 状态 | 含义 |
| --- | --- |
| resolved | 当前候选目录内只有一个支持的静态目标 |
| ambiguous | 多个候选，保留列表；不任取一个 |
| unresolved | 未找到目标、参数/赋值遮蔽、动态接收者、语法错误或不支持的绑定形式 |
| external | 显式模块/链接指向候选目录之外；不表示已证明是已安装第三方依赖 |
| unsupported | 已知链接目标类型不受图规则支持，或带查询参数的文档 URL |
| outside_workspace | 绝对路径、越界相对路径或编码后的路径逃逸 |

`resolved` 仍是 G1 候选事实，不代表已发布、可以直接服务检索或完整运行时调用图。语法错误、通配导入及未建模的绑定形式会把该文件标为 incomplete；不从不完整来源/目标生成已解析关系。调用目标只接受直接声明且未被装饰器改写的函数；不将函数指针常量、赋值别名或类构造自动当成静态函数目标。动态方法派发、函数对象属性、导入重导出链、类型推断、宏展开和 cfg 求值均不做推断。Rust impl 仅保留检查信息，方法接收者不解析。Python 方法内的裸名跳过类作用域，局部参数/赋值不会误指同名全局函数。

Markdown 支持 CommonMark 链接（含已定义的引用式链接），代码围栏和 image 不当成文档引用。章节锚点规则固定为小写、保留字母数字与 `-_`、空白转 `-`，不是对所有渲染器锚点算法的兼容承诺；重复标题不自动猜编号后缀，目标保留 ambiguous 和诊断。HTML 自定义锚点、目录隐式 README 跳转暂不支持。链接目标经 URL percent 解码后再次检查 workspace 边界；不读取未提供文件验证其存在。未索引目标表达为“未索引/未确定”，不能声称文件不存在。

G1 的解析依赖保守记录整个同 namespace 代码目录或 Markdown 目录的来源版本，包括负解析。G2 必须额外固定目录修订，检测新文件、删除和选择规则变化；仅核对旧文件摘要不能证明负解析仍成立。该保守读集可能增加存储/校验成本，待真实底座测量后再缩小。

## G2/G3 必须兑现的维护行为

正文与向量只有宿主这一套。图只保存身份、关系、必要遍历属性、版本和位置，引用已有 Entity/Fragment；graphdb 连接跟随宿主 generation 生命周期。

完整重建在候选 generation 内构建，验证后由宿主 manifest 一次选择文本与图产物。graph-only 重建复用正文/向量。首版按文件增量维护事实、重算受影响目录引用，再构建完整图候选并切换图选择；不在当前图内原地打补丁。普通文本更新前由宿主持久化失效栅栏，即使关闭图 feature 也不能保留旧图就绪状态；重开还须核对实际文本目录，以检测不认识栅栏的旧二进制写入。正文原地替换期间或图未确认、版本不匹配时禁止图扩展。定义、模块导出、章节锚点变动重新解析依赖引用，不重复 Embedding。删除、排除、解析失败使旧关系失效。完整协议见 [G2 实施计划](G2_IMPLEMENTATION_PLAN.md)。

图查询固定只读事务，返回前批量核验来源、目标和解析读集及目录版本；未知提交隔离图实例，关闭并重开后核对回执。原生连接按操作独占；非 Send 事务在同一个有界 blocking 任务内创建、使用和释放。

G2 先接最小 in-process 图构建选项和内部 graph-only 操作，参数不稳定且不增加查询命令。G3 再稳定 `IndexOptions.graph`，增加 `ZvecGrep::relations`、`ContextOptions.graph`、关系证据/诊断与分域 Info 状态，CLI/MCP 同步接入。显式 relations 未就绪时返回错误；可选搜索增强失败时保留原始召回和诊断。初始上限为 5 种子、2 跳、20 补充节点、100 条关系检查、16 KiB 附加证据；超限标记截断，并受期限、workspace 与索引排除规则约束。结果过滤和中间路径过滤分别处理。

## 验收边界

G1 用例核对静态解析、身份、范围、预算、取消、来源替换/删除后的纯重新计算，以及固定题目的基础边；它们不验证真实持久化增量更新、旧读者、原子发布、恢复或查询增益。

G2 需要真实事务/部分写入/发布竞争/取消/未知提交/旧读者/重开测试。既有只读向量测试的沙箱差异继续保留，不改断言隐藏。G3 才能验证反向查询、有界路径、证据复取和降级。G4 按固定协议报告证据质量、延迟、构建/更新、RSS 和磁盘增量，代码与文档分别验收；实验默认关闭。
