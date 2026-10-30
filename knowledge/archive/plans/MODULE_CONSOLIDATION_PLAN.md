# zg-knowledge 模块收敛实施计划

> 归档于 2026-10-28。本计划已实施，保留原步骤供溯源。执行结果见 [收敛报告](../../MODULE_CONSOLIDATION_REPORT.md)；其中的交接顺序和待修复项是当时记录。

日期：2026-10-27。状态：**模块收敛已实施，验证与遗留项见 [收敛报告](../../MODULE_CONSOLIDATION_REPORT.md)。**

顺序调整：用户随后明确要求“先收敛模块，再让另一位 agent 修复”。因此本轮保留 A1/A2 已知问题作为基线，先执行 R1–R4；不再以修复全部既有 Clippy/格式及取消问题作为启动条件。未修复项不得被标为本轮通过。下文保留原交接流程作后续参考，以本段和报告记录为准。

本计划接在另一位 agent 的 G2 收尾路线中：**A1/A2 形成可回归检查点 → 模块收敛 → A3 最终规模测量与 G2 汇总 → G3 → G4**。本文件不修改现有 G2 任务状态，不表示已暂停或接管另一会话。

## 1. 目标与边界

让 zg-knowledge 专注于 zg 的图能力，移除早期独立知识库 SDK 留下的另一套摄入基础设施。

| 模块 | 收敛后的职责 |
| --- | --- |
| zg-engine | 文件扫描、格式与编码、检索 Entity/Fragment、Embedding、正文/向量索引、任务调度、图构建与发布编排、查询编排 |
| zg-knowledge | 图事实与版本、身份与消歧、Markdown 关系解析、图存储适配、导入进度、根与回执、图操作错误 |
| zg-storage | SHA-256、文件持久化、按需启用的 zvec 底层工具；不拥有图或知识生命周期 |
| graphdb 上游 binding | 数据库与连接、原生结果和错误、事务、原生句柄生命周期 |

本次不增加依赖、公共 SDK、模型运行时、数据库后端或公共工具 crate。不让 knowledge 反向依赖 engine。独立 SDK 代码退役后通过现有历史记录和交接快照追溯，不保留一个无人使用的 legacy feature。

以下行为保持不变：

- 默认 zg 的索引与搜索、模型授权/下载/缓存、分段及 EntityId 分配。
- Rust/Python 同次 Tree-sitter 解析，Markdown 图规则及锚点、解析读集、事实排序和诊断。
- 原始文件摘要与已解码 UTF-8 坐标的区别；BOM、CRLF、Python 编码声明的处理。
- 图 Schema、格式版本、稳定 ID、全部已持久化指纹与字节编码。
- STATE/POLICY/HEAD、候选、图租约、资源准入、隔离、清理及成功结果优先于后到取消的语义。
- 另一位 agent 交接时已经验证的 Resume/operation/attempt 行为；不重新定义重试代次或发布资格。

## 2. 当前依据

当前入口 [lib.rs](../../../rust/crates/zg-knowledge/src/lib.rs) 同时导出独立文档处理、Embedding、执行器、快照、投影和实际图模块。正式 engine 接线主要使用 relations、graph，以及 processing 中的上下文与指纹工具。

- `src/models.rs`（已退役） 只有 MediaType，不是 engine 的模型实现。
- `src/documents.rs`（已退役） 包含独立快照解析和分段，但其中 markdown_events 被正式 [Markdown 关系解析](../../../rust/crates/zg-knowledge/src/relations/markdown.rs) 调用。
- [processing.rs](../../../rust/crates/zg-knowledge/src/processing.rs) 混合了正式图链路需要的 ProcessingContext/canonical_json/stable_digest 与独立 Parser/Extractor 契约，必须拆出使用边界后再删。
- 正式代码事实使用 [engine 同次解析入口](../../../rust/crates/zg-engine/src/extraction/code.rs)，KG 的 [resolve_code](../../../rust/crates/zg-knowledge/src/relations/code.rs) 负责消歧，不是第二套代码解析器。
- `src/projection.rs`（已退役） 的独立正文/向量 collection 未接入 zg 图增强；清理不删除任何真实用户目录。

