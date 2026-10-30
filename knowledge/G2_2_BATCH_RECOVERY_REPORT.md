# G2.2 分批导入与显式恢复核对验收

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-26。本轮接续 [宿主协议验收](G2_2_HOST_PUBLICATION_REPORT.md)，完成正式候选路径的分批导入，以及实验性 in-process 恢复核对和继续选择。旧报告及其 663/0/10 回归保持原快照；本轮结果单独记录。

**本轮指定的分批导入与显式恢复核对已完成并验证。** G2.2 整体仍有宿主任务重放、原生取消/期限桥接及读者/清理门；没有公开图查询。

## 1. 实施范围与预算

| 位置 | 本轮行为 |
| --- | --- |
| [图导入](../rust/crates/zg-knowledge/src/graph/import.rs) | ImportBudget、计划摘要、同事务进度、完整前缀核验与显式固定计划续写 |
| [物理行映射](../rust/crates/zg-knowledge/src/graph/codec.rs) | 用借用的 Mutation 视图按节点先于边的顺序切批；不复制正文、向量或 AST |
| [图 Schema](../rust/crates/zg-knowledge/src/graph/schema.rs) | 仅新 batched candidate 显式创建独立版本的 KgImportProgress；普通 open 不加表 |
| [正式宿主构建](../rust/crates/zg-engine/src/pipelines/indexing/graph.rs) | create_batched_build + write_facts_batched；继续使用短根/回执事务及原选择协议 |
| [显式恢复](../rust/crates/zg-engine/src/pipelines/indexing/graph/recovery.rs) | 在既有写入准入和 blocking worker 内重开、核验、记录已知效果或继续选择 |
| [公共类型](../rust/crates/zg-engine/src/api/index/graph.rs)、[示例](../rust/crates/zg-engine/examples/graph_recovery.rs) | ZvecGrep::recover_graph；没有默认后端时先返回 unsupported，不创建工作区文件 |

没有增加第三方依赖、宿主 FFI、额外正文/向量 collection 或 Embedding。

当前默认每批最多 **256 条事实行、256 KiB 编码参数**，另有固定的一次进度更新。字节计数包含类型标签、字段名及 JSON 转义；单行超限、零预算和总编码输入超预算在事实写入前拒绝，不将大行强行塞入单批。

宿主仍限制 64 MiB 输入、1,000,000 项逻辑输出和 120 秒协作检查期限；图适配默认限制 100,000 条聚合事实行（含边）、每次查询结果和 1,000,000 条中间行。逐条参数化执行前及批次提交前检查取消/期限。上述数值是当前保护值，不是规模 Go，也不是原生执行时长、原生分配或进程 RSS 的硬上限。

完整目录和事实仍在内存中编码、验证；分批解决的是写事务和提交粒度，尚未实现端到端流式解析/校验。没有将 transaction admission timeout 宣称为内核执行时限。

## 2. 导入正常路径与失败行为

1. 验证完整 FactCatalog，生成稳定行顺序。计划摘要绑定事实摘要、预算、全部批次边界和每批摘要。
2. 在空候选登记单行 KgImportProgress；已有候选必须匹配原固定计划。打开不完整候选使用显式 open_import，不自动初始化空目录、补 Schema 或标记就绪。
3. 每个短写事务先比较期望进度，再写本批事实和累计完成批次、行数及前缀摘要。事务回滚不能留下独立于事实的进度推进。
4. 新的显式 READ ONLY 事务确认该批进度。取消只阻止后续批次，不抹去已经提交的前缀；结果未知时仍隔离原实例、保留宿主 pending 标记。
5. 重开后从真实表读回所有已提交行，按原预算重建批次前缀并核验数量和摘要。不能仅凭进度字段跳过数据，也不因目录存在就重发。
6. 完整行集逐项一致后，才在同一事务保存 KgFactMetadata 和 finalized。此时只是事实完整，仍未提交 GraphRoot/Receipt。
7. 继续执行既有短根/成功回执事务、新只读确认、checkpoint 和 HEAD/manifest 选择。候选进度不替代成功回执，成功回执不替代宿主选择。

图适配器可对同一固定输入和计划显式续写；更换内容、选择指纹或批次预算返回冲突。已经提交的同类边不会因续写重复插入。普通 read_facts 不接受仅有部分导入的候选。

