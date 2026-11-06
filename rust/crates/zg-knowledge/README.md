# zg-knowledge

zg 的图事实与持久化组件。代码图和文档图分别处理；engine 负责来源读取、解码、检索分段、Embedding、正文/向量索引、任务和最终发布。这里不再提供独立知识库 SDK、快照存储、模型提供者、线程池或 zvec 投影 collection。

## 模块

| 模块 | 职责 |
| --- | --- |
| relations | decoded UTF-8 来源版本、稳定节点、引用点、作用域/绑定、代码消歧、CommonMark 章节/链接/锚点、歧义与诊断 |
| relations::bound | 与正式 EntityId 绑定的事实、目录修订和解析读集；不保存正文或向量 |
| processing | 输入/输出预算、取消和期限，以及稳定 canonical JSON/摘要；不拥有队列或 worker |
| graph（graphdb feature） | graphdb 适配、固定业务点／关系、私有解析表、事务、受预算限制的分批导入、根/回执、查询取消、关闭重开 |
| query | 视图与预算契约、类型化分页读取、有界代码及语义对象 explore、来源授权交集及必要业务历史；租约、正文和异步任务由宿主提供 |
| semantic | 语义身份、实体、Episode、事实、来源关联及结构化变更契约；当前目录、身份登记、完整来源贡献替换及持久化校验；公开读取复用 query::explore |
| error | 图处理边界的分类错误；原生事务 outcome 由 GraphError 单独表达 |
| service（graphdb feature） | KgService 薄门面和 GraphHandle；复用读租约，构建候选但不选择 zg 发布版本 |

Rust/Python/Go/Java/C/C++ 的 Tree-sitter 留在 engine，同次解析产生检索实体和图事实。Markdown 关系解析固定使用 pulldown-cmark=0.13.5 与 ENABLE_TABLES；这次只调整实现位置，不替换 engine 的 Markdown 检索分段。

engine 的图查询公开路径重导出这里的同一组请求、结果、视图、证据及预算类型；工作区、取消和宿主能力检查仍归 engine。纯查询契约不启用 graphdb。类型归属、新语义契约及持久化交接见 [M1 记录](../../../knowledge/M1_TYPE_OWNERSHIP_AND_CONTRACT.md)。

## 构建与测试

纯库编译不需要 zvec 或 graphdb 原生库：

```bash
cargo check --manifest-path rust/Cargo.toml --locked -p zg-knowledge --lib -j 14
```

真实双库共存测试仍保留 zvec 测试依赖。运行测试前按 [Rust 运行库说明](../../README.md) 配置原生库；需要 ANN 修复制品的环境见 [只读修复](../../../knowledge/ANN_READONLY_DIGEST_DRIFT.md)。默认测试和纯库编译的依赖范围不同。 删除包内 build.rs 后，原生共存测试需要显式设置动态加载路径，不能只设置链接目录：macOS 使用 DYLD_LIBRARY_PATH，Linux 使用 LD_LIBRARY_PATH；不要默认加载旧预编译库。

```bash
# macOS 原生测试；目录替换为经核验的修复库目录。
export ZVEC_LIB_DIR=/path/to/verified-zvec/lib
export DYLD_LIBRARY_PATH="$ZVEC_LIB_DIR${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}"
cargo test --manifest-path rust/Cargo.toml --locked -p zg-knowledge -j 14
cargo test --manifest-path rust/Cargo.toml --locked -p zg-knowledge --features graphdb --all-targets -j 14
cargo run --manifest-path rust/Cargo.toml --locked -p zg-knowledge --features graphdb --example fact_roundtrip -j 14
cargo run --manifest-path rust/Cargo.toml --locked -p zg-knowledge --features graphdb --example graph_build -j 14 -- \
  /tmp/catalog.json /tmp/build-spec.json /tmp/new-graph-candidate
```

`graph_build` 接收 FactCatalog、BuildSpec JSON 和一个必须不存在的新候选目录，演示无效 attempt 拒绝、重复提交幂等、checkpoint、关闭重开及根/回执核对。输出 `candidate_committed=true`、`published=false`、`search_ready=false`：底层 committed 只证明图内根与回执，不等于宿主 HEAD/manifest 已选择，也不等于分域 search ready。fact_roundtrip 仅演示事实往返；完整联合构建、graph-only 与维护示例位于 [engine](../zg-engine/examples/graph_build.rs)，恢复示例位于同目录。

## 维护约束

- 图位置绑定原始/解码摘要和实际 EntityId；返回证据前仍须核对来源、目标及解析依赖。不存在第二套历史正文存储。
- 相同维度不代表向量空间兼容；正式模型与索引兼容性由 engine 管理。本次移除旧 EmbeddingSpace 不等于 engine 已新增全部空间身份规则。
- write_facts_batched_admitted 在初始写入、各批事务和最终事实标记前执行事务外准入；拒绝保留已确认前缀，不自动提交图根或发布。
- 新候选的 TP 导入默认每事务最多 4,096 条事实／4 MiB 序列化参数，任一先到即切批；参数字节不等于原生内存上限。已有候选按 build.json 和图内进度保存的预算恢复，不用新默认值重分批；事实与进度仍同事务提交并独立确认。
- GraphRoot 与成功 Receipt 在短写事务中提交，再用新只读事务确认。图根已提交与工作区已选中分别表达。
- GraphLimits.control 按操作桥接取消和期限；open、begin/commit/rollback、checkpoint 的上游接口不接受 CancelToken，不能承诺整个调用的硬墙钟上限。
- GraphDbStore::read_session 共享 Database owner 与隔离状态；连接、只读事务和取消控制按操作独立。engine 的既有读缓存/租约管理生命周期。
- Unknown 保持隔离，关闭并可写重开后显式核对。只读检查不恢复、不补回执、不选择候选。

