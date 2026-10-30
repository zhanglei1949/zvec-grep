# G2.2 宿主协议、写前栅栏与模型前分流验收

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-26。本轮接通真实 zg 索引与图候选的发布路径，完成用户指定的 STATE/POLICY/HEAD、文本写前失效栅栏及模型获取前预检查。本文接续 [图内根/回执首步记录](G2_2_BUILD_COMMIT_REPORT.md)，不把本轮结果回填为旧阶段的验证。

**G2.2 尚未整体完成。** 当前事实导入仍调用单事务 `write_facts`；受预算控制的分批导入、完整显式恢复、原生取消/期限桥接及后续联合读会话仍待推进。没有公开关系查询或 CLI/MCP 图选项。

## 1. 实施范围

| 入口 | 行为 |
| --- | --- |
| [API 类型](../rust/crates/zg-engine/src/api/index/graph.rs) | 不依赖原生库的 GraphOptions、模块根和结果；IndexOptions.graph / IndexResult.graph 未设置时不增加序列化字段 |
| [工作区策略](../rust/crates/zg-engine/src/workspace/graph_policy.rs) | POLICY 跨 generation 保存意图；None 继承，显式 Off 先保存策略再撤销 HEAD；失败候选不回滚已接受意图 |
| [宿主轻量协议](../rust/crates/zg-engine/src/storage/graph_state.rs) | 版本检查、STATE/HEAD、规范目录摘要和未知候选清理保护；默认构建也编译 |
| [真实存储写入](../rust/crates/zg-engine/src/storage/store/mod.rs) | replace、失败记录、删除前保存 mutating；显式 checkpoint 完成后才写 stable；close 或只读打开不修复栅栏 |
| [索引入口](../rust/crates/zg-engine/src/pipelines/indexing/service.rs) | 先取得既有写入准入，完成参数/能力检查和实际来源预检查，再决定正常文本路径、graph-only 或 unchanged |
| [候选构建](../rust/crates/zg-engine/src/pipelines/indexing/graph.rs) | 同次事实绑定、未变事实/实体映射核验、全目录解析、根/回执提交、新只读确认、关闭重开及宿主选择；每个原生连接/事务在所属 blocking task 内完成生命周期，不跨 await |
| daemon | 不同显式图策略按顺序排入有界队列；None 不覆盖显式配置；watch 模板清除 graph，在真正执行时继承最新 POLICY |

继续只保存一套正文和向量，没有调用 KG 独立 ProjectionIndex、重复 Embedding、自建 FFI 或增加第三方依赖。

## 2. 运行与失败行为

### 文本写前栅栏

首次图构建在已知宿主目录上建立 stable 基线。一次索引任务第一次可能改变业务内容时，存储层先持久化 `mutating` 和下一修订，再写正文/索引。多份文件及删除共享此次未完成修订；必要 checkpoint 成功、没有 NotIndexed/Deleting 中间状态后，才保存新目录摘要并转 stable。A→B→A 仍推进修订。

无法读取已知 STATE 格式或不能保存 mutating 时，修改被阻止。文本已保存但 stable 写入失败时，保留 mutating；关闭只负责持久化已接受的原生写入，只读重开只检查。当前不自动恢复该状态；后续显式核对或重建不能省略。

规范目录摘要来自完整 `list_files()`，包含原生路径编码、file_id、资格、内容摘要和长度；不使用 mtime、失败文案或 generation 代替内容。图内绑定另外核对宿主实际 EntityId 集合，避免同内容不同实体映射仍被视为一致。

### 模型前分流

预检查在 `acquire_index_models` 前执行，使用现有扫描、排除、读取和指纹校验。读取实际原始字节，不依赖 watcher changes 或 mtime。仅当已保存文本描述、选择规则与完整来源均可证明未变，才允许 graph-only/unchanged；预检查失败或超预算不通过加载模型补足证明。

