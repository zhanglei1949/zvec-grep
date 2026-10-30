# G3.3 交付记录：代码图 Constrain

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-28。状态：本地实现与验证已完成。范围到 G3.3；不推进 G3.4、G3.5，不修改语雀。基线和运行日志见 [reports/g3.3](archive/pre-g4-validation/README.md#g3-3 "包内：knowledge/reports/g3.3")，G3.2 见 [Augment 记录](G3_2_IMPLEMENTATION_REPORT.md)。

## 实现

| 层 | 交付 |
| --- | --- |
| KG | `resolve_scope` 唯一定位锚点并完整遍历；full 取选中节点，own 扣除直接子节点。通过同版本 TextMap 与真实正文／片段元数据生成不可由客户端构造的 ResolvedScope |
| engine | Required 视图、模型前范围与用户过滤交集、判空和表达式预算检查；复用读取准入、候选租约及资格核验 |
| 检索 | StorageSearchFilter 增加 FragmentId；FTS、向量、prefer_symbol、各查询组及自适应加深均在 TopK 前带同一硬范围，不并入全库结果 |
| 结果 | graph.scope 返回锚点、完整性、范围／片段计数及空原因；正文逐项复核，移除越界 outline、container、签名和文档元数据 |
| 入口 | Rust、CLI JSON、MCP、daemon 同语义；协议 14 独立声明 `constrain.code.v1`，不支持的旧端明确拒绝 |

范围及用户过滤交集为空时，不获取模型、不发送 Embedding、不提示查询授权。daemon 的 Wait／Background 在安排刷新前检查空范围；预检查消耗从后续图预算扣除，结果累计统计。授权检查与执行分离时重新选视图，不复用失去租约的范围。

查询取消和期限沿用同一请求上下文。原生资格核验与范围计算留在一个同步任务中，取消监视器随任务结束并 join；任务持有租约和准入锁，异步调用方退出不会提前清理原生句柄。临时正文快照另设 256 MiB 上限。

## 使用

```json
{"mode":"constrain","scope":{"anchor":{"by":"name","name":"caller"},"traversal":{"max_hops":1}}}
```

保存为 `scope.json` 后运行 `zg query --fts target --graph-request scope.json --preview full`。Rust 使用同形的 GraphQueryMode；MCP search 接收 `graph` 及 `graphExecution`。`authorization::inspect_constrain_scope` 供刷新协调读取空范围和消耗，不返回可复用的范围或句柄。

## 验证

- 原生底座测试在同一实体的不同片段放置范围外高分项，核验全文和向量的 TopK 前过滤、空集合及重开行为。
- Constrain 定向测试覆盖多组／融合、FTS／向量／符号路由、全库有匹配但范围内无匹配、空范围与用户交集、模型前过滤预算、缺图、失败不降级、own 跨界、活动查询取消、其他读者和锁释放。
- 同一发布候选比较 Rust、CLI direct/server、MCP 的 Augment／Constrain 结果；daemon 检查空范围的 Background／Wait 均不刷新，以及预检查不能重置预算。
- engine/KG 原生库串行回归通过 507／22 项；后续改动另跑受影响的定向与入口回归。默认构建的全工作区单元／集成测试通过，源码迭代期间的 doctest 依赖不一致保留日志，冻结源码后的全工作区 doctest 已通过。

默认与 graphdb 的全工作区 Clippy `-D warnings`、中间 knowledge-graph feature 检查、格式和 diff 检查均通过。生命周期测试的轮询同时等待任务成功和已请求的持久化计数，不再把任务先完成的瞬间当成状态就绪；断言和期限保持原约束。

最终命令、结果、原生依赖和源码摘要以 [verification.json](archive/pre-g4-validation/README.md#g3-3 "包内：knowledge/reports/g3.3/verification.json") 为准；不累计相互重叠的通过数。硬件／外部模型前置的既有 ignored 测试未启用。

## 保留边界

- 首版只接受可由已有片段精确表示、且单片段有连续源坐标的显示范围。跨界内容无法覆盖或非连续显示坐标返回 `scope_not_representable`；不放宽到整实体、不查询后裁剪、不临时重嵌入。
- 锚点未唯一定位、遍历／映射未完成、预算不足、视图或文本不一致均失败，不能返回伪空结果或退回普通搜索。
- 精确范围只保证检索成员资格；FTS TopK、ANN 和结果合并仍返回有界排名样本。未承诺穷举召回或实际效果收益。
- 代码域 ready、历史 260 个抽取缺口、构建耗时及 G2 故障窗口继续按原记录跟踪。未接入文档图，也未修改构建／发布／恢复策略。
