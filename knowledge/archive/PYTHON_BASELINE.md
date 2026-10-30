# 已退役 Python 基线

Python Phase 1–3 源码、测试、示例、Spike 和构建配置已从活动目录移除。完整材料和本页原始运行说明保存在 [Python 基线归档](python-baseline.zip)；恢复流程见 [历史索引](../HISTORY.md#python-基线恢复)。

归档中的 `RESTORE.md` 说明环境准备、原生 wheel 来源和复验命令。需要先解压到独立目录，再执行其中的 Python 命令。原生 wheel 与构建回执单独保留在本机忽略目录 `knowledge/.artifacts/spike-0/`，不属于源码归档或通用平台分发。

当前开发入口见 [Rust 组件](../../rust/crates/zg-knowledge/README.md)。历史 Python Go 与测试结果不表示 Rust 已完成行为等价。
