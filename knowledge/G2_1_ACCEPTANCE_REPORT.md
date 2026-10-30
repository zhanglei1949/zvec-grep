# G2.1 同次绑定、事实 Schema 与真实往返验收

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-23；补修：2026-10-24。G2.1 已完成：正式索引内部同次事实绑定、八张事实表与结构关系、真实 graphdb/zvec 关闭重开验证、Scope/Binding/Diagnostic 的 L1 体量及合成 ID 检查。

原始完整 feature 回归 **547 通过、0 失败、10 个既有手工测试忽略**；默认 KG 的 32 项通过。2026-10-24 补修后的完整回归仍有一项 ANN 只读摘要失败，已独立复现并记录；本次补修及验证范围见第 6 节；2026-10-26 原生补丁验收已通过，见第 7 节。G2.1 交付时未进入 G2.2，没有 STATE/POLICY/HEAD、GraphRoot/Receipt、联合选择、graph-only 调度或公开图查询，仍为 `published=false`、`search_ready=false`。G2.1 没有新增依赖、修改上游、提交或推送。ANN 修复后的 G2.2 进度单列于 [图内提交记录](G2_2_BUILD_COMMIT_REPORT.md)，不回填为 G2.1 的能力。

## 1. 实现与职责

| 位置 | 本轮变化 |
| --- | --- |
| engine `extraction/code.rs`、`extraction/mod.rs` | 观察器接收实际 ChunkOptions；共享 EntityId/FragmentId 绑定函数；可选事实失败保留同一棵树生成的检索实体 |
| engine `graph_facts/binding.rs` | 消费同次临时序号，绑定实际宿主 EntityId；捕获预算、模块诊断、成功文本写后接收事实、checkpoint 后核对完整宿主目录 |
| engine `graph_facts/module_roots.rs` | 显式模块根、规范指纹、最长前缀、真实路径边界和语言不匹配诊断 |
| engine indexing pipeline | PreparedFile 暂存；替换失败不接收成功事实；准备阶段取消/期限错误不再转成 failed 文件。正常服务入口仍不启用图，公共选项留 G2.2 |
| KG `relations/bound.rs` | 绑定后的文件/事实、构建内合成 ID、完整目录修订和紧凑解析结果；保留 G1 数据类型 |
| KG `relations/code.rs`、`markdown.rs` | 公共 resolver 保持原契约；内部逐条消费结果，不同时保留 R×F 份依赖版本 |
| KG `graph/` | 用具体 GraphDbStore 替换旧通用端口；Schema、typed 行转换、结构边、事务、错误与新只读确认 |
| Cargo feature | 新增默认关闭的 `zg-engine/knowledge-graph-graphdb`，启用既有候选检查及 `zg-knowledge/graphdb` |

