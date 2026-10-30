# G3.1 review 与修复

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-28。基线为 [G3.1 交付时的源码摘要](archive/pre-g4-validation/README.md#g3-1 "包内：knowledge/reports/g3.1/verification.json")，review 前逐文件核对一致。HEAD 仍为 `b5ea18d8f031d7ccae992221b10d5cab242e4e17`；未提交、未修改语雀。

## 发现与处理

| 级别 | 问题与影响 | 修复与证据 |
| --- | --- | --- |
| P1 | reader 独立于租约持有者，退出作用域时可能先释放读锁和读取租约，再销毁 reader；并发写入／清理存在窗口 | reader 纳入同一 Lease，字段顺序保证原生 reader、读取会话先于准入锁释放；真实 explore 与既有租约回归通过 |
| P2 | 名称／位置定位只扣 candidates，不扣 nodes；nodes=1 仍能返回两个定位候选 | 同一次原子 charge 扣两个计数；新增多义候选用例，预算不足明确失败 |
| P2 | 默认 daemon 先 canonicalize 工作区，再经 engine 判断缺图后端；可能返回路径错误或产生多余访问 | 运行时包装层提前交给 engine 做后端准入；缺失路径用例确认返回 unsupported，且不创建运行时或加载模型 |
| P2 | daemon 分发超时被转成通用 bridge 错误，调用方 signal 未参与分发等待；CLI 也可能丢失本地 engine 错误报告 | 分发受请求 token／期限约束，保留 Cancelled／DeadlineExceeded；CLI 对本地／远端 engine 错误使用原报告。暂停时钟测试覆盖超时、预取消和等待中取消 |
| P2 | graph-only 因正文／片段绑定不一致而拒绝后，外层只保留分类码，重建提示丢失 | 将安全 help 保留在 GraphIndexResult.diagnostics，并由 CLI 输出。真实存储用例保留 EntityId、改变片段结构，确认拒绝复用、保留显式重建提示、零模型获取及文本存储不变 |

主要入口：[读取与租约](../rust/crates/zg-engine/src/pipelines/explore/mod.rs)、[定位预算](../rust/crates/zg-knowledge/src/query/explore.rs)、[daemon 分发](../rust/crates/zg-daemon/src/lib.rs)、[运行时准入](../rust/crates/zg-daemon/src/workspace_runtime.rs)、[构建诊断](../rust/crates/zg-engine/src/pipelines/indexing/graph.rs)。

## 核对范围

本轮对照了实体具体种类、直接包含边、已解析边与 UnresolvedRef 的互斥存储、构建重解析、TextMap 和正文／片段绑定、发布证明、按需分页、遍历／过滤、证据与资源所有权，以及 Rust／CLI／MCP／daemon 的调用路径。

这些部分未发现需要再次迁移 v2 格式或改变 G2 发布协议的明确问题。没有扩大抽取器能力，也没有放宽代码域 ready。分页资格核验仍扫描完整提交，成本保留单独记录。

## 验证

使用 Rust 1.98.0、原 graphdb 提交和已核验的 zvec 修复制品，隔离缓存、注册表和测试工作区。结果与源码摘要见 [verification.json](archive/pre-g4-validation/README.md#g3-1-review "包内：knowledge/reports/g3.1-review/verification.json")，日志在同目录。

| 检查 | 结果 |
| --- | --- |
| KG explore 算法与预算 | 8 通过 |
| engine 真实 explore／graph-only 绑定拒绝 | 5 通过 |
| engine 读取缓存与租约 | 11 通过，1 项既有手工基准忽略 |
| daemon，默认／图 feature | 各 82 通过 |
| CLI 参数回归 | 42 通过 |
| CLI 错误报告 | 1 通过 |
| Rust／CLI direct／daemon／MCP 真实入口对照 | 1 通过 |
| 默认／图 feature Clippy | 均通过，`-D warnings` |

本轮运行受影响的定向回归，没有重复上一轮全工作区的 875／766 项测试。初次 daemon 测试受沙箱监听限制，随后在获准的本机端口环境重跑通过；失败日志保留为 daemon-sandbox.log。客户端分发取消与 engine 资源回归分别验证，不据此承诺原生 open、事务收尾或跨进程故障下的硬墙钟上限。

## 后续

[G3 计划 §5.2](G3_IMPLEMENTATION_PLAN.md#52-augment先合并再补图) 已细化 G3.2 的 A0—A6：覆盖基线、真实命中交接、共享读取、种子映射、一次有界 augment、降级与入口、验证。G3.2 尚未实现；Constrain、文档图、构建性能及 G4 效果不在本轮。

260 个不完整来源仍是历史冻结语料数据；当前整仓覆盖待 A0 重跑。当前 Schema／事实格式维持 v2，daemon 协议维持 13；计划中的协议 14 随 G3.2 交付再改。
