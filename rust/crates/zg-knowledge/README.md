# zg-knowledge

zg 的图事实与持久化组件。代码图和文档图分别处理；engine 负责来源读取、解码、检索分段、Embedding、正文/向量索引、任务和最终发布。这里不再提供独立知识库 SDK、快照存储、模型提供者、线程池或 zvec 投影 collection。

## 模块

| 模块 | 职责 |
| --- | --- |
| relations | decoded UTF-8 来源版本、稳定节点、引用点、作用域/绑定、代码消歧、CommonMark 章节/链接/锚点、歧义与诊断 |
| relations::bound | 与正式 EntityId 绑定的事实、目录修订和解析读集；不保存正文或向量 |
| processing | 输入/输出预算、取消和期限，以及稳定 canonical JSON/摘要；不拥有队列或 worker |
| graph（graphdb feature） | graphdb 适配、v2 业务实体／直接关系、私有解析表、事务、受预算限制的分批导入、根/回执、查询取消、关闭重开 |
| query | 视图与预算契约、类型化按需读取、有界代码 explore；租约、正文和异步任务由宿主提供 |
| error | 图处理边界的分类错误；原生事务 outcome 由 GraphError 单独表达 |

Rust/Python 的 Tree-sitter 留在 engine，同次解析产生检索实体和图事实。Markdown 关系解析固定使用 pulldown-cmark=0.13.5 与 ENABLE_TABLES；这次只调整实现位置，不替换 engine 的 Markdown 检索分段。

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
- GraphRoot 与成功 Receipt 在短写事务中提交，再用新只读事务确认。图根已提交与工作区已选中分别表达。
- GraphLimits.control 按操作桥接取消和期限；open、begin/commit/rollback、checkpoint 的上游接口不接受 CancelToken，不能承诺整个调用的硬墙钟上限。
- GraphDbStore::read_session 共享 Database owner 与隔离状态；连接、只读事务和取消控制按操作独立。engine 的既有读缓存/租约管理生命周期。
- Unknown 保持隔离，关闭并可写重开后显式核对。只读检查不恢复、不补回执、不选择候选。

模块收敛范围、逐项测试去向和新旧契约比较保留在 [收敛报告](../../../knowledge/MODULE_CONSOLIDATION_REPORT.md) 历史快照。A1/A2 修复、L1 资源实测、代码/文档分域状态及 HEAD fsync、R1/R2 残留见 [G2 总验收](../../../knowledge/G2_ACCEPTANCE_REPORT.md)；G2.4 已带残留收尾，不代表完整原生故障门通过。G3.1 已接入代码 explore，见[实现与验证](../../../knowledge/G3_1_IMPLEMENTATION_REPORT.md)；代码 Augment、Constrain 已接入，文档图查询待接入。历史独立 SDK 和迁移报告保留在 [历史索引](../../../knowledge/HISTORY.md)，不作为当前功能说明。

物理 Schema 与事实格式均为 v4，私有结构包含绑定可见性、通配导入、名称路由、固有 impl 归属和路径型类型别名；旧候选须显式重建。业务实体和关系模型保持不变。CONTAINS 只保留直接父子边；CALLS／IMPORTS／LINKS 的每个已解析引用在边上保存一次，未解析引用单独保存。作用域、绑定和目录留给构建／恢复，不参加业务遍历。TextMap 按来源共享，不复制正文、片段或向量。

代码抽取支持 Rust 模式／条件绑定、仓库内通配导入和重导出，以及 Python with、异常变量、推导式、global/nonlocal/del 和模式捕获。未知外部通配名称、动态目标和歧义保留引用诊断；Python 通配导入、宏模式绑定及新式注解作用域仍显式不支持。无约束、仅含生命周期参数或无参数的固有 impl 支持类型与方法均唯一的 `Type::f()`、`Self::f()`、use 别名及无泛型的路径型 type 别名；类型／const 泛型、生命周期约束或特化、where 条件 impl、trait 派发、接收者类型推断和宏展开仍不支持。来源完整性与静态解析率分别报告，ready 条件不变。

公开查询使用定位、节点、邻接、引用和来源的类型化分页读取，每页最多 256 条。发布资格仍逐页核对完整提交摘要和文本绑定，成本单独计时；业务查询不调用 read_facts 或构造 FactCatalog。旧格式明确拒绝，由显式构建产生新候选。
