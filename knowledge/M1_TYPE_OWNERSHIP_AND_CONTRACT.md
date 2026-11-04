# 类型归属与公共语义契约

本文保留 M1 确立的类型归属及公共契约设计。公共存储、来源维护和公开读取已在 M2／M3 完成，当前版本与能力见 [S2 验收矩阵](S2_ACCEPTANCE_MATRIX.md)。

## 现有类型的归属

| 归属 | 保留或收敛的定义 | 边界 |
| --- | --- | --- |
| KG 查询契约 | NodeRef、Locator、EntityKind、RelationKind、ViewStamp、GraphStamp、EvidenceRef、ByteSpan、预算、请求、结果、RankedHit、SeedExpansion | 复用 `zg-knowledge::query` 和 `relations` 的现有实现；engine 公开模块重导出同一 Rust 类型，默认值和结构校验只有一份 |
| engine 入口 | ExploreOptions、ContextOptions、GraphOptions、图开关及已编译能力 | 工作区、锁等待、取消、刷新及宿主支持范围由 engine 检查；有效的通用 KG 请求不自动获得宿主能力 |
| engine 检索域 | Entity／EntityFragment、EntityId／FragmentId、FileRecord、Range、CodeMetadata | 正文与片段仍归 engine；已有 TextMap／BoundFileFacts 负责绑定，不复制正文，不合并图节点身份与检索身份 |
| KG 构建输入与存储 | FactNode／FileFacts、BoundFileFacts、EntityRecord、native 行、Root／Receipt | 抽取事实、来源绑定、业务读取投影和物理存储保持各自职责；SourceNode 概念沿用这些现有结构，不新增同义节点结构 |
| engine 产品结果 | GraphSupplement、GraphEvidence、GraphSeed、GraphScope | 保留普通命中关联、正文复用及降级等产品组织；其节点、关系、视图、命中与扩展记录使用 KG 类型 |
| CLI／MCP／daemon | 原参数与协议包装 | 直接复用 engine 公开请求／结果，不再增加业务类型副本 |

`zg-knowledge` 成为 engine 的常规纯 Rust 依赖。原生 graphdb 仍由 `knowledge-graph-graphdb` feature 控制；`knowledge-graph` 继续控制宿主事实捕获／检查路径。没有新增 crate、存储接口或适配器注册层。

收敛删除 engine 的成套查询类型与逐字段转换，并移除 explore、Augment、Constrain 中为这些同构类型进行的 JSON 往返。原有五组公开 JSON schema 在修改前冻结，作为回归样本；结构、枚举、字段、默认值和 required 均需保持一致。

未发布 Rust API 的归属调整：`GraphHit`、`GraphSeedExpansion` 合并使用 `RankedHit`、`SeedExpansion`；GraphQueryMode 的宿主能力检查归 engine 的请求入口，`is_off` 和结构校验复用 KG；共享类型的校验错误为 KG QueryError，engine 入口统一转换为 EngineError。未增加旧名称兼容别名，JSON 请求／响应保持原形。

GraphSeed 的命中反序列化随共享 RankedHit 统一拒绝未知字段；此前 GraphHit 会忽略未知字段。协议 14 生成的字段不变，未扩大旧端与未来协议混用的承诺。

## 缺失的语义模型

新增 `zg-knowledge::semantic`，仅包含公共记录、结构化输入及纯校验／身份匹配。现有源码模型、正文位置、证据和标识检查继续复用。

| Rust 契约 | 目标字段与存储对应 |
| --- | --- |
| Identity | `space_id / id`；单列主键使用规范 JSON 二元组编码，分隔符及 Unicode 不产生键冲突 |
| ObjectId | 区分来源节点、语义实体、Episode、Fact；这是逻辑目标类型，查询版本仍由既有 NodeRef／读取视图约束 |
| SourceRef | 固定 `source_id / source_version`；语义来源 ID 可为事件等宿主标识，文件路径仍沿用原安全校验 |
| SemanticEntity | Identity、kind、name、aliases、attributes；映射固定 SemanticEntity 表，附加属性编码到 `attributes_json STRING` |
| Episode | Identity、kind、SourceRef、context、可选发生时间、接入时间、content_ref、active／withdrawn；对应固定 Episode 表及宿主正文引用 |
| Fact | Identity、subject、predicate、实体／标量 object、context、active／superseded／retracted、supersedes、变更时间和 attributes；实体事实映射 SEMANTIC_RELATION 边，标量历史放固定辅助记录，不新增 Fact 业务点 |
| SourceAssociation | 有类型的实体或 Fact 目标、固定版本 SourceEvidence、support／correction 用途；对应辅助来源关联，不参加业务遍历 |
| Mention | ref_id、来源节点 owner、语义目标及已有 EvidenceRef；版本来自所属 SourceChange，不为每条提及重复保存来源版本，映射 MENTIONS 边 |

时间字段在 Rust 契约中采用 UTC Unix 毫秒，落盘按原生时间字段转换。attributes 是 JSON 对象，保留数值、布尔、数组等内部值；对象身份、关系名、状态及更正独立存储，不允许附加属性覆盖公共字段。

SourceEvidence 复用 EvidenceRef 的区间、摘要及引用位置校验；它对照 SourceRef 校验来源一致性，适用于非文件来源。代码证据仍要求规范工作区路径，语义来源不借此扩大代码文件访问范围。原文存在性、版本与摘要实际匹配在宿主绑定和 M2 读取中核验。

### 身份和属性处理

EntityInput 表达新增或匹配输入。显式 ID 优先；没有 ID 时只在同身份空间、同 kind 内进行精确、区分大小写的 UTF-8 名称／别名匹配。返回 New、Unique、Ambiguous 或 Conflict，不生成猜测的目标，不自动跨空间合并。已有目录须经过身份唯一性及记录校验；New 的稳定 ID 分配与映射保存由 M2 构建完成。

EntityInput 的 attributes 缺省表示保留，显式空对象与缺省分别表达；显式 null、数组、字符串等被拒绝。与既有属性冲突时不深度合并，保留显式冲突结果。完整 Fact 记录和更正 replacement 要求提供 attributes 对象，不能通过省略属性静默清空已有值；更正后的历史保存由 M2 执行。

### 来源替换和事实更正

SemanticInput 明确一个身份空间、update_scope、来源变更、Episode 与 FactChange。输入校验拒绝范围外来源、重复来源／记录、跨空间端点及同目标竞争更正。

SourceChange 区分 Upsert 和 Delete。Upsert 的 semantic 缺省、Complete、Partial、Failed 分别表达；只有 Complete（包括明确的完整空集）或显式 Delete 才声明替换贡献。Partial／Failed 不冒充空结果或删除，正文换版后的就绪判定留在 M2。

FactChange 明确替代／撤回目标、预期状态、上下文与更正证据。替代要求新的 active Fact 指向准确前序；check_current 对照已经核验的当前记录，状态、目标或上下文失配返回明确的 Conflict。已替代／撤回事实不能按 active 前提重放激活。更正链完整性、多来源去重、撤销后可见性和原子写入由 M2 验收，M1 不执行持久化变更。


## 当前版本

查询契约由 zg-knowledge 定义、engine 重导出；Schema／事实格式为 v7，协议 15。抽取指纹改变时旧图须显式重建。必要更正与退出历史不等同于多版本图，来源删除不会使已替代事实自动复活。
