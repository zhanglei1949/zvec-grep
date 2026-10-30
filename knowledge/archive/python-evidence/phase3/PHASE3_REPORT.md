> 历史 Python 验收记录；执行命令需要先恢复基线。原始文档和源码保存在 [Python 基线归档](../../python-baseline.zip)，本可阅读副本仅更新归档引用。

# Phase 3 文档构建与发布验收报告

日期：2026 年 10 月 21 日。

Phase 3 的文档写路径已实现：**已捕获修订 → 结构解析 → 分段 → 全文/可选向量候选 → 持久化校验 → 图内原子发布 → 新只读事务确认 → 关闭重开核对**。当前交付限于 [实施计划](../../../knowledge-phase3-plan.md) 的构建侧子集。公共 search、RRF 和证据查询仍由 Phase 4 交付，知识抽取和真实模型仍留后续阶段。

全量回归 **134 项通过，无失败、无跳过**，包含真实 graphdb/zvec 集成；5 个 SDK 边界的 SIGKILL/重开核对和两套原生故障测试通过。实际 SDK 的 10/100 来源档位及最终数值见第 4 节。结构化证据、源码/脚本摘要和环境保存在 [RESULTS.json](RESULTS.json)，接口/DDL/状态细节见 [CONTRACTS.md](CONTRACTS.md)。本报告的通过结论不包含第 8 节列出的未交付能力和未覆盖窗口。

## 1. 实现与交付映射

| 计划子阶段 | 实际交付 | 主要入口 |
| --- | --- | --- |
| 3.0 契约与探针 | 主键、typed 过滤、关系访问计划；显式 Read；双 collection；原文映射；依赖许可与文件摘要 | [gate.py（归档）](../../python-baseline.zip) |
| 3.1 Schema | schema_version 仍为 1；追加构建/发布控制表，不改 Phase 2 表；部分初始化及归属检查 | [publication_store.py（归档）](../../python-baseline.zip)、[kb.py（归档）](../../python-baseline.zip) |
| 3.2 读事务与资源 | 独立 cursor 的 READ ONLY；退役根背压与清理预留；索引句柄、逐项状态、共享准备额度 | [graph_store.py（归档）](../../python-baseline.zip)、[execution.py（归档）](../../python-baseline.zip) |
| 3.3 文档处理 | Markdown/纯文本 Parser、独立 SectionSegmenter、UTF-8 原始字节映射、覆盖校验与分阶段缓存 | [documents.py（归档）](../../python-baseline.zip) |
| 3.4 Embedding | 框架自有契约、完整空间指纹、预计算清单持久化与校验、显式 synthetic 测试提供者 | [embedding.py（归档）](../../python-baseline.zip) |
| 3.5 候选索引 | 全文与向量 collection 分离、创建意图/归属、版本化投影、逐项核对与缺失 ID 补写 | [index_catalog.py（归档）](../../python-baseline.zip)、[search_index.py（归档）](../../python-baseline.zip) |
| 3.6/3.7 发布与核对 | 固定构建意图、当前尝试及基准；短图事务发布与回执；新 Read 确认；显式 reconcile | [builds.py（归档）](../../python-baseline.zip)、[build_models.py（归档）](../../python-baseline.zip) |

来源捕获继续返回 `IngestResult.search_ready=false`，不因后续构建改写历史回执。`BuildResult.search_ready` 表示本发布的必需索引产物已确认，另有 text_ready/vector_ready、空间和覆盖；`capabilities.search_api_available=false`。没有把元数据图或测试向量称为实体关系抽取或真实语义检索。

更新方式区分 unchanged、partial_processing、full_rebuild，返回原因、单元数量、复用阶段及产物清单。片段与实际投影写入通过有界检查读取。operation_id 固定输入、目标与基准，attempt 隔离重试；同操作成功重放返回原回执，不重新发布。原回执已被新发布取代仍为真实成功，当前选择通过 publications.current 检查。

本阶段保留全部旧版和失败候选，不实现来源删除、自动续跑、应用 GC 或旧库迁移。按当前开发期无历史业务数据的前提显式重建旧开发库，不在 open 中补表或覆盖目录。M1 的 Entity/Assertion/Support/Evidence 表未创建或冻结。

## 2. 环境、依赖与探针结论

运行环境为 macOS 15.1.1 / arm64、Python 3.13.3。graphdb 固定 `9d6e966ef0ae214cff8d5018efe1dafb37bc2a99`，复用已记录的 libcxx-csv-fallback wheel；zvec 固定 `a4347e31f1b8e216cffba59285427a3b78551229`、原 wheel 未修改。wheel 摘要及 provenance 随结构化结果保存。本轮未重新编译或升级原生依赖。