历史 [retention 验收](../../G2_3_RETENTION_REPORT.md) 为 693 通过、0 失败、10 忽略，包含 engine/KG/storage/daemon/protocol 五组件。编写本计划时，恢复代码已经出现后续 Resume 改动，历史结果不能证明当前源码通过。具体源码、测试数及原生制品以第 6 节的正式交接快照为准。

## 3. 文件调整范围

表中是未来代码实施范围；本轮不执行任何一项。

### 3.1 zg-knowledge

| 路径（相对 rust/crates/zg-knowledge） | 调整 | 前提及限制 |
| --- | --- | --- |
| src/processing.rs | 保留 ProcessingContext、canonical_json、stable_digest；移除独立 Descriptor、Stage、Parser/Extractor、块/分段/声明候选和快照范围契约 | 保留 processing:: 下三个正式入口及行为，避免 A1/A2/G3 全面修改 import；图 Span/SourceVersion 不动 |
| src/relations/markdown.rs | 接收 documents.rs 的私有 markdown_events 实现 | 保持 pulldown-cmark=0.13.5、ENABLE_TABLES、offset 语义、规则指纹和输出排序；不改锚点或链接规则 |
| src/documents.rs | 上述入口移出且测试迁移后删除 | 不把 SectionSegmenter 接到 engine，也不替换现有检索分段器 |
| src/embedding.rs、src/projection.rs | 删除独立提供者、预计算向量及候选 collection 实现 | 正式向量生成和存储继续使用 engine；原生共存/逐项响应/只读测试另行保留，见第 4 节 |
| src/execution.rs | 删除独立 WorkerPool、JobHandle、队列及准入实现 | 不改 engine/daemon 的调度、blocking 任务或取消桥接 |
| src/snapshot.rs、src/models.rs | 删除独立 SnapshotStore、SourceSnapshot、MediaType | stable_digest 改为直接使用已有 zg_storage::sha256_hex；正式图事实不增加历史正文副本 |
| src/workflow.rs | 完成规则迁移核对后删除该测试专用状态机 | 取消、未知提交、过期身份及重试规则必须有正式链路覆盖；不能只删测试 |
| src/error.rs | 删除只服务退役组件且已无引用的错误分支 | 保留正式图/关系错误及结构化映射，不把底座错误并成无法分类的字符串 |
| src/lib.rs | 删除退役模块导出，修正过期模块说明 | 最终保留 error、processing、relations 和 feature-gated graph，不新增占位公开 API |
| src/graph/*、src/relations/{bound,code,mod}.rs | 原则上保持 | 只允许移除退役依赖所需的机械调整；Schema、事务和协议行为变化退回 G2 单独处理 |
| examples/components.rs | 删除独立快照→分段→向量→投影示例 | 不再提供另一条 collection 写路径 |
| examples/fact_roundtrip.rs | 保留并核对编译 | 仍是底层事实往返示例，不代表 engine 联合发布 |
| tests/* | 按第 4 节拆分、迁移和退役 | 必须留下旧用例到新用例或退役原因的清单 |

不为 MediaType、少量状态枚举或不同含义的 Range 新建共享 crate。KG 的事实节点、engine 的检索实体、未实现的知识声明不是同一对象，不合并类型。

### 3.2 依赖与构建

| 路径 | 调整范围 |
| --- | --- |
| rust/crates/zg-knowledge/Cargo.toml | 移除生产代码不再使用的直接 zvec 依赖；真实共存测试需要的 zvec-rust/tempfile 等转为 dev-dependencies；保留固定 CommonMark 和 graphdb feature |
| rust/crates/zg-knowledge/build.rs | 删除前核对原生集成测试是否依赖其链接/运行库配置；必要的测试运行库环境放入现有测试执行入口，不因删除 build.rs 使单包测试失去可运行性 |
| rust/crates/zg-storage/Cargo.toml、src/lib.rs、build.rs | 将 zvec 模块、运行依赖和构建配置放入显式 zvec feature；默认不启用，只用 hash/filesystem 的使用方不被强制链接 zvec |
| rust/crates/zg-storage/tests/readonly_collection.rs | 为该原生测试 target 声明 required-features=["zvec"]；测试命令显式启用，不能因 feature 门控而漏跑 |
| rust/crates/zg-engine/Cargo.toml | 对 zg-storage 显式启用 zvec，维持 engine 现有能力；不新增反向依赖 |
| rust/crates/zg-knowledge/Cargo.toml 的 dev-dependencies | 共存测试通过 dev-dependency 启用 zg-storage/zvec；不新增第二份绑定或模型运行时 |
| rust/Cargo.lock | 只接受上述依赖归属/feature 调整的必要变化，不升级现有版本 |
| rust/scripts/check.sh、.github/workflows/rust-ci.yml | 仅在现有命令需要显式 zvec feature 或测试环境配置时调整，不新建 CI workflow |

Cargo 的测试依赖和 feature 会统一；不能以 all-targets 测试依赖图证明生产依赖已经移除。分别核对 cargo tree 的正常依赖、纯库编译及全量测试路径。zg-storage 的 build dependency 也必须按 feature 关闭，不能只给 src/zvec.rs 加 cfg。

目标是 knowledge 的正常库构建只使用图/关系所需依赖；其共存测试仍能链接真实双库。若运行库加载问题尚未解决，保留必要的链接配置并记录遗留，不能把“删掉构建入口”本身作为完成判据。

### 3.3 engine、共享工具与文档

- engine 正式实现只允许必要的依赖声明和测试接线；本轮不改模型、扫描、解码、分段、索引格式、公开查询和恢复接口。
- zg-storage 已有 SHA-256、原子替换和目录同步继续复用。不把不可覆盖快照安装误当作普通原子替换直接合并；旧快照存储按退役处理。
- engine 的元数据 fingerprint 与 KG canonical_json 本轮不统一算法。先固定 KG 序列化输出，避免因“公共工具提取”触发图规则、Schema 或导入计划变化。
- 更新 knowledge/README.md、knowledge/QUERY_ENHANCEMENT.md、knowledge/G2_IMPLEMENTATION_PLAN.md、rust/README.md、rust/crates/zg-knowledge/README.md、rust/crates/zg-storage/README.md 的当前职责与命令。
- 未来新增 knowledge/MODULE_CONSOLIDATION_REPORT.md 和独立机器记录；历史 G2.0–G2.3、迁移、Python、Spike 报告及其摘要保持原样。
- G3/G4 实施文档由另一条主线继续维护，本轮不顺带改融合策略、验证语料或标签。已指出的文档→代码评测越界另行修正。

## 4. 测试迁移

### 4.1 处理规则

先导出交接版本的实际用例清单，再为每条旧用例标记：原样保留、拆分迁移、已有正式覆盖或随功能退役。每条“已有覆盖”必须给出具体目标用例及断言；只写“engine 已覆盖”不合格。

测试总数可以减少，但正式图链路覆盖不能减少。历史 693 不是收敛后的最低数量或固定目标。新测试验证行为和风险，不为维持数量而复制旧状态机或增加镜像测试。

### 4.2 原组件用例的去向

以下名称来自计划编写时的源码；执行时先与交接清单核对。

| 旧测试位置/用例 | 去向与保留要求 |
| --- | --- |
| tests/components.rs：immutable_snapshot_roundtrip_metadata_ranges_and_encoding、corrupted_or_incomplete_snapshot_never_becomes_success、same_content_concurrent_writers_and_reopen | 随独立 SnapshotStore 退役；快照存储特有的 blob/meta 原子安装测试不伪装为正式 zg 能力。正式来源版本、编码和范围断言仍由 engine graph_binding 与图租约测试覆盖 |
| commonmark_bom_crlf_tables_fences_and_section_hierarchy | 将图解析仍需的中文、围栏、表格、章节与解码范围断言迁入 KG tests/relations.rs；保留 engine 的 BOM/CRLF 绑定测试。不迁移旧块编号和原始快照 AST 形状 |
| empty_plaintext_long_unicode_and_overlap_coverage、segment_gaps_duplicate_ids_order_digest_and_foreign_snapshot_are_rejected | 独立 Segmenter/快照约束退役；engine 自身分段、Unicode 与重叠测试原样保留，不把两套预算单位强行对齐 |
| stage_fingerprints_and_provider_replacement、parse_and_segment_share_one_captured_provider_descriptor | Parser/Segmenter 描述符契约退役；实际图配置、规则、目录和 Schema 指纹必须保持，并增加固定字节的指纹回归 |
| provider_output_ranges_cycles_budgets_and_incomplete_status | 将仍适用于事实的 UTF-8 范围、父子环、预算、取消、期限和 incomplete 不参与 resolved 的断言保留在 tests/relations.rs、engine graph_facts.rs；Parser 输出契约退役 |
| two_extractors_use_owned_types_and_validate_evidence、independent_empty_extractor_and_explicit_incomplete_parse、extraction_rejects_missing_subject_conflicting_object_bad_value_and_evidence | 独立 KnowledgeExtractor/Assertion 候选契约退役，不迁入 engine 的检索 Entity。G3 的证据回取与缺失诊断仍按其正式任务验证，不声明本轮已完成 |
| embeddings_precomputed_spaces_finiteness_and_per_item_coverage | 预计算清单/Provider 契约退役；正式模型结果的数量、维度、有限值和截断索引仍由 engine models/spi.rs 的 validates_all_representable_provider_output_failures 等既有测试验证 |
| nested_credentials_and_precomputed_manifest_changes_affect_contracts | 预计算清单和独立 Descriptor 测试退役；不复制这套 Descriptor。正式模型凭据脱敏测试保持；完整 EmbeddingSpace 身份增强作为后续独立任务，不能声称已等价迁移 |
| tests/execution_workflow.rs 六项、src/workflow.rs 四项 | 独立线程池/纯状态机退役。将提交前后取消、Unknown 不重试、资源持有、调用方退出、兄弟请求隔离、过期尝试拒绝分别映射到 engine 原生控制/恢复/租约测试及 daemon 调度测试；A1/A2 仍缺的正式断言先补齐再删 |
| tests/native_projection.rs 三项、src/projection.rs 内单元测试 | 退役独立 collection 的 owner、格式和稳定投影 ID；保留通用逐项计数校验到 zg-storage 的现有测试，保留真实逐项错误验证到 engine storage 测试；Flat/HNSW 只读文件不变验证继续在 zg-storage 执行 |
| tests/support/mod.rs、examples/components.rs 引用的确定性向量提供者 | 随上述独立示例/投影测试删除；正式 engine 测试复用已有模型夹具，不引入第二套公共测试模型 |

tests/components.rs 和 tests/execution_workflow.rs 最终删除前，必须完成上述拆分和用例清单核对，不只通过删文件消除编译错误。

### 4.3 必须保留的正式覆盖

| 测试位置 | 要求 |
| --- | --- |
| KG tests/relations.rs | 保留当前事实/范围/父子关系/链接/预算测试，接收必要的 CommonMark 断言 |
| KG tests/graphdb_binding.rs | 保留全部 graphdb 事务与双库共存测试，包括 native_binding_links_beside_the_host_zvec_library、真实全文/向量/图回取、关闭重开和只读文件检查；仅调整测试依赖 |
| KG tests/graphdb_schema.rs、src/graph/*tests.rs | 保留类型/DDL/事务、批次、根/回执、取消、Unknown 和原生拥有者测试，包含交接后 A1/A2 新增内容 |
| engine tests/graph_facts.rs、tests/graph_binding.rs | 保留固定 16 题、代码消歧、负例、Markdown 边界、同次 EntityId、持久化后重新消歧及完整重算对照 |
| engine 内 graph 控制、retention、恢复、文本栅栏、读租约及默认构建测试 | 原样保留交接版行为；本轮不能让未配置/Unknown/取消静默变成成功 |
| storage tests/readonly_collection.rs | 真实 Flat/HNSW 只读打开、查询、关闭后文件集合、长度、mtime 与摘要不变；显式 zvec feature 下必跑 |
| engine 普通搜索/索引/模型、daemon/protocol 回归 | 关闭图时行为不变；不使用 KG 独立测试通过替代正式链路 |

### 4.4 本轮新增的必要验证

- 对固定事实/配置输入记录收敛前后的 canonical JSON 字节、Schema/规则/目录/计划指纹、节点与引用 ID，要求一致。
- 用同一冻结 Markdown 输入比较章节、链接、锚点、diagnostics 和 decoded UTF-8 范围，要求一致。检索 Entity/Fragment 和 EntityId 同时保持。
- 对正式图检查跨关闭重开读取，来源版本、实体映射和 root/receipt 不因模块归属改变而失配。
- 单独验证 knowledge 纯库、storage 无 zvec、storage 有 zvec、engine 默认、knowledge-graph 和 knowledge-graph-graphdb 六条编译/运行路径。
- 在未配置 zvec 原生库的干净构建环境中验证 knowledge 纯库和 storage 无 zvec，不借用已有 target 缓存误判依赖已移除；该检查不运行 native dev 测试。

## 5. 分步实施与完成条件

| 步骤 | 操作 | 完成条件 |
| --- | --- | --- |
| R0 交接冻结 | 接收 A1/A2 源码、用例和环境清单，确认共享写入已停止；记录基线摘要 | 第 6 节记录完整；当前快照回归结果可复查，不使用历史通过数替代 |
| R1 解开正式依赖 | 保留 processing 正式入口；将 markdown_events 移入关系解析；直接复用 storage 摘要函数 | 指纹/事实输出一致，正式图测试仍可编译和运行 |
| R2 退役独立 SDK 链 | 按文件表删除独立实现，先迁移有效断言，再删专属测试/示例/导出 | 无活动引用，测试迁移清单逐项有结论；engine 正式实现不变 |
| R3 整理依赖 | storage zvec feature、engine 显式启用、KG 生产/测试依赖分离，核对测试运行库加载 | 纯库路径无不必要原生依赖，真实共存/只读测试未漏跑 |
| R4 最终回归与交回 | 完整回归、指纹/重开比较、文档和机器记录 | 新基线及遗留项落盘，移交 A3；不提交或推送 |

每步依赖上一步，不让删除、接口重命名和原生构建配置同时大范围变化。出现正式行为变化时停止扩大范围，定位到当前步骤；回退仅限本轮文件，不能用 reset/clean 或整目录覆盖处理共享工作区。

## 6. 与另一位 agent 的交接边界

### 6.1 交接前

- G2 agent 继续 A1/A2，负责 engine graph 构建/恢复 API、候选和回执、原生故障测试及对应记录。
- 模块收敛侧只做只读审查和本计划，不修改源码、Cargo、脚本、CI 或共享状态文档，不与其并发运行构建/故障测试。
- 可以继续整理只读清单；G4 语料设计等工作若在其他会话进行，不得同时改本轮共享文件或更新评测标签来适配实现结果。
- 本文没有向另一会话发送停手、接管或继续执行指令。实际交接需要两边明确停止/开始写入，不能从对方暂时无输出推断已经停手。

### 6.2 R0 交接材料

| 材料 | 必须包含 |
| --- | --- |
| 源码快照 | 分支/HEAD、staged/unstaged/untracked 清单、实际文件 SHA-256；补丁必须包含未跟踪的源码。无提交时保存可恢复的本地源码副本，不能只留 git diff |
| 功能边界 | Resume 等新增接口、operation/attempt 与 fencing 规则、已知/未知结果、候选选择与清理约束；列出尚未完成事项 |
| 用例清单 | 实际逐 target 测试名称、通过/失败/忽略、所需 feature 和平台；A1/A2 新用例单列 |
| 原生环境 | Rust 版本、graphdb 提交/归档摘要、zvec 修复制品/实际加载路径及摘要；编译并发沿用 14 |
| 执行证据 | 命令、exit code、日志、所测源码摘要；真实底座、宿主协议注入、适配层注入和未覆盖窗口分开 |
| 文件所有权 | G2 agent 暂停共享代码与状态文档写入；说明还有哪些其他会话在写。R0 未完成不进入 R1 |

A1/A2 检查点允许保留已明确记录的未覆盖原生窗口，但不能因此宣布完整 G2 故障验收通过。代码编译/回归失败必须先定位；模块收敛不负责掩盖、删除或重命名这些失败用例。

### 6.3 收敛期间

- 收敛侧独占第 3 节文件和全量验证窗口；G2 agent 不推进 A3 的最终测量和 G3 代码接线。设计讨论可以继续，不能同步修改处理接口、事实、Schema、Cargo 或共享文档。
- 原则上不动 engine graph 构建、恢复 API、读租约、A1/A2 实现及其错误语义。若测试迁移发现必须补断言，记录具体目标，限制在测试；必须改行为时退回 G2 单独处理。
- 独立 CARGO_TARGET_DIR 只隔离编译产物，不隔离源码、Cargo.lock、原生共享制品、应用状态或测试端口。对这些资源分别约定路径；不覆盖另一会话的动态库，也不删除其缓存。
- 测试前后核对本轮外文件摘要。出现外部修改，重读并合并，作废受影响的验证快照；不把他人变更算作本轮修改或恢复成旧文件。

### 6.4 交回 A3

- 交回实际文件清单/摘要、依赖 feature 变化、测试迁移清单、新的逐 target 结果、运行库说明及遗留事项。
- G2 agent 明确接收后，再恢复 A3/G3 写入；后续步骤使用新模块边界和命令。
- A3 的最终构建/更新/重开、RSS、磁盘和资源检查成本必须在收敛后的冻结版本上重测；此前数据可保留作对照。
- 完整联合构建示例放在 rust/crates/zg-engine/examples；KG 保留底层事实示例，不通过反向依赖实现 engine 编排。

## 7. 验证命令与证据要求

下列命令是未来执行模板，本轮未运行。使用独立 target 和应用状态目录；graphdb 与修复 zvec 的路径取自正式交接记录，不硬编码另一会话的临时目录。真实原生测试未运行时标记未运行，不能用 cargo check 或替身替代。

```bash
# 工具链与并发：Rust 1.98.0，14 jobs；运行库环境先按交接记录配置。
rustup run 1.98.0 cargo check --manifest-path rust/Cargo.toml \
  --locked --offline -p zg-storage --no-default-features --lib -j 14
rustup run 1.98.0 cargo check --manifest-path rust/Cargo.toml \
  --locked --offline -p zg-knowledge --no-default-features --lib -j 14

# 无需 graphdb 的关系与默认协议回归；保留各组件实际测试清单。
rustup run 1.98.0 cargo test --manifest-path rust/Cargo.toml \
  --locked --offline -p zg-engine -p zg-knowledge -p zg-storage \
  --features zg-engine/knowledge-graph,zg-storage/zvec \
  --all-targets -j 14 -- --test-threads=1

# 正式五组件真实底座回归；不能漏掉 protocol 包。
rustup run 1.98.0 cargo test --manifest-path rust/Cargo.toml \
  --locked --offline -p zg-engine -p zg-knowledge -p zg-storage \
  -p zg-daemon -p zg-daemon-protocol \
  --features zg-engine/knowledge-graph-graphdb,zg-storage/zvec \
  --all-targets -j 14 -- --test-threads=1

rustup run 1.98.0 cargo check --manifest-path rust/Cargo.toml \
  --locked --offline --workspace --all-targets -j 14
rustup run 1.98.0 cargo fmt --manifest-path rust/Cargo.toml --all -- --check
git diff --check
git diff --cached --check
```

另执行 engine/daemon 默认构建的文本栅栏、清理保护、未配置恢复和正文读缓存测试，核对普通索引/搜索；Clippy 对五组件图 feature 和默认 engine/daemon 均执行 all-targets、-D warnings。涉及 zg-storage feature 的 workspace、CI 和单包测试命令显式覆盖开启与关闭两种状态。

用 cargo tree -e normal 检查 production 依赖，用单包纯库编译验证；再单独核对 dev/build 依赖和真实测试加载的动态库。运行测试时 Cargo feature 合并是预期现象，不据此宣称纯库需要或不需要 zvec。

验收报告包含：旧用例→新用例/退役原因、逐 target 数量、新旧指纹/事实一致性、关闭重开、默认行为、原生身份、源码摘要和所有未验证项。历史日志及失败证据不覆盖，不以总通过数或缓存命中代替这些检查。

## 8. 本次明确后置

- engine 与 KG 共用一个 Markdown 语法结构模块：另立行为变更任务，先比较标题、围栏、章节范围、UTF-16 分段预算与 decoded UTF-8 图位置，再决定是否需要轻量公共 crate。
- 更完整的 EmbeddingSpace 身份、预处理及归一化兼容规则：作为 engine 的独立设计/实现，不在删除旧 SDK 时偷偷改变已有模型/索引契约。
- G3 的 relations、caller/callee、query/explore、图扩展与融合策略，以及 G4 数据集和收益评估。
- Windows、新语言、文档到代码关联、真实知识抽取、旧库迁移和历史正文存储。

## 9. 本轮文档交付

最初编写本计划时仅新增本文件，未执行代码调整。用户随后授权先收敛再修复，本次源码、依赖、测试和验证交付以 [收敛报告](../../MODULE_CONSOLIDATION_REPORT.md) 为准；历史验收保持不变。