## 3. 显式恢复契约

入口是 `ZvecGrep::recover_graph(GraphRecoveryOptions)`，不获取模型、不检查远程 Embedding 授权、不调用 Embedding。调用方指定 graph_build_id，可显式指定文本 generation；省略 generation 时使用当前已选 generation。

| 动作 | 行为 |
| --- | --- |
| Inspect | 取得既有工作区准入，只读打开实际文本/图，检查来源、实体映射、目录、批次前缀和根/回执；只报告效果，不写尝试诊断、checkpoint 或 STATE |
| Reconcile | 必须提供 Inspect 返回的 operation_id 和 attempt；重新可写打开并验证，checkpoint 后写回宿主已知效果；不修改成功回执、STATE 或选择指针 |
| Select | 同样要求 operation/attempt；仅对根/成功回执完整、当前已选文本 generation、stable STATE、实际来源/实体/目录、策略和期望旧 HEAD 都匹配的候选继续选择 |

结果将 facts_ready、candidate_committed、selected 和 eligible 分开表达，并返回批次计数与诊断。eligible 表示当前基准和来源允许该候选参与选择，不代替代码/文档各自的覆盖与就绪。结果仍无法判定时返回 recovery_required，不用 false 冒充已确认未生效。

- 部分事实但没有根/回执：核对实际前缀且确认两种提交记录均不存在后，可记为已知未提交。宿主后续索引创建新 build/operation，不自动恢复原任务；图适配层的固定计划续写是独立、显式的能力。
- 事实完整但没有根/回执：仍未提交，Select 拒绝；不会在恢复时补造成功回执。
- 根/回执存在且实际已选中：报告真实选择。来源或 STATE 已变化时仍可报告 selected=true，但 eligible=false；核对不恢复旧图资格。
- 根/回执存在但未选中：匹配当前基准时可显式 Select；另一个 HEAD 已发布、策略改变或来源变化时拒绝覆盖。
- 完整重建的候选 generation 未被 manifest 选择：即使该目录有准备好的 HEAD，也报告未选中。Reconcile 确认此效果后，既有宿主构建恢复才可清理；图恢复入口不补造文本 manifest。
- 指针、根、回执、归属或实际事实矛盾，无法重开，或者格式未知：保留证据并报 recovery_required。不按时间戳猜测，不自动删除。

恢复不修复 mutating STATE、不恢复模型/正文处理、不自动重放 operation，不跨越原有文本格式门。继续选择前关闭图原生拥有者，仍由原 GraphHead 比较/选择协议处理竞争和结果未知。被丢弃的异步调用不能提前释放后台 worker 持有的写锁。