需要正常文本更新时，先前输入不作为新版本依据；索引使用本次读取并重新核对来源。启用图的实际流水线对候选核对内容摘要，覆盖同尺寸、同 mtime 的更新。已保存策略为 Off 时，普通索引保留原有模型/授权语义。

候选准备的当前输入上限为 64 MiB、输出上限为 1,000,000 项、协作检查期限为 120 秒；这些是实现保护值，不是规模 Go，也不是原生查询可被抢占的证明。取消/期限向内核的完整桥接尚未完成。

### 提交与真正的选择点

增量/graph-only 的选择点是当前 generation 的 HEAD。完整重建则先在未选 generation 内准备 HEAD，随后以宿主 manifest 选择整个 generation。准备好的 HEAD、已提交的 GraphRoot/Receipt 都不能提前当作宿主已选择。

增量文本更新在关闭写句柄后，先保存本次 manifest 中的选择规则和索引描述，再构建/切换 HEAD。元数据写入失败时不会创建新图候选，也不会让新图绑定尚未保存的选择规则。完整重建则继续使用最后的 manifest 切换作为共同选择点，不能混用两条顺序。

完整重建在 manifest 选择前重新检查取消，保存 select_pending 并实读最终选择。结果只在真正选择后置 `graph_selected=true`；准备阶段的域就绪为 `not_ready(manifest_not_selected)`。进入最终选择后收到取消，按已确认效果返回并保留晚到取消诊断。

候选导入、提交、checkpoint 或选择进入可能不确定的窗口前先写 pending。未知结果优先于取消/普通失败；相关候选和 generation 受清理保护。图内成功回执保持不可变。实际选择文件写入失败时返回 recovery_required，不能将目录存在当成成功，也不能盲删候选。

后台图任务持有同一份工作区写入准入的 Arc；异步调用方丢弃 future 时，写锁仍保持到原生任务与句柄结束。丢弃 future 不表示已经中断原生操作，不能让后续写者提前进入。

已知图失败可在文本成功后返回图降级结果，图失败不进入 `files_failed`。公开结果位于 `IndexResult.graph`，包含 text_published、graph_candidate_committed、graph_selected、graph_build_id、mode、分域 ready/计数与诊断。HEAD 外层为 camelCase；configuration 沿用 engine 类型字段，分域覆盖是文件数。

## 3. 验证与证据边界

真实验证使用现有固定 graphdb binding/native 与 ANN 补丁库。底座没有替换；服务用例仅将模型计算替换为确定性测试实现，不下载模型、不向远端请求。

| 验证 | 证据 |
| --- | --- |
| 正常联合发布与完整重建 | 正式 service + 真实 zvec/graphdb，Rust BOM/CRLF 与中文 Markdown；manifest 和新 HEAD 对应同一 generation |
| 首次启用、配置变化、有效图复用 | 已有文本原样保留；测试 factory 在任何模型获取时 panic，实际 graph-only/unchanged 成功，无远程授权或 Embedding；有效图复用不改图目录文件摘要 |
| 同 mtime 内容变化 | 同尺寸代码修改并恢复 mtime；没有授权时回到普通文本错误路径；授权后实际更新并发布新图 |
| 排除目标 | 删除对应宿主记录和图成员，文档引用不再解析到旧目标，Embedding 调用数不增加 |
| 默认无 graphdb feature | 明确启用先返回 unsupported，不写 POLICY、不获取模型；真实存储仍执行栅栏、A→B→A、失败和删除检查 |
| 真实文件系统拒绝写入 | mutating 保存失败阻止文本写；stable 保存失败保留脏状态；只读重开不修复 |
| HEAD 与 manifest 间取消 | 真实根/回执关闭重开已确认，候选 HEAD 已准备；取消后旧 manifest 保持，已确认未选候选可由既有归属清理 |
| 最终选择 I/O 失败 | 用真实目录写权限拒绝 manifest 写入，旧指针保持；成功回执及候选保留，普通恢复/清理拒绝删除 pending generation |
| 增量选择元数据失败 | 拒绝新扫描规则的 manifest 写入；旧规则、旧 HEAD 均保持，不创建新图候选 |
| 异步调用方被取消 | 真实 OS 写锁证明：调用 future 被 abort 后，后台 worker 仍持有准入；操作结束后才允许另一个写者进入 |
| daemon | 显式 Code→Documents→Off→Code 分别执行；兼容请求可合并；watch 不重放旧图设置 |
| 旧布局 | 当前 enabled manifest 缺少 storageGeneration 时，在策略/模型变化前按 rebuild_required 拒绝；保留原生数据，不修改发布校验 |

