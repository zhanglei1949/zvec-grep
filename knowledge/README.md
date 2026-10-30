# zg 知识图谱开发资料

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

当前主线是 zg 的代码查询与文档查询增强，两者分别推进。这里记录本地实验、设计和验证，不代表 zg 已发布图谱能力；独立 KnowledgeBase SDK 后置。

文件读取、解码、检索分段、Embedding、正文/向量索引及任务调度由 zg-engine 承担；[zg-knowledge](../rust/crates/zg-knowledge/README.md) 处理图事实、消歧和可选 graphdb 持久化。[zg-storage](../rust/crates/zg-storage/README.md) 提供共享存储工具，图复用宿主正文和向量。

## 当前计划

| 文档 | 用途与状态 |
| --- | --- |
| [G0–G4 查询增强](QUERY_ENHANCEMENT.md) | 阶段范围与总体顺序；具体进度结合各阶段记录阅读 |
| [G2 实施计划](G2_IMPLEMENTATION_PLAN.md) | graphdb 接入、构建、发布与恢复；G2.4 已带残留收尾，逐项结论见总验收 |
| [全量图构建性能优化](GRAPH_BUILD_OPTIMIZATION.md) | P0 typed batch 已完成；graph-only 中位耗时下降 49.1%，后续优化完整读回，暂不进入 AP |
| [G3 实施计划](G3_IMPLEMENTATION_PLAN.md) | G3.0—G3.3 代码 explore／Augment／Constrain 已完成本地验证；文档图后置 |
| [G4 验证集设计](G4_VALIDATION_SET_DESIGN.md) | 执行已完成，整体验证未全部通过；见[验证报告](G4_VALIDATION_REPORT.md) |
| [Markdown 解析整合](MARKDOWN_UNIFICATION_PLAN.md) | 计划记录 P0–P2 已完成，P3 已确定版本处理；P4/P5 仍待收尾 |

已完成的模块收敛计划和旧独立 SDK 交付计划归入 [历史资料](HISTORY.md#历史计划与长期设计)。

## 实施与验证记录

记录只适用于各自标注的源码、原生底座和运行环境；历史通过数不代表当前工作区已验证。

- [G4 代码图验证结果](G4_VALIDATION_REPORT.md)与[原始证据归档](archive/g4-validation/README.md)
- [G0/G1 查询增强](QUERY_ENHANCEMENT_REPORT.md)
- [G2.0 binding 接入与原生共存](G2_0_ACCEPTANCE_REPORT.md)
- [G2.1 事实持久化](G2_1_ACCEPTANCE_REPORT.md)与[候选事实格式](G2_1_FACT_SCHEMA.md)
- [G2.2 宿主发布](G2_2_HOST_PUBLICATION_REPORT.md)、[图根提交](G2_2_BUILD_COMMIT_REPORT.md)、[分批导入与恢复](G2_2_BATCH_RECOVERY_REPORT.md)、[原生取消与期限](G2_2_NATIVE_CONTROL_REPORT.md)
- [G2.3 文本恢复与图读租约](G2_3_RECOVERY_LEASES_REPORT.md)、[候选保留与清理](G2_3_RETENTION_REPORT.md)、[故障窗口与修复](G2_3_FAILURE_WINDOWS_REPORT.md)
- [G2 分域总验收与残留](G2_ACCEPTANCE_REPORT.md)、[G2.4 L1 资源数据](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/resources.json")及[正确性核验](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/verification.json")
- [G3.0 查询类型、参数与执行契约](G3_0_CONTRACTS_REPORT.md)
- [G3.1 业务图、TextMap 与代码 explore](G3_1_IMPLEMENTATION_REPORT.md)及[后续 review 修复](G3_1_REVIEW_REPORT.md)
- [G3.2 代码图 Augment](G3_2_IMPLEMENTATION_REPORT.md)与[G3.3 代码图 Constrain](G3_3_IMPLEMENTATION_REPORT.md)
- [模块收敛结果与测试去向](MODULE_CONSOLIDATION_REPORT.md)
- [graphdb binding 接入要求](GRAPHDB_RUST_BINDING_REQUIREMENTS.md)与[ANN 只读摘要漂移记录](ANN_READONLY_DIGEST_DRIFT.md)

## 历史资料

[历史索引](HISTORY.md) 收录独立 SDK 的长期设计、旧计划、Python 基线与迁移记录。Python 源码、测试、示例和 Spike 已归档，恢复方式见 [基线恢复](HISTORY.md#python-基线恢复)，退役过程见 [退役记录](RETIREMENT.md)。这些资料不作为当前 zg 功能说明或 G0/G1 的实现前置条件。