先按 [原生只读修复说明](../rust/README.md#native-read-only-fix) 准备固定补丁库并设置 SDK 环境变量。本次 macOS 验收额外使用 `DYLD_LIBRARY_PATH="$ZVEC_LIB_DIR"` 固定运行时加载目录，顺序运行 Cargo 原生检查，并核对实际加载路径及 SHA；未验证其他平台的加载设置。

示例从仓库根执行，先从 Inspect 输出取得真实 generation、operationId 和 attempt，再决定是否写回核对或选择：

```bash
cargo run --manifest-path rust/Cargo.toml -p zg-engine \
  --features knowledge-graph-graphdb --example graph_recovery -- \
  /path/to/workspace BUILD_UUID GENERATION_UUID

cargo run --manifest-path rust/Cargo.toml -p zg-engine \
  --features knowledge-graph-graphdb --example graph_recovery -- \
  /path/to/workspace BUILD_UUID GENERATION_UUID reconcile OPERATION_UUID ATTEMPT
```

将 `reconcile` 换成 `select` 才会尝试选择；不会发布未选中文本 generation。示例参数都是占位符，不自动推断最近一次任务。

## 4. 真实验证与证据边界

| 验证 | 实际覆盖 |
| --- | --- |
| 小批次、字节预算和超限 | 真实参数化行写入；3 行批次、字节切分、超大行/总输入拒绝、空恢复目录不创建数据库 |
| 中断与续写 | 首批新只读确认后取消；关闭、可写重开，核验前缀后续写；改预算或选择输入拒绝，最终完整事实与原 Catalog 相等 |
| 进度与实际数据矛盾 | 真实删除已写 KgFile 行后，inspect_import 拒绝进度账本，不能跳过缺失事实 |
| 进程退出 | 子进程在首批确认后 exit(23)，跳过 Rust 析构；父进程可写重开，核验仅有已提交前缀并续写 |
| 只读恢复检查 | 正式 service + 真实 zvec/graphdb；注入宿主 select_pending 后 Inspect，整个图/文本目录文件摘要不变 |
| 已提交/未选中和竞争 | 真正构建批次与根/回执；显式选择成功；另一候选不能覆盖已更新 HEAD；错误 operation/attempt 被拒绝 |
| 部分与无回执 | 部分导入及事实完整但尚无根/回执分别核对；均不能选择；dirty STATE 原样保留 |
| 元数据矛盾 | 修改宿主固定输入使其与真实根不一致；返回 recovery_required，保留尝试证据 |
| manifest 实际写入失败 | 原有真实文件系统拒绝用例扩展：只读检查保留 pending，不能选择未发布 generation；可写核对后既有恢复可丢弃已确认未选产物 |
| 默认构建 | 未编译 graphdb 时恢复先返回 unsupported；既有文本栅栏回归继续执行 |

### 最终检查

| 检查 | 结果 |
| --- | --- |
| 五组件图 feature all-targets 完整回归 | **670 通过、0 失败、10 项既有忽略**；engine 517、KG 63、shared storage 5、daemon 79、protocol 6 |
| 分批导入定向测试 | **4 通过**；含真实数据矛盾检查和子进程退出 |
| 宿主图定向测试 | **24 通过**；含本轮恢复、已有维护和发布边界 |
| 默认构建专项 | **11 通过、0 失败**，含无 graphdb 的恢复拒绝与真实文本栅栏 |
| 图 feature / 默认 Clippy | `-D warnings` 均通过；默认 workspace 全 targets 编译通过 |
| 格式与摘要 | cargo fmt、git diff --check、219 项受检源码/依赖摘要及文档相对链接通过 |
| 原生加载 | 完整回归实际加载固定补丁目录的 zvec；原制品与 staged runtime SHA 一致，graphdb 固定提交/35 个原生归档摘要核对通过 |

命令、源码/原生身份、日志及分项统计保存在 [机器记录](archive/pre-g4-validation/README.md#g2-2-batch-recovery "包内：knowledge/reports/g2.2-batch-recovery/verification.json")。模型计算只在 service 夹具内替换为确定性提供者，图与文本/向量存储使用真实底座。旧 HEAD 变化时 Inspect 同样报告 eligible=false，选择时仍做最终基准比较。修正该诊断前的一次 670/0/10 完整回归及其源码摘要另存，不与最终源码的回归合并。

早期宿主运行出现 21 通过、3 失败，三项均只观察到 embedding.index.3.proxima 摘要变化。当时存在其他 Cargo 构建/原生制品 staging 活动，且没有捕获该进程实际加载库的摘要，原因仍未归属；原始失败日志及差异清单保留，不用重跑抹去。随后固定从独立只读补丁制品目录加载原生库，并顺序执行 Cargo 检查，单列记录实际加载路径和 SHA。仅据这些环境下的结果验收，不宣称旧远端预编译库或所有环境均已修复。

本轮“未知回包”是对真实已提交/已选择效果的宿主标记注入；进程退出发生在已确认批次之后。它们不等于绑定在原生提交中途丢失响应、SIGKILL 中断提交或运行中的原生取消/期限竞争；这些仍未覆盖。没有跨平台、规模 Go 或性能增益结论。

## 5. 后续门

- 完整宿主 operation/attempt 重放、正文处理和 dirty STATE 的显式修复流程；目前只核对候选，不建设自动任务恢复。
- 原生提交响应丢失、中途进程终止与取消/期限桥接；当前批次检查是协作边界。
- 联合图读会话、旧读者租约及完整 GC/drop 接线；本轮没有新增扫描图目录删除旧产物的入口。
- daemon 能力协商、正式 CLI/MCP 图选项、公开查询和规模/资源测量。

同步更新 [G2 实施计划](G2_IMPLEMENTATION_PLAN.md)、[知识目录](README.md) 和 Rust 组件说明。未改上游 binding，未提交或推送。
