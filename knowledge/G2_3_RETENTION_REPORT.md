# G2.3 候选保留、磁盘准入与可恢复清理

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-26。承接 [文本恢复与读租约](G2_3_RECOVERY_LEASES_REPORT.md)，本轮补齐候选资源检查和显式清理的失败续作。**G2 整体仍未完成。** 不新增依赖，不改变 Schema、ID、规则指纹、默认搜索行为，不创建第二套正文或向量。

## 1. 候选资源检查

[CandidateGuard](../rust/crates/zg-engine/src/pipelines/indexing/graph/resources.rs) 统计同一工作区全部 generation 的图候选，包含当前选择、旧版本、结果未知及已移到 trash 的产物。达到限额时拒绝新候选，不自动删除；已确认的当前图仍可复用。

| 内部默认值 | 限额 |
| --- | --- |
| 工作区图候选 | 8 份 |
| 图产物及清理元数据逻辑长度合计 | 1 GiB |
| 单候选逻辑长度 | 256 MiB |
| 文件系统最低剩余空间 | 256 MiB |
| 新候选额外余量 | 64 MiB |
| 目录遍历条目 | 100,000 个 |

这些是当前实现的保护值，尚未作为公开配置或规模承诺。统计拒绝符号链接及异常文件类型；磁盘探针按实际设备匹配挂载点，覆盖 macOS Data volume 的 firmlink。统计仅涉及图目录，沿用宿主写入锁。

检查发生在创建前、每批导入前、事实完成标记前、根提交前、checkpoint 确认后及 HEAD/manifest 选择前。[分批接口](../rust/crates/zg-knowledge/src/graph/import.rs) 增加事务外的可拒绝准入回调，原 observed 接口保持原义。中途拒绝保留已确认的批次前缀，不生成成功根；底座接口可按原计划继续导入。完整宿主 operation/attempt 重放仍未接通。

这是阶段边界检查，不能替代硬磁盘配额或原子空间预留。一批事务、DDL、checkpoint 和其他进程可能在两次检查之间消耗空间；实际写失败仍按原生结果处理。统计成本随图目录条目增长，其开销和默认值调优留待 G2.4 测量。普通文本更新已成功而可选图失败时，沿用现有降级语义。

## 2. 显式清理与失败续作

[清理记录](../rust/crates/zg-engine/src/storage/graph_state/discard.rs) 保存到 `graph/discards/<buildId>.json`，位于被删目录之外。调用方先取得现有写入准入、等待图读租约释放，并核对候选身份和 HEAD。

| 状态 | 已确认的动作 | 重试行为 |
| --- | --- | --- |
| registered | 外部记录已保存 | 将 builds 候选原子移到同 generation 的 trash；若上次移动成功但状态未保存，识别实际路径后继续 |
| detached | 移出后的状态已持久化 | 删除剩余文件；部分删除不丢失操作身份 |
| removed | 删除及父目录持久化完成 | 相同身份重复确认成功；Inspect 只读返回记录 |

阶段之间同步父目录。存在 HEAD 引用、矛盾的路径、过期身份或符号链接时拒绝删除，包括注册清理后 HEAD 被重新指向候选的情况。清理记录保存失败或物理删除失败返回 recovery_required；用原 operation/attempt 再次执行 Discard。只读检查不推进清理。

未解决记录或残留 trash 会阻止当前默认构建的 generation 清理，这项保护不依赖 graphdb feature。完成记录继续保留，计入字节和条目预算；没有新增后台 GC，也没有自动裁剪记录。文件删除是同步文件系统操作，不承诺中途硬取消，执行资源保留到真实结果返回。

## 3. 不完整 Schema 的处理边界

根提交调用之前必须先持久化 commit_pending。只有身份匹配，且持久化状态为 registered/validated/failed、commitEffect=not_applied、selectionEffect=not_selected 的未选中候选，才允许依靠协议证据清理不完整 Schema。

这种候选不能原生打开时，Inspect 返回可供重试的身份，并明确标记 known_precommit_protocol_evidence、native_candidate_not_inspected；不报告事实或图根就绪。该判断依赖上述持久化屏障，不冒充原生读取证明。

可能已经调用根提交的候选仍须重新可写打开，核对实际批次、根和回执；commit_pending/unknown 不能走提交前捷径。无法核验的产物继续保留。已提交候选的清理记录保留历史 candidate_committed，删除后的 facts_ready 始终为 false。

## 4. 验证与证据

| 类别 | 本轮覆盖 |
| --- | --- |
| 真实 graphdb＋zvec | 已选图加七份未知候选达到上限，复用成功，第九份拒绝，HEAD 不变且未知候选不删除 |
| 真实 graphdb＋宿主协议注入 | 实际 Schema 停在导入前，已知提交前状态可检查身份、删除及重试；相同产物标为 commit_pending/unknown 后拒绝删除 |
| 真实 graphdb＋准入注入 | 第一批后及最终标记前拒绝，实读确认前缀和未完成状态，无根/回执；关闭重开后按同计划续作并核对全部事实 |
| 真实文件系统 | 移动成功但状态未保存、实际部分删除后权限拒绝、移动后记录目录拒写；保留身份并可续作。HEAD 保护、符号链接拒绝、重复成功确认 |
| 资源边界 | 跨 generation/未知/trash 计数，当前候选增长，元数据条目上限，注入剩余空间的边界，实际文件系统剩余空间探针 |

磁盘不足边界使用注入的剩余空间值，未制造物理 ENOSPC。Schema/HEAD 状态注入和文件系统权限拒绝不代替原生提交中途响应丢失、强杀或断电测试。

完整五组件 all-targets 图 feature 回归：**693 通过、0 失败、10 项既有忽略**。逐包为 engine 534、KG 69、storage 5、daemon 79、protocol 6。本轮新增 12 项用例，均参加该回归；没有增加忽略项。默认构建另跑协议/清理 16 项、文本恢复拒绝 1 项，以及原正文读缓存 10 项（1 项既有手工性能用例忽略），均通过。

原生 graphdb 固定提交为 3bf0d532ce928c02782c76d42cc289b1e49e8df4；zvec 使用 [ANN 只读修复制品](ANN_READONLY_DIGEST_DRIFT.md)。运行环境为 macOS arm64、Rust 1.98.0、编译并发 14。完整回归在沙箱外执行，支持既有 loopback HTTP 夹具；不以确定性模型夹具推断真实模型性能。

图 feature 五组件与默认 engine/daemon 的 Clippy 均通过 -D warnings；默认 workspace 全 targets 编译、cargo fmt 和 git diff --check 通过。完整回归冻结的 234 项源码/依赖与最终源码一致。命令、原始日志、逐包结果、原生归档和实际动态库加载证明，以及默认构建、Clippy、格式和链接检查结果，见 [机器记录](archive/pre-g4-validation/README.md#g2-3-retention "包内：knowledge/reports/g2.3-retention/verification.json")。开发期编译/lint 失败日志保留，不计为通过证据。历史验收记录保持原样。

## 5. 剩余工作

- 固定 operation/attempt 的宿主任务重放。
- 原生提交中途响应丢失、进程终止及取消交错的组合验证。
- G2.4 实际规模、构建/更新/重开成本、新旧版本并存空间和资源检查开销测量，随后逐项汇总 G2 验收。
- 公开关系查询、图扩展、CLI/MCP 和证据回取留在 G3。

未提交或推送代码，不删除原仓库、旧知识库、模型或缓存。
