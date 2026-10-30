# 历史实现与验证

当前开发只使用 Rust。Python Phase 1–3 的源码、测试、示例、Spike、配置和依赖版本整体归档；旧结果保留其适用版本，不代表 Rust 已完成真实摄入、发布或恢复。

- [Python 退役验收](RETIREMENT.md)
- [138 项迁移矩阵](migration/rust-test-matrix.json) 与 [原迁入来源清单](migration/source-manifest.json)：其中 Python 路径指归档中的原始位置
- [Rust 组件清理报告](migration/RUST_CONSOLIDATION_REPORT.md) 与 [当时机器记录](migration/rust-consolidation-validation.json)
- [早期迁入/R0 验证归档](archive/migration-history.zip)：7 份原始报告/JSON，内含摘要清单
- [Python Spike-0](archive/python-evidence/spike_0/SPIKE0_REPORT.md)、[Phase 2](archive/python-evidence/spike_1/PHASE2_REPORT.md)、[底座实测](archive/python-evidence/validation/BASE_REPORT.md)
- [Python Phase 3 契约](archive/python-evidence/phase3/CONTRACTS.md) 与 [验收](archive/python-evidence/phase3/PHASE3_REPORT.md)
- [原 Python Phase 3 计划](archive/knowledge-phase3-plan.md)

## 历史计划与长期设计

2026-10-28 整理：已完成或退出当前主线的计划放入 archive/plans，原路径保留跳转；归档只调整位置和链接，不更新原有结论。当前任务从 [计划索引](README.md#当前计划) 进入。

| 文档 | 保留用途 |
| --- | --- |
| [独立 SDK 交付计划](archive/plans/knowledge-delivery-plan.md) | 旧 R0/R1、Spike 与 Phase/Milestone 安排；不作为当前任务清单 |
| [模块收敛计划](archive/plans/MODULE_CONSOLIDATION_PLAN.md) | 已实施的调整步骤；结果见 [收敛报告](MODULE_CONSOLIDATION_REPORT.md) |
| [Python Phase 3 计划](archive/knowledge-phase3-plan.md) | 冻结 Python 构建路径的历史约定 |
| [通用项目边界](knowledge-project-boundary.md)与[总体设计](knowledge-system-design.md) | 独立 SDK 的长期设计参考；其中的模块和接口不作为当前 zg 的实现约束 |

## Python 基线恢复

[python-baseline.zip](archive/python-baseline.zip) 保留 84 个原始文件，内含逐文件摘要、依赖 gitlink 清单及 `RESTORE.md`。解压到独立临时目录，以 `RESTORE.md` 为准准备环境；不要解压覆盖当前开发树。当前仓库不保留可直接运行的 Python SDK、测试、Spike、打包入口或虚拟环境。

原生 wheel 与构建回执单独保留在本机 `knowledge/.artifacts/spike-0/`，归档内记录其 SHA-256；需复制到恢复目录相同位置。这些 wheel 仅适用于对应 macOS arm64 / CPython 3.13 组合。新机器准备纯 Python 依赖可能需要包索引或另备缓存；未验证新机离线重建。

退役前已将归档解压到临时目录，确认导入恢复目录源码，并用原冻结解释器/环境实际运行 138 项测试通过、无跳过。随后才删除原活动目录和 `.venv`。这验证了源码归档与原生集成可复验，不是重新创建环境的验收。

Python 基线归档 SHA-256：`14973e013285f0ee5836a74e1d907f92e238eba086c94fc0c539693aeb16dc84`。
早期迁入归档 SHA-256：`5a931335fa8b1b8c97f4feba651379680e9f1cb0092fed0a02fb5c1ade3a3893`。
