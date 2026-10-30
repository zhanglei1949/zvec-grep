# 两套 Markdown 解析整合实施计划

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-27。状态：**P0/P1/P2 已完成；P3 定为折进未发布的索引格式 v2、不提升版本号**。P0 产物见 [reports/markdown-unification/](archive/pre-g4-validation/README.md#markdown-unification "包内：knowledge/reports/markdown-unification")；P1 新建 rust/crates/zg-markdown 并让 KG relations/markdown.rs 消费它（contracts.rs 冻结夹具字节不变通过）；P2 已把 engine extraction/markdown.rs 的自研行扫描换成 zg_markdown::parse、修复 D1–D4，engine Markdown 单测 6/6、全量 lib 450 通过（唯一失败为既有 ANN 只读漂移 flaky，已 stash 对照证明与本改动无关）；P3 因 Rust v2 未发布，直接折进 v2、不动 CURRENT_INDEX_VERSION，本地旧 v2 索引需 --rebuild 重分段，该有意分歧已登记进 [RUST_MAIN_PARITY.md 第 I 节](../rust/RUST_MAIN_PARITY.md)。本文件是详细实施计划；供知识/wiki 构建遵循的先验已写入 [.qoder/repowiki/wiki_plan.yaml](../.qoder/repowiki/wiki_plan.yaml)。本计划承接 [模块收敛计划 §8](MODULE_CONSOLIDATION_PLAN.md) 明确后置的“engine 与 KG 共用一个 Markdown 语法结构模块”一项。

## 1. 目标与边界

把 Markdown 的**结构识别**（标题、围栏、链接、锚点 slug）收敛为单一事实源，由 engine 检索分段与 KG 图事实共同复用；消除两套解析在标题识别上的语义分歧，为 G3 图增强检索（Constrain 模式的 TextMap 硬过滤）打好实体↔节点边界一致的基础。

整合的是**底层结构识别**，不是章节跨度算法，也不是下游产物。以下保持不变：

- engine 的扁平不相交章节 + 字符预算分块 + `MarkdownMetadata` + `ExtractedEntity`/fragment 产出。
- KG 的嵌套章节（`CONTAINS` 层级）+ `node_id` + 锚点/重复 discriminator + `DocumentLink` 引用解析 + `FileFacts`。
- 图 Schema、稳定 ID 规则、`EntityId=(file_id, content, source_range)` 派生、已持久化指纹的字节编码语义（除本计划刻意变更的 engine 分段版本与 KG rules 指纹外）。

本次不新增模型运行时、数据库后端或第二套检索；不把 KG 的图事实语言支持面扩到 Rust/Python/Markdown 之外。

## 2. 当前依据（现状，已核验）

- engine 侧 [extraction/markdown.rs](../rust/crates/zg-engine/src/extraction/markdown.rs) 是自研按行扫描：`scan_headings`（``` / ~~~ 围栏开关、ATX 1–6、setext `===`/`---`）→ `build_sections`（扁平章节 + breadcrumb）→ `section_range`/`markdown_metadata`，常量 `DEFAULT_MARKDOWN_CHUNK_CHARS=3600`、`overlap=540`，无标题时回退 `extract_plain_text_entities`。它是 TS [src/engine/extraction/markdown/extractor.ts](../src/engine/extraction/markdown/extractor.ts) 的 parity 移植（`scanHeadings`/`buildSections`/`computeFenceLines`/`splitMarkdownSection`）。
- KG 侧 [relations/markdown.rs](../rust/crates/zg-knowledge/src/relations/markdown.rs) 用 `pulldown-cmark=0.13.5`（`ENABLE_TABLES`）：`extract_markdown` 产标题/链接，`append_sections` 建**嵌套**章节节点，`anchor_slug` 生成 GitHub 风格 slug + 重复 discriminator，`resolve_documents`/`visit_documents`/`link_target`/`percent_decode` 做跨文档链接解析；`rules_fingerprint` 锁定 `pulldown-cmark=0.13.5` 与 `explicit-md-links+unicode-slug-v1`。
- 分歧点（engine 朴素扫描 vs CommonMark）：缩进 1–3 空格的 ATX 标题、围栏长度/嵌套/信息串、标题文本含行内代码时的取值、引用块/列表内嵌套标题；且 engine **完全不抽链接与锚点**，KG 必须要这两者。
- 两侧章节语义**本质不同**：engine 章节止于“下一标题”（扁平、不相交，服务不重叠分块）；KG 章节止于“下一同级或更高级标题”（嵌套、可重叠，服务 `CONTAINS`）。故共享单元只能是**标题树 + 链接 + 围栏 + slug**，不能是预算好的章节区间。

## 3. 关键约束（决定方案形状）

- **依赖方向**：KG 不得依赖 engine（[doc2 §6](https://yuque.alibaba-inc.com/proxima-se/cg5gr6/khyqauxnxe0be129) “接口类型不依赖 zg-engine”、收敛计划“不让 knowledge 反向依赖 engine”）。
- **可选依赖门控**：`zg-knowledge` 在 engine 中是 `knowledge-graph` feature 下的 optional 依赖（[zg-engine/Cargo.toml](../rust/crates/zg-engine/Cargo.toml) `knowledge-graph = ["dep:zg-knowledge"]`），而 engine 核心 `extraction/markdown.rs` 常驻编译。
- **推论（归属唯一解）**：共享 Markdown 结构解析既不能放 engine（KG 不可反向依赖），也不能放 KG（engine 核心不可依赖 optional crate），只能放**新的中立 crate**。这回答了收敛计划 §8 悬置的“是否需要轻量公共 crate”——需要，且是唯一干净归属。
- **基座选择**：以 KG 现有的 `pulldown-cmark` 解析为基座（规范、能力全、KG 已锁指纹），engine 反向复用它并丢弃朴素扫描；**不是**让 KG 复用 engine 的朴素扫描（那会降级 KG 正确性并缺链接/锚点）。
- **TS↔Rust parity 是过渡期约束**：TS engine（根 `src/`，npm `@zvec/zvec-grep`）后续整体退役、迁移到 Rust（`rust/crates/*`）。退役前 [RUST_MAIN_PARITY.md](../rust/RUST_MAIN_PARITY.md) 要求 Rust 匹配 TS 可观察行为，而 Markdown 章节边界经 `EntityId` 进入共享 `.zvec-grep/` 索引格式，故 engine 切换分段会临时与 TS 分歧——须按第 6 节时序处理。

## 4. 共享 crate 设计：`zg-markdown`

- 位置 `rust/crates/zg-markdown`（经 `members = ["crates/*"]` 自动成为 workspace 成员）。
- 依赖仅 `pulldown-cmark =0.13.5`（`default-features=false`，`ENABLE_TABLES`），从 KG 迁入；纯 Rust、无原生依赖，可进 CI 全平台矩阵。
- 公共 API（中立类型，不含 `EntityId`/`FileFacts`/chunk 概念，不计算章节区间）：
  - `pub const PARSER_VERSION: &str`——解析器版本/选项的单一指纹源（例如 `pulldown-cmark=0.13.5+tables-v1`）。
  - `pub fn parse(text: &str) -> MarkdownStructure`，`MarkdownStructure { headings: Vec<Heading>, links: Vec<Link> }`。
  - `Heading { level: u8, text: String, slug: String, span: Range<usize>, breadcrumb: Vec<String>, anchor_discriminator: usize }`，`span` 为 decoded UTF-8 字节区间。
  - `Link { span: Range<usize>, destination: String }`。
  - `pub fn anchor_slug(title: &str) -> String`（GitHub 风格，从 KG 迁入）。
- 消费方各自派生章节：engine 由 `headings` 算扁平不相交区间；KG 由 `headings` 算嵌套区间并建 `FactNode`/`Anchor`/引用。crate 对两侧下游一无所知。

## 5. 分步实施与完成条件

| 步骤 | 操作 | 完成条件 |
| --- | --- | --- |
| P0 分歧审计 | 冻结一套 Markdown 语料，覆盖 ATX(1–6/尾随`#`/`#`后无空格负例)、setext(`===`/`---`)、thematic break(`---`/`***`/`___`)与 setext 歧义、围栏(```/~~~`/信息串/缩进/未闭合/块内类标题行)、表格、链接(内联/引用/自动、相对/绝对/外部、百分号编码、锚点)、CJK 标题、BOM、CRLF、纯前言无标题、重复标题；对同语料分别跑 engine 扫描与 KG CommonMark，产出差异报告并逐项归类“engine 缺陷/有意差异” | 交付 `knowledge/reports/markdown-unification/divergence.{json,md}`；分歧未厘清不进入 P1（落实 §8“先比较后改”） |
| P1 落地 crate + KG 切换 | 新建 `zg-markdown`；把 `zg-markdown` 补进 [rust-ci.yml](../.github/workflows/rust-ci.yml) 逐包 check 循环与 [check.sh](../rust/scripts/check.sh) 循环（否则新 crate 永不被独立 check）；KG `relations/markdown.rs` 改为消费 `zg_markdown::parse`，`rules_fingerprint` 由 `PARSER_VERSION` 派生 | KG `tests/relations.rs`+`contracts.rs` 冻结夹具通过；图事实除刻意变更的 rules 指纹外字节一致；engine 本步不动 |
| P2 engine 核心切换 | engine `extraction/markdown.rs` 用 `zg_markdown::parse` 的标题树替换 `scan_headings`，保留自身扁平章节算法、UTF-16 分块、`markdown_metadata`、前言实体、无标题回退；`zg-markdown` 作为 engine **非可选**依赖加入 | engine `extraction` 单测通过；默认 feature（不开 knowledge-graph）下 engine 纯库编译通过 |
| P3 索引影响处置 | 因章节边界变化 → Markdown `EntityId` 变；定位并提升索引/分段格式版本常量，使旧索引被判过期并触发 Markdown 全量重建；按第 6 节协调 TS 时序 | 版本常量落定；升级路径与“换 Embedding 模型须 `--rebuild`”同类的重建语义写清；黄金测试固定语料→期望章节区间 + 同版本内 EntityId 稳定 |
| P4 跨侧一致性验证 | 断言同一 Markdown 文件下 engine 实体区间与 KG 章节 span 共享同一标题边界，使 [graph_facts/mod.rs](../rust/crates/zg-engine/src/graph_facts/mod.rs) 的 `bind_entity_indices` 映射干净；KG 侧对比 canonical JSON/node_id/两种 Schema 指纹；engine 侧量化 EntityId churn | 一致性测试通过；除刻意指纹/版本变更外无意外漂移；全 CI 绿（见第 7 节） |
| P5 文档 | 更新 [MODULE_CONSOLIDATION_PLAN.md §8](MODULE_CONSOLIDATION_PLAN.md)（标记后置项已解决）、KG/engine README、`rust/README.md`、CI 说明、wiki_plan.yaml（把“规划中”改为已落地） | 文档与代码一致；无死链 |

每步依赖上一步，不让“删旧扫描、加新 crate、改索引版本”同时大范围变化；出现正式行为变化时停止扩大范围，定位到当前步骤。

## 6. 行为变更、索引影响与 TS 退役时序

- **engine 一次性重建**：Markdown 分段边界变化 → `EntityId` 变 → 需全量重索引 Markdown（类比换 Embedding 模型必须 `zg --index --rebuild`）。属预期成本，须在发布说明写明。
- **KG 指纹变更**：`rules_fingerprint` 改由 `PARSER_VERSION` 派生 → 触发一次 graph-only 重建（不写 zvec、不重算向量），成本低。
- **TS 时序（关键）**：TS 退役前，Rust engine 切换分段会与 TS 分歧、破坏共享 `.zvec-grep/` 索引兼容。三选一，推荐 A：
  - **A（推荐）**：把 P2/P3 的 engine 切换**排期到 TS 退役之后或与之同批**，parity 约束自然消失，Rust 成为单一实现后自由切换。
  - **B**：TS 退役前就要切，则**提升索引格式版本**区分 TS 建与 Rust 建的索引并强制重建，接受短期两格式并存。
  - **C**：同步迁移 TS extractor 到同一 CommonMark 分段——与“TS 待退役”方向相悖，不推荐。
- 决策依据：先确认 TS 退役时间表，再定 P2/P3 落点；本计划不擅自设定 TS 退役时间。

## 7. 验证命令与证据要求

下列为执行模板，本轮未运行。使用独立 target 目录，工具链 Rust 1.98.0。

```bash
# 新 crate 纯库与全平台可编译（无原生依赖）。
rustup run 1.98.0 cargo check --manifest-path rust/Cargo.toml --locked --offline -p zg-markdown --all-targets -j 14
# KG 切换后关系/契约回归（无需 graphdb）。
rustup run 1.98.0 cargo test --manifest-path rust/Cargo.toml --locked --offline -p zg-knowledge --all-targets -j 14
# engine 默认 feature（核心分段常驻，不开图）与图 feature 两条路径。
rustup run 1.98.0 cargo test --manifest-path rust/Cargo.toml --locked --offline -p zg-engine --all-targets -j 14
rustup run 1.98.0 cargo test --manifest-path rust/Cargo.toml --locked --offline -p zg-engine -p zg-knowledge --features zg-engine/knowledge-graph --all-targets -j 14
# 格式与元数据。
rustup run 1.98.0 cargo fmt --manifest-path rust/Cargo.toml --all -- --check
```

- `cargo check` 不编译 tests，断言“测试通过”必须用 `cargo test` 或 `--all-targets`，不能以 check 绿冒充。
- 证据要求：P0 差异报告逐项归类；P1/P4 冻结夹具的 canonical JSON、node_id、两种 Schema 指纹前后对照；P3 EntityId churn 量化与版本常量变更点；逐 target 测试数量。历史通过数不作为本轮判据。
- CI 连带：新增 crate 必须补进 rust-ci.yml 与 check.sh 的逐包 check 循环；`knowledge-graph`/`knowledge-graph-graphdb` 为非默认 feature，`--workspace --all-targets` 不激活，图路径须显式 `-p <crate> --features <f>` 覆盖。

## 8. 风险与权衡

- **新 crate 开销**：workspace/CI/Cargo.lock 增量；因可选依赖约束无更干净归属（放 KG 破坏 engine 核心隔离、放 engine 破坏 KG 独立性、放 zg-storage 语义不符——storage 是无策略持久化原语）。
- **检索质量回归**：分块边界变化可能影响召回；用 P0 审计 + [规划文档](https://yuque.alibaba-inc.com/proxima-se/cg5gr6/fygih1gn6wy02z7h) 要求的 benchmark（普通 zg vs 图增强 zg）对照把关，不靠格式通过率替代质量判断。
- **时序耦合**：engine 切换与 TS 退役强相关；若 TS 退役延期，P2/P3 须相应顺延或走 B 方案版本隔离。
- **指纹/版本漂移**：KG rules 指纹与 engine 分段版本都会变，须在同一轮内成对核对，避免“图重建了但文本索引版本没升”这类半迁移状态。

## 9. 备选方案（若评估后整合成本过高）

保留两套解析，但新增**分歧守卫测试**：冻结语料上断言 engine 章节与 KG 章节的标题边界在容差内一致，防止继续漂移，并文档化差异。代价是 G3 Constrain 模式的 TextMap 需长期容忍两侧边界不一致——鉴于 [doc2 §3](https://yuque.alibaba-inc.com/proxima-se/cg5gr6/khyqauxnxe0be129) 的硬过滤要求（实体/片段 ID 须完全落在获准文本区间），**推荐走整合而非守卫**。

## 10. 本次明确后置

- engine 的 UTF-16 分段预算与 KG 的 decoded UTF-8 图位置单位统一：不同度量，各自保留，不在本计划内对齐。
- 超出标题/链接的 CommonMark 细节（HTML 块、复杂列表嵌套等）：仅当 P0 审计显示其影响章节边界或锚点时才纳入。
- TS extractor 的迁移/删除：属 TS 退役项目，不由本计划承载。
- `zg-markdown` 是否进一步承载代码/其他格式的结构识别：本计划只处理 Markdown，不预先泛化。
