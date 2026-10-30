> 历史 Python 验收记录；执行命令需要先恢复基线。原始文档和源码保存在 [Python 基线归档](../../python-baseline.zip)，本可阅读副本仅更新归档引用。

# Spike-1/2 底座实测结论

结论（2026-10-21）：**阶段 A 在确认的本地开发档位下 Go，可进入 Phase 3 的详细设计与实施；Phase 3 尚未开始。** 18 项隔离试验全部通过，两套原生故障测试通过；项目回归 82 项通过，无跳过。试验代码是底座探针，不交付正式发布、检索或知识维护 API。

## 1. 判定范围与证据

| 风险 | 结果与依据 | 边界 |
| --- | --- | --- |
| R1 组合可运行 | 沿用 [Spike-0](../spike_0/SPIKE0_REPORT.md) 固定 wheel；此次真实 graphdb/zvec 测试通过 | graphdb 使用 libcxx-csv-fallback 构建变体；许可证问题仍按 Spike-0 跟踪，Go 不是对外发布许可 |
| R2 目标工作集 | 10/100/1,000 来源三档通过，含三个版本；另测两个旧 Read 与候选构建和选择切换 | 只证明目标档位可行，没有找到容量天花板 |
| R3 一致性与恢复 | 最小根选择、失效、视图版本、回执原子更新；读集/新增支持竞争、取消、独立读者、真实进程退出与原生未知提交测试通过 | 最小控制模型及原生故障窗口，不能替代正式跨存储协议 |
| R4 有效视图 | 两种策略的同一语义预期全部通过；延迟均达标，先用实时计算 | 实验领域、owner 扫描和人工预期；最终 M1 Schema 须重新评测 |
| R5 索引契约 | 逐项失败补写、FTS/ANN 标量过滤、图侧复核、补取和重开通过 | 合成向量；空间兼容由实验适配器检查，模型质量未测 |

正式配置为 [profile.json](profile.json)，完整结果及源码/配置哈希为 [RESULTS.json](RESULTS.json)。唯一采用的正式运行目录是 `.artifacts/spikes/local-dev-1000-v2-final/`；目录包含每项 stdout/stderr、result.json、冻结 profile 与环境。结构化汇总另存于本目录，避免依赖被忽略的原始产物。正式数据未混用前期探索结果。

环境：Apple M4 Pro、24 GiB 物理内存，macOS 15.1.1 arm64，Python 3.13.3；工作目录位于 `/dev/disk3s1`，报告时可用约 38 GiB。未记录 SSD 型号、独占 CPU 或清除 OS 页缓存，性能数值按单次暖环境结果解释。

- graphdb：`9d6e966ef0ae214cff8d5018efe1dafb37bc2a99`，libcxx-csv-fallback；zvec：`a4347e31f1b8e216cffba59285427a3b78551229`，未修改变体。实际 wheel SHA256 见结果环境字段。
- 用户确认负载：1,000 来源，每修订 8 个 1 KiB 片段，同时保留 current/candidate/retained 三版，384 维测试向量；每个隔离 worker RSS ≤ 2 GiB，查询 p95 ≤ 200 ms，构建与重开各 ≤ 120 s。
- 查询预热 5 次、采样 100 次，top_k=10，双读者共 200 次；p95 使用 nearest-rank。RSS 为进程 ru_maxrss，包含夹具和原生库，不能相加或当作 SDK 三存储组合内存。
- graphdb 显式配置 TP、`transaction_timeout_ms=1000`、`max_intermediate_rows=1000000`。这些是试验资源配置，不自动成为 SDK 默认值。

## 2. graphdb 契约与故障分层

实验发布协调器先核验根版本和支持集合版本，在一个短 Update 事务中提交根选择、关联失效、视图版本与 operation_id 回执。不使用 MERGE；稳定业务主键、事务和串行写协调可复用 Phase 2 幂等写法，见 [来源探针（归档）](../../python-baseline.zip) 与 [Phase 2 记录](../spike_1/PHASE2_REPORT.md)。回执保留到后续发布之后，原操作按自己的回执核对。

显式 Read 与 Update 可以并存：Python 可写连接的 `begin()` / `transaction()` 默认开启 **Update**，读者须执行 `BEGIN TRANSACTION READ ONLY`。探针还验证该事务拒绝写入。旧 Read 未释放时，另一个 cursor 完成 Update；旧读者看到原状态，新读者看到新状态。两个独立只读 cursor 在十次发布期间检查根与回执一致。目标规模的留存试验先建 84,100 节点，保留两个原生 Read，再构建 41,000 候选节点并从 r0 切换到 r1：旧读者仍见原节点数和 r0，当前读者见 125,100 节点和 r1。发布和构建未等待读者释放。