上述文件系统故障与阶段钩子用例不等同于原生提交响应丢失或进程终止；后两项仍未覆盖。未知提交分类的代码路径、持久化 pending 保护和已通过的事务回滚也不能替代这些实测。

以下结果对应同一份固定源码；中间运行及其源码快照单列保存，不计入最终验收。

| 最终检查 | 结果 |
| --- | --- |
| 五组件图 feature all-targets 完整回归 | **663 通过、0 失败、10 个既有忽略项**；engine 514、KG 59、shared storage 5、daemon 79、protocol 6 |
| 默认 workspace、all targets 编译 | `--no-default-features` 通过 |
| 无 graphdb feature 的专项回归 | **10 通过、0 失败**，含真实存储栅栏及能力拒绝 |
| 图 feature / 默认构建 Clippy | `-D warnings` 均通过 |
| SDK 原生加载复核 | 实际加载 ANN 补丁库，staged runtime SHA 一致；共享只读回归通过 |
| 格式、差异与源文件核对 | cargo fmt、git diff --check、194 项受检源码/依赖摘要通过 |

最终命令、源码/原生摘要与运行日志见 [机器记录](archive/pre-g4-validation/README.md#g2-2-host-publication "包内：knowledge/reports/g2.2-host-publication/verification.json")。本次完整范围增加 daemon/protocol，不能把它相对旧 557/0/10 的差值当作新增测试数。旧阶段 59 项 KG 回归、本轮中间版本完整回归和最终回归分别保留，不合并为一次执行。

准备 [修复后的原生库](../rust/README.md#native-read-only-fix) 与既有 graphdb 构建后，从仓库根运行：

```bash
cargo test --manifest-path rust/Cargo.toml --locked --offline \
  -p zg-engine -p zg-knowledge -p zg-storage -p zg-daemon -p zg-daemon-protocol \
  --features zg-engine/knowledge-graph-graphdb --all-targets -j 14 -- --test-threads=1
cargo test --manifest-path rust/Cargo.toml --locked --offline \
  -p zg-engine --no-default-features --lib graph_ -j 14 -- --test-threads=1
```

完整回归在沙箱外执行既有回环 HTTP 用例；workspace registry、授权测试 key 与应用状态隔离于 `/tmp`，没有修改 HOME。原生验证限于当前 macOS arm64、固定 graphdb 和 zvec 补丁制品；旧远端预编译库仍未修复。

## 4. 仍待完成

- 将事实导入拆为受预算控制、可核对的批次；当前完整候选写入仍为单事务。
- 接通完整显式核对/续建及 host operation/attempt 重放；不按时间戳或目录存在自动恢复。
- 验证原生提交响应丢失、进程终止、真正的取消/期限竞争；完成图读会话租约、旧读者和 GC 接线。
- 对接 daemon 协议能力协商和 CLI/MCP 图选项、正式状态/诊断及可运行外部示例；当前没有公开图查询。
- 测量规范目录/实体核验、重复图候选与磁盘保留的资源代价。没有负载 Go、查询增益或跨平台结论。

本轮更新 [G2 实施计划](G2_IMPLEMENTATION_PLAN.md) 与 Rust README，保留上阶段原始报告。未修改或发布上游 binding，未提交或推送远端。
