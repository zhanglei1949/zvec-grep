# G2.2 原生取消与期限桥接验收

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-26。本轮接续 [分批导入与显式恢复核对](G2_2_BATCH_RECOVERY_REPORT.md)，完成宿主取消/期限到 graphdb 查询取消句柄的桥接。前两份 G2.2 报告保留各自固定快照与历史结论；本轮新增结果单独记录。

**原生查询取消桥接已接通；G2 整体尚未完成。** 当前没有公开关系查询，宿主任务重放、dirty STATE 修复、读者租约与安全清理仍待后续实施。

## 1. 实现

| 位置 | 行为 |
| --- | --- |
| [QueryControl](../rust/crates/zg-knowledge/src/graph/control.rs) | 框架自有的一次性取消控制；内部持有可跨线程的上游 CancelToken，不暴露连接或 FFI 句柄。首次观察到的取消/期限原因保留，后到请求不能改写 |
| [图适配器](../rust/crates/zg-knowledge/src/graph/graphdb.rs) | GraphLimits.control 传入所有参数化查询和 DDL 的 QueryOptions；保留默认结果/中间行和准入限制，移除事实写入前唯一绕过选项的 query_one 路径 |
| [宿主控制](../rust/crates/zg-engine/src/pipelines/indexing/graph/control.rs) | 每个已获写入准入的阻塞操作配一个 Tokio watcher；等待宿主 token 或同一 ProcessingContext 期限，触发原生取消，不新增线程池或后台扫描 |
| [构建与选择](../rust/crates/zg-engine/src/pipelines/indexing/graph.rs)、[恢复](../rust/crates/zg-engine/src/pipelines/indexing/graph/recovery.rs) | 旧图读取、候选创建/导入、根/回执事务、确认与显式恢复中的查询统一使用该控制；manifest 选择前的图核对同样接入 |
| [事实最终确认](../rust/crates/zg-knowledge/src/graph/import.rs) | finalized/事实元数据事务已成功后，新的只读核对被取消仍保留事实提交已发生的 outcome 与底层 cause；不据此宣称 GraphRoot 或 HEAD 已提交/选择 |

watcher 归 worker 所有。异步调用方退出不能提前撤销桥接或释放 worker 持有的真实写锁；native 调用和连接仍在同一 blocking task 内完成生命周期。正常 worker 结束会停止 watcher，不因销毁监听任务而补发取消。运行时销毁监听 future 时，析构路径会请求原生取消；本轮只验证了调用方退出与正常结束，没有执行运行时强制关闭验收。

取消句柄不复位。显式重试/恢复使用新操作和新句柄，不能携带已触发的句柄要求后续查询成功。图适配器仍可不配置 control；正式宿主路径主动注入。

没有增加依赖、宿主 FFI、正文/向量副本或模型调用。

## 2. 错误与提交边界

- 宿主期限通过 CancelToken 中止查询时，框架分类为 Deadline，同时保留原生 kind=Cancelled、执行 outcome、阶段和嵌套原因。先观察到显式取消时保持 Cancelled。这里的先后指框架观察顺序，不承诺恢复两个并发信号的外部时间顺序。
- Unknown、不可用和清理错误仍要求 recovery_required，不能被后到取消降低保护等级。隔离实例不能继续查询/写入，需释放拥有者后重新可写打开、核验真实根与成功回执。
- 已确认提交后的只读确认被取消，不撤销已经发生的事务效果。事实 finalized、根/回执提交和 HEAD/manifest 选择分别判断；成功回执仍不可变。
- QueryControl 只对上游实际接受 QueryOptions 的查询/执行生效。当前 binding 的打开、连接、事务 begin/commit/rollback 和 checkpoint 不接受 CancelToken；文件系统持久化也不属于这套取消机制。正在执行这些步骤时仍等待真实结果、保留准入，不能超时返回后立即释放拥有者或宣称未生效。
- transaction_timeout 仍只是上游准入/重试限制。宿主图操作沿用 120 秒 ProcessingContext，桥接后可请求中断运行中的查询，但不是整个索引请求的硬墙钟上限；来源扫描等前置阶段及不接受 token 的调用不在该保证内。资源限制也不是内核总内存/RSS 硬上限。

## 3. 测试与证据

新增 [图适配测试](../rust/crates/zg-knowledge/src/graph/control_tests.rs) 与 [宿主控制测试](../rust/crates/zg-engine/src/pipelines/indexing/graph/control_tests.rs)，共 **7 项**，完整回归包含这些测试，不重复累加。

