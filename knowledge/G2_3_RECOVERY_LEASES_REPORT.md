# G2.3 文本恢复、联合读租约与候选清理

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-26。本轮承接 [G2.2 原生控制](G2_2_NATIVE_CONTROL_REPORT.md)，推进显式文本恢复和读者生命周期。**这些能力已接入实验性代码，G2 整体仍未完成。** 不增加正文/向量副本、Embedding、依赖或公开关系查询。

## 1. 本轮变更

| 位置 | 行为 |
| --- | --- |
| [公共恢复类型](../rust/crates/zg-engine/src/api/index/graph.rs)、[SDK](../rust/crates/zg-engine/src/lib.rs) | 新增 recover_text_state；默认检查，修复要求 generation、operation、attempt、textRevision 全部匹配。recover_graph 增加 Discard 和 discarded 结果 |
| [文本恢复](../rust/crates/zg-engine/src/pipelines/indexing/graph/text_recovery.rs) | 使用实际只读 zvec 目录和现有扫描器核对全部文件已索引、成员集合及原始字节摘要；修复先撤销 HEAD，再递增文本修订、保存新身份的 stable STATE 并实读确认 |
| [联合读缓存](../rust/crates/zg-engine/src/storage/read_session.rs)、[图会话](../rust/crates/zg-engine/src/storage/read_session/graph.rs) | 图拥有者放入原 ReadSession，原生句柄先于 residency 锁释放。缓存键包含 HEAD、文本状态、扫描规则与不可变构建输入摘要；每次借出和读取均检查当前 manifest/POLICY/STATE/已确认选择，Unknown 不能授予读资格 |
| [共享构建输入](../rust/crates/zg-engine/src/storage/graph_state/build_record.rs) | 将原 BuildRecord、规则指纹和 EntityId 绑定核验移到 builder/reader 共用位置，保持现有 JSON、ID 与 Schema 含义 |
| [图适配器](../rust/crates/zg-knowledge/src/graph/graphdb.rs) | read_session 共享上游 Database 拥有者和隔离标记；连接、显式只读事务和取消控制按操作独立，读取取消不污染后续请求，不添加 Send/Sync 或 FFI |
| [候选恢复](../rust/crates/zg-engine/src/pipelines/indexing/graph/recovery.rs) | Discard 要求检查返回的身份，重新可写核验、关闭原生拥有者并确认没有 HEAD 引用后删除。沿用写入准入，等待活动租约结束；不另建 GC 线程或后台扫描 |

多个 engine 缓存通过弱拥有者登记共享同一路径的原生图实例；登记没有独立租约、TTL 或工作线程。原生 open/close 不持有全局目录映射锁。缓存的图 owner 不绑定某次请求的取消控制，实际操作通过新的只读 facade 注入本次 QueryOptions。

普通文本搜索仍使用原 acquire，不因编译图 feature 自动打开图库。acquire_graph/graph_facts 是供 G3 接线的内部同步读接口，连接和事务应在宿主有界 blocking 操作内完成；没有把同步原生读取接到异步查询线程上。

## 2. 恢复和失败边界

- 只读打开和检查不会修复 STATE、补写回执或选择图。修复仅针对文本实际已经完整、当前来源仍匹配的中断栅栏；不会补写部分正文/向量，不从 dirty 推断之前的任务成功。
- 修复要求再次匹配检查时的完整身份、当前 generation 与 STATE。来源新增、删除、内容变化、未完成文件和过期身份均拒绝。快照核验复用现有 64 MiB 输入预算，不加载模型或检查远程模型授权。
- HEAD 先撤销，stable STATE 使用递增修订与新操作身份；不让旧图重新有效。保存或实读结果不确定时报告 recovery_required，后续应再检查实态。已确认保存不被后到取消改成 cancelled。这里没有新增完整宿主任务重放或修复回执日志。
- 图读取核对保存的目录摘要、generation 和实际 EntityId 摘要。它建立的是索引视图；G3 返回证据前仍须核对原始文件/解码范围及解析依赖，不能据此声称当前源码未变化。
- 借出的旧会话可以保留原生拥有者；HEAD、策略、状态或选择诊断失效后，不允许它返回有效图结果。写者先驱逐闲置缓存，再等待全部活动 residency 租约释放。
- Discard 只删除经当前恢复路径可重开并核验的未选中候选。任何 HEAD 引用都阻止删除，包括未被主 manifest 选择的候选 generation 的 HEAD。未知/损坏且不能核验的产物仍保留，未增加仅凭目录或诊断文件删除不完整 Schema 的捷径。
- 清理是显式动作，没有增加自动候选保留策略或磁盘预算。原有 generation 清理继续保护未核对结果；这些能力不代表所有 GC/资源门都已完成。
- 当前 recover_text_state 要求 knowledge-graph-graphdb。默认构建明确返回 unsupported，不创建工作区或加载模型；默认文本写前栅栏保持可用。

## 3. 真实测试