同目录进程打开矩阵：writer + writer 拒绝，writer + readonly 拒绝，readonly + writer 拒绝；无 writer 时两个 readonly 进程可同时持有。可写实例中共享 native 实例的 cursor 可并发读取；另以不同 read_only 配置打开同进程实例也被拒绝。只读写入被拒绝，目录内容摘要在只读检查前后不变。

| 证据层 | 实际执行 | 能说明什么 |
| --- | --- | --- |
| Python/SDK 适配注入 | Phase 2 提交响应丢失、事务失败、取消竞争，随后关闭/可写重开并按操作 ID 核对 | 验证 SDK 核对流程，不能等同原生内部故障 |
| 真正进程终止 | 子进程在 commit 前及实际 commit 后收到 SIGKILL，重开核验根与回执全无或全有；只读检查不改文件 | 覆盖 Python 边界，没有在任意 WAL syscall 中途杀进程 |
| 原生故障测试 | `graphdb_durable_memory_tests`、`graphdb_tp_wal_tests` 均实际运行；WAL append/footer/sync 和 checkpoint 边界注入失败/子进程退出 | `wal_after_sync` 故障真实抛出 CommitOutcomeUnknown，实例隔离后重开恢复已持久化结果；包含内部崩溃边界 |
| 未覆盖 | 物理断电、任意文件系统/损坏、每个 SDK 跨存储中断窗口 | 不能由上述测试推定通过，Phase 3/5 须补其协议边界 |

原生目标使用 Spike-0 相同源变体和构建缓存，以 14 并发编译两个测试目标及必要静态库，未重建或安装 wheel。结果保留测试源码哈希、执行命令及 stdout。原生控制测试与 Python 发布回执探针分别通过，尚未把原生内部注入与完整 SDK 发布回执组合成一次测试。

## 3. 目标规模结果

图工作负载采用实验性统一 KG 节点表与 LINK：来源 1,000、修订 3,000、片段/声明/证据各 24,000、支持 48,000、实体 1,000、更正 100；合计 125,100 节点、220,100 边。逻辑正文 24,576,000 字节。它不是最终 M1 物理 Schema；主要构建通过原生 CSV COPY，不能当作逐文档 SDK 端到端吞吐。

| 目标试验 | 峰值 RSS MiB | 构建 s | 只读重开 s | 查询 p95 ms | 原生目录 MiB |
| --- | --- | --- | --- | --- | --- |
| 图：125,100 节点 / 220,100 边 | 537.8 | 1.545 | 4.690 | 0.303；双读者 0.308 | 82.7 |
| 两个旧 Read + 候选构建/切换 | 542.7 | 1.535 | 4.626 | 语义一致性检查 | 82.7 |
| zvec：24,000 投影 / 384 维 | 319.9 | 1.869 | 0.011 | ANN 0.850；FTS 9.799 | 67.0 |

普通图规模试验构建时没有旧 Read，留存成本由单独的双读者候选试验覆盖；后者 RSS 包含后续重开。图有界查询从当前修订开始检查限定邻接路径；zvec 的 FTS 使用所有正文均命中的低选择性词，ANN 使用合成向量及固定版本过滤。图与索引的延迟没有包含真实模型、混合融合及完整证据封装。目录大小是可见文件合计，不能解释为物理写字节或总磁盘峰值。重开在同一 Python 进程中执行，OS 缓存温热，不是冷启动。

## 4. zvec 逐项状态、候选与空间

原生批次以重复主键制造真实部分失败，逐项结果 `[true,false,true]`，只对失败的稳定 ID upsert 后计数为 3。全文与向量均验证按 business_id、revision、build、space 下推过滤；flush 并释放引用后，只读重开结果一致。

故意放入 24 个更接近查询的旧候选时，top10/top20 图侧校验后均无有效命中，补取至 top40 得到 28 个候选，其中 1 个有效。该成本包含实际 graphdb 选择读取，表明过滤和补取是必需行为，不能保证每次补取都能补满。实际图选择在索引返回后改变时，旧命中仍被图侧拒绝。最终 SDK 查询必须基于固定只读图视图复核，不能照搬探针中用于制造变化的读取方式。

