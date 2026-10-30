# Rust 组件与迁入文件清理验收

> 本页记录当时的组件测试结果。后续 Python 退役回归发现向量只读测试在沙箱内失败、沙箱外通过；当前环境边界、归档与新回归结果见 [退役验收](../RETIREMENT.md)，本页历史通过数不代表当前门禁通过。

2026-10-22。范围：不依赖 binding 的组件整理、共享底层工具和历史文件清理。不是完整 Rust 重写验收。原 R0 报告和原始机器记录保留在 [历史归档](../archive/migration-history.zip)；本轮结果单独记录在 [机器记录](rust-consolidation-validation.json)。

## 实现调整

| 项目 | 本轮结果 |
| --- | --- |
| 共用存储工具 | 新增 [zg-storage](../../rust/crates/zg-storage/README.md)。engine 与 KG 共用 SHA-256、原子元数据写入/目录同步、collection 打开/创建、标量字段、批量 fetch、写响应计数校验；各自保留 Schema、错误映射和所有权规则 |
| 快照与投影 | KG 的不可覆盖快照安装、原文位置/身份、向量空间、候选稳定 ID 与逐项状态保留。移除只转调 `write_batch` 的 `fill_missing`，补写统一使用 `write_batch` |
| 取消 | 直接使用宿主的 `CancellationToken`。每个任务使用子 token；父取消向下传播，单任务取消不影响父或兄弟。保留有界 worker、准入、排空和提交结果优先规则 |
| 公开接口 | 删除未接入服务的来源/修订/回执/分页等占位记录、未使用格式常量；真实处理与证据类型保留。尚未形成实际持久化契约，不提前冻结公开 API |
| 测试支撑 | 确定性 Embedding 移到 `tests/support`，组件示例显式使用该测试代码；预计算入口保留在运行库。尝试/回执状态规则仅编译进单元测试。删除只验证自身的图内存替身；内部 GraphStore 端口保留 |
| 默认构建 | `zg → engine → zg-storage` 已通过 release 检查。`zg-knowledge` 仍为 workspace 组件，不加入生产 zg 依赖图，不扩展默认搜索行为 |

解析和分段没有强行合并：KG 使用快照原始 UTF-8 字节及完整覆盖，宿主 chunker 使用不同的位置与预算语义。未加入模型运行时，也未新增图数据库后端。

## 文件与文档

- 入口简化为 [knowledge README](../README.md)，保留总体设计、范围和交付计划；新增 [历史索引](../HISTORY.md)。
- Python 运行说明归档到 [PYTHON_BASELINE.md](../archive/PYTHON_BASELINE.md)，原 Python Phase 3 计划归档，原链接保留短跳转，历史报告内容及摘要不改写。
- 删除 5 个 `.artifacts/migration/sdk-crash/*/kb` 生成数据库：文件逻辑大小 **49,015,919 字节（约 46.7 MiB）**，文件分配块合计 49,975,296 字节。该统计不是文件系统实际回收空间的保证。
- 删除 12 个 Python/工具缓存目录。21 份同目录试验结果、fixture、核对 JSON、日志的 SHA-256 在清理前后保持一致；详见机器记录的逐路径清单。
- 保留冻结 Python SDK、测试、示例、Spike、`.venv`、wheel、构建回执、六项未初始化的依赖 gitlink。对迁入清单中的 **77 个源码及支撑文件**复核，无差异。不修改原独立仓库或已有知识库。

## 验证

| 检查 | 实际结果 |
| --- | --- |
| KG + 共享存储测试 | **31 通过，0 失败，0 忽略**；含真实 zvec 写入、逐项失败、补写、空间校验、只读及重开 |
| engine `utils::` | **22 通过**；原不可覆盖竞争测试与摘要测试移入共享 crate 继续运行 |
| engine `storage::` | **77 通过，2 原有忽略**；忽略项为 macOS zvec array_take 缺失路径和手工性能比较 |
| 冻结 Python 回归 | **138 通过**；SDK 与测试源码未改 |
| 组件示例 | 快照→分段→测试向量→候选写入→重开成功；明确 `published=false search_ready=false graph_backend=not_configured` |
| 默认 zg 构建 | `cargo check --locked --offline --release -p zg -j14` 通过 |
| Clippy / 格式 | 三个变更 crate 的 all-targets Clippy `-D warnings`、rustfmt 通过 |
| 依赖与差异 | 仅一套 workspace zvec 绑定；原锁文件中已有包未移除或升级；`git diff --check` 通过 |

使用本机 Rust 1.98.0、macOS arm64、14 并发及已有宿主 zvec 原生库；没有据 Rust wrapper 版本推断 C++ 提交。release 检查有工具链 `rust-objcopy` 找不到 libLLVM 的非致命剥离告警，相关命令退出码均为 0。完整 workspace 门禁和其他平台未在本轮执行。

原始命令日志保存在忽略目录 `.artifacts/migration/rust-consolidation/`，命令结果、日志摘要、原生库摘要、当前源码摘要及清理证据见机器记录。原 R0 的 29 项与本轮 31 项范围不同：删除弱占位/替身自测、迁移测试位置，并加入共享响应校验与宿主取消传播验证，不能据总数比较能力覆盖。

## 保留的接入门

真实来源注册、修订/回执持久化、图侧原子发布、旧读视图和未知提交恢复仍等待 [graphdb Rust binding](../GRAPHDB_RUST_BINDING_REQUIREMENTS.md)。没有对这些能力作成功声明。`ProjectionIndex::fetch` 仍只检查候选；没有公开知识检索，也没有合并 zg/KG 正文或向量存储。

未增加 Windows 支持，未提交或推送。完成真实行为等价验收前，Python 基线和相关源码依赖登记继续保留。

## 后续记录整理

2026-10-22：`migration/` 精简为迁移矩阵、来源清单、本报告和本轮机器记录 4 份文件。早期 7 份报告/JSON 按原路径、原字节合并到 `archive/migration-history.zip`，内含摘要清单，解压可复核。只更新文档引用；`spikes/`、Python/Rust 源码、测试与现有验收结果不变。本次仅核对归档、链接、源码摘要和差异格式，未重新运行测试。
