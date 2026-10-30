# G2.1 候选事实格式与适配边界

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-23。对应 `zg-knowledge::relations::bound` 与可选的 `zg-knowledge::graph`。这是已实现的候选事实格式；不表示联合发布、增量恢复或图查询已经交付。

## 1. 输入与绑定

正式索引的内部暂存位接收本次读取、解码后的正文和模型实际采用的 `ChunkOptions`。Rust/Python 的事实观察器与检索实体共用同一棵 Tree-sitter 树，随后由共享的 `extraction::bind_entities` 分配宿主 EntityId，再消费该次提取的临时 `entity_indices`。不在索引结束后重新解析并猜测对应关系。

`BoundFileFacts` 保存自有事实、宿主文件记录以及按节点/引用排列的实际 EntityId 集合；持久化内容中没有 `entity_indices`、AST、正文或向量。范围沿用宿主解码后的 UTF-8 `[start,end)`，原始摘要与解码摘要分别保存。UTF-8 BOM、CRLF 和 Python encoding cookie 的转换由宿主解码器负责。

事实先随 PreparedFile 暂存，文本 `replace_file` 成功才进入本次成功集合；checkpoint 后核对完整宿主目录，移除与最终状态、摘要、长度或实体数不符的暂存项。未变化文件不假装已经重新提取，G2.2 必须补齐或核验复用事实。尚未公开 `IndexOptions.graph`；当前只有内部接线与检查入口，不改变默认索引行为。

模块根采用显式相对路径、语言、namespace、前缀与布局。配置顺序不影响指纹，重复根/namespace、绝对或父路径、Markdown 模块根被拒绝；使用前核对真实目录不能越出 workspace。最长前缀优先。语言不匹配时保留普通索引并诊断，不能用配置把 Python 的语法树解释成 Rust。缺少映射的代码不生成唯一图关系。

可选事实处理失败或超出捕获预算时，保留正常检索实体并记录诊断；代码观察器失败不触发第二次 Tree-sitter 解析。取消、期限错误继续向上传递，准备阶段不会把旧成功文件改成 failed。此行为不替代 G2.3 的提交竞争和恢复协议。

## 2. 表、字段与格式