详细 [事实格式](G2_1_FACT_SCHEMA.md) 与 [精确 DDL](archive/pre-g4-validation/README.md#g2-1 "包内：knowledge/reports/g2.1/schema.cypher") 已落盘。KgFactMetadata 是候选格式元数据，不替代发布根或成功回执。

图中没有正文、AST 或向量，也没有临时 entity_indices；只有实际 EntityId 映射。此次宿主接入不创建 KG 独立 collection，不调用第二次 Embedding。独立 ProjectionIndex 组件继续保留原有组件职责。

## 2. 验证范围与结果

| 验证 | 实际执行与判据 |
| --- | --- |
| 同次提取 | Rust/Python 在默认和非默认 ChunkOptions 下，观察器与原检索提取输出相同；没有索引结束后的第二轮解析绑定 |
| 真实双库 | 正式 indexing pipeline + 真实 IndexStore/zvec + 真实 GraphDbStore；Rust、Python、两份 Markdown 和 unsupported C++ 共五个文件。测试模型提供确定性向量，未使用真实模型运行时 |
| 实体与证据 | 回取真实 canonical Entity，核对每份来源的完整实体摘要、所有绑定 ID、引用位置对应正文；覆盖 BOM、CRLF、中文及 Python latin-1 cookie |
| 关闭重开 | 双库 checkpoint、释放拥有者、只读重开，事实和 EntityId 映射一致；图处理前后 Embedding 调用数不变 |
| 八表往返 | 独立真实图用例覆盖全部八张事实表、13 类关系、node/import/dynamic 三类绑定、重复锚点、resolved/ambiguous/unsupported/负解析、无 span 与空 span、u64 边界 |
| 结构复用 | 重开读回完整 scopes/bindings/Lookup/诊断，替换另一个来源的目标候选后调用同一纯 resolver，与重新产生对应输入的完整重算一致；不是持久化增量发布验收 |
| 失败 | 真实重复主键触发事务失败后没有部分业务行；删除结构边后拒绝不完整读回；拒绝非空创建、DDL-only 中断候选、未知格式和只读写入 |
| 预算与取消 | 真实图在取消或聚合预算不足时不写入；小 MATCH 行预算仍能写边。宿主使用内存存储替身验证替换/Embedding 失败不接收事实、图预算失败保留文本、准备阶段取消保留旧成功文件 |
| 大语料正式适配 | 冻结的 L1 目录通过正式 `write_facts` 写入、checkpoint、释放、只读重开、完整解码和目录摘要核对；与体量探针使用同一 catalog digest |

节点 parent 校验同时收紧为只能指向此前节点，拒绝自身为 parent。没有修改 G1 的正常逻辑身份或来源范围契约。

## 3. L1 三表体量与合成 ID

输入在本轮实现过程中的一个时点冻结：`git ls-files --cached --others --exclude-standard -z -- rust/crates` 中扩展名为 `.rs` 的 **232 个文件**，排除 target、thirdparty 和忽略文件。逐文件摘要、显式模块映射、输入选择规则见 [捕获清单](archive/pre-g4-validation/README.md#g2-1 "包内：knowledge/reports/g2.1/l1-capture.json")。后续实现与测试文件仍有变化，当前工作树复跑不保证同一数量或摘要；比较时必须先固定新的清单，不能把早期估算的 222 当作门槛。

L1 使用模型无关的捕获入口和**合成的 indexed 目录资格**测量事实存储，不代表已为整个仓库建立真实文本索引。真实宿主资格和 EntityId 由上一节五文件双库用例独立验证。

连续两次提取结果逐字节相同；改为 `max_chunk_chars=256, overlap=32` 后，结构、ordinal、三类合成 ID 均相同。该语料的 EntityId 集合也未变化，符合宿主“完整语义 Entity + 可变 Fragment 分段”的实现，不能据此声称分段参数未生效。

| 新增事实表 | 行数 |
| --- | ---: |
| KgScope | 15,313 |
| KgBinding | 23,013 |
| KgDiagnostic | 1,414 |
| 合计 | 39,740 |

| 六类结构边 | 条数 |
| --- | ---: |
| KgScopeParent | 15,081 |
| KgScopeOf | 15,313 |
| KgInScope | 23,013 |
| KgBindNode | 4,403 |
| KgRefInScope | 40,108 |
| KgDiagOf | 1,414 |
| 合计 | 99,332 |

其余五张事实表共 44,986 行：File 232、Node 4,635、Reference 40,108、CatalogRevision 11、Anchor 0。完整图共 84,726 个事实节点、148,927 条边；五表基线为 49,595 条边，完整边数约为其 3.003 倍。L1 只含 Rust；Markdown 与 Python 的形态由功能夹具验证，不从这些数值外推其他语言。

| 单次探索测量 | 五表基线 | 包含三表与六类边 | 差额 |
| --- | ---: | ---: | ---: |
| checkpoint 后文件长度合计 | 22,339,065 B | 36,538,545 B | 14,199,480 B，约 13.542 MiB |
| 参数化写入并提交 | 40.028 s | 91.017 s | 50.989 s |
| checkpoint | 0.482 s | 0.604 s | 0.122 s |

两个新目录使用相同空 Schema；基线只省略三表与六类关系的**行**，不写 ready 元数据，也不作为合法生产后端。用同一事务组织、同一来源和规则比较，逐表计数与原始时间见 [volume.json](archive/pre-g4-validation/README.md#g2-1 "包内：knowledge/reports/g2.1/volume.json")。磁盘指标是普通文件长度合计，包含原生元数据与 checkpoint，不是文件系统已分配块；计时是开发机单次顺序样本，期间有编译检查活动，未隔离为性能基准。未测 RSS、尾延迟或给出性能 Go。

第一轮大语料写边失败为 `update MATCH row budget exceeded`：所选上游在 Update MATCH 中先展开端点，之后才计算 WHERE（固定提交的 `src/execution/command_update.cc`）。已改为在模式属性里绑定主键，先按主键属性约束两端候选，保持 `max_intermediate_rows=1,000,000`；额外通过 16 行预算的真实回归。没有调高预算掩盖笛卡尔积。

## 4. 原始环境、命令和记录

沿用 G2.0 的 graphdb `3bf0d532ce928c02782c76d42cc289b1e49e8df4` 和无软链独立源码包重建的原生库，ABI 1.3；zvec Rust 0.7.2。macOS 15.1.1 / arm64、Rust 1.98、编译并发 14。本轮未修改或重新发布上游 binding，原生身份见 [G2.0 记录](archive/pre-g4-validation/README.md#g2-0 "包内：knowledge/reports/g2.0/verification.json")。

```bash
cd rust
cargo test --locked --offline -p zg-engine -p zg-knowledge \
  --features zg-engine/knowledge-graph-graphdb --all-targets -j 14 -- --test-threads=1
cargo test --locked --offline -p zg-knowledge --no-default-features --all-targets -j 14
cargo check --locked --offline -p zg-engine --no-default-features --all-targets -j 14
cargo clippy --locked --offline -p zg-engine -p zg-knowledge \
  --features zg-engine/knowledge-graph-graphdb --all-targets -j 14 -- -D warnings
cargo fmt -p zg-engine -p zg-knowledge -- --check
```

完整 feature 回归中 engine 491 项、KG 56 项通过；10 个忽略项为原有手工模型/性能用例，本轮新增及 graphdb/zvec 原生验证没有跳过。格式、Clippy、默认 feature 编译和 `git diff --check` 通过。

完整回归首轮在沙箱内为 423 通过、11 失败：十项回环 HTTP 测试无法监听端口，一项用户目录工作区锁无写权限。后续把 `ZVEC_GREP_WORKSPACE_REGISTRY`、授权 key 与 `ZVEC_GREP_HOME` 隔离到 `/tmp`，获准在沙箱外运行，最终完整通过；未修改 HOME、断言或忽略列表。机器可读命令、源文件摘要、结果与日志索引见 [verification.json](archive/pre-g4-validation/README.md#g2-1 "包内：knowledge/reports/g2.1/verification.json")。

模型无关的规模复跑入口：

```bash
cargo run --locked --offline -p zg-engine --features knowledge-graph \
  --example graph_fact_capture -j 14 -- /absolute/repository /tmp/new-g2.1-capture
ZG_G2_FACT_CATALOG=/tmp/new-g2.1-capture/catalog.json \
ZG_G2_VOLUME_REPORT=/tmp/new-g2.1-volume.json \
  cargo test --locked --offline -p zg-knowledge --features graphdb \
  --lib structure_cost_probe -j 14 -- --nocapture
cargo run --locked --offline -p zg-knowledge --features graphdb \
  --example fact_roundtrip -j 14 -- /tmp/new-g2.1-capture/catalog.json /tmp/new-g2.1-graph
```

最后一个目录必须是新目录。未提供 L1 输入时，体量测试仍实际运行真实底座，但使用小型功能夹具，不能计为 L1 结果。原生依赖需按既有构建说明准备；开发命令可复用 GRAPHDB_NATIVE_DIR，不写入生产依赖绝对路径。

## 5. 下一步与未覆盖项

G2.1 交付时的下一项为 G2.2：模型获取前预检查、持久化策略/失效栅栏、候选构建与根/回执、新只读确认、HEAD/manifest 选择及 graph-only 编排。当前图侧子集及剩余接线见 [G2.2 进度](G2_2_BUILD_COMMIT_REPORT.md)。

本轮没有验证真正的提交响应丢失、进程终止、断电、原生调用中的取消/期限桥接，或宿主发布与恢复竞争。Unknown 的适配层隔离不能替代原生故障与回执核对；旧读者、增量依赖失效和租约/清理仍按 G2.2/G2.3 验收。没有查询增益结论、公开 relations、CLI/MCP、Windows 支持或通用 KnowledgeBase SDK。

## 6. 2026-10-24 补修与审查收口

### 6.1 Python 扩展名

模块映射改为复用宿主 FileFormat 的扩展名目录，接受 `.py`、`.pyw`、`.pyi`，不再把后两者作为图映射错误传播到文本索引。包文件 `__init__` 在这些后缀下保持包身份；未知后缀仍拒绝，不猜测语言。此修复不增加类型存根优先级或运行时调用图语义。

新增模块映射回归先在旧实现上实际失败，错误为 `module extension mismatch`；修复后通过。另一个正式 pipeline 用例用内存存储替身和测试 Embedding 验证四个 Python 文件全部成功索引、绑定图事实与实际分配的实体 ID；真实双库用例继续保留。

### 6.2 代码消歧变更

Rust、Python 各增加一个真实 graphdb 用例。先用宿主解析器与绑定入口建立两文件候选，写入、checkpoint、释放拥有者，再只读重开取回 scopes/bindings/Lookup。分别检查：

| 变更 | 复用的持久化输入 | 必须发生的结果 |
| --- | --- | --- |
| 目标函数改名 | 调用文件及其 scopes、导入绑定、引用和 EntityId 映射 | 原唯一调用变为未解析，旧目标不再保留 |
| 导入别名改指另一个函数 | 未变目标文件的完整事实 | 调用转向指定的新定义，不能继续指向旧定义 |
| 参数遮蔽导入别名 | 未变目标文件的完整事实 | 调用变为未解析，诊断原因为 `shadowed_or_dynamic_binding` |

每个场景只替换一个变化文件的事实；其余输入直接复用图内读回结果。结果与独立完整重新解析相同，且核对目录修订变化、目标 ID 与负解析结果；已保存的旧候选仍保持原样。调用文件未变时引用 ID 不变，调用文件改版时引用 ID 随来源版本改变，不能当作跨修订逻辑符号身份。

这些用例为隔离持久化与消歧，对诊断入口产物显式设置合成 indexed 资格；不代表执行了真实文本更新或持久化增量发布。真实宿主资格继续由双库流水线用例验证，G2.2 维护与发布仍未实施。

### 6.3 验证与历史记录

| 执行 | 结果 |
| --- | --- |
| graph_binding 定向回归 | 8 通过，含新增扩展名用例及两种语言的真实图重开变更测试 |
| 完整 feature 全 targets，串行、沙箱外 | **546 通过、1 失败、10 个既有手工用例忽略**；失败为原 native_projection 的 ANN 摘要断言，未全绿 |
| Cargo 中断后单独补跑 relations | 4 通过；没有重跑失败的 ANN 用例来获取通过结果 |
| feature 全 targets Clippy | `-D warnings` 通过 |
| 默认 engine 全 targets 编译 | `--no-default-features` 通过 |
| 格式与差异 | `cargo fmt --check`、`git diff --check` 通过 |

新增四项测试在完整回归中均通过。原始 547 通过属于补修前的冻结记录；本次完整命令返回 101，不能改写为全部通过。早期测试中曾误把引用 ID 当作跨修订稳定身份；已按既有版本化引用契约修正测试，未修改生产身份规则。

原始 [verification.json](archive/pre-g4-validation/README.md#g2-1 "包内：knowledge/reports/g2.1/verification.json") 和 L1 测量继续冻结在 2026-10-23 的源文件与输入版本，不改写为补修后执行结果，也不把此次补修算作新的 L1 规模测量。补修的命令、源文件和文档摘要、早期失败与最终日志见 [补修机器记录](archive/pre-g4-validation/README.md#g2-1-followup "包内：knowledge/reports/g2.1-followup/verification.json")。

### 6.4 ANN 摘要漂移

已建立独立的 [ANN 只读摘要漂移记录](ANN_READONLY_DIGEST_DRIFT.md) 和 [观察元数据](archive/pre-g4-validation/README.md#g2-1-followup "包内：knowledge/reports/g2.1-followup/ann-observation.json")。另一会话的历史失败缺少原始日志，作为外部会话观察保留；它与本轮独立复现分别记录。本轮在沙箱外、`--test-threads=1` 下已独立复现相同现象：唯一变化是 `embedding.index.3.proxima`，完整日志及前后摘要见 [复现记录](archive/pre-g4-validation/README.md#g2-1-followup "包内：knowledge/reports/g2.1-followup/ann-reproduction.json")。原摘要断言、ANN 文件检查及忽略列表保持不变，没有通过重试收口。

本次补修时根因仍未定位。G2.2 的 graph-only 与后续 ReadSession 接入须独立验证宿主 zvec 只读文件行为；历史通过不能证明当前只读文件契约可靠，也不能把组件报告直接认定为 graphdb 事务错误。后续根因与修复见第 7 节。

## 7. 2026-10-26 ANN 原生修复验收

已定位只读 streamer cleanup 刷新秒级元数据、底层映射仍可回写的组合问题；在 SDK 对应的固定提交上构建无补丁/补丁库进行对照，原失败可稳定触发，补丁库通过。修复及构建入口见 [独立记录](ANN_READONLY_DIGEST_DRIFT.md#2026-10-26-根因修复与验收)。

通过 SDK 正常 `ZVEC_LIB_DIR`/`ZVEC_INCLUDE_DIR` 接入并核对 staged 原生制品摘要后，engine、KG、shared storage 的完整图 feature 回归为 **557 通过、0 失败、10 个既有手工用例忽略**。KG 原摘要断言保持，增加跨秒与分阶段检查；共享层验证 Flat/HNSW 三种距离度量、两个读者、查询/拒绝写入及 close/Drop；正式宿主 IndexStore 验证全文/向量/证据读取和关闭不改变文件内容或 mtime。Clippy 通过。

该结果绑定 [修复记录](archive/pre-g4-validation/README.md#ann-readonly-fix "包内：knowledge/reports/ann-readonly-fix/verification.json") 的源码、补丁和 native SHA；未经替换的旧远端预编译库仍受影响。未发布上游新 SDK，部署须选用明确的补丁制品，不把本地通过扩展为所有平台和版本的保证。G2.2 可在这份已验证的 native 基线上推进；G2.1 原有 L1、547/0/10 和补修 546/1/10 记录保持原样。