| 测试 | 实际覆盖 |
| --- | --- |
| dirty_text_recovery_requires_current_sources_and_never_revives_old_head | 实际空文本/图 generation；只读检查不改 STATE/HEAD；身份缺失或过期、来源新增、预取消拒绝；修复推进修订并撤销旧 HEAD；重试旧身份拒绝；新图重新构建 |
| graph_nonempty_dirty_repair_preserves_text_and_rebuilds_without_embedding | 实际 Rust 与 Markdown 三文件、中文/BOM/CRLF；核对实际 EntityId 的图租约及预算错误分类；注入真实文本 checkpoint 之后的 dirty 标记，检查双库文件摘要不变；来源内容不匹配拒绝；修复后 graph-only 重建，正文/向量文件摘要及 Embedding 调用数不变 |
| graph_leases_follow_head_and_reject_stale_state_without_poisoning_later_reads | 两份真实图、同 generation HEAD 切换、两份 engine 缓存共享原生 owner；Unknown 选择、dirty STATE、Off 均拒绝；逐请求 native 取消及后续成功、不可变构建输入损坏拒绝；旧拥有者保留/释放；活动租约阻塞写者；HEAD 引用和过期身份阻止 Discard；退役候选实物删除 |
| readonly_sessions_share_owners_and_quarantine_but_not_cancel_controls | 非空真实八表图库；只读 session 禁写、原生取消不污染新 session；适配层 Unknown 注入共享隔离；最后 facade 释放前原生 owner 保留，释放后可写重开 |
| text_recovery_without_native_feature_does_not_create_workspace_files | 默认构建经公开 SDK 返回 unsupported，目录不存在；不以图 feature 测试代替默认构建 |

图租约测试逐文件比较图库只读打开/查询/关闭前后的文件集合、长度、mtime 和 SHA-256。非空恢复测试逐文件比较实际正文/向量目录摘要；原有 Flat/HNSW 只读探针继续参加完整回归。HEAD 绕过准入的切换和 dirty/Unknown 标记属于宿主协议注入，Unknown 隔离属于适配层注入；不冒充原生提交中途故障。

### 执行结果

| 检查 | 结果 |
| --- | --- |
| 五组件图 feature all-targets | **681 通过、0 失败、10 项既有忽略**；engine 523、KG 68、shared storage 5、daemon 79、protocol 6 |
| 默认 feature 协议与恢复 | **12 通过、0 失败**；graph_ 专项 11，新增文本恢复拒绝 1 |
| 默认 feature 原正文读缓存 | **10 通过、0 失败、1 项既有手工性能用例忽略** |
| 静态检查 | 图 feature 与默认 Clippy 均通过 -D warnings；默认 workspace 全 targets 编译通过 |
| 格式与证据 | cargo fmt、git diff --check、229 项源码/依赖摘要、35 个原生归档和相对链接检查通过 |

完整回归包含本轮 4 项图 feature 用例；默认构建另验证新恢复拒绝用例，不重复合计覆盖数。完整回归冻结后仅补默认构建的条件 lint 声明和一处 import 换行，图 feature 的业务代码不变；最终默认测试、两种 Clippy 和格式检查重新通过。两个文件在完整回归快照中的摘要与最终摘要分别记录，不把它们伪装成同一字节快照。开发期间的编译、Clippy 和格式问题已经修正，原失败日志保留，不作为通过证据。

原生 graphdb 固定提交为 3bf0d532ce928c02782c76d42cc289b1e49e8df4；zvec 使用 [ANN 只读修复制品](ANN_READONLY_DIGEST_DRIFT.md)。macOS arm64，Rust 1.98.0，编译并行 14。完整回归在沙箱外执行，以支持既有模型服务夹具监听 loopback；模型计算使用现有确定性夹具，不使用真实模型性能作结论。

命令、原始日志、源码和依赖摘要、原生来源及逐包结果统一保存在 [机器记录](archive/pre-g4-validation/README.md#g2-3-recovery-leases "包内：knowledge/reports/g2.3-recovery-leases/verification.json")。历史 G2.0–G2.2 报告保留原样。

## 4. 使用入口

先检查当前已选 generation：

```bash
cargo run --manifest-path rust/Cargo.toml --locked -p zg-engine \
  --features knowledge-graph-graphdb --example text_state_recovery -j 14 \
  -- ROOT GENERATION
```

确认检查结果后，用其中的身份显式修复：

```bash
cargo run --manifest-path rust/Cargo.toml --locked -p zg-engine \
  --features knowledge-graph-graphdb --example text_state_recovery -j 14 \
  -- ROOT GENERATION repair OPERATION_ID ATTEMPT TEXT_REVISION
```

图候选的 Inspect/Reconcile/Select/Discard 使用 [graph_recovery 示例](../rust/crates/zg-engine/examples/graph_recovery.rs)。原生构建和库路径准备沿用 [Rust README](../rust/README.md)，不修改生产依赖来源。

## 5. 剩余 G2 工作

- 固定 operation/attempt 的完整宿主任务重放；不把新建 build 或只核对已有回执算作完成。
- 原生提交中途响应丢失、强杀/断电及取消交错；当前新增的协议/适配层注入不替代这些窗口。
- 无法重开的部分 Schema 候选处理、候选保留/磁盘准入及清理失败窗口的完整覆盖。显式 Discard 和现有 generation 保护不等于完整 GC 策略。
- G2.4 的真实规模、构建/更新/重开成本和新旧图并存空间测量，以及最终逐条验收汇总；不沿用代理 Schema 或 Debug 小语料数字宣称 Go。
- 内部图租约接到公开关系查询、遍历、CLI/MCP 和证据回取属于 G3，不提前宣称已经可查询。

本轮不提交或推送，不删除原仓库、旧知识库、模型或缓存。
