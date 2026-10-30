# G3.2 交付记录：代码图 Augment

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-28。源码基线为 `b5ea18d8f031d7ccae992221b10d5cab242e4e17` 加当前工作区改动；未提交或推送。最终验证状态见 [G3.3 记录](G3_3_IMPLEMENTATION_REPORT.md) 和 [执行证据](archive/pre-g4-validation/README.md#g3-3 "包内：knowledge/reports/g3.3")。

## 已实现

- 普通检索及多查询合并后，保留真实 EntityId、FragmentId、源区间、最终优先级及查询来源；最多选 5 个唯一种子，一次完成至多两跳扩展。
- 支持 Rust/Python 的 CONTAINS、CALLS、IMPORTS；重复调用和自递归保留逐引用边。容器跨定义、同名歧义及映射缺失分别诊断。
- 复用同一文本租约、候选选择和 G3.1 的完整资格核验；业务读取按需分页。没有二次召回、另一份向量或额外 Embedding。
- 普通 items、group_results、排序及配额保持原行为，关系和证据另放 graph；已返回的正文用引用关联。
- 兼容图不可用或图阶段预算不足时，先核验文本安全再降级。取消、总期限及文本失配明确失败。writer 借读保留安全文本并报告图不可用。

## 公开入口

Rust 使用 `ContextOptions.graph` 及可选的 `graph_execution`。CLI 支持 `--graph-mode off|augment`、互斥的 `--graph-request` 和 `--graph-execution` JSON 文件；MCP search 使用同一类型。daemon 协议为 14，独立声明 `augment.code.v1`，旧端不能删除 graph 参数后重试。

```sh
zg query --fts caller --graph-mode augment --preview full
```

## 验证与边界

真实 zvec/graphdb 的模型替身用例覆盖普通结果等价、两跳与多引用、Imports、writer、缺图／不完整代码域、预算、期限及文本变更。跨入口测试比较同一请求的 Rust、CLI direct/server、MCP 图结果，并检查 Embedding 调用数。

覆盖基线保持独立：A0 冻结清单含 382 个 Rust/Python 来源，110 个抽取完整、272 个不完整；历史 260 个缺口全部保留。该扫描不证明文本索引或整仓代码域 ready。完整提交／绑定核验仍有全量成本；构建性能和 G4 效果收益均未在本轮解决。
