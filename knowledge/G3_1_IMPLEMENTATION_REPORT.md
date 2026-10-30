# G3.1：业务图、TextMap 与代码 explore

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-28。基于 HEAD `b5ea18d8f031d7ccae992221b10d5cab242e4e17` 和实施前已有的 G2／G3.0 未提交工作；基线见 [baseline.json](archive/pre-g4-validation/README.md#g3-1 "包内：knowledge/reports/g3.1/baseline.json")。本轮未改语雀、未提交或推送。与基线相比修改 55 个既有文件，其他 1,182 个文件字节未变，无文件缺失；完整范围见 [change-scope.json](archive/pre-g4-validation/README.md#g3-1 "包内：knowledge/reports/g3.1/change-scope.json")。

后续 review 的问题、修复和定向验证见 [review 记录](G3_1_REVIEW_REPORT.md)。本文的全量通过数保留为首轮交付基线。

## 交付

| 部分 | 实现 |
| --- | --- |
| 业务图 v2 | 实体增加具体种类、限定名和 UTF-8 位置；CONTAINS 只连直接父子；每个已解析引用保存在一条 CALLS／IMPORTS／LINKS 边上，其他引用保存在 UnresolvedRef。作用域、绑定、锚点、目录供构建与恢复使用 |
| TextMap | 正文提取时核对源字节，分配 EntityId 后绑定；映射按来源共享。graph-only 核对实际正文、来源区间和片段 ID／区间，不能仅凭 ID 集合复用向量 |
| 格式与发布 | Schema／事实格式 v2，模型和映射规则进入指纹；沿用候选、分批事务、根／回执、重开确认和 HEAD 选择。回执绑定根元数据，根和元数据绑定逐行摘要。旧候选需显式重建 |
| 只读视图 | 复用 ReadSessionLease 和读写准入；核对选择状态、文本版本、根／回执、代码域和实际文本绑定。资格核验逐页扫描，业务读取不构造 FactCatalog、不调用 read_facts |
| explore | Locate、Neighbors、Expand、ShortestPath、References；caller／callee 是 CALLS 入／出向。分页上限 256，共用请求预算。名字多义不选锚点；边保留每次引用，节点去重 |
| 证据与资源 | 优先复用同视图正文；回源从同一份字节完成摘要、解码和切片。连接／事务留在同步 worker 内，取消后由 worker 退出释放租约 |
| 公开入口 | Rust `ZvecGrep::explore`，CLI 七个便捷操作与 JSON 请求，MCP 结构化输入／输出，daemon 协议 13。CLI index 的 `--graph-options` 与 MCP index 的 `graph` 透传相同策略 |

Rust 公共 DTO 在默认 feature 下可解析；缺图后端时，在工作区或模型操作前返回 unsupported。图能力沿可执行程序依赖链显式传递。MCP Agent／Full 均按编译能力发现 explore；daemon 在 /healthz 声明 `explore.code.v1`，旧服务明确拒绝，不回退普通搜索。

## 查询语义

- explore 只读兼容且已发布的代码视图，不刷新、构图、恢复、Embedding 或加载模型。首版支持 Rust／Python、CONTAINS／CALLS／IMPORTS。
- 请求与操作 boundary 取交集，显式空集保持为空；展示过滤不阻断中间路径。Expand 用最短距离筛选 min_hops；ShortestPath 在跳数范围内按稳定次序返回一条简单路径。
- 普通列表资源截断返回 partial；未完成定位或路径搜索、取消、超时、证据失效均返回明确错误。
- 默认总期限 30 秒、图阶段 5 秒、2,000 节点、10,000 边、64 KiB 结果 JSON。资格核验单独计时并计入图阶段期限；原生 open／事务收尾没有可中断接口，不能据此承诺硬墙钟时延。

## 验证

测试与检查结果见 [verification.json](archive/pre-g4-validation/README.md#g3-1 "包内：knowledge/reports/g3.1/verification.json")，原始日志保留在同目录。环境为 macOS、Rust 1.98.0，graphdb `3bf0d532ce928c02782c76d42cc289b1e49e8df4`，zvec 修复制品摘要见记录。

| 检查 | 结果 |
| --- | --- |
| graphdb feature，全工作区 all-targets | 875 通过，0 失败，10 项既有忽略 |
| 默认 feature，全工作区 all-targets | 766 通过，0 失败，10 项既有忽略 |
| 后补 TextMap 真实存储用例 | 1 通过 |
| 最终 CLI 帮助，默认／graphdb feature | 各 7 通过 |
| Clippy，默认／graphdb feature | 均通过，`-D warnings` |
| 格式、diff、图 feature 调试二进制构建 | 均通过 |

端到端测试使用真实 zvec／graphdb 和本机模拟 Embedding 服务，未用外部模型推理代替查询验证。初次沙箱限制阻止本机端口测试，随后在获准环境完成全量回归；最终测试未注入会干扰旧 CLI 断言的 HOME 环境变量。

重点覆盖：

- 真实 zvec／graphdb 候选构建、发布、关闭重开；重复调用、自递归、直接包含和逐引用证据。
- 目标删除／恢复、歧义变化、已解析与未解析互斥；旧视图和引用拒绝。
- 中文、emoji、CRLF、空正文、重叠片段、非连续 TextMap；缺映射或正文／片段变化不能通过绑定校验。
- 定位多义与 UTF-8 位置、边界交集、循环、跳数、并列路径、无路径与预算未完成；只读数据摘要保持不变。
- Rust API、CLI direct／server 和 MCP 返回相同视图、关系与证据；查询期间 Embedding 请求数不变；协议／能力缺失明确失败。
- 默认 Off 检索、排序、授权、writer 借读、缓存退休及清理租约的既有回归。

原生分页中名称、来源和 ID 使用绑定参数；后端不绑定 SKIP 参数，偏移仅由内部 u64 渲染为数字，页大小固定 256。

## 边界与后续

本轮保留严格代码域就绪条件。此前冻结仓库语料的 260 个不完整来源仍作为覆盖缺口记录；本轮用完整小语料验证功能，未重跑整仓覆盖或扩大静态解析能力。G2 的原生故障窗口、构建性能残留仍以原报告为准。

资格核验仍需扫描完整提交和宿主文本绑定，只把扫描与业务读取分开；未宣称解决全量核验成本。没有批量导入优化或原地更新改造。Augment、Constrain、文档图查询和 G4 效果评测后续推进，本轮不作查询收益结论。