精确 DDL 由 [schema.rs](../rust/crates/zg-knowledge/src/graph/schema.rs) 生成，归档见 [schema.cypher](archive/pre-g4-validation/README.md#g2-1 "包内：knowledge/reports/g2.1/schema.cypher")。`SCHEMA_VERSION=1`、`FACTS_FORMAT_VERSION=1`，G1 事实契约保持 `zg-relations-v1`。Scope/Binding/Diagnostic 的 G1 类型仍没有新增业务 ID 字段。

| 表 | 主键与关键内容 |
| --- | --- |
| KgFile | path；FileId、索引状态、内容摘要、长度、text revision、generation、实体数/实体集合摘要；可空的事实语言/模块/规则/完整性。unsupported、failed 等目录成员可只有文件记录 |
| KgNode | id；文件、ordinal、类型、名称、限定名、父节点、位置及实际 EntityId 集合 |
| KgScope | 合成 id；文件、ordinal、kind、owner |
| KgBinding | 合成 id；文件、ordinal、scope ordinal、名字、node/import/dynamic 标签、可空目标属性和 visible_from |
| KgReference | id；文件、ordinal、owner、scope ordinal、kind、可空的解析状态/原因、位置、Lookup 与实际 EntityId 集合 |
| KgAnchor | 合成 id；文件、ordinal、slug、节点、重复 slug discriminator |
| KgDiagnostic | 合成 id；文件、ordinal、code、成对可空的 span |
| KgCatalogRevision | revision；domain/language/namespace、成员数量与摘要、配置与选择指纹 |

另有 **KgFactMetadata 元数据表**，记录候选的事实格式、Schema 指纹、配置/选择指纹和完整候选摘要；不计入八张事实表。它随全部事实在同一个事务中写入，不能替代 G2.2 的 GraphRoot、成功 Receipt 或宿主 HEAD。

FileId 使用 UINT64 保存并在读回时检查 u32 范围；长度、修订、ordinal、visible_from 和范围使用 UINT64，usize 转换显式检查，不静默缩窄。实际探针已验证所选 binding 的无符号类型。

结构化列统一使用版本化 canonical JSON：`{"value": ..., "version": 1}`。对象键排序，数组保持语义顺序，字符按 JSON 转义；不使用分隔符拼接。不维护第二份完整 facts payload。未知版本、缺字段、顺序缺口、错误目标、结构边缺失或重复均拒绝读为成功。

## 3. 身份、结构与依赖

```text
scope/binding/diagnostic/anchor id =
  stable_digest(FACT_CONTRACT_VERSION, normalized_file_path, kind, ordinal)
```

这是同一规则/输入下的构建内身份；不是跨修订实体追踪。更换检索分段参数不能改变结构、ordinal 或这些 ID。节点/引用/锚点也保存 ordinal，保证关闭重开后不会按数据库返回顺序还原数组。重复锚点保留各自记录，缺失 span 的 null 与 `[0,0)` 不混同。

13 张关系表保存包含、父节点、Scope parent/owner、Binding scope/target、Reference scope、Diagnostic 来源、锚点目标、解析/歧义目标及目录依赖。全部端点都是节点表。Scope parent 必须指向同文件更早的 scope；节点拒绝自身为 parent。标签、属性和边在读回时交叉核对。

已解析目标使用 KgResolved，歧义目标使用 KgCandidate；两者的 `expected_target_version` 绑定目标节点、SourceVersion、text revision 和实体集合摘要。负解析同样通过 KgExpectRevision 绑定目录修订，目录成员由 KgMemberOf 保存，不产生引用数 × 文件数的依赖边。

完整宿主目录参与修订摘要。文档目录包含 unsupported/failed 等成员，用来区分已索引的非 Markdown 目标与未索引目标；代码按 language/namespace 分组。当前保守地把整个宿主目录摘要也纳入修订，可能扩大失效范围，尚未优化。

G1 的公共 resolver 仍返回原来的读集；候选目录通过访问式内部入口逐条消费，避免同时保留 R×F 份来源版本。读回结构后重用同一纯规则 resolver，并重新编码核验全部存储属性和边，不另写 Cypher 名称解析规则。

## 4. 数据库生命周期

`GraphDbStore` 直接调用上游 binding。旧通用 GraphStore/Transaction/Value/Rows 端口已移除。每个实例持有一个 Database，读写操作独占各自 Connection；写准入只在该实例内串行。事务由内部同步 `with_read` / `with_write` 控制，宿主必须把完整调用放在有界 blocking 任务中，不能跨 await 持有事务。

新候选仅在不存在或空目录中初始化，DDL 在空闲连接逐项执行。事实写入后，通过新连接的显式 READ ONLY 事务确认全部行和边；关闭全部原生拥有者后再重开。重复写入已完成候选被拒绝，打开不迁移、不补表或修复；中断初始化、未知格式和错误结构不能变成可用事实。

边写入以 `MATCH (a {pk:$source}), (b {pk:$target})` 定位端点。所选上游的 Update MATCH 先扩展顶点，再执行 WHERE；把两端只写在 WHERE 中会先产生笛卡尔积并触发行预算。模式属性让原生先按主键约束端点候选，再组合两端；验收同时保留小预算回归和 L1 写入。

错误保留原生 kind/stage/outcome、事务/变更代码及 cleanup/cause，不携带原生消息或正文，不按错误字符串分类重试。Unknown 或不可用连接使实例停止后续操作；这是局部隔离边界，尚无宿主恢复核对。确认失败保留已经提交的候选效果，取消不能把它改成未发生。

当前 `read_facts` 是完整候选检查与结构复用接口，不是 G3 查询性能接口。没有 GraphRoot/Receipt、STATE/POLICY/HEAD、联合选择、graph-only 调度或公开 relations/search-ready 能力。相关验证见 [G2.1 验收记录](G2_1_ACCEPTANCE_REPORT.md)。