新增 `markdown-it-py==4.0.0`、`mdurl==0.1.2`，均为纯 Python/MIT，固定在 [pyproject.toml（归档）](../../python-baseline.zip)。探针读取已安装分发的许可证分类、LICENSE 文件摘要及依赖元数据，核对无原生扩展文件；Token 仅在 Parser 适配器内部出现。没有引入模型 SDK 或自动下载权重。

| 底座试验 | 实测结果与设计影响 |
| --- | --- |
| graphdb 主键 | 执行计划为 UniquePropertyLookup(id)，不使用行号作业务身份 |
| typed owner 过滤 | 实测为 LabelScan + Filter；试验的二级索引 DDL 不支持，typed 列不等于已有二级索引 |
| 关系读取 | 主键锚定后使用 NativeCsrExpand；端点分别 MATCH 后再连边，避免先组合两个表再过滤 |
| READ ONLY | 可写宿主和只读打开均可显式 BEGIN TRANSACTION READ ONLY；SDK 独立 cursor 不长期持全局写锁 |
| 双 collection | text-only 与 vector collection 分别写入、逐项检查、过滤、flush、fetch、关闭重开 |
| 原文映射 | 中文/BOM/CRLF、围栏中假标题均按原始 UTF-8 字节校验 |
| 原生物理合并 | append-only + 缺失 ID 补写路径保留全部投影 ID；探针合并前约 81.31 MiB，合并后约 4.64 MiB，数值仅为此夹具 |

Markdown 使用 CommonMark + table；保存标题与章节层级，分段为连续原文区间，按章节、字节预算和 UTF-8 边界拆分。当前没有合成标题前缀，也不渲染 HTML、抓取链接或读取外部引用。frontmatter/footnote 等没有专用插件语义，原文仍被覆盖。

## 3. 正确性、故障与并发证据

基线 82 项，本阶段新增 52 项，最终 134 项实际通过。测试实现及完整用例名见 [文档构建集成测试（归档）](../../python-baseline.zip)、[文档单元测试（归档）](../../python-baseline.zip)、[Embedding 单元测试（归档）](../../python-baseline.zip) 与 [准入测试（归档）](../../python-baseline.zip)。

| 证据类别 | 覆盖与边界 |
| --- | --- |
| 纯逻辑/契约 | Parser/Segmenter 替换、UTF-8 边界、结构与分段覆盖、指纹、预计算缺项/重复/摘要/维度/非有限值/空间校验、共享额度扩容 |
| 真实底座集成 | 捕获/构建/重开、A→B→A、配置变化与阶段复用、向量世代、仅全文、空文档、同字节独立来源、有界检查与归属 |
| 并发与原子性 | 同来源基准、新捕获使旧构建过期、旧 attempt、不同来源合并、同目标复用、空间竞争、A 发布后 B 取代再确认 A |
| 取消与资源 | 发布前取消、提交后迟到取消、关闭等待、真实中间行上限、长 Read 退役根背压、释放后重试、分段/向量膨胀前拒绝、磁盘不足 |
| 适配层注入 + 真实核对 | 事务体回滚、commit 响应丢失、索引响应丢失、创建完成但 marker 响应丢失、提供者结果已保存但终态丢失；不冒充内部原生故障 |
| 真正逐项状态 | 同批重复 ID 使部分项失败，其余项成功；稳定码不泄露正文；相同已有内容复用、不同内容拒绝、缺失项补写 |
| 只读隔离 | 清洁、合并后的索引及 100 来源库按字节副本读取；原库文件摘要不变；副本预算失败不写原库 |

[crash.py（归档）](../../python-baseline.zip) 由父进程实际 SIGKILL 自己创建的子进程，每例独立新库；只读检查之后可写重开，并按原 operation_id 核对：

| 终止边界 | 核对结果 | 不变量 |
| --- | --- | --- |
| 发布事务体执行后、commit 前 | not_committed | 原选择与回执一致，没有部分业务发布 |
| commit 后、确认前 | committed | 查原回执恢复成功，不重新构建 |
| 提供者已派发、结果未保存 | provider_unknown | 不自动重复调用提供者 |
| native collection 创建后、ready marker 落盘前 | not_committed | 按持久创建意图核验并修复登记，不覆盖陌生目录 |
| native 向量合并/flush 完成后、图发布前 | not_committed | 旧发布的全部投影仍可核验，再显式重试 |

5 例均通过，异常退出后的元数据只读检查原库文件摘要不变。最后一例发生在 native 操作返回后的 SDK 边界，不能描述成 optimize 内部 syscall 中断。

另外实际重跑 `graphdb_durable_memory_tests`、`graphdb_tp_wal_tests`，两套通过，覆盖固定构建变体的原生持久化故障和恢复。它们与 SDK 回执 SIGKILL、适配层响应丢失是三类不同证据，不合并声称覆盖所有组合窗口。

## 4. 实际 SDK 资源档位

