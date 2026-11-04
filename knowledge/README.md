# zg 知识图谱

当前能力、边界和未完成项统一见 [S2 验收矩阵](S2_ACCEPTANCE_MATRIX.md)。六语言静态代码图、公共语义模型、explore 与代码 Augment／Constrain 已完成本地功能验证；四语言效果评测 R0—R5 及 C++ 根限定／Go range 遮蔽修复完成。构建成本优化继续暂缓，完整八语言和完整 S2 尚未交付。

- [公共模型与代码图设计及后续计划](COMMON_MODEL_AND_EIGHT_LANGUAGE_PLAN.md)
- [查询增强总体设计](QUERY_ENHANCEMENT.md)
- [知识图谱系统设计](knowledge-system-design.md)与[项目边界](knowledge-project-boundary.md)
- [候选事实格式](G2_1_FACT_SCHEMA.md)及[GraphDB 接入要求](GRAPHDB_RUST_BINDING_REQUIREMENTS.md)
- [Markdown 整合计划](MARKDOWN_UNIFICATION_PLAN.md)
- [四语言查询评测协议](FOUR_LANGUAGE_QUERY_EVALUATION_PLAN.md)与[冻结题集](../benchmarks/zg-retrieval/README.md)
- [存储只读问题](ANN_READONLY_DIGEST_DRIFT.md)与[剩余故障窗口](G2_3_FAILURE_WINDOWS_REPORT.md)

文件读取、解码、分段、Embedding、正文／向量索引和任务由 zg-engine 管理；zg-knowledge 管理图事实、消歧、公共语义契约及 GraphDB 持久化。查询契约只有一份，宿主复用已有类型和来源证据。

本轮已完成阶段的新增实施报告和逐次运行归档已清除。评测仍使用的冻结来源、标注、协议、调用矩阵和模型身份保存在 benchmarks；历史输入内容及默认预算不变。另任务的源码和性能记录保留。
