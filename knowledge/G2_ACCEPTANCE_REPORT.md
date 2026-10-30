# G2 总验收：真实图构建、维护、选择与核对（带残留）

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-28。本文汇总 G2.0–G2.4 的实现、故障证据和本地 L1 Release 测量。阶段报告保留各自源码快照与历史测试数，不改写为本次结果。

**结论限定为：真实 graphdb 图产物已能在 zg 宿主中持久化构建、维护、选择与核对，G2.4 带明确残留收尾。** 本次 L1 的文档域为 `ready`，代码域为 `not_ready(incomplete_coverage)`；多项性能阈值未达标，HEAD fsync 与两个原生事务中途窗口未直接验证。没有公开 relations、图搜索扩展或 CLI/MCP 图命令，也没有搜索质量提升结论。

## 1. 证据、环境与语料

- 阶段证据：[G2.0 原生接入](G2_0_ACCEPTANCE_REPORT.md)、[G2.1 事实持久化](G2_1_ACCEPTANCE_REPORT.md)、[G2.2 宿主发布](G2_2_HOST_PUBLICATION_REPORT.md)、[图根/回执](G2_2_BUILD_COMMIT_REPORT.md)、[分批恢复](G2_2_BATCH_RECOVERY_REPORT.md)、[原生控制](G2_2_NATIVE_CONTROL_REPORT.md)、[G2.3 恢复与租约](G2_3_RECOVERY_LEASES_REPORT.md)、[保留与清理](G2_3_RETENTION_REPORT.md)、[故障窗口](G2_3_FAILURE_WINDOWS_REPORT.md)。
- 本次原始数据：[resources.json](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/resources.json")、[verification.json](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/verification.json")、[逐文件冻结清单](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/corpus-manifest.json")及同目录 `raw/`。
- 环境：Apple M4 Pro、24 GiB 内存、macOS arm64、内部 APFS SSD；Rust 1.98.0，Release，编译并发 14，Cargo 串行。
- graphdb 固定为 `3bf0d532ce928c02782c76d42cc289b1e49e8df4`；zvec 使用 `/private/tmp/zg-ann-fix/native-baseline/lib/libzvec_c_api.dylib`，SHA-256 为 `50e243d8a941419c30bf11b4e5b4e8852275584109d647d86b3fa4129d805912`。
- 模型固定 `local/potion-code-16m-v2`、CPU、禁止远程授权。口径为**冷进程、热模型文件缓存**，不包含冷下载。
- 冻结清单为 1,018 个文件、9,923,500 字节；其中代码 356 个、文档 157 个。宿主实际扫描/索引 1,004 个文件，不能与冻结清单分母混用。清单 SHA-256 为 `ddfa6dc45afeff7cb575ef046d8514eade1b8febbd9f8a6a4aa894ae1a2aa160`。
- 每个场景使用独立临时副本和新进程，至少三轮。RSS 约 10 ms、工作区逻辑字节约 50 ms 采样；峰值均为 observed sampled peak，不是硬峰值，磁盘字节不是已分配块。

复现入口：

```bash
python3 rust/benchmarks/graph-build/runner.py --repeats 3
```

## 2. 分域结果与维护正确性

联合全量构建的三轮中位数如下。覆盖是完成事实绑定的文件数，不是百分比。

| 域 | 状态 | 覆盖文件 | 节点 | 引用 |
| --- | --- | ---: | ---: | ---: |
| 代码 | `not_ready(incomplete_coverage)` | 353 / 356 | 6,069 | 56,449 |
| 文档 | `ready` | 157 / 157 | 922 | 757 |

2026-10-28 交接复核补充：上表代码分母 356 是外层冻结清单数，宿主实际必需代码来源为 355；`.github/scripts/readme-demo.py` 被默认 `hidden=false` 排除。原配置另漏配两个 Rust 基准文件的模块根，实际为 353／355 有事实，不能将外层差额解释为三个提取失败。新交接按逐来源记录并检查事实完整性，历史数字不代表全部来源已完整解析；详见 [交接记录](G2_G3_HANDOFF_REPORT.md)。本表保留原测量口径，原始日志不改写。

代码域未因文档域成功而被标记为就绪。联合构建、graph-only 和维护场景均要求候选 committed、HEAD selected 且二者 build ID 一致；所有场景 `files_failed=0`。unchanged 复用原 build ID 且不新建候选；单文件更新推进文本修订和目录摘要；目标新增/删除改变目录与所选 HEAD。三轮 graph-only 均为 `mode=graph_only`、`text_published=false`、零模型事件、零下载。

## 3. L1 资源实测

时间使用示例外层完整墙钟，不用 `IndexResult.duration_micros` 的文本内部耗时代替图维护成本。RSS 和工作区增量均为三轮观测值。

| 场景 | 时间中位数 / 最大值（秒） | observed sampled RSS 中位数（B） | 工作区观测增量中位数（B） |
| --- | ---: | ---: | ---: |
| 文本全量 | 10.067 / 11.025 | 782,729,216 | 147,235,833 |
| 文本＋图全量 | 198.546 / 200.108 | 1,164,935,168 | 261,350,773 |
| 首次 graph-only | 182.453 / 184.913 | 1,093,746,688 | 152,317,167 |
| unchanged | 8.375 / 8.390 | 1,089,126,400 | 111,264 |
| 单 Rust 文件更新 | 195.482 / 199.935 | 1,191,641,088 | 157,881,999 |
| 单 Markdown 文件更新 | 197.223 / 204.141 | 1,205,796,864 | 157,857,041 |
| 目标新增 | 189.302 / 190.206 | 1,328,758,784 | 157,860,430 |
| 目标删除 | 192.430 / 196.008 | 1,290,780,672 | 152,375,096 |

联合构建完成后的当前图产物中位数为 52,145,842 B，zvec 产物中位数为 109,031,923 B。维护场景新旧图并存的最高观测值为 204,461,094 B；逐样本还分别记录未选候选、attempt/discard 元数据、HEAD 和活动 generation。成功联合样本没有遗留未选候选，不能据此忽略构建期间的并存成本。

目录摘要探针直接复用私有 `catalog_digest`，输入 1,018 条真实冻结 `FileRecord`。三次耗时为 243、177、178 µs，中位数 178 µs、最大 243 µs，摘要完全一致。探针进程过短，RSS 采样未形成可靠峰值（中位数为 0，仅一次观测到 16,777,216 B），因此不对该探针作 RSS 结论。

## 4. §10.2 阈值判定

| 测量项 | 目标 | 实测 | 判定 |
| --- | --- | --- | --- |
| graph-only 时间 | ≤ 文本全量 20% | 18.1246 倍（1,812.46%） | **失败** |
| 联合减文本的图部分差值 | ≤ 文本全量 30% | 18.7233 倍（1,872.33%） | **失败** |
| graph-only 模型路径 | 零模型加载/下载 | 三轮均零模型事件、零下载 | 通过 |
| 单文件图维护 | ≤ 500 ms @L1 | Rust 195.482 s；Markdown 197.223 s | **失败** |
| 当前图 / zvec 磁盘 | ≤ 50% | 47.826% | 通过 |
| 新旧图并存峰值 | ≤ 当前图 ×2.1 | 3.9212 倍 | **失败** |
| 联合 / 文本峰值 RSS | ≤ 1.5 倍 | 1.4883 倍 | 通过 |
| 目录摘要 @约 1k | ≤ 100 ms | 中位数 0.178 ms | 通过 |
| 目录摘要 @10k / 100k | ≤ 1 s / 记录成本 | 未运行 | 未运行 |

**候选重建模式代价过高。** graph-only 已远超文本全量的 100%，且新旧图并存观测峰值超过图产物三倍，两条既定风险规则均被触发。图内原地增量修改因此列为 G3 前置风险；本轮不实现性能重构，也不以阈值未达标否定已经通过的提交、选择和维护正确性断言。

## 5. G2-A01–A20 逐项验收

“通过”仅指对应行为和已有证据；带残留或未达标项不外推为完整故障门或性能 Go。

| 编号 | 判定 | 源码与测试证据、边界 |
| --- | --- | --- |
| G2-A01 | 通过（限定） | G2.0 §§1–6：`graphdb_binding`、`graphdb_schema` 真实双原生写读重开；独立无软链源码包复现。未执行全新递归 Git 检出。 |
| G2-A02 | 通过 | G2.0 §4 与 `zg-knowledge/src/graph/tests.rs`：参数/结果、回滚、旧读视图、新只读确认和只读拒绝写。 |
| G2-A03 | 通过 | G2.1 §§1–2：`extraction/code.rs`、`graph_facts/binding.rs` 同次消费实际 ChunkOptions/AST/字节和 EntityId；不同分段与 Embedding 计数测试。 |
| G2-A04 | 通过 | G2.1 §2 与 `graph_binding`/`graph_facts`：中文、BOM、CRLF、latin-1、范围、未覆盖证据及原生路径诊断。 |
| G2-A05 | 通过 | G2.1 失败用例、G2.2 写前栅栏与 G2.3 `text_recovery.rs`：准备/Embedding/文本写失败不保留有效旧边，来源变化不混版。 |
| G2-A06 | 通过 | G2.1 `module_roots.rs`、代码/Markdown 规则测试：namespace/包根隔离、不支持布局、分域和链接目标诊断。 |
| G2-A07 | 通过 | G2.1 §§2、6 与 `graph/tests.rs`：八表及 scopes/bindings/Reference.scope/诊断/实体摘要重开往返，复用后重消歧与完整计算一致，重复标题保持歧义。 |
| G2-A08 | 通过 | G2.1 §6 真实重开变更测试；本次 L1 目标新增/删除三轮核对目录摘要、文本修订与 HEAD 变化。 |
| G2-A09 | 通过 | G2.2 分批/恢复、G2.3 固定身份 Resume；本次 unchanged 三轮复用 build ID 且不新建候选。失败/过期 attempt 由恢复测试拒绝。 |
| G2-A10 | 通过 | G2.2 宿主模型前分流测试与本次 graph-only 三轮：无模型获取、授权、Embedding 或下载；文本变化走正常路径，不兼容描述明确拒绝。 |
| G2-A11 | 通过 | G2.1 Schema 校验、G2.2 `build.rs` 与 `build_root_*`：边端点/目标版本、失败不选中、根与不可变回执同事务、新只读确认。 |
| G2-A12 | 通过 | G2.2 宿主发布、`graph_state/tests.rs` CAS：全量 manifest 联合选择、增量 HEAD 选择、基准竞争不覆盖。 |
| G2-A13 | 通过 | `storage/store/tests.rs` 与默认 feature 图测试：active 文本写前持久化栅栏，无 graphdb 二进制仍可失效、Off 和保护未知候选。 |
| G2-A14 | 通过 | G2.3 恢复/租约测试：完整目录、实体和 generation 核验拒绝旧二进制修改、负解析目标及半替换；预算不足只报未就绪。 |
| G2-A15 | 通过（带残留） | G2.3 故障报告：提交前后取消、Unknown 隔离、manifest 切换后响应丢失与已成功选择效果。R1/R2 原生事务中途窗口未运行。 |
| G2-A16 | 通过（带残留） | G2.2/G2.3 恢复测试：DDL 前诊断、Unknown 不追加、确认失败不改回执、pending 重开核对；只读不修复、不盲重试/删除。 |
| G2-A17 | 通过 | G2.3 租约与保留测试：旧读者/新构建共存、来源复核、活动句柄阻止删除、清理失败不撤销成功。 |
| G2-A18 | 通过 | G2.2 状态类型与本次 L1：Off/失败/格式/来源证据各有分域状态；实际代码 `incomplete_coverage`、文档 `ready` 分开返回。 |
| G2-A19 | 通过（性能未达标） | G2.3 `resources.rs`、分批/保留测试及本次资源记录：队列、批次、行数、候选、新旧图和隔离保留受限；时间与并存磁盘阈值失败见 §4。 |
| G2-A20 | 通过 | G2.2/G2.3 daemon、watcher 与协议测试：None 继承、不同策略串行、Off、重启继承和能力检查；普通索引/搜索与默认构建参加回归。 |

## 6. 故障证据与未运行项

| 类型 | 已有直接证据 | 不能外推的结论 |
| --- | --- | --- |
| 真实原生 | 活跃导入取消/期限回滚、根/回执写前取消与失败回滚、提交后新只读确认、提交完成后进程退出重开 | 未在 root/receipt 原生事务写入中途 SIGKILL，未在原生 commit 中途注入取消/期限 |
| 宿主/文件系统注入 | dirty STATE、HEAD/manifest 竞争、manifest 写失败、清理部分失败、恢复与固定身份重放 | HEAD rename 后及父目录 fsync 中/后的直接故障窗口只部分覆盖 |
| 适配层响应丢失 | 已提交后 Unknown 隔离并重开找回回执；真实 manifest 切换后响应丢失经 Inspect/Reconcile 收敛 | 不冒充原生中途丢响应或断电半状态 |

明确未运行：R1 原生 root/receipt 事务中途 SIGKILL、R2 原生 commit 中途取消/期限、HEAD fsync 直接故障窗口、L2 外部冻结仓库、L3 合成语料、目录摘要 10k/100k、冷模型下载、其他 OS/架构、断电和其他原生版本。

## 7. 最终回归与静态核验

以下结果均来自当前工作区的实际命令，Cargo 命令串行执行，测试使用 `--test-threads=1`，编译并发为 14；最终退出码是修正 lint/格式问题后的重跑结果，不用历史通过数替代。

| 检查 | 命令与范围 | 退出码 | 实际结果与原始日志 |
| --- | --- | ---: | --- |
| 真实原生五包回归 | `cargo test --locked --offline -p zg-engine -p zg-knowledge -p zg-storage -p zg-daemon -p zg-daemon-protocol --features zg-engine/knowledge-graph-graphdb,zg-storage/zvec --all-targets -j 14 -- --test-threads=1` | 0 | **691 passed / 0 failed / 10 ignored**；engine 543、knowledge 57、storage 6、daemon 79、daemon-protocol 6；[日志](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/final-five-package.log") |
| 默认 feature engine 图测试 | `cargo test --locked --offline -p zg-engine --no-default-features --lib graph_ -j 14 -- --test-threads=1` | 0 | **17 passed / 0 failed / 0 ignored**，444 filtered out；[日志](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/final-default-engine-graph.log") |
| 默认 feature knowledge 测试 | `cargo test --locked --offline -p zg-knowledge --no-default-features -j 14 -- --test-threads=1` | 0 | **19 passed / 0 failed / 0 ignored**，含 1 项 doctest；graphdb 门控目标为 0 项，未计入通过数；[日志](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/final-default-knowledge.log") |
| 默认配置 Clippy | Rust 1.98.0 `cargo-clippy clippy --locked --offline -p zg-engine -p zg-daemon --all-targets -j 14 -- -D warnings` | 0 | engine、daemon 及其依赖目标无警告；[日志](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/final-clippy-default.log") |
| 图 feature 五包 Clippy | 与真实原生五包相同的五个包及 feature，`--all-targets -j 14 -- -D warnings` | 0 | 五包及示例、测试目标无警告；[日志](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/final-clippy-graph.log") |
| workspace rustfmt | Rust 1.98.0 `cargo-fmt fmt --all -- --check` | 0 | 全 workspace 格式检查通过；[日志](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/final-fmt.log") |
| 最终 diff 检查 | `git diff --check` | 0 | 无空白错误；[日志](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/final-diff-check.log") |

原生复核命令退出码均为 0：`rustc 1.98.0 (88d9e12ae 2026-08-18)`；graphdb HEAD 为 `3bf0d532ce928c02782c76d42cc289b1e49e8df4`；`libzvec_c_api.dylib` SHA-256 为 `50e243d8a941419c30bf11b4e5b4e8852275584109d647d86b3fa4129d805912`。使用 `DYLD_PRINT_LIBRARIES=1` 启动本次五包回归的 knowledge 测试二进制并执行 `--list`，实际加载路径为 `/private/tmp/zg-ann-fix/native-baseline/lib/libzvec_c_api.dylib`，进程退出码为 0；见[原生摘要](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/final-native.log")与[dyld 原始记录](archive/pre-g4-validation/README.md#g2-4-resources "包内：knowledge/reports/g2.4-resources/final-dyld.log")。

G2 到此带残留收尾。本轮未进入 G3 公开查询/融合或 G4 质量对照，后续状态以各阶段报告为准；候选重建成本和代码域覆盖不足必须在后续阶段入口前显式处理，不能由本报告包装为查询可用或质量收益。