相同维度的不同空间由实验 wrapper 在查询前拒绝；zvec 本身不识别模型语义。正式提供者、索引适配器仍须实现空间指纹和校验。合成向量只测存取路径，不测 ANN 召回率、Embedding 或抽取质量。

## 5. 有效视图策略

两版使用相同原始图输入及独立手写预期：两个来源共同支持、删除任一/全部支持、同条件未决争议、不同限定条件共存、人工替代/撤销、独立人工支持跨来源删除保留、精确前提失效、Scope 不重判全库争议及人工元数据随目标返回。每策略每档位 10 项语义检查全部通过。

1,000 实体组包含 16,000 原始节点；物化版额外 1,000 View 节点。四轮更新每事务处理 100 组：

| 策略 | 查询 p50 / p95 ms | 四轮更新 s | 逻辑记录写入次数 | 峰值 RSS MiB | 目录 MiB |
| --- | --- | --- | --- | --- | --- |
| 实时 | 0.333 / 0.354 | 1.054 | 4,000 | 74.8 | 6.12 |
| 物化 | 0.120 / 0.129 | 15.789 | 8,000 | 104.5 | 9.39 |

选择：**以实时有效视图作为当前档位的起点**，保留以后物化的优化空间。两者均远低于 200 ms 阈值；物化读取更快，但本夹具额外视图维护使更新和空间成本上升。owner 条件没有二级索引，更新包含扫描，不能把差值归因为策略本身或推广为通用性能规律；逻辑写次数也不是 WAL/磁盘写放大。M1 的实际邻接模型、共享支持闭包、更大扇出或吞吐目标确定后须重新评测。

## 6. 探索失败与结论修正

- v1 默认中间行预算下，目标图 COUNT 报 `intermediate row budget exceeded`；失败保留在初始试验产物。v2 显式配置 1,000,000 行预算后，按相同规模、RSS、延迟和时间阈值全量重跑，未移动验收阈值。该配置是 Go 的适用条件。
- 最初的 held-reader 探针错把 Python `begin()` 的 Update 当作 Read，发生阻塞/超时；此前“旧 Read 阻塞 Update”的解释撤回。修正为显式 READ ONLY、加拒写断言后，全部正式试验重跑。相关 `contracts-initial`、`view-initial`、`reader-overlap-candidate` 为夹具用法错误，不能作为底座失败或成功证据。
- 一次控制夹具按字符串操作 ID 排序后取末项，误判最新回执；改为比较回执版本并重新运行。`view-v2-target/graph-contracts` 不用于正式判定。
- 较早的正式候选目录已被本报告唯一运行取代。最后仅修正 runner 超时诊断，避免从超时武断归因于并发契约；随后再次执行两套原生测试和完整 18 项，结果与源码哈希一致。

## 7. 复现与交付

在仓库根目录、已具备 Spike-0 固定环境和构建缓存时执行：

```bash
.venv/bin/python -m spikes.validation.native --build
.venv/bin/python -m spikes.validation.run --output .artifacts/spikes/new-validation-run
.venv/bin/python -m unittest discover -s tests -t . -v
.venv/bin/ruff check zknowledge tests spikes examples
.venv/bin/ruff format --check zknowledge tests spikes examples
git diff --check
```

输出须为新目录，避免重用旧数据。`native` 不带 `--build` 直接执行已构建测试；完整 runner 校验原生证据、固定依赖、profile 和运行前后源码哈希，任何失败、超时或缺项不能 Go。`--only` 可诊断单项，但不出阶段 A Go。原生证据默认位置为 `.artifacts/spikes/native/native-results.json`。新增可回归测试见 [集成契约测试（归档）](../../python-baseline.zip) 与 [有效视图/夹具测试（归档）](../../python-baseline.zip)。

最终检查：`unittest` 82 项通过、无跳过；全库 `ruff check` 通过；本轮 10 个探针/新增测试文件格式检查通过；`git diff --check` 通过。全库 `ruff format --check` 仍报告既有 `zknowledge/sources.py` 一处换行格式差异，本轮未修改该业务文件。回归日志位于 `.artifacts/spikes/regression/tests-final.log`。

README、总体设计 §5.1/§5.3/§5.7/§8 和交付计划 §3/§4 已回填；项目范围基线未改。三份文档写入前核对基准 SHA256，未发现并发改动。当前交付仍是 Phase 2 SDK 加 Spike，正式 Markdown、Embedding、候选索引与发布属于下一步 Phase 3；正式查询闭环属于 Phase 4。不提交、不推送远端。
