# G3 实施计划：关系探索与图增强检索

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-29。状态：**G3.0—G3.3 已完成；P0／P1／P2 已完成；P3 Release 默认预算验证通过，Debug 仍超时**。G2 已[带遗留项收尾](G2_ACCEPTANCE_REPORT.md)：代码域覆盖不足，构建成本未达标，业务模型与 TextMap 的交接已在 G3.1 完成。本计划保留其构建、发布、恢复和租约机制，先交付代码图查询，再复用接口接入文档图。

G3.0 记录见[基础契约](G3_0_CONTRACTS_REPORT.md)，已交付能力见 [G3.1 记录](G3_1_IMPLEMENTATION_REPORT.md)。G3.2 与 G3.3 的 Rust／CLI／MCP／daemon 同请求对照、默认／原生受影响回归及静态检查已通过。本轮到 G3.3 为止，不推进 G3.4、G3.5。详见 [Augment 记录](G3_2_IMPLEMENTATION_REPORT.md) 和 [Constrain 记录](G3_3_IMPLEMENTATION_REPORT.md)。

设计依据：

- [KG 模块封装与接口设计](https://yuque.alibaba-inc.com/proxima-se/cg5gr6/khyqauxnxe0be129)：读取版本为 2026-10-27 08:50:07 UTC，以第 2—4 章的数据模型和查询语义为准。
- [知识图谱 FY27 S2 规划](https://yuque.alibaba-inc.com/proxima-se/cg5gr6/fygih1gn6wy02z7h)：读取版本为 2026-10-27 07:29:54 UTC，用于范围、优先级和演进顺序。
- 本地代码核对基于 HEAD `b5ea18d8f031d7ccae992221b10d5cab242e4e17` 及当前未提交实验代码。源码事实只用于确定改动入口，不作为正式图谱模型的约束。

规划文档中的 `query` 对应 zg 的 `context`。两份文档详略不同时，按配套接口设计收敛：公开结构化关系入口是 `explore`；KG 的 `query` 是可选便利接口；`locate / relations / expand / shortest_path` 是 explore 内部操作，不分别增加顶层服务入口。

## 1. 交付范围

G3 交付三个可独立使用的能力：

| 能力 | 用户任务 | 执行方式 |
| --- | --- | --- |
| explore | 查调用者、被调用者、依赖、路径及未解析原因 | 结构化定位与图查询，不使用 Embedding 或 LLM |
| Augment | 搜到实现后，补充调用关系及相关代码 | 普通检索、多查询合并后，沿图补充；保留普通结果排序 |
| Constrain | 只在指定关系范围内检索 | 先完整计算图范围，再在范围内执行全文／向量检索 |

代码图先支持现有抽取器实际覆盖的 Rust/Python 静态关系；Markdown 章节和文档链接随后接入同一套接口。动态派发、类型推断、宏展开及新增语言不随查询接口一并承诺。

文档与代码的显式关联按 S2 后续阶段交付：接口允许使用实际构建支持的 LINKS，不在首轮查询开发中补抽取器或猜测同名符号。Agent memory、第二个应用、独立 KG 生命周期、通用 SDK、KG `query`、任意 Cypher、多轮自动规划及全路径枚举均不作为 G3 首轮前置。

G3 验证查询语义和资源边界；实际召回增益、Agent 成功率及成本对照归 [G4](G4_VALIDATION_SET_DESIGN.md)。

## 2. 分层与 G2 交接

### 2.1 职责与调用

| 所在层 | 保留职责 | G3 新增工作 |
| --- | --- | --- |
| zg 应用层 | 工作区、刷新、授权、检索策略、多查询检索与结果合并、全局预算及上下文组织 | `ZvecGrep::explore`、三种 context 模式、读取视图接入与结果转换 |
| 共享检索组件 | 一个检索计划的全文／向量召回、prefer_symbol 和路由融合 | 视图约束、硬范围、按需获取模型；继续复用现有 pipeline |
| KG 图增强检索 | 输入命中映射、图扩展、范围计算及证据组织 | `augment / resolve_scope`，不接管 zg 多查询策略 |
| KG 图模块 | 业务实体／关系语义、受控图读写；内部封装 graphdb | 定位、邻接、扩展、路径和引用检查；参数化、有界查询 |
| graphdb | 持久化、事务和原生查询执行 | 复用固定上游 binding，不新增通用多后端存储接口 |

```text
zg.explore ────────────────→ KG.explore
zg.context(Off) ───────────→ 检索 → zg 合并
zg.context(Augment) ───────→ 检索 → zg 合并 → KG.augment → zg 组织上下文
zg.context(Constrain) ─────→ KG.resolve_scope → 范围内检索 → zg 合并
```

共享检索先作为 engine 内现有组件整理，不为“未来拆分”另起一套检索实现或新 crate。KG 类型不依赖 zg-engine，不持有宿主 zvec 写连接，不通过回调再次调用 `ZvecGrep::context`。zg 接入首版只需 ReadViewProvider 和 EvidenceProvider；RecallProvider 留给实际使用 KG `query` 的调用方，不要求为占位接口实现第二条召回链路。

### 2.2 交接状态与剩余差距

| 已核对的本地实验入口 | 与目标设计的差距 | 处理归属 |
| --- | --- | --- |
| [graph/schema.rs](../rust/crates/zg-knowledge/src/graph/schema.rs) | 已升级实体直连业务边＋UnresolvedRef 的 v2 格式 | G3.1；显式构建新候选，查询拒绝旧格式 |
| [graph.rs](../rust/crates/zg-engine/src/storage/read_session/graph.rs) 的 GraphReadSession | 公开 explore 已使用按需批量读取；资格核验仍逐页扫描完整提交 | G3.1 复用拥有者和租约；read_facts 留给构建／恢复 |
| [indexed_search/service.rs](../rust/crates/zg-engine/src/pipelines/indexed_search/service.rs) | 当前先 query_models，再借读取会话 | 调整为先选视图、计算严格范围，再按需获取模型 |
| [StorageSearchFilter](../rust/crates/zg-engine/src/storage/types.rs) | 有 file_ids、entity_ids，没有片段 ID 硬范围 | 补片段过滤及底座能力验证；不能用实体白名单冒充所有细粒度范围 |
| [context.rs](../rust/crates/zg-engine/src/pipelines/indexed_search/context.rs) | 已有多查询分组、合并、覆盖优先；ContextResult 尚无 graph | 保留排序，携带内部命中来源与片段 ID，另加图结果 |

查询目标采用语雀模型：file 根和业务 Entity、直接 CONTAINS／引用型边、UnresolvedRef；已解析引用不再另存完整引用记录。逻辑跳数以业务边计数，不能把旧表的辅助连接算作两跳。旧事实格式只供内部对照，不通过永久双写或全图转存满足新查询。

### 2.3 交接清单与并行边界

正式图查询接入前，构建侧需给出下列可验证输入：

| 交接项 | G3 依赖的行为 |
| --- | --- |
| 发布视图 | 已确认选中的构建、稳定文本版本、规则／配置／Schema 指纹及分域状态；候选提交不等于可查询 |
| 业务图契约 | 直接业务关系及逐引用 ref_id；同构建中每个引用只存在于边或 UnresolvedRef，辅助记录不进入遍历 |
| TextMap | 按来源共享 `source_id / source_span / entity_id / content_span`；关联实际片段元数据，明确 UTF-8 字节单位及同版本摘要 |
| 所有权与并发 | 可借出的文本／图资源；原地更新受同一读写准入保护，清理须等待读者及原生句柄退出 |
| 能力与覆盖 | 实际支持的域、语言、关系和映射规则；pending、failed、unsupported 与合法空图可区分 |

当前 Schema 的转换、映射落盘及重建由单独的构建交接改动完成，不在 G3 查询入口中自动升级、不在查询时补写。若 G2 暂时保留旧实验格式，G3.0 的纯类型、参数、预算及适配边界可以先开发；依赖新模型的真实查询验收等兼容候选可发布后再执行。事实重算用例不能替代这一交接。

### 2.4 交接改动

| 对象 | 确定的契约 | 写入与读取归属 |
| --- | --- | --- |
| 业务 Entity | 构建内唯一 `id`、宿主来源 `source_id`、`kind / name / span`；代码和文档均有 file 根，span 为解码后 UTF-8 半开区间 | KG 投影并保存；NodeId 与 zg 检索 EntityId 分开解释，不承诺跨编辑稳定 |
| CONTAINS | 仅连接同来源直接父子节点，无环，非根节点唯一父节点；不含 ref_id | KG 结构校验；一条业务边即一跳 |
| 引用型边 | CALLS／IMPORTS／LINKS 直接连接业务节点；每次引用保存 `ref_id / span / locator`，调用按需保存 receiver／arity | 多次调用和自递归分别存边；不另存完整已解析 Reference |
| UnresolvedRef | `id=ref_id`，保留 owner、关系种类、位置、定位信息和原因；同构建内与已解析边互斥 | 候选事务内转换；目标删除前保留入边引用，新增符号也重算受影响解析域；失败／取消不记 not_found |
| TextMap | `source_id / source_span / entity_id / content_span`；entity_id 为实际 zg 检索实体 ID，每段等长且字节一致 | zg 同次提取生成并在分配 ID 后绑定；KG 按来源共享保存，随候选核验和发布 |
| 来源覆盖 | 同一宿主扫描清单派生必需来源；逐来源记录模块映射、事实完整性及未就绪原因 | 分开报告来源覆盖、引用解析和正文映射能力；文件数相等不代表 ready |
| 读取需求 | None／Optional／Required 加所需域、关系和 TextMap 能力；能力须属于所选视图 | ReadViewProvider 核验；文档域 ready 不授予代码域查询资格 |

作用域和绑定继续供解析阶段使用，增量按受影响解析域取得同版本解析产物；不把旧八表事实格式保留为第二套公共查询模型。辅助数据、来源清单和映射均不进入业务遍历。名称和限定名不作唯一键。

TextMap 在实际正文提取时记录区间，拼接／删减逐段映射；片段区间和 FragmentId 复用 zg 元数据，不逐节点、引用或片段复制映射。原始／解码摘要及文本版本放在来源／构建记录中。无正文与必要映射缺失分别诊断。当前 `entity_ids` 包围区间绑定不能作为精确映射契约。

业务模型和 TextMap 一次升级 Schema、事实格式及映射规则指纹，通过显式重建生成新候选；查询不升级旧格式。正文、检索 EntityId 和片段结构相同才复用既有向量，否则由 zg 正常重建文本。旧构建待读者退出后清理。

按需读取分为资格核验和业务读取：打开视图核对选择、根／回执、版本及分域状态，完整来源与实体绑定有界分批核对；业务操作按需 locate／get_nodes、neighbors、get_references、read_text_map。遍历按层批量读取，使用稳定分页和同一预算。`read_facts` 留给构建核验与显式诊断，不作为公开查询的默认读取路径。必要资格核验不因优化而改为抽样。

### 2.5 开发顺序

| 顺序 | 工作 | 交付内容 |
| --- | --- | --- |
| 1 | 契约细化与耗时观测 | 模型、映射、覆盖及读取需求契约；按阶段记录成本，保持原执行策略 |
| 2 | 代码域覆盖修正 | 查清原 353／356 的逐文件原因；修正模块根配置和完整性判定，保留冻结清单与历史结果 |
| 3 | 业务图投影和 TextMap | 新格式候选，完成实际文本绑定、重开及发布核验 |
| 4 | 按需读取及 Provider | 同版本读取视图、域／能力检查、有界图操作 |
| 5 | G3.1 代码 explore | caller／callee 起步，扩展、路径与引用检查；Rust／CLI／MCP／daemon 同语义 |
| 6 | G3.2—G3.4 | 先 Augment，再 Constrain，随后复用到文档图 |

第 1—5 步已完成，本轮按 G3.2 → G3.3 推进，完成 G3.3 后收口；G3.4、G3.5 后续安排；不做构建性能重构。构建耗时继续区分旧图读取、事实收集／解析、候选创建／导入、读回核验、根／回执提交、重开确认和发布选择；嵌套计时不能直接相加。

构建成本分析从第 1 步开始。先定位导入、重复读全图、提交核验和 checkpoint 的占比，再决定后续优化；若需要原地图增量，另行落实读写准入、版本和恢复协议。G3.1 可用固定候选开发，约 195 秒的单文件维护成本仍是日常刷新联调前的遗留项。G2 原生故障窗口残留继续由原报告跟踪。

本轮结果见 [交接记录](G2_G3_HANDOFF_REPORT.md)：契约已细化，两个 Rust 基准模块根已修正，原第三项是隐藏目录的分母口径差异。当前冻结语料的 368 个代码来源均有事实，其中 260 个受抽取器整文件保守降级影响，代码域仍未 ready；不能在后续模型迁移时直接忽略这些标记。graph-only／单文件更新仍由候选导入主导；本轮只记录成本，不改执行策略。

## 3. 读取视图、版本与执行约束

### 3.1 同一次请求使用同一视图

`KgReadView` 是请求持有的资源句柄，不能通过 JSON 传输。对外返回可序列化的 `ViewStamp`，至少标识工作区、发布 generation、text_revision、graph_build_id、Schema、规则／配置及解析目录版本；值复用已核验的宿主记录，不新增另一套发布状态。

NodeRef 为 `{graph_build_id, node_id}`。RankedHits、ResolvedScope、引用 ID 查询及证据读取均携带并核对 ViewStamp；单独的字符串 ID 不足以复用跨请求结果。过期引用返回 `view_mismatch`，不按名字重新绑定或套用旧偏移。范围对象由服务生成，客户端不能提交自造片段白名单充当已验证范围。

`open_view` 的图要求：

| 要求 | 行为 |
| --- | --- |
| None | 只借文本资源，不打开 graphdb；Off 使用 |
| Optional | 能借兼容图则保留；否则记录明确原因；Augment 使用 |
| Required | 没有兼容且已选中的图就失败；explore／Constrain 使用 |

ReadViewRequest 另带所需域、关系及正文映射能力。Required 必须满足本次需求；Optional 不满足时返回文本视图及原因。None 不允许夹带图需求。来源 ready、引用是否解析和正文能否精确映射分别核对，不以“有图库”代替能力证明。

顺序为工作区解析和刷新协调 → 读取准入 → 选定视图 → 执行查询 → 核验证据／组装结果 → 释放资源。刷新沿用 zg 策略；KG 查询不启动构图、补写或恢复。元数据核验不能代替原始／解码摘要及证据范围核验。

同 generation 的原地更新不能仅靠版本戳保证安全，须保留读写准入直到读取结束。图候选不可变仍需租约阻止删除。已持有的兼容旧视图可读到请求结束；无法证明旧文本仍安全时拒绝继续。HEAD 改变使后续借用重新选视图，不要求关闭仍被读者占用的原生实例。

### 3.2 writer 借读与异步边界

普通 Off 保留现有 writer 借读流程。Augment 借 writer 且无兼容联合视图时，只在文本读取仍安全的前提下返回普通结果并诊断。explore／Constrain 不借这种会话，按准入预算等待可用视图或明确失败。

ReadViewProvider 由 engine 实现，复用 ReadSessionCache／ReadSessionLease；不再增加全局图会话管理器。EvidenceProvider 从同视图检索实体或受控来源读取，做摘要、UTF-8 范围及位置转换；无可验证的同版本文本则不返回有效证据。来源文件一次读取为请求内不可变字节，再对这份字节核验摘要、解码和切片，不先验文件、随后另读内容。

读取优化不能降低 G2 的资格检查：仍需识别目录成员、实体映射和不认识图栅栏的旧写入者造成的变化，不能以抽样或 HEAD 未变代替必要核验。“不读全图”限制的是图查询和物化方式，不等于跳过来源资格检查。

宿主异步编排可以持有资源租约；graphdb 连接、事务、游标留在同一个有界同步任务中，不跨 await，返回独立结果。取消只作用于本次操作，等待 worker 退出后释放句柄，不关闭共享库终止单次请求。

### 3.3 一份总预算

RequestContext 共享取消、绝对截止时间、访问节点／边、映射与候选数量、输出字节预算。多查询、多个种子、分批查询及自适应召回均扣同一份总额度，不逐次重置。

首轮建议默认值：种子最多 5、扩展最多 2 跳、补充节点最多 20、增强阶段业务边检查最多 100、图附加输出最多 16 KiB。explore／scope 的扫描上限独立配置，初始取 2,000 个节点、10,000 条边；严格范围另限 5,000 个过滤单元和 256 KiB 过滤表达式，底座能力更低时按其限制拒绝。不把“仅返回 20 个补充节点”误用于严格范围截断。这些是保守初值，调整须保留超限语义，不作为容量结论。

新增图请求的默认总执行期限取 30 秒，图阶段取剩余期限与 5 秒的较小值；调用方可收紧，放宽受服务上限控制。这是初始配置，不是延迟承诺。Off 保持既有行为；lock_timeout_ms 只表示锁等待预算，不充当总执行期限。会话打开与版本核验也计时，不另建无关的 120 秒上下文。

## 4. API 与返回语义

代码 explore、Augment 和 Constrain 已实现；文档域后置。KG 签名是契约示意，代码通过 Provider 与纯函数接入。

```rust
// zg 的异步入口
ZvecGrep::explore(ExploreOptions) -> EngineResult<ExploreResult>
ZvecGrep::context(ContextOptions { graph, .. }) -> EngineResult<ContextResult>

// KG：视图和执行上下文均由接入层提供／核验
KgService::open_view(ReadViewRequest, RequestContext) -> Result<KgReadView>
KgService::explore(&KgReadView, ExploreRequest, RequestContext) -> Result<ExploreResult>
KgService::augment(&KgReadView, RankedHits, AugmentOptions, RequestContext)
    -> Result<GraphSupplement>
KgService::resolve_scope(&KgReadView, ScopeSpec, RequestContext) -> Result<ResolvedScope>
```

KG 签名描述操作契约，不要求同步阻塞宿主异步线程。KG 公共类型不包含 zg ContextItem、graphdb Connection／Transaction 或物理行号。首版不实现 KgService::query，也不改 G2 的维护 API。

| 类型 | 必须表达的内容 |
| --- | --- |
| ExploreOptions | root、ExploreRequest、ExecutionOptions、锁等待与取消；不提供刷新或模型参数 |
| ExploreRequest | Locate、Neighbors、Expand、ShortestPath、References 五类操作；caller/callee 是 CALLS 的入／出向便捷参数 |
| ExploreResult | view、定位状态／候选、nodes、relations、paths、references、evidence、execution；按操作填充，不把空路径与定位失败混为一谈 |
| Locator | NodeRef，或来源路径＋字节位置，或名称＋可选来源／kind；名字不作唯一键，位置落在嵌套节点时按显式 kind／最内层规则定位 |
| RankedHits | ViewStamp、实际 EntityId／FragmentId、命中范围、合并后优先级及所有查询来源；不从最终展示文本反推 |
| ScopeSpec | 唯一锚点、关系类型、方向、min/max hops、是否包含起点、正文区域与遍历边界；不接收自然语言规划 |
| ResolvedScope | ViewStamp、完整性证明、来源区间、可硬过滤单元、映射诊断及资源统计；只允许完整对象进入召回 |
| EvidenceRef | 同版本来源、原始／解码摘要、UTF-8 区间；引用边带 ref_id，CONTAINS 附抽取来源 |

GraphQueryMode 使用显式枚举：Off、Augment { seeds, expansion }、Constrain { scope }。默认 Off；一请求只选一种模式；rg＋非 Off 在工作区、模型和数据库操作前拒绝。

### 4.1 explore

| 操作 | 承诺的语义 |
| --- | --- |
| Locate | 完成定位后明确 unique／ambiguous／not_found；未完成定位返回错误，不把截断候选当唯一 |
| Neighbors | 按关系类型和方向返回一跳业务边；caller 查 CALLS 入边，callee 查出边 |
| Expand | 按 min/max hops 执行有界遍历；默认 min=1，包含起点需显式设置 |
| ShortestPath | 返回约束内一条最少跳路径；并列时固定顺序选一条；未完成搜索不报告无路径 |
| References | 按引用 ID 或节点／来源、种类、状态查询；统一组合已解析边与 UnresolvedRef，候选按需重算 |

Locate 可直接返回多义候选；其他操作须有唯一锚点。锚点多义或未找到时返回定位结果且不执行遍历；Constrain 则拒绝继续检索。跨版 NodeRef／引用请求始终报错。

遍历只经过业务边；辅助节点、作用域、构建元数据及候选目标不进入路径。权限／工作区边界约束中间节点；展示过滤只影响返回，两者参数分开。CONTAINS 只走直接父子边，不能把传递包含当一跳。

按节点去重不合并引用事实：两次调用保留两条 ref_id 及两处证据，自递归保留。遍历按层批量读取，使用稳定的节点／引用 ID 顺序；访问集合防止循环，不承诺枚举全部路径。最短路径返回的每一步及探索返回的关系均保留其引用位置。

数量／输出限制可返回 `partial` 和停止原因；`complete` 只指声明的图、过滤和跳数范围执行完成，不代表动态调用全部可知。批量读取逐项说明缺失；解析失败、pending、超时和取消不能伪装成 not_found。必要来源证据失效时不能返回该结果为完整已核验路径。

## 5. 图增强检索流程

### 5.1 Off

保持现有请求规范化、召回、排序、模型授权、writer 借读与结果序列化行为，不打开图。不因编译图 feature 自动启用图查询；新增 graph 字段缺省或省略时与旧请求等价。

### 5.2 Augment：先合并，再补图

1. zg 按原策略执行各检索计划；单计划内部仍由共享组件进行全文／向量召回及路由 RRF。
2. zg 完成多查询结果合并、去重和覆盖优先选择，同时保留实际实体、命中片段及查询来源。
3. 按合并后优先级选最多 5 个可映射业务节点。优先使用命中片段的位置映射，避免把容器命中扩成其中所有符号；去重后同一节点仍保留全部查询来源。无法唯一映射则记录原因，不凭同名选种子。
4. 调用一次 KG augment；沿指定业务边扩展，在共享预算内读取并核验证据，不再次全文／向量召回、不重新 Embedding。
5. 新补充片段放入 ContextResult.graph。已在普通结果中的内容只关联其结果标识并合并关系证据，不重复复制；正文区间、引用位置和查询来源不能因去重丢失。

items、group_results 的顺序、得分和原配额不变。补充结果按种子输入优先级、跳数和稳定 ID 输出，该顺序只用于确定性和截断，不作为相关度分数，不进入 RRF。zg 控制最终上下文长度，先保留普通结果，图补充用独立配额；渲染截断仍须保留所展示关系对应的证据，不能悬空。

G3.2 的 A0—A6 已完成：覆盖基线、命中交接、共享读取、种子映射、扩展、公开入口和交付核验均已有记录。G3.1 修复记录见 [review 记录](G3_1_REVIEW_REPORT.md)，本轮证据保存在 [g3.2](archive/pre-g4-validation/README.md#g3-2 "包内：knowledge/reports/g3.2")。

A0 的冻结清单含 382 个 Rust/Python 来源：110 个抽取完整、272 个不完整（含 1 个隐藏路径）；历史 260 个缺口仍全部不完整。主要诊断为绑定形式不支持（249 个来源）、Rust impl 解析不支持（145）、通配导入不支持（73），计数可重叠。绑定与通配导入的并集为 272；impl 诊断本身不将文件标为不完整。该扫描不核验文本索引、不发布图，也不代表代码域已就绪。后续覆盖补齐按第 12 节执行，历史结果保留。

| 顺序 | 改动与入口 | 完成条件 |
| --- | --- | --- |
| A0 覆盖基线 | 复用现有 coverage 工具，重新冻结当前源码、模块根和来源清单，按诊断归类；260 个缺口只作为历史基线 | 区分抽取不完整、配置缺失和非代码来源，确定优先修复项；不放宽 ready，不把整仓修复作为小语料开发的前提 |
| A1 命中交接 | 调整 indexed_search/context.rs 的结果组装，在既有去重、覆盖选择之后保留 SearchHit 的真实 EntityId、命中 FragmentId／区间、最终优先级和全部查询来源 | 普通 items／group_results 的排序、分数、去重键和配额逐项等价；不从展示正文或 trace 反推片段；RankedHit 用 source_spans 分段列表表达非连续映射，替换未发布的单区间字段 |
| A2 共享读取 | 在 indexed_search/service.rs 接入可选图描述和同一文本租约，整理 G3.1 的读取／证据接入供内部复用 | 选定文本代次和图候选后再检索；普通召回、图读取和证据共享租约。完整图资格核验留在图阶段，成功前不颁发已核验图视图；不逐种子调用公开 explore，也不另建会话管理器 |
| A3 种子映射 | KG 接收 RankedHits，按 EntityId／FragmentId 和 TextMap 核验身份、反向映射区间，再通过有界业务节点读取定位 | 按最终优先级选最多 5 个唯一节点；同节点合并所有命中／查询来源。跨多个同级定义的容器命中、同名多义或映射不完整分别诊断；不扩成全部符号，不凭名字猜测 |
| A4 第一条调用链 | KG 增加 augment，先做显式一跳 CALLS 入／出向，再补现有契约允许的两跳、CONTAINS／IMPORTS 和过滤 | 一次调用处理所有种子，共用预算。补充包含种子关联、节点、逐引用关系、证据和完成度；已在普通结果中的正文只关联，不重复复制；不作二次召回或 Embedding |
| A5 降级与入口 | ContextOptions 接通 Augment，新增可选 graph_execution，复用已有 ExecutionOptions；ContextResult 增加可省略的 graph。CLI／MCP／daemon 同步接入 | 缺图 feature、未知参数和未支持关系前置拒绝；兼容图不可用时，仅在普通结果仍安全的条件下返回普通结果及降级原因；取消、总超时和文本失配明确失败 |
| A6 验证与记录 | 运行受影响包、真实双库及公开入口对照，更新使用说明和交付记录 | 完整小语料验证功能；整仓覆盖、资格核验成本和 G4 效果分别报告。G3.1、Off、授权、writer 借读与清理回归保持通过 |

读取和预算顺序：

1. 验证模式、关系和服务上限，创建一次请求上下文。Off 继续使用既有路径；缺省新增字段不改变旧 wire。
2. 沿用刷新与授权策略，取得普通检索所需的文本租约，并记录对应的图候选描述；读取资源的打开时间计入总期限。尚未通过完整资格核验时，不把候选描述当成图能力证明。
3. zg 执行既有召回和多查询检索与结果合并。在同一租约内保留已选命中的正文、片段元数据和身份，构造 RankedHits。
4. 图阶段开始后，使用剩余请求期限与 5 秒的较小值完成资格核验、映射、扩展和证据；所有种子共用节点／边／映射／输出预算，不逐查询组重置。普通检索仍使用原配额；原生资源全程由租约持有。
5. 合并返回数据，按普通结果优先组织输出。图不可用或图阶段预算耗尽时，检查 may_degrade 和文本安全后再降级；不能把文本版本失配当成普通图不可用。

writer 借读只有普通文本结果安全时才能沿用；缺少兼容联合视图时返回明确的图降级状态，不另开图查询、不伪造联合 ViewStamp。图状态建议区分 complete／partial／degraded；图未核验时 view 可缺省，并保留原因。可用图结果必须有真实 ViewStamp、种子关联和完整证据引用。

公开入口：CLI query 增加 `--graph-mode off|augment` 和互斥的 `--graph-request <JSON>`；JSON 使用 GraphQueryMode，可指定种子数、关系、方向、跳数和过滤。MCP search 接收同一 graph／graph_execution 类型，展示独立图补充段；rg 仍只允许 Off。G3.2 交付时将 daemon 协议升至 14，并声明 `augment.code.v1`；旧 daemon 明确拒绝，客户端不得删去 graph 重试。Constrain 由 G3.3 接通。

定向回归至少包括：多查询融合／非融合的普通结果等价、同节点多片段与多查询来源、跨定义容器、非连续映射、循环与重复引用、已返回正文去重、空普通结果、图不可用／失配／预算耗尽降级、文本失配失败、取消／总超时失败、writer 借读降级，以及 Rust／CLI／MCP／daemon 的同请求对照。

### 5.3 Constrain：先圈定完整范围

1. 在任何查询模型获取前借 Required 视图，唯一定位锚点；按 ScopeSpec 完整计算范围，核验必要来源与映射。
2. 范围默认包含选中节点的完整 span；只查自身正文时显式选择 own，按 span 扣除直接子节点区间。结构后代是否进入范围由 CONTAINS／起点设置决定，不在映射时擅自扩大。
3. 通过 TextMap 和已有片段元数据，将获准区间转换为可硬过滤的单元，并与用户 QueryFilter 取交集。必要映射缺失、范围超限或未完成直接失败。
4. 完整范围或与用户过滤的交集为空，直接返回明确空原因与范围依据，不获取模型、不发 Embedding 请求，也不触发无用的远程授权提示。现有 QueryAuthorization 探测也须遵守此顺序；不能在客户端预先按普通向量请求触发授权。授权与执行若分成两次请求，执行时重新选视图和核对范围，不复用失去租约的范围对象。成功定位但没有可检索正文与映射丢失分别诊断。
5. 非空时按实际路由取得模型并授权；所有查询组、显式 routes、prefer_symbol 补充路由和自适应加深都携带同一硬范围。沿用现有融合与多查询合并，结果不得与全库召回取并集。
6. 组装正文、outline、container／上下文时继续遵守范围；不能召回片段合法却展示范围外的整个实体或源码。结果复核是防错措施，不能代替底座硬过滤。

严格范围完整不代表近似向量检索穷举完整；二者在结果中分开表达。

本轮执行顺序与状态：

| 顺序 | 工作 | 状态 |
| --- | --- | --- |
| C1 | Required 视图、唯一锚点与 full／own 范围；同版本 TextMap 和真实片段 | 已实现；完整范围不可表示时明确失败 |
| C2 | 图范围与用户过滤取交集，模型与授权前判空 | 已实现；空范围零模型／Embedding／授权定向验证通过 |
| C3 | FragmentId 硬过滤接入 FTS、向量、prefer_symbol、分组与自适应召回 | 已实现；原生底座 TopK 前过滤及多路查询定向验证通过 |
| C4 | 正文边界复核、共享预算、取消、期限和租约 | 已完成；活动取消、其他读者与租约释放及原生回归通过 |
| C5 | Rust／CLI／MCP／daemon 与使用说明、交付记录 | 已完成；入口对照、默认／原生回归及静态检查通过 |

首版支持完整 safe fragment；逐片段显示须有连续源坐标。非连续映射保留区间列表，无法在现有输出类型中准确表示时返回 `scope_not_representable`。不改分段、不重嵌入、不放宽代码域 ready。daemon 保持协议 14，独立声明 `constrain.code.v1`；仅支持 Augment 的旧端不能执行 Constrain。

### 5.4 硬过滤的实现顺序

- EntityId 白名单只用于该实体的全部可检索内容均在范围内的情况。
- 其他情况扩展 StorageSearchFilter 的 FragmentId／document_id 限制。现有 zvec 片段已经保存 document_id；需用真实底座验证全文和向量两条路径都能在 TopK 前生效，不能仅根据字段存在宣称支持。
- 跨界片段不能直接放行，不能检索全库 TopK 后裁剪。也不能静默丢弃必须覆盖的跨界内容，却报告范围完整可检索。
- 首版只接收能由现有安全单元精确表示的范围。若需要拆分跨界片段，交由显式文本重建／分段任务处理；查询时返回 `scope_not_representable`，不临时重嵌入或退回整文件。能力应随响应／Schema 说明该限制。
- 白名单大小、查询表达式字节和批次数均受预算约束；超过上限明确失败。首版不为大集合另起临时全库向量索引。分批方案只有在能保持过滤及排名语义后才启用。

## 6. 结果、状态和错误

ContextResult 新增可省略的 graph；KG 返回 AugmentResult／ScopeResolution，由 zg 转换为 GraphSupplement。Constrain 的 items／group_results 只含范围内命中，graph 保存范围依据；Off 不增加图执行内容。

| graph 内容 | 语义 |
| --- | --- |
| view | ViewStamp 或本次图不可用的原因；不暴露原生路径／句柄 |
| seed_matches | 普通命中到节点的映射、查询来源及无法映射原因 |
| supplements | 新片段或普通结果引用，附种子、方向、跳数和补充原因 |
| relations / evidence | 业务关系、逐引用位置及已核验来源；不重复持久化证据记录 |
| scope | Constrain 的锚点、声明范围、可检索范围与映射状态；大集合只返回摘要和计数 |
| execution | complete／partial／degraded、停止原因、节点／边／候选计数、输出字节、阶段耗时 |

执行完成度、解析覆盖、证据核验及索引新鲜度分别报告。普通图查询成功但存在未解析调用，不等于执行 partial；声明范围已完成但解析覆盖不全，应单独标记覆盖原因。已建图、已选中、查询能力支持与源文件新鲜度也不合并为一个布尔值。

| 情况 | Augment | explore／Constrain |
| --- | --- | --- |
| 图未配置、未就绪、格式不兼容或图阶段预算用尽 | 文本安全时返回普通结果＋degraded 原因；已核验的部分补充可带 partial | 明确错误；不返回成功空范围或全库结果 |
| 正常数量／输出截断 | partial，说明原因 | explore 可 partial；Constrain 范围计算不接受 partial |
| 图阶段子期限到期，总期限未到 | 文本安全时可降级 | 错误 |
| 请求取消／总截止时间到期 | 整个请求失败 | 整个请求失败 |
| 视图错配／必要证据失效 | 不消费错配数据；文本安全时丢弃图补充并说明原因 | 不将错配范围或未核验路径作为有效结果 |
| 文本不安全／恢复结果未知 | 失败，不以图可选为由返回成功 | 失败 |

错误保留类别、阶段、原始原因和可恢复提示：参数、歧义／定位、版本、能力、资源、截止时间、取消、冲突及恢复分别表达。诊断不输出凭据或无关原文。首版不持久化查询缓存；以后缓存键须包括完整视图及范围／过滤规则，不能跨版本复用白名单。

## 7. Rust API、CLI、MCP 与 daemon

以 Rust 类型和语义为单一来源，CLI／MCP 做转换与展示，不各自实现关系推断。

| 入口 | 拟新增内容 |
| --- | --- |
| Rust API | `ZvecGrep::explore`；ContextOptions.graph、ContextResult.graph；类型在默认构建也可解析，缺图后端时返回 unsupported |
| CLI | `zg explore` 下提供 locate／callers／callees／neighbors／expand／path／references；现有 `zg query` 增加 off／augment／constrain 和结构化 scope 参数，复杂输入支持 JSON 文件 |
| MCP | 搜索工具增加 graph 参数，新增 `zvec_grep_explore`；Agent／Full toolset 都能按构建能力发现探索能力；rg 工具保持 Off |
| daemon | 新增 Explore 命令／结果，Context 和 QueryAuthorization 传递图约束；以剩余时长传递总期限，转发取消和工作区准入，不另开图任务队列 |
| Info／能力 | 返回分域就绪原因、实际支持的操作／关系／语言、严格过滤粒度、当前兼容格式及检查时间 |

能力随 daemon /healthz、工具 Schema 和实际响应返回，不要求每次调用先发 info。能力声明只说明支持什么，不证明当前工作区已 ready。轻量 Info 不能只凭 HEAD 文件存在就宣称视图已核验；未做必要检查时报告 unknown／not_checked，Required 查询仍自行核验。

当前 daemon 协议版本是 13；explore 在 /healthz 校验协议与 explore.code.v1 能力，执行端要求版本请求头。旧服务不能识别图约束时明确拒绝，客户端不得删除 graph 重发普通请求。默认 Off 的旧请求保持兼容语义。

G3 是 Rust 新增能力，不要求给待退役的 TS 实现补图接口；保留 TS 既有工具契约，扩展 Rust 兼容夹具并记录新能力差异。不能为了保持“Agent toolset 仅一个工具”的旧断言而隐藏 explore，也不能让旧端声称已执行严格范围。

## 8. 实施顺序与交付条件

| 阶段 | 主要修改 | 可以交付的结果 | 依赖 |
| --- | --- | --- | --- |
| G3.0 契约与读取边界 | 纯类型、参数校验、预算、Provider 边界、版本戳；确认 G2 交接；保留命中 ID 的内部结构 | 默认 Off 等价；未知能力／跨版输入明确拒绝；接口不依赖 engine 的 KG 测试 | 可与 G2 并行，真实图门等待兼容产物 |
| G2→G3 交接 | 按 2.4—2.5 细化契约、修正覆盖、分析构建耗时，再交付业务图／TextMap 和按需读取 | 可发布重开的兼容产物与有界读取接口；前两步已完成，后续交接纳入 G3.1 | 复用 G2 生命周期，不提前做性能重构 |
| G3.1 代码 explore | 有界业务图读取、定位、入／出向引用、扩展、最短路径及引用检查；源码证据 | Rust API、CLI、MCP／daemon 贯通 caller/callee 与探索；不加载模型 | G2 发布视图＋目标业务模型／映射 |
| G3.2 代码 Augment | 多查询合并后调用 augment；独立图补充、来源关联及降级 | 三入口均可图补充，普通结果顺序／配额不变 | G3.1；无需等严格范围可表示性全部解决 |
| G3.3 代码 Constrain | 模型前范围计算、片段硬过滤、全部召回路由及输出范围检查 | 可精确表示范围内的检索；空范围不加载模型；不支持的细范围明确失败 | TextMap、实际片段过滤能力及 G3.0 资源契约 |
| G3.4 文档图接入 | Markdown file／section、CONTAINS／LINKS、重复锚点与反向引用；复用前三种查询方式 | 文档 explore／Augment／Constrain；与 engine 分段、位置映射一致 | Markdown 整合 P4 的跨侧验证及对应图构建支持 |
| G3.5 收口与 G4 接线 | 状态和错误对齐、兼容回归、使用说明及评测输出 | 可复现请求、分域结果、访问量／证据／耗时记录 | 对应阶段的真实底座验证 |

每阶段同步交付可用的 Rust API／CLI／MCP 切片，不等所有功能完成才暴露第一版 explore。代码图和文档图分别报告完成度。对应 S2 顺序：11 月代码 explore、12 月代码 query、1 月文档图；具体日期由 G2 交接和实施结果更新，不把本地实验通过数作为排期保证。2 月文档代码关联不提前混入 G3.1。

构建 Schema、TextMap 的写入改动先与 G2 维护任务明确文件边界，再落地；G3.0 先完成不依赖该改动的工作。源码接入时重新核对工作区，不覆盖其他任务的未提交修改。

## 9. 代码入口与拟改文件

以下“新增”均是计划路径，不表示当前已存在。

| 位置 | 改动 |
| --- | --- |
| `zg-knowledge/src/query/`（新增） | 中立输入／输出、视图与预算、explore／augment／scope 语义；不增加独立 SDK 生命周期 |
| [zg-knowledge/src/graph/](../rust/crates/zg-knowledge/src/graph/) | 内部按需查询及 graphdb 参数化适配；读新业务模型，不暴露原生句柄 |
| `zg-engine/src/api/explore.rs`、`api/context/graph.rs`（新增） | 公共请求／结果、默认关闭与参数校验；从 engine 类型显式转换到 KG |
| [lib.rs](../rust/crates/zg-engine/src/lib.rs)、[service.rs](../rust/crates/zg-engine/src/service.rs) | 公开入口、服务关闭状态、刷新／准入、取消与结果映射 |
| [indexed_search/service.rs](../rust/crates/zg-engine/src/pipelines/indexed_search/service.rs) | 先视图／范围、后模型，保持普通请求和 writer 语义 |
| [indexed_search/context.rs](../rust/crates/zg-engine/src/pipelines/indexed_search/context.rs) | 保留内部命中身份和来源，合并后增强，约束结果组织 |
| [indexed_search/pipeline.rs](../rust/crates/zg-engine/src/pipelines/indexed_search/pipeline.rs)、[storage.rs](../rust/crates/zg-engine/src/pipelines/indexed_search/storage.rs) | 单计划携带范围；所有路由和自适应加深取交集；不改现有路由排名公式 |
| [read_session.rs](../rust/crates/zg-engine/src/storage/read_session.rs)及[图会话](../rust/crates/zg-engine/src/storage/read_session/graph.rs) | Provider 适配与租约复用；公共查询不走全图 read_facts |
| [storage/types.rs](../rust/crates/zg-engine/src/storage/types.rs)、[fragments/mod.rs](../rust/crates/zg-engine/src/storage/fragments/mod.rs) | 片段硬范围、空集合语义、过滤表达式预算及真实底座验证 |
| [api/info.rs](../rust/crates/zg-engine/src/api/info.rs) | 能力、覆盖和经核验的就绪状态，避免一次 info 隐式加载全图 |
| [zg-cli](../rust/crates/zg-cli/src/)、[MCP](../rust/crates/zg-transport-mcp/src/)、[daemon 协议](../rust/crates/zg-daemon-protocol/src/lib.rs)、[daemon](../rust/crates/zg-daemon/src/)、[可执行入口](../rust/crates/zg/src/main.rs) | 命令、Schema、协议能力、执行分发及一致渲染；feature 沿可执行依赖链显式传递 |

## 10. 验证安排

G3.1 按阶段实施并运行相关测试；真实底座查询、并发和取消验证不能用纯类型替身代替。

| 验证项 | 需要证明的行为 |
| --- | --- |
| 普通检索 | Off／缺省结果、排序、配额与原行为一致；不打开图；rg＋图参数提前拒绝 |
| 业务模型 | 两次调用同一函数保留两条引用证据；辅助节点不入路径；跨版 ID 拒绝；真实新格式候选重开可查 |
| 定位与探索 | 同名多义不自动选；入／出向正确；自递归／循环有界；两跳准确；未完成路径搜索不报无路径 |
| 图增强 | 多查询合并后仅一次增强；来源关联保留；不新增检索／Embedding 调用；base items 与 group_results 不变 |
| 严格范围 | FTS／向量／prefer_symbol／所有查询组均受 TopK 前硬过滤；范围外高分干扰项不能进入；全库匹配但范围内无匹配返回空 |
| 粒度与内容 | 容器跨界片段、非连续 TextMap、嵌套章节均验证；无法精确表示则拒绝；正文／outline 不越界；范围不能因整文件放行被扩大 |
| 空与错误 | 完整空范围无模型获取／授权／Embedding；未完成／超限／缺映射不伪装为空；图可降级与文本不安全严格区分 |
| 视图与来源 | 查询期间 HEAD 切换、原地更新、来源修改／删除、旧 NodeRef／Scope 混入；不返回旧偏移的有效证据，不提前释放租约 |
| 预算与取消 | 多查询累计限额；图子期限和总期限区别；worker 退出后释放资源；单请求取消不关闭其他读者 |
| 三入口 | Rust／CLI／MCP 同请求语义一致；daemon 取消和版本拒绝；旧端不能去掉 Constrain 约束重试；默认构建无需 graphdb |

小型功能夹具用于确定性正确性；G4 继续复用现有 benchmark 及其固定问题，不在 G3 另建平行评测项目。向 G4 输出模式、ViewStamp、命中／关系／证据 ID、完成度、停止原因、访问量及阶段耗时；图可用不等于查询收益已证明。

检查入口沿用 [Rust 开发说明](../rust/README.md) 和 [check.sh](../rust/scripts/check.sh)。默认构建与显式 `knowledge-graph-graphdb` 分开验证，覆盖 engine、KG、storage 的原生路径及 CLI／MCP／daemon 协议；按当前源码记录实际 target 和结果，不套用历史通过数。原生提交中断等 G2 残留继续归 G2 报告，不因查询回归通过而宣布解决。

## 11. G3.1 本轮执行记录

范围：代码域 explore，Rust／CLI／MCP／daemon 语义一致。explore 只读已发布视图，不提供刷新或模型参数，不启动构建或恢复。Augment、Constrain 和文档图查询后续推进。保持 G2 候选导入、发布、恢复和清理协议，不做性能重构、不放宽代码域 ready 判定。

| 顺序 | 工作 | 状态 |
| --- | --- | --- |
| 1 | 固定源码和原生依赖基线、更新现有计划 | 已完成；见 reports/g3.1/baseline.json |
| 2 | 业务实体具体种类、直接关系、互斥 UnresolvedRef；构建辅助数据私有 | 已完成；模型、映射、原生查询与维护回归已通过 |
| 3 | 真实提取 TextMap、实际实体／片段核对、v2 候选发布重开 | 已完成；模型、映射、原生查询与维护回归已通过 |
| 4 | 发布资格分页核验、按需业务读取、Provider、同版本证据和取消 | 已完成；模型、映射、原生查询与维护回归已通过 |
| 5 | 定位与 caller/callee 贯通，再补扩展、路径和引用 | 已完成；模型、映射、原生查询与维护回归已通过 |
| 6 | CLI／MCP 图构建配置和 explore、协议 13、默认 feature、回归和交付记录 | 已完成；默认／graphdb feature 全工作区回归、Clippy 与入口对照均通过 |

沿用 G3.0 默认期限与预算。节点去重但不合并引用边；名称多义不遍历；Expand 按最短距离筛 min_hops；ShortestPath 在跳数限制内返回稳定的一条简单路径；boundary 取交集，result_filter 不作路径准入。未完成定位／路径搜索、取消／超时和必要证据失效明确报告，不能伪装为空。

固定支持语料验证功能；整仓覆盖、构建性能和 G4 效果分别记录。每阶段失败先修复，再推进后续阶段。当前轮不修改语雀、不提交或推送。

## 12. 代码域覆盖补齐（2026-10-29）

本轮先处理绑定与通配导入，保持 G3.0—G3.3 的请求、版本、证据、预算、取消和只读契约。继续使用现有 Tree-sitter grammar、宿主来源清单和显式模块根；不缩小必需来源分母，不放宽 ready，不做性能重构、文档图或 G4 效果评测。P0、P1 记录保留；本轮 P2 的发布复核、引用统计及生命周期 impl 范围见本节末尾。

| 顺序 | 优先级与支持范围 | 验证与交付 | 状态 |
| --- | --- | --- | --- |
| C0 | P0：冻结当前源码、已有改动、原生版本、宿主清单与模块根；按实际语法细分诊断 | 同一冻结输入做改前／改后对照，保留历史 260 个路径；来源处理、引用解析、正文映射分开报告 | 已冻结；见 reports/code-coverage/baseline.json 与 before.json |
| C1 | P0：构建私有作用域／绑定表示；记录名称路由、导入优先级与未知绑定影响 | 遮蔽与正例同时验证；辅助信息落盘后能重新解析，不新增公共业务节点／关系 | 已完成；最终语义与原生回归通过 |
| C2 | P0：Rust match 分支与守卫、if/while let 和条件链、let-else、for 与解构模式；区分变量、常量、构造器及字段名 | 初始表达式用原作用域；成功分支、失败分支及相邻分支不泄漏绑定；逐引用核对目标与位置 | 已完成；最终语义与原生回归通过 |
| C3 | P0：优先 Rust super::*，再接仓库内限定／分组通配、别名、重导出与枚举成员；尊重可见性与显式绑定优先级 | 冲突、循环、未知外部导出不伪造唯一目标；无法完整表示绑定影响仍为 incomplete；一处语法导入只保留一份引用 | 已完成；最终语义与原生回归通过 |
| C4 | P0：Python with/async with、except/except*、推导式／生成器与海象表达式、global/nonlocal/del、注解赋值和 match/case 捕获 | 推导式首个迭代表达式在外层；global/nonlocal 路由、异常变量清理及默认参数作用域准确；不执行用户代码 | 已完成；最终语义与原生回归通过 |
| C5 | P0：规则／辅助格式兼容、候选发布重开、目标与导出增删后的重解析 | 保留已解析边／UnresolvedRef 互斥；graph-only 核对真实正文／片段，不获取模型、不写向量；旧规则拒绝复用，显式重建 | 已完成；辅助格式 v3，混合小语料 graph-only 发布／重开／导出增删通过 |
| C6 | P0：真实覆盖与公开查询回归 | 默认／graphdb feature 受影响包、串行原生用例、Rust/CLI/MCP/daemon 对照、Clippy、格式及 diff；逐文件报告残留 | 已完成；完整回归及最终补充改动的定向回归通过 |
| C7 | P1：唯一类型／impl 上下文下 Type::f、Self::f 和明确别名；不推断接收者、trait 派发或泛型约束 | 同名自由函数、多 impl、歧义、目标删除；另行推进，不计入当前绑定与导入交付 | 已完成；见本节 P1 记录 |

完整性指结构、引用与绑定影响均已处理，不等于所有引用都可静态解析。动态、外部和歧义目标保留独立状态；尚未处理的绑定结构仍阻止文件完整和代码域 ready，不能仅删诊断或把所有引用改成 Dynamic。

新增支持用最小标注语料逐引用验证 owner、目标、UTF-8 区间、状态和原因；Python ast/symtable 辅助核对绑定，Rust 合法小语料与语言作用域规则交叉检查。每阶段先跑相关回归，再扫描冻结输入。只有全部宿主必需来源、实际文本绑定及既有准入条件均通过，才能报告整仓 ready。

执行基线和证据保存在 [reports/code-coverage](archive/pre-g4-validation/README.md#code-coverage "包内：knowledge/reports/code-coverage")。默认 Off、explore 零模型零写入、Augment 原结果不变、Constrain 模型前范围计算与 TopK 前硬过滤均作为回归约束。当前轮不改语雀、不提交或推送。

本轮 P0 已完成。固定 396 个候选来源在同一份冻结正文上由 113 个抽取完整提升到 396 个；加入本轮新增源码后的清单为 401／401，历史 260 个缺口均已恢复抽取完整。结果见 [覆盖对照](archive/pre-g4-validation/README.md#code-coverage "包内：knowledge/reports/code-coverage/coverage-summary.json")。清单包含 1 个隐藏路径，统计口径仍是候选事实抽取，不代替宿主实际选中来源、真实文本绑定和发布视图核验，不据此宣称整仓 ready 或调用图完整。

Rust 已补模式与条件作用域、普通限定导入的完整路径、通配绑定、可见性／显式名称优先级和重导出；Python 已补声明路由、上下文／异常绑定、推导式、模式捕获和默认参数作用域。点号导入结合显式导入引用处理无 __init__.py 的包，保留根名称重绑定的拒绝逻辑。辅助 Schema／事实格式升为 v3，规则指纹同步更新；旧候选由显式构建替换，不在查询中迁移或删除。

ready 判定源码与基线一致。新增的真实双库用例先建立文本索引，再用会拒绝模型获取的 manager 执行 graph-only，核对文本／向量文件摘要不变；关闭重开后验证 Rust 通配、Python global／推导式／命名空间包，以及导出删除与恢复。仍不支持的宏模式用于保留 explore 拒绝和 Augment 降级的负例。

图 feature 全工作区与默认 engine／KG 全目标回归通过；最后补充 Python 点号导入后，复跑 30 项原生图构建／G3 用例、KG 原生全目标、38 项语义／绑定检查、Rust／CLI／MCP／daemon 同请求对照及默认／图 Clippy、格式检查。命令、版本、源码摘要及失败过程见 [验证记录](archive/pre-g4-validation/README.md#code-coverage "包内：knowledge/reports/code-coverage/verification.json")。Rust 辅助夹具只计编译验证，独立运行被 SIGKILL，不计为通过证据；Python 自建夹具的符号表及执行检查通过，均未执行用户代码。

P0 完成时的保留项：impl 显式关联调用交给下述 P1；Python 通配导入、新式类型／注解作用域和宏模式绑定明确不支持，类型推断与宏展开未增加。构建性能、完整资格核验成本、G2 故障窗口及 G4 效果评测不在本轮结论中。未修改语雀，未提交或推送。

### P1：固有 impl 显式关联调用

支持同一命名空间内、非泛型且无 where 条件的固有 impl：`Type::f()`、impl 内 `Self::f()`，以及 `use Type as Alias`、无泛型的路径型 `type Alias = Type`（含明确的别名链）。类型与方法均须唯一，跨文件沿用模块根和可见性。显式 `Type::method(receiver)` 可定位声明，不推断接收者。泛型／条件 impl、trait 派发、限定类型语法、`obj.method()`、`self.method()` 继续保留未解析诊断；同名候选、未知归属或循环别名不得被猜成唯一目标。

| 顺序 | 内容 | 状态 |
| --- | --- | --- |
| P1.0 | 冻结 HEAD、已有源码摘要和 ready 实现；确定支持范围及兼容策略 | 已完成；reports/impl-calls/baseline.json |
| P1.1 | 同次遍历记录私有 impl 归属和类型别名，保留直接词法包含；解析显式类型／Self／别名到唯一方法 | 已完成；12 项正反语义测试通过 |
| P1.2 | 升级辅助格式和规则指纹；落盘重开后用同一解析器恢复归属，目标增删／歧义变化重解析 | 已完成；真实双库发布、重开、增删／歧义转换通过 |
| P1.3 | 正反语料、发布重开、graph-only 零模型／零向量写入及 G3.0—G3.3 回归，默认／graphdb Clippy 与格式检查 | 已完成；默认／图后端全目标回归、跨入口对照及静态检查通过 |

实现复用私有作用域／绑定，不新增 Impl 业务节点或关系，不调整公共 API／协议。辅助格式 v4；旧候选须显式重建，不在读取时迁移。旧验证记录不覆盖本轮改动；最终命令、源码摘要和未覆盖项单独保存在 reports/impl-calls。ready 源码和原有负例保持，暂不做性能重构、整仓发布或效果收益推断。

P1 已完成。类型归属、Self 和路径型别名保存在构建私有作用域／绑定中，方法保留原声明位置与词法包含；不新增业务节点或重复引用。限定名称先合并类型候选，再查方法，避免两个同名类型中只有一个定义方法时误报唯一。泛型参数仍遮蔽同名模块类型；trait 派发、条件 impl 和未知归属保留诊断。

12 项新增语义检查覆盖多 impl、跨文件与别名链、可见性、重复调用、自调用、闭包内 Self、循环别名、泛型遮蔽和取消／预算。真实发布用例核对 graph-only 不获取模型、不改写文本／向量；关闭重开后验证调用边，以及目标删除、变为歧义和恢复。Rust／CLI／daemon／MCP 对同一 Type、Self 和别名调用返回一致，查询无模型请求。

图后端 engine／KG 全目标 669 项、默认构建 521 项通过，各有 10 项既有忽略；最终语义回归 50 项通过，默认／图后端 Clippy、格式与 diff 检查通过。编译器辅助语料以常量求值核对关联调用，未执行程序。记录见 [P1 验证](archive/pre-g4-validation/README.md#impl-calls "包内：knowledge/reports/impl-calls/verification.json")。当前候选来源抽取为 404／404 完整，仍不代替宿主清单、实际文本绑定、整仓发布或调用关系完整性验证；未做性能重构或效果评测。

### P2：真实仓库发布复核与生命周期 impl

先用真实仓库的冻结副本、宿主实际扫描和显式模块根复核 v4 发布，不以候选抽取扫描代替 ready。正文由既有本地模型建立；模型文件复制到隔离缓存，禁止远程 Embedding。graph-only 核对向量／正文摘要及零模型事件，关闭重开后通过公开 explore 验证版本、正文绑定和代码域准入。按所选产物的逐引用记录分别统计 CALLS 的状态、未解析原因与位置，不把声明诊断数当成缺失调用数。

| 顺序 | 内容 | 状态 |
| --- | --- | --- |
| R0 | 冻结 HEAD、已有源码、原生依赖和 ready 判定，更新本节 | 已完成；reports/code-readiness/baseline.json |
| R1 | 实际宿主来源、模块根、文本绑定、v4 提交／选择／重开和公开查询复核；形成逐来源就绪及引用解析基线 | 复核完成；发布 403／403 ready，默认公开查询资格核验超时，见下文 |
| R2 | 仅补生命周期参数的固有 impl；支持类型／方法唯一的 Type、Self 和既有明确别名；类型／const 泛型、生命周期约束、where 和 trait 派发继续拒绝 | 已完成；4 项新增正反语义测试及同一冻结语料重建通过 |
| R3 | 同一冻结输入做改前／改后引用对照，新增正反回归及真实发布重开；默认／图后端检查，更新本计划结果 | 已完成；语义、真实发布重开、默认／图后端回归和入口对照通过；整仓查询超时保留 |

Rust 主源码以 src 为模块根，辅助测试／示例目录另设命名空间；配置显式记录，不改生产模块发现规则。缺映射或未就绪逐项报告，不排除必需来源、不放宽 ready、不增加查询时修复。R1 若遇既有资源上限或处理失败，保留失败证据与原因；修正配置问题后重试，不通过提高上限掩盖问题。

生命周期支持只省略不参与本轮方法选择的无约束生命周期参数，不省略类型参数或条件；保留多 impl 歧义、可见性、别名循环、取消与预算。G3.0—G3.3 的只读、版本、证据、过滤、排序和降级契约不变；候选构建／发布／恢复与租约机制不变。暂不做接收者类型推断、性能重构或 G4 效果评测，不更新语雀、不提交或推送。

R1 结果：冻结副本共 404 个 Rust/Python 候选，宿主默认选择 403 个，均发布为代码域 ready；未选的 .github/scripts/readme-demo.py 保留在对照清单。文本索引成功处理 1,145 个文件，graph-only 零模型／零下载、向量与正文摘要未变；新进程重开并核对原生提交证明后导出 57,446 处 CALLS：8,475 已解析、11 歧义、7,907 外部、41,053 未解析。明细见 reports/code-readiness/baseline-references.json，分类和逐来源状态见 baseline-summary.json。

R1 发现独立遗留：公开 explore 对整仓图的资格核验在默认期限内失败，Debug／Release 均返回 ZG.ENGINE.DEADLINE_EXCEEDED；首次复制的 Debug 可执行文件曾被 SIGKILL，原因未确定，失败记录保留。发布 ready 与默认预算下公开查询可用分别报告。本轮不调整预算、不跳过完整性／正文核验，也不以此声称读取视图验证通过；资格核验成本留给后续性能工作，不能据发布结果宣称整仓默认查询可用。

R2 结果：支持无约束的命名生命周期及匿名生命周期 `'_`；`'static` 特化、同一生命周期重复绑定形成的等值条件、生命周期 bounds／where、类型／const 泛型和 trait impl 仍不参与静态方法选择。保留多 impl 歧义、可见性和目标增删重解析。不新增业务节点、关系或持久化字段；Schema／事实格式仍为 v4，规则指纹升为 v4，旧规则候选须显式重建。

同一冻结输入重建后仍为 403／403 发布 ready，63,421 处引用的 ID、来源和位置不变。CALLS 已解析数从 8,475 增至 8,493，未解析从 41,053 降至 41,035；歧义 11、外部 7,907 不变。12 处生命周期 impl 的限制解除，6 个来源新增解析 18 处调用；原有已解析引用的目标和状态均未变化。graph-only 零模型／零下载，向量与正文摘要不变；新进程核对提交证明及事实重开通过。明细见 [逐引用对照](archive/pre-g4-validation/README.md#code-readiness "包内：knowledge/reports/code-readiness/reference-comparison.json")。

最新工作区另做 405 个候选来源的抽取扫描，全部完整；该扫描包含新增的离线审计工具，仅证明抽取覆盖，不代替上述冻结语料的宿主选择、发布或公开查询结果。

R3 结果：图后端 engine／KG 全目标 674 项、默认构建 521 项通过，各有 10 项既有忽略；最终定向语义回归 54 项通过。Rust／CLI／daemon／MCP 对生命周期 Type／Self 调用返回一致，既有 P1、Augment、Constrain 与空范围行为保持。真实发布用例覆盖关闭重开、目标删除、歧义及恢复，graph-only 与 explore 不获取模型、不写向量。默认／图后端 Clippy、格式与 diff 检查通过，Rust 常量求值辅助夹具编译通过，未执行程序。命令、源码与依赖摘要、逐引用统计及保留失败见 [P2 验证记录](archive/pre-g4-validation/README.md#code-readiness "包内：knowledge/reports/code-readiness/verification.json")。

P2 请求范围已完成。整仓发布 ready 与静态解析改善已验证；默认预算下整仓公开查询仍因资格核验超时而不可用，后续单独处理。本轮未放宽 ready、预算或完整性检查，未增加接收者类型推断或性能重构。

### P3：整仓 explore 资格核验

目标：当前冻结仓库在新进程、无核验成功缓存、默认预算下完成公开 explore。保留全量图内容、Root／Receipt、宿主来源、实体／正文／片段／TextMap 及首尾选择状态核对；G3.0—G3.3、ready、期限与结果预算不变。继续查询 v4 候选，不在读取时迁移、重建或加载模型。

| 顺序 | 内容 | 状态 |
| --- | --- | --- |
| Q0 | 固定源码／原生基线，补齐超时路径可用的分段耗时和实际扫描量 | 已完成；失败与成功均保留 ZG_GRAPH_PROFILE 记录 |
| Q1 | graphdb 提供受控只读扫描，KG 一次扫描、每批最多 256 行；保留规范顺序、摘要、重复边和资源上限 | 已完成；原生游标 7 项、原生库及 ABI 52 项通过，v4 候选兼容 |
| Q2 | 按测量减少来源查找与文本核验重复访问，保留真实正文和片段核对 | 已改来源索引并补访问量；文本核验首测约 0.27 秒，暂不重组正文批处理 |
| Q3 | 仅在仍有必要时优化规范序列化分配；新旧摘要须逐字节一致 | 已实现缓冲复用及借用标量编码；v4 字节／摘要对照通过，Debug 仍超时 |
| Q4 | 新旧核验对照、篡改／缺失／多余数据、版本／取消／租约、整仓默认查询及 G3 回归 | Release 整仓及 G3 回归通过；Debug 整仓超时保留 |

已确认现有引用分页每页扫描 53,109 条；完整读取约 209 次扫描，边表还会重复排序及 SKIP。单事务、包含图打开的 5 秒只读诊断仅完成 105／208 个有数据页，尚未计算行摘要或核对正文。原始失败保留在 reports/code-readiness；该诊断不作为完整性能基准。

不以增加预算、减少检查范围或缓存 HEAD／ready 成功状态解决。原生连接、游标及事务留在受控同步任务内，取消后清理完成才释放租约。新扫描保留 v4 规范行序与摘要；如果必须增加物理索引，只由显式构建产生。当前轮不改语雀、不提交或推送。

Q1 首轮 Release 公开查询在原有 v4 产物上完成，进程耗时 4.489 秒；结果 complete，资格核验约 1.179 秒（图证明 0.905 秒、文本绑定 0.269 秒），业务阶段约 0.046 秒。图证明共 19 次扫描、245,791 行、970 个批次。该记录为单次诊断，不作为稳定性能基准。Debug 仍在旧的逐行 JSON 对象构造／规范序列化中耗尽期限，因此继续实施 Q3；失败保留为 scan-prototype-debug。

Q3 保留每行一个可复用规范字节缓冲区，直接校验并读取原生标量，不构造通用 JSON 对象或复制整行字符串。单个序列化器处理借用行的尝试没有改善 Debug 时间，已撤回，失败记录保留；不为 Debug 调整编译优化、期限或检查范围。Release 与 Debug 分别报告，最终结果以同一源码的公开复测为准。

P3 结果：同一已发布冻结语料（403 个代码来源、1,145 个宿主来源）保持原 v4 构建和文本版本。3 次新进程 Release explore 均 complete／ready，进程耗时 2.696～3.853 秒；图打开加资格核验 2.074～2.190 秒，其中完整图证明 0.404～0.429 秒、正文绑定 0.243～0.264 秒。未调预算、未使用核验成功缓存；这些是本机少量样本，不是冷盘或性能保证。关闭诊断再测为 2.658 秒，除耗时外结果相同。

每次完整图证明处理 19 张表、245,791 行、970 批，规范字节 108,293,531。排序只保留行定位和键，属性按批读取；本语料键状态峰值计数 3,034,936 字节，不代表进程 RSS。资格核验仍覆盖全图，成本随数据量增长；本轮消除重复全表扫描，不承诺与查询结果规模成正比。正文绑定仍逐来源核对实际实体／正文／片段及 TextMap，未重组批量接口。查询前后 120 个文本／向量文件、116 个图目录文件及工作区清单摘要不变。

默认 engine／KG 521 项、图后端 676 项通过，各有 10 项既有忽略；最后补充原生非法输入守卫后，原生 52 项、KG 24 项、跨 Rust／CLI／MCP／daemon 对照通过，扫描夹具调整后再复跑 7 项。默认／图后端 Clippy、格式、FFI、ABI 布局及显式原生目录拒绝检查通过。原生 Clippy 的唯一告警在未修改的 result.rs:876（manual_is_multiple_of），单独记录并对该既有规则作命令级例外；未修改无关实现。Release 构建保留既有 rust-objcopy 缺 libLLVM 的 strip 告警，构建成功。

graphdb 客户端 C ABI 升为 1.4，修改在本地子模块内；Schema／事实格式 v4、daemon 协议 14、规则和 ready 判定不变，不需重建该 v4 候选。基线、命令、分段统计、失败记录、最终源码与原生摘要见 [P3 验证记录](archive/pre-g4-validation/README.md#query-qualification "包内：knowledge/reports/query-qualification/verification.json")。

仍有遗留：最终 Debug 新进程在原 5 秒图预算耗尽时读取到第 882／970 批，未完成图证明和正文核验，明确返回 deadline_exceeded。原生打开及 Debug 下完整序列化／摘要成本尚在，不能据 Release 通过宣称 Debug 已解决。本轮按 Release 结果交付，Debug 保留独立后续项；未通过放宽期限、编译优化配置或完整性检查处理它。
