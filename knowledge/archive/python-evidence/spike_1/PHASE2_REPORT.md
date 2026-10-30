> 历史 Python 验收记录；执行命令需要先恢复基线。原始文档和源码保存在 [Python 基线归档](../../python-baseline.zip)，本可阅读副本仅更新归档引用。

# Phase 2 实现与验收记录

日期：2026 年 10 月 21 日。

结论：来源注册、不可变快照/修订、完整正文单元、幂等摄入、显式回执核对及可替换文档处理契约已实现。最小来源写入探针通过，完整阶段 A Go **未通过**；不得将本记录作为完整 Spike-1/2、知识抽取或检索能力的验收。

## 环境与复现

本次环境为 macOS 15.1.1 arm64、CPython 3.13.3；复用 Spike-0 已安装 wheel，原生集成测试通过其 provenance 检查，没有重新下载或编译依赖。

- graphdb：`9d6e966ef0ae214cff8d5018efe1dafb37bc2a99`，沿用 Spike-0 的 libcxx-csv-fallback 构建变体。
- zvec：`a4347e31f1b8e216cffba59285427a3b78551229`，沿用未修改源码构建产物。
- 原始组合构建边界与许可证待办继续以 [Spike-0 报告](../spike_0/SPIKE0_REPORT.md)为准。

```bash
.venv/bin/python -m spikes.spike_1.source_probe
.venv/bin/python -m examples.phase2_ingest
.venv/bin/python -m unittest discover -s tests -t . -v
.venv/bin/ruff check zknowledge tests examples spikes/spike_1
```

原生探针结果：[SOURCE_PROBE_RESULT.json](SOURCE_PROBE_RESULT.json)。测试日志保存在本地 `.artifacts/phase-2/tests.log`，不是发布包依赖。

## 原生探针的证据边界

| 项目 | 结果与证据种类 |
| --- | --- |
| 稳定 ID 与幂等 | 真实 TP 事务、参数化查询、重复写入不增加记录 |
| 提交可见性 | 同一串行化连接的业务读取可见；不外推独立读连接/并发读者 |
| 事务原子性 | 事务体注入异常，真实底座回滚主体变化及回执 |
| 回执核对 | 实际提交后注入确认响应丢失，隔离连接，实际关闭/可写重开后查回执 |
| 只读重开 | 实际重新打开并读取既有结果，不执行修复写入 |

开发中还确认：TP DDL 不能放在普通数据事务中；分页 LIMIT 参数在当前底座路径不可用。实现采用初始化空闲连接建表，以及经整数范围验证的内部 LIMIT 字面量；业务值仍使用参数，不开放原始 Cypher。

真实 CommitOutcomeUnknown 故障窗口、进程强杀/断电、跨进程准入、独立并发读者、容量/RSS 和 zvec/有效视图性能均未验证。fsync/原子替换是实现机制，不能替代上述试验。

## 已交付行为

- SourceSpec 显式来源键；同键领域/位置不一致返回冲突。Source ID 由 KB 身份与来源键确定，领域作为一致性约束；相同内容不同键仍是独立来源。
- Schema 1 含来源、修订、正文单元、操作、尝试、回执及图内身份记录，并建立来源—修订—正文关联；owner/state 是检查列，不承诺有二级索引。
- 捕获 UTF-8 原始字节，保留 BOM/CRLF。一个修订对应完整正文单元；不做 Markdown 语法解析。
- 当前输入/捕获指纹相同返回 unchanged；配置变化新建处理单元；A→B→A 复用修订但记录新选择。失败不能复用成成功。
- 图事务原子写入修订/单元、最近成功摄入选择和回执；准备记录独立先写入。来源竞争与尝试代次检查阻止覆盖和迟到提交。
- 已提交操作按 operation_id 返回原回执，即使来源后来已选用其他修订；不重新应用历史操作。
- ingestions.get/snapshot 只读检查状态与固定输入（原文件删除后仍可读取），reconcile 在可写重开后显式核对 committed/not_committed。未完成准备记录不能未经核对盲目重试。
- 提交前取消保留原选择；提交后的迟到取消保留成功。提交或确认不确定时隔离实例，禁止继续业务写入。
- 快照唯一临时文件、文件/目录 fsync、原子替换、完整元数据与摘要/长度校验；同内容并发安装、编码冲突及严格范围校验。无引用文件暂留，不自动 GC。
- DocumentParser / KnowledgeExtractor Protocol、自有结果类型、阶段指纹、同步校验辅助函数和协作式预算/取消契约。未连接真实解析器/模型，不宣称就绪或语义质量。

## 验收结果

当前共 **72 项测试通过，0 跳过**：44 项单元测试、28 项真实依赖环境中的集成测试，包含原有 45 项回归。集成测试内的故障注入不等同真实底座故障触发。

| 验收范围 | 对应测试 |
| --- | --- |
| 原生命周期/适配器、准入与错误 | 原 Phase 1 测试集；范围读取越界按新严格契约更新 |
| 注册/分页、独立来源支持前置归属 | test_sources 的注册、分页、共享字节与图关系测试 |
| 重复摄入、修订/配置变化、回执重放 | test_sources 的修订、重选与处理变化测试 |
| 中文/BOM/CRLF/空文档、路径输入、排除与预算 | test_sources 的原始字节、路径策略、增长拒绝测试 |
| 事务失败/孤立文件/重试、来源竞争与过期尝试 | test_sources 的回滚、并发、活动操作和代次测试 |
| 提交前/后取消、未知提交、确认失败与重开 | test_sources 的取消、响应丢失及显式核对测试 |
| 旧 Schema 拒绝、初始化中断、关闭排空 | test_sources 与 test_kb_lifecycle |
| 快照并发安装/损坏/元数据中断 | test_snapshot_store 的完整性测试 |
| 两个 Parser/两个 Extractor 替换与契约错误 | test_document_processing；全部为测试实现 |
| executor 排队上界 | test_jobhandle 的有界 backlog 测试 |

示例已运行：首次摄入 captured、相同输入 unchanged、修订 captured；只读重开后新旧正文均可核验。结果中的 search_ready 始终为 false。

## 限制与后续

Phase 1 Schema 0 不兼容，不做自动升级或覆盖。需要复用旧空库路径时由调用方显式清理后重建；不得据此清理包含业务数据的目录。

来源和修订列表 limit 范围为 1–1000，游标按稳定 ID 排序，是有界结果检查而非跨页快照隔离。捕获准入预留输入大小的四倍；未单独配置输入上限时取候选空间额度的四分之一，文件增长超过已准入尺寸则拒绝。这是保守逻辑额度，不是 RSS/实际磁盘空间的硬限制或调优结论。

同步契约辅助函数可检查提供者前后状态与输出范围，不能强杀任意提供者或证明其不会越权外发；宿主须信任并管理注入实现。Descriptor 常见凭据字段检查不能证明所有配置值不含秘密。

下一步在完整依赖门槛核验后进入 Phase 3：选择和锁定实际 Markdown 解析依赖、维护原始字节映射、章节分段、Embedding/zvec 候选、知识发布及完整恢复协议。KnowledgeExtractor 的真实模型实现与领域质量评测留 M1。
