# zg-knowledge 模块收敛与 G2 修复交接

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-27。按用户最新指令，先收敛模块，再交回另一位 agent 修复 A1/A2 问题。本轮保留已知问题，不宣称 G2 整体或故障验收完成。

## 1. 调整结果

| 位置 | 结果 |
| --- | --- |
| zg-knowledge | 保留 error、processing、relations、可选 graph；移除独立快照、MediaType、Parser/Segmenter/Extractor、Embedding、ProjectionIndex、WorkerPool 和测试专用状态机 |
| processing | 保留 ProcessingContext、canonical_json、stable_digest 的正式路径；不改现有图构建/恢复调用方 |
| Markdown | 私有 CommonMark 入口移入 relations/markdown.rs；版本、选项、章节/链接/锚点及位置规则保持 |
| zg-storage | hash/filesystem 默认可用；zvec 及其原生构建依赖由显式 feature 启用；engine 显式启用 |
| KG 依赖 | 无生产 zvec 或独立 build.rs；共存测试仍保留 zvec 测试依赖，不省略真实双库验证 |
| engine | 除 zg-storage feature 声明外不改正式实现；模型、分段、索引、EntityId、图事务/发布/Resume 与读租约保持原文件字节 |
| 脚本与 CI | 在现有入口显式运行 storage zvec 测试，避免原生 target 因 feature 关闭而被漏跑 |

退役 13 个文件、3,348 行原文件内容，含独立组件和专属测试/示例；原内容以 [仅供溯源的压缩快照](archive/pre-g4-validation/README.md#module-consolidation "包内：knowledge/reports/module-consolidation/retired-components.tar.gz") 保留，不参与构建。没有移动或删除用户知识库、模型、缓存、graphdb/zvec 源码或原生库。原工作区的未提交修改保留，没有 commit/push。

## 2. 测试去向

收敛前 KG 有 70 项用例：42 项正式图/关系/Schema/共存用例原样保留；其余 28 项按断言拆分迁移，或随独立 SDK 契约退役。每项旧测试均有记录，见 [逐项清单](archive/pre-g4-validation/README.md#module-consolidation "包内：knowledge/reports/module-consolidation/test-migration.json")。不把测试退役算作修复，也不把较少的总数当作覆盖下降或提升的证明。

- CommonMark 章节、表格、围栏、Unicode 与范围断言迁入 tests/relations.rs；现有 engine BOM/CRLF/编码声明绑定测试保留。
- 原生混合写入结果、补写、全文/向量过滤和重开迁入 storage tests/write_statuses.rs；通用响应计数及 Flat/HNSW 只读不改文件测试保留。
- 新增 contracts.rs 和冻结夹具，对照重构前的事实、诊断、解析结果、ID、decoded UTF-8 范围、canonical JSON 及两种 Schema 指纹。
- 预计算向量空间、独立快照安装、Parser/Extractor 契约和独立线程池的专属测试退役；不声称 engine 已新增对应能力。模型输出维度/有限值等正式检查继续由既有模型测试验证。
- 独立 Completion 状态机不能证明真实 Resume 正确。本轮保留相关已知缺口，交给下一轮验证。

## 3. 验证记录

改动前先保存本地源码副本与摘要，再运行临时采集入口冻结契约；采集入口随后删除。夹具不是改动后重新生成的期望值。改动后两项契约测试及迁移的 Markdown/原生写入测试通过。

在清除 zvec/graphdb 环境变量、使用新 target 目录的情况下，分别编译 knowledge 和无 zvec 的 storage 纯库，通过；正常及构建依赖树不含 zvec/graphdb。测试依赖单独启用原生库，不能与纯库结果混为一谈。KG 删除包内 build.rs 后，真实共存测试按组件 README 显式设置动态加载路径；本轮固定指向经过校验的 ANN 补丁库，不假定 ZVEC_LIB_DIR 自动为测试安装 rpath。

完整五组件图 feature 回归 **671 通过、0 失败、10 项既有忽略**：engine 535、KG 45、storage 6、daemon 79、protocol 6。相对 A2 日志中的 695，退役/拆分 28 项旧用例并新增/迁移 4 项；engine 用例数保持。没有通过删除 A1/A2 测试来降低失败数。

最终默认构建、静态检查、源码与制品核对结果，以 [机器记录](archive/pre-g4-validation/README.md#module-consolidation "包内：knowledge/reports/module-consolidation/verification.json") 为准。本轮保留的已知失败见下一节，不能读作“所有检查通过”。运行环境为 macOS arm64、Rust 1.98.0、编译并发 14，真实底座沿用固定 graphdb 提交和 ANN 只读修复制品；不增加其他平台或性能结论。

## 4. 保留的已知问题

| 问题 | 本轮边界 |
| --- | --- |
| Resume 提交后取消/期限可能覆盖已提交结果 | 不改 recovery.rs、Candidate::resume、取消 watcher 或对应恢复 API。迁移不会消除该窗口，也没有用旧线程池测试声称已覆盖 |
| A2 覆盖表对 HEAD 持久化及 manifest 响应窗口的证据不足 | 原报告保持原样。实现代码和切换前取消用例不能直接作为对应故障窗口通过证据 |
| 既有 Clippy too_many_lines | Candidate::resume 的 114 行问题保留，收敛组件单独检查与 engine 整体检查分别报告 |
| 既有 rustfmt 差异 | graph.rs、recovery.rs、graph_indexing.rs 的原有差异保留。只格式化本轮修改的文件，不批量重写另一位 agent 的代码 |
| 原生提交中途崩溃/取消、规模和成本 | 继续按 G2 待办处理；已有提交后退出、适配层未知回包和本次模块回归不替代原生中断门 |

历史验收和故障报告均不改写。模块收敛不扩大 A1/A2 的通过范围。

## 5. 交给 G2 agent 的修复清单

1. 先读取当前源码及本报告/机器记录，不覆盖回收敛前的 KG 目录。保持 processing::ProcessingContext、canonical_json、stable_digest 入口和新 zvec feature 边界。
2. 修复 Resume 的提交后结果语义：提交前仍响应取消；一旦已提交，后到取消不能返回成普通未生效。必要确认使用独立、受预算约束的上下文与原生取消控制；确认失败保留已知提交效果并要求显式核对。
3. 增加确定性的“已提交/确认之后取消，再执行宿主重检”用例。只重跑已有正常 Resume 用例不足以验证该修复。
4. 修正 A2 窗口表：HEAD 文件/目录持久化的实际失败点与 manifest 切换后响应丢失分别说明；补测试或标为部分覆盖/残留，不能以 before_manifest 取消证明切换后行为。
5. 处理 Candidate::resume 的 Clippy 及三个文件的格式问题；抽取共用提交阶段时避免改变根/回执、错误 outcome 和取消优先规则。
6. 按新依赖运行默认与图 feature 回归，显式执行 storage zvec 原生测试，记录新的源码/原生摘要和逐 target 数量。历史 693/694/695 不再是收敛后的固定数量。
7. 修复与回归完成后再做 A3 最终规模/成本测量和 G2 汇总；公开图查询和融合仍属于 G3。

本轮未向不明确的会话发送修复指令。以上清单可直接作为下一位执行者的输入；不能据此推断其他会话已停止或已经开始修复。