| 测试 | 实际覆盖与限制 |
| --- | --- |
| 两种停止原因与优先级 | 一次性句柄保留首个原因；期限重分类保留已提交 outcome 与原生种类；Unknown 保留恢复要求 |
| 活跃原生导入取消 | 真实 TP 图库，在 native import 的生产者回调进入第二批时从另一测试线程取消；首批待提交行全部回滚，释放并可写重开后仍为零。分别覆盖显式取消/期限标记，不作为任意长 CPU 查询抢占时限的证明 |
| 宿主转发到原生读取 | 真实图库事先准备；worker 在 gate 后以已触发 QueryOptions 执行原生只读核验，确认收到 native Cancelled，期限映射为 Deadline；没有用 cooperative check 代替原生错误 |
| 异步调用方退出 | 真实 OS 文件锁保持占用，caller abort 后 watcher 仍接收宿主取消；实际原生读取停止，worker 退出后锁释放，新句柄能只读重开 |
| 正常结束 | worker 完成时销毁 watcher，不额外请求取消；不是高负载运行时关闭测试 |
| 已提交后注入 Unknown | 真正写入全部事实与根/回执后，在适配器 finish 注入响应未知；后到取消不解除隔离；关闭、可写重开后读回同一成功回执，幂等提交不改写它。注入点在适配层，不是原生提交中途丢响应 |
| 提交后进程退出 | 子进程确认真实根/回执提交后 exit(29)，跳过析构和显式 checkpoint；父进程可写重开，从 WAL 核验成功回执并幂等读取。不等同于提交中途 SIGKILL 或断电 |

### 回归结果

| 检查 | 结果 |
| --- | --- |
| 五组件图 feature all-targets | **677 通过、0 失败、10 项既有忽略**；engine 520、KG 67、shared storage 5、daemon 79、protocol 6 |
| 新增原生控制测试 | **7 项**全部纳入上述完整回归；开发定向验证分别为 KG 4、engine 3 |
| 默认 feature 专项 | **11 通过、0 失败**，含图恢复拒绝、POLICY/STATE/文本写前栅栏 |
| 编译与静态检查 | 图 feature 和默认 Clippy 均通过 `-D warnings`；默认 workspace 全 targets 编译通过 |
| 格式与一致性 | cargo fmt、git diff --check、223 项受检源码/依赖摘要和文档链接通过 |

五组件结果来自同一次固定源码、真实 graphdb＋zvec 的 all-targets 执行。模型计算只在既有服务夹具中替换为确定性实现。定向检查与完整回归有重叠，不合计为额外覆盖数量。默认 feature 的图协议/文本栅栏继续保留，未启用图的用户无需原生 graphdb。

使用 macOS arm64、Rust 1.98.0、编译并行 14，Cargo 检查顺序执行。graphdb 固定提交为 3bf0d532ce928c02782c76d42cc289b1e49e8df4，子模块保持干净；35 个原生归档逐项核验。zvec 使用只读修复制品，运行时库 SHA-256 为 50e243d8a941419c30bf11b4e5b4e8852275584109d647d86b3fa4129d805912；显式设置 DYLD_LIBRARY_PATH，完整测试日志保留实际加载路径。没有据此宣称远端预编译包或其他平台已更新。

命令、日志、源码/依赖与原生摘要、分项统计见 [机器记录](archive/pre-g4-validation/README.md#g2-2-native-control "包内：knowledge/reports/g2.2-native-control/verification.json")。旧报告中的 ANN 失败证据保留原样，本轮不重新归因。首次 Clippy 指出新代码的按值借用和测试 unwrap 问题，已修正；原日志与最终检查分别保存。

## 4. 尚未覆盖与下一步

- 原生提交执行中丢失响应、提交中途进程强杀/断电，及这些窗口与取消/期限的交错。
- 不接受 QueryOptions 的 begin/commit/rollback/checkpoint 等调用的可中断性，需要上游接口和独立真实探针；本仓库不增加第二套 FFI。
- 宿主固定 operation/attempt 的完整任务重放、dirty STATE 显式修复与失败分支核对。当前 recover_graph 只核对候选效果并按既有基准继续选择。
- 读者租约、旧候选保留与安全清理、规模/成本验收及 G2.3/G2.4 的剩余门。

下一步先定义 dirty STATE 的显式修复入口与原生故障注入边界，保持默认只读打开不修复、不凭诊断猜测成功。公开 relations、搜索扩展和 CLI/MCP 仍留 G3。
