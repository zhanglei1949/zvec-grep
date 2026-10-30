# zg 查询增强 G0/G1 实施记录

日期：2026-10-22。执行范围与后续门见 [G0–G4 说明](QUERY_ENHANCEMENT.md)，机器记录见 [validation](query-enhancement-validation.json)。本轮未提交或推送。

## 已落地

- `zg-knowledge::relations`：来源版本、decoded UTF-8 范围、文件/模块/符号/章节、词法绑定、调用/导入/文档链接引用、目标候选与保守解析读集。
- `zg-engine/knowledge-graph`：默认关闭的候选事实检查入口，Rust/Python 使用与宿主实体提取相同的 Tree-sitter 树；关闭时 `zg` 的正常依赖树没有 KG。没有新 grammar、模型运行时或第三方依赖。
- Markdown 复用 KG 固定 CommonMark 事件入口，提取章节标题与显式链接。重复锚点、未确定目标、外部链接、路径越界、指向代码文件分别表达。没有跨代码/文档图关联。
- 逻辑节点 ID 排除正文和位置；证据位置绑定原始/解码摘要、generation 和同次提取的实体索引。局部绑定、动态接收者、装饰器改写、函数指针常量等不会被猜成唯一静态函数调用。
- 8 个固定来源、16 个问题（代码 8、文档 8）与证据初稿；问题/标签在语料之外。测试验证字节、来源摘要和基础边，标签待人工复核。
- 可运行 `graph_facts` 示例、默认构建检查与 feature 验证脚本；README、边界、总体设计、交付计划及 binding 接入要求已经明确当前执行优先级。

## 验证结果

| 检查 | 结果 |
| --- | --- |
| 全 workspace Clippy，all-targets，图 feature，`-D warnings` | 通过 |
| 默认 feature 的全 workspace all-targets check | 通过 |
| 全 workspace 串行回归，沙箱外 | 41 个 harness：755 通过、0 失败、原有 10 ignored |
| 最终关系集成测试 | 23 通过、0 失败、0 ignored |
| 最终事实契约测试 | 4 通过、0 失败 |
| 最终宿主 extraction 测试 | 33 通过，包含同树提取结果不变的新增用例 |
| 示例 | 8 个来源、21 个引用：16 resolved、4 unresolved、1 unsupported；`published=false`、`search_ready=false` |
| 格式、文档回读/链接、固定输入摘要与 diff 空白检查 | 通过 |

共新增 28 个用例：关系集成 23、事实契约 4、同树提取回归 1。整体回归完成后补充了 Rust 类型作用域限制及一个用例，此后对最终源码重新运行关系/契约/extraction 测试、Clippy 和默认构建；不把两轮重叠测试相加成新的总数。

初次沙箱全量运行在既有 auth 测试停止：一项本地 socket PermissionDenied、两项错误文本断言失败。沙箱外 auth 五项通过，但并行全量又遇到两个既有 daemon 启动用例的 `invalid daemon instance record`。沙箱外串行全量 755 项通过，包含 daemon 的全部 13 项。尚未确定并行启动锁失败的根因，没有修改、跳过或放宽断言。日志保存在本地忽略目录 `knowledge/.artifacts/graph-g1/`。

串行整体回归真实执行了现有 zvec 组件、存储、只读与重开测试；这不等同于 graphdb＋zvec 图发布验收，也不撤销此前只读向量测试的环境差异记录。

## 尚未交付

上游 graphdb HEAD 为 `9d6e966ef0ae214cff8d5018efe1dafb37bc2a99`，本轮检查时 Rust 目录仍为 `?? rust/`，不属于可锁定的提交。G2 的版本接入门未满足；本轮没有再探测未提交 binding 的查询/事务实现，也没有修改 graphdb 仓库或用绝对路径接入。

当前没有持久化图、联合发布/恢复、真实图增量维护、反向查询、有界遍历、relations API/CLI/MCP 或图增强搜索。引用删除/变化的用例是事实重新计算测试。图侧增量、旧读者、未知提交和故障恢复仍需 G2 的真实底座验收。

初始标签仍待人工复核，未运行模型检索对照或性能评估；小型夹具不能证明真实仓库查询增益。下一门是获得可锁定、具备参数化查询、类型化结果、显式读写事务和提交错误分类的上游版本，然后实施 G2。