模块收敛范围、逐项测试去向和新旧契约比较保留在 [收敛报告](../../../knowledge/MODULE_CONSOLIDATION_REPORT.md) 历史快照。A1/A2 修复、L1 资源实测、代码/文档分域状态及 HEAD fsync、R1/R2 残留见 [G2 总验收](../../../knowledge/G2_ACCEPTANCE_REPORT.md)；G2.4 已带残留收尾，不代表完整原生故障门通过。G3.1 已接入代码 explore，见[实现与验证](../../../knowledge/G3_1_IMPLEMENTATION_REPORT.md)；代码 Augment、Constrain 已接入，文档图查询待接入。历史独立 SDK 和迁移报告保留在 [历史索引](../../../knowledge/HISTORY.md)，不作为当前功能说明。

物理 Schema 与事实格式均为 v8，事实契约为 `zg-relations-v3`；旧候选须显式重建。SourceNode 沿用 `KgEntity`，新增固定 SemanticEntity／Episode 点、MENTIONS／SEMANTIC_RELATION 边及来源／标量事实辅助记录。`FactCatalog::with_semantic` 接收当前规范目录，复用既有写读、分批导入和提交核验；`inherit_semantic` 保留范围外记录并接收宿主明确的来源观察，`apply_sources` 应用结构化完整结果，缺省／部分／失败保留旧贡献；公开语义读取通过既有 explore 返回有类型对象及出处。原生时间保留为 TIMESTAMP_TZ，属性为独立 JSON 对象，kind／predicate 不生成新表列，来源支持退出使用既有辅助表的 Withdrawn 角色，失效提及使用固定 KgDeferredMention 辅助表，均不参与业务路径。显式更正／撤回及 Episode 退出保存为固定 KgSemanticChange 辅助记录，最近语义尝试以 KgSemanticOperation 保存；既有来源替换不会重置已退出事实。Rust 宿主通过 `ZvecGrep::apply_semantics` 提交完整候选，恢复重放 build.json 中的固定输入；来源维护、必要更正、重开和固定输入恢复已通过本地功能验证。

CONTAINS 只保留直接父子边；CALLS／IMPORTS／LINKS 的每个已解析引用在边上保存一次，未解析引用单独保存。EXTENDS／IMPLEMENTS 已贯通 Rust 显式 supertrait／trait impl 与 Python 静态基类的抽取、解析和公开遍历；动态、泛型及隐式实现边界仍显式未解析。作用域、绑定、来源支持及目录辅助记录不参加业务遍历。TextMap 按来源共享，不复制正文、片段或向量。

代码抽取支持 Rust 模式／条件绑定、仓库内通配导入和重导出，以及 Python with、异常变量、推导式、global/nonlocal/del 和模式捕获。未知外部通配名称、动态目标和歧义保留引用诊断；Python 通配导入、宏模式绑定及新式注解作用域仍显式不支持。无约束、仅含生命周期参数或无参数的固有 impl 支持类型与方法均唯一的 `Type::f()`、`Self::f()`、use 别名及无泛型的路径型 type 别名；类型／const 泛型、生命周期约束或特化、where 条件 impl、trait 派发、接收者类型推断和宏展开仍不支持。来源完整性与静态解析率分别报告，ready 条件不变。

公开查询使用定位、节点、邻接、引用和来源的类型化分页读取，每页最多 256 条。发布资格仍逐页核对完整提交摘要和文本绑定，成本单独计时；业务查询不调用 read_facts 或构造 FactCatalog。旧物理格式（含 v7）明确拒绝并保留旧库，迁移或重建另行处理；不按空图继续，也不在查询中迁移。

M3 语义读取复用现有 SourceFilter、ObjectId、KgReadView、RequestContext 和 EvidenceProvider；Provider-owned SemanticAccess 默认拒绝，来源授权先于共享支持累计。当前／必要更正记录显式区分，历史正文不可用时清除未验证偏移并返回原因。`resolve_semantic_scope` 使用既有 TextMap／fragment 映射，非完整结果或缺失映射不能扩大范围。产品文档及语义 Context 检索仍待后续接入，详见 [当前验收矩阵](../../../knowledge/S2_ACCEPTANCE_MATRIX.md)。

M4／M6 通过既有 lookup、binding 行和查询路径增加 Go／Java／C／C++ 静态范围；Java 显式继承／实现与 C++ 具名基类复用 EXTENDS／IMPLEMENTS。辅助行 ID 保持 v7 的既有算法。当前物理格式中已核验的 v2 事实可供代码重新解析，当前查询及新提交仍要求 v3；这不包含旧物理格式的自动迁移。逐语言支持边界及剩余工作见 [当前验收矩阵](../../../knowledge/S2_ACCEPTANCE_MATRIX.md)。

M7 六语言混合工作区与公共模型的四入口功能验收见[当前验收矩阵](../../../knowledge/S2_ACCEPTANCE_MATRIX.md)。JS／TS 代码图本阶段跳过；同语料成本测量完成但六项门禁未过，不将功能通过记为完整 S2 或低成本增量维护完成。

一致性、发布版本、分页和旧格式处理以[本轮实现说明](../../../knowledge/KG_CONSISTENCY_IMPLEMENTATION.md)为准。当前事实只由有效 Support 支撑；缺省／部分／失败输入保留历史，版本未核验的支持不参与当前查询。查询引用绑定逻辑 generation；RelationTypes／Neighbors 使用稳定游标。完整候选重写的成本不等同于物理增量。
