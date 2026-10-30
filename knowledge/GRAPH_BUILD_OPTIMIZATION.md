# 全量图构建性能优化进度

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-30。状态：P0 typed batch 已完成并通过本地正确性回归；AP 初始装载未实施，后续转为完整读回路径优化。本文是当前进度与下一步计划，不改写 G2.4 的历史验收数据。

## 1. 范围与不变量

本轮只优化新鲜全量候选构建。单文件变化仍重建完整候选，不实现 facts diff、依赖闭包、已发布图原地 delta、upsert、搜索、CLI/MCP 或 G3/G4 产品能力。

P0 保留 TP、声明式 schema、FactCatalog/FactRows、ImportPlan、逻辑 mutation 顺序、plan/prefix digest、默认 256 mutations/256 KiB 预算、断点恢复、完整语义读回、Metadata、Root/Receipt、checkpoint 和 HEAD CAS。candidate committed、host selected、search ready 仍是三个独立状态。

## 2. P0 已完成内容

- 节点复用现有 `table!` 行声明生成 typed columns，通过 `Transaction::import_batches` 写入，不为各表维护第二套 schema。
- 边复用 `RELATIONS` 分组，通过 `EndpointMapping` 按业务主键批量解析端点，移除逐边 `MATCH + CREATE` 热路径。
- 保留 resolved reference 的 `ref_id/data`、合法平行边和 append 语义；重复主键、缺失端点、类型错误或导入失败回滚整个逻辑批次。
- native batch 按最多 65,536 行切分，但不改变 ImportPlan 的逻辑批次、进度或恢复分区。
- 新增逐行/批量完整事实一致性、平行边及失败原子性测试。

主要实现位于 `rust/crates/zg-knowledge/src/graph/{schema.rs,codec.rs,import.rs,import_tests.rs}`；benchmark 入口为 `rust/benchmarks/graph-build/runner.py`。

## 3. 正确性与静态检查

| 检查 | 结果 |
| --- | --- |
| 批量导入专项测试 | 7 passed / 0 failed |
| 默认 feature `zg-knowledge --all-targets` | 33 passed / 0 failed |
| 图 feature 五包 `--all-targets` 串行回归 | 777 passed / 0 failed / 10 ignored |
| 默认配置 Clippy `-D warnings` | 退出码 0 |
| 图 feature 五包 Clippy `-D warnings` | 退出码 0 |
| workspace rustfmt check | 退出码 0 |
| `git diff --check` | 退出码 0 |

五包为 zg-engine、zg-knowledge、zg-storage、zg-daemon、zg-daemon-protocol。测量使用相同 graphdb HEAD `3bf0d532ce928c02782c76d42cc289b1e49e8df4` 和 zvec 库 SHA-256 `50e243d8a941419c30bf11b4e5b4e8852275584109d647d86b3fa4129d805912`；子模块工作区内容是否冻结不能仅由 HEAD 推断。

## 4. L1 Release 性能结果

环境为 Apple M4 Pro、24 GiB 内存、Rust 1.98.0、冷进程和热模型文件缓存。每个场景三轮，时间为示例外层完整墙钟，RSS 为 observed sampled peak。

| 场景 | P0 中位耗时（秒） | 最大耗时（秒） |
| --- | ---: | ---: |
| text_full | 13.868 | 14.509 |
| text_and_graph | 143.089 | 143.417 |
| graph_only | 104.410 | 105.116 |
| unchanged | 47.511 | 47.689 |
| update_rust | 154.148 | 157.585 |
| update_markdown | 149.975 | 150.539 |

graph-only 逐行 TP 基线的中位/最大耗时为 205.055/208.559 秒，P0 降至 104.410/105.116 秒，中位耗时下降 49.1%。observed peak RSS 中位数从 1,022,066,688 B 降至 997,146,624 B。profile 中 `import.batch_write` 从约 106.6 秒降至约 8.6 秒，剩余主要成本是 `import.inspect` 与 `graph.commit_build::graph.read_facts` 两次完整语义读回，各约 22 秒。

### 门禁判定

| 项目 | 目标 | P0 实测 | 判定 |
| --- | --- | --- | --- |
| graph-only / text-full | ≤20% | 7.5289 倍 | 未通过 |
| 联合构建相对文本增量 | ≤30% | 9.3180 倍 | 未通过 |
| 联合/文本 observed peak RSS | ≤1.5 倍 | 1.5551 倍 | 未通过 |
| graph-only 模型事件与下载 | 均为 0 | 三轮均为 0 | 通过 |
| 单文件完整重建 | ≤500 ms | Rust 154.148 s；Markdown 149.975 s | 未通过 |
| 当前图/zvec 磁盘 | ≤50% | 42.03% | 通过 |
| 新旧图并存峰值 | ≤当前图 2.1 倍 | 4.0047 倍 | 未通过 |
| 目录摘要 @约 1k | ≤100 ms | 中位 0.214 ms | 通过 |

P0 可独立交付的是写入机制与上述实测收益，不代表整体性能目标已经达成，也不代表全量重建已适合高频单文件维护。

### 对照口径限制

逐行基线与 P0 使用同机、相同原生库和相同 L1 选择策略，但语料并非逐字节一致：文件数从 1,288 增至 1,289，新增 `knowledge/reports/g4/baseline.json`（原件已迁入 [G4 归档](archive/g4-validation/README.md)），同时包含本次源码变化。两次 manifest SHA-256 分别为 `86784249f697bb4b3ccb34ac2a613db9fa97a3bc6e3807b2ea27044cb72d2f9c` 和 `9c7864dd37247da37fde9ffc3e9d47207f73b542a9a03f8eda0bdca6bbbfe798`。因此该结果是同策略测量对照，不宣称严格同语料 A/B；后续优化必须先冻结同一语料。

## 5. 后续计划

1. 冻结同一份语料、源码和原生制品，排除报告目录反向污染 inventory，保存 manifest 后再做对照。
2. 拆解 `import.inspect` 与 `graph.commit_build::graph.read_facts`，评估复用同一只读快照或流式核验结果，减少重复扫描和中间事实复制。
3. 不直接删除最终核验；完整 FactCatalog、FactRows、Metadata、Root/Receipt、StoreCounts、digest 和 fail-closed 语义必须保持一致。
4. 次要分析 `import.write` 自身约 17 秒开销，区分逻辑批次推进、分组和进度确认成本。
5. 优化后重跑三轮 graph-only、六场景 Release 测量及受影响正确性回归，不放宽现有性能门禁。
6. 只有 profile 再次证明主要瓶颈是原生写入、逐行业务键解析或事务提交时，才重新评估声明式 AP 初始装载；当前不实施 AP P1/P2。

原始证据已归档，包内路径为 `knowledge/reports/graph-build-optimization/`。冻结提交以本文的摘要和复现入口为准；大体量 raw 运行输出、临时 workspace 和本机构建产物不作为产品源码提交。