最终档位使用 [profile.json](profile.json)，运行前冻结数值与源码/脚本摘要。10/100 来源各在独立 worker 中执行，图、全文、向量及 Python 处理处于同一进程；每来源 8 个 1 KiB 章节、3 个修订、384 维 synthetic 向量，另保留 2 个失败发布尝试。两个旧 Read 跨越其中一次来源的完整捕获/构建/发布；其余全部来源的三版数据保留。全批次长 Read 的耗尽另列第 5 节，不与有界读取负载混淆。

| 来源数 | 捕获加构建 s | 只读重开 s | 峰值 RSS MiB | 保留磁盘 MiB | 只读副本 MiB | 校验 p95 ms | 判定 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 10 | 10.58 | 0.15 | 406.27 | 18.99 | 11.57 | 6.64 | 通过 |
| 100 | 117.64 | 1.13 | 577.02 | 94.48 | 24.30 | 12.69 | 通过 |

阈值未改变：每 worker 峰值 RSS ≤ 2 GiB，捕获加构建和只读重开分别 ≤ 120 秒，内部发布校验读取 p95 ≤ 200 ms。RSS 为进程峰值，包含组合底座及重开；只读重开包含建立受预算约束的私有索引副本。磁盘是关闭后保留数据大小，副本大小另外列出，没有连续采样合并时的瞬时磁盘峰值。

内部校验延迟测量 `_confirm` 的图回执、快照/投影完整性及 FTS/ANN 验证读取，先预热 5 次再测 100 次；它不是 Phase 4 search/RRF/证据封装的延迟，也不是 ANN 召回质量评测。阈值通过只适用于本 profile 的一次固定运行点，100 来源构建的余量仍小，不外推生产容量或 1,000 来源 SDK 吞吐。

100 来源最终保留 302 个修订、2,702 个 ContentUnit、2,416 个准备片段、4,832 个图侧投影、300 个发布和 302 个构建操作；全文和向量各 2,416 项，包含失败候选。全部 16 类控制节点共 14,074 个、两类关系共 3,004 条；完整分组、文件计量和只读不变核验见 [结构化结果](RESULTS.json) 中 inventory，不仅统计当前发布。1,000 来源实际 SDK 验收留 M5，既有 Spike 代理模型不能代替它。

## 5. 保留的失败与修正

| 发现/失败 | 实际处理与验证边界 |
| --- | --- |
| 两个原生 Read 横跨完整多来源批次 | 固定 64 个退役根额度耗尽；SDK 增加保守背压和 worker 失败/确认预留。资源 profile 明确为有界 Read，数值阈值不变；耗尽、保留当前选择及释放后重试单独验证 |
| 未分别锚定关系端点 | 组合 MATCH 后统一过滤产生昂贵中间组合；捕获和片段发布改为各自主键锚定后连边，保持 Phase 2 语义与回归 |
| 小批次 HNSW flush | 多个约 5 MiB 物理块累积，启动原生 optimize；阈值 16 块，逐 collection IO 串行，并保留版本化 ID及暂存预算，不删除应用产物 |
| 已有 ID 原生 upsert + optimize | 负向控制中 stats 报 16 项但 fetch 缺失 next-14；SDK 禁止原地覆盖，只对缺失 ID 补写，相同内容先 fetch 复用 |
| 原生只读在清洁关闭后仍写向量文件 | “仅异常会话禁读”不足，最终正常只读改为私有字节副本，增加副本额度与清理；异常会话仍须可写重开核对 |
| 仅在 open 关闭 mmap | 实测无效，原生从持久版本恢复配置；不把此选项当成已修复只读问题 |
| 改为 buffer 存储的探索 | 合并/索引打开出现原生错误，没有采用或改变固定底座；最终使用字节隔离保护原库 |
| 同一 v2 profile 的首个正式 100 来源运行 | 130.54 秒超过 120 秒，按失败保留；完整计数改用目录条目减少重复 stat、避免同一清单的冗余验证后重跑，没有提高阈值 |

原生只读负向控制与 SDK 隔离对照见 [readonly_negative.py（归档）](../../python-baseline.zip)，upsert/合并负向控制见 [upsert_negative.py（归档）](../../python-baseline.zip)。负向复现表示相应底座路径不满足需求，不能记作它们已通过。最初资源、只读和索引失败的摘要随 RESULTS.json 保留，临时 `.artifacts/phase3/` 原始现场没有清理。

只读问题的固定源码链是 [VectorColumnIndexer::CreateProximaIndex](https://github.com/alibaba/zvec/blob/a4347e31f1b8e216cffba59285427a3b78551229/src/db/index/column/vector_column/vector_column_indexer.cc) → [Index::Open](https://github.com/alibaba/zvec/blob/a4347e31f1b8e216cffba59285427a3b78551229/src/core/interface/index.cc)：read_only 被传入 StorageOptions，但 storage 初始化没有据此获得只读约束。copy_on_write 是另一个字段。collection 的 recovery 从持久版本加载 enable_mmap，见 [collection.cc](https://github.com/alibaba/zvec/blob/a4347e31f1b8e216cffba59285427a3b78551229/src/db/collection.cc)。这是定位依据，已观测文件变化来自上述真实负向试验；未证明所有内部分支的完整因果，也没有修改上游库。

## 6. 运行与复现

需已安装固定原生 wheel、当前纯 Python 解析依赖。测试和示例默认新建临时库；试验输出使用不存在的新目录，不能覆盖业务库。

```bash
.venv/bin/python -m examples.phase3_build
.venv/bin/python -m unittest discover -s tests -t . -v
.venv/bin/python -m spikes.phase3.gate --output .artifacts/phase3/replay-gate.json
.venv/bin/python -m spikes.phase3.crash --output .artifacts/phase3/replay-crash
.venv/bin/python -m spikes.phase3.scale --output .artifacts/phase3/replay-scale
.venv/bin/python -m spikes.phase3.upsert_negative --output .artifacts/phase3/replay-upsert-negative.json
.venv/bin/python -m spikes.phase3.readonly_negative --output .artifacts/phase3/replay-readonly-negative.json
.venv/bin/python -m spikes.validation.native --output .artifacts/phase3/replay-native
.venv/bin/ruff check zknowledge tests spikes examples
.venv/bin/ruff format --check zknowledge tests spikes examples
git diff --check
```

[phase3_build.py（归档）](../../python-baseline.zip) 完成预计算首版→重复构建→新修订失败保留旧版→同操作显式重试→只读重开。向量全部为明确 synthetic 夹具。并发示例为集成测试 `test_old_read_snapshot_coexists_with_publication` 与 `test_later_publication_before_earlier_confirmation_is_success`；前者固定真实 Read，后者以屏障控制 A/B 的确认顺序，可单独通过 unittest 运行。

[inventory.py（归档）](../../python-baseline.zip) 对正常关闭后的试验库做全物理计数与原文件摘要检查，使用方式为 `--root 试验目录/100/kb --output 新JSON路径`。它是内部试验工具，不是公共 Cypher 或历史查询入口。

## 7. 文档修改摘要

| 文件/章节 | 同步内容 |
| --- | --- |
| README | Phase 3 交付现状、依赖、示例、捕获/构建/公共检索就绪区分、只读副本成本和原生限制 |
| 总体设计 §4.2.1、§5.3、§5.7、§5.8 | 实际 Parser/Segmenter/Embedding、独立读取与退役预算、文档发布/核对实现范围、索引隔离和资源检查 |
| 总体设计 §7、§8 | 已交付构建/检查 API、保持 Schema 1、当前物理映射、纯 Python 许可和固定原生变体 |
| 交付计划 §4.3、A20 | 交付状态、实际 SDK 档位、原生限制、Phase 4/5/M1/M2/M4/M5 后续边界 |
| Phase 3 计划状态与 §8 | 实施结果、与探针收敛后的契约、保留的失败记录 |
| CONTRACTS/RESULTS/本报告 | DDL/对象映射、接口/状态/默认配置、测量和证据分类 |

写入前已与开始时记录的哈希比对，上述根文档没有其他会话的新修改；在最新内容上定点更新。项目边界文档保持原哈希。没有提交、推送或修改已固定的 submodule。

## 8. 未覆盖与后续进入条件

- Phase 4 才交付公共 text/vector/hybrid 检索、RRF、Scope、旧候选补取及证据封装；当前只有发布验证读取。
- M1 才交付真实知识抽取、声明/支持/人工效果和真实本地模型基线；当前测试向量不提供语义质量证据。
- 单来源可完整切换空间，多来源局部换空间明确拒绝；全库覆盖与追赶留 M2。没有最小片段差异算法、任意历史查询或应用 GC。
- Python 边界 SIGKILL、适配注入与两套原生故障已测；原生任意 WAL/optimize syscall + SDK 回执的组合、物理断电、网络文件系统及长期反复恢复留 Phase 5/M5。
- 逻辑额度/检查点不是原生 RSS 或外部提供者的硬隔离；副本计量为逻辑文件字节，不是文件系统物理分配/系统页缓存峰值。暂存盘瞬时峰值未连续测量。
- 异常退出后的只读只提供图/任务元数据，原生索引先经可写重开与显式核对；正常只读副本可增加打开成本，原生缺口仍需上游跟踪。
- graphdb 顶层许可证缺口仍是对外发布条件，非当前本地开发阻塞。没有用此报告替代后续首版 A01–A27 的完整验收。

下一阶段可在已验证的候选/发布/清单契约上实施 Phase 4；先明确固定视图、覆盖与查询预算，再接公共检索和证据返回。
