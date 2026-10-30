# Python 基线退役验收

2026-10-22。Python Phase 1–3 从活动开发树退出，当前开发使用 Rust。退役与 Rust 行为等价验收分开：真实来源摄入、图侧发布和恢复仍待 graphdb binding 接入。

## 归档与清理

- [python-baseline.zip](archive/python-baseline.zip) 保存 **84 个原文件**，包含 SDK、测试、示例、Spike、打包配置、原运行说明和旧计划；另有摘要清单、六项 gitlink 元数据及 `RESTORE.md`。大小 233,457 字节（约 228 KiB）。
- SHA-256：`14973e013285f0ee5836a74e1d907f92e238eba086c94fc0c539693aeb16dc84`。已校验压缩包和逐文件摘要，解压后再次核对。
- 删除活动 `zknowledge/`、`tests/`、`examples/`、`spikes/`、`pyproject.toml`、旧迁入运行入口与 `.venv`。被删除虚拟环境的普通文件逻辑大小约 117.7 MiB；这不是文件系统实际回收空间的测量。
- 移除仅为旧 KG 新增暂存的六项依赖 gitlink 和 `.gitmodules`。这些目录均未初始化、内容为空。宿主的 Rust zvec、Tree-sitter、模型依赖与运行源码未改动。
- 本机原生 wheel、构建回执和旧验证日志保留在忽略目录 `.artifacts/`；它们不在源码归档中，也不是跨平台分发。关键验收文档保留为 [可阅读副本](HISTORY.md)，原始字节仍在归档内。
- [138 项迁移矩阵](migration/rust-test-matrix.json) 保留所有原测试标识和状态，并补充归档定位。原独立 `z-knowledge-graph` 仓库未改动。

## 恢复检查

归档解压到独立临时目录，确认实际导入恢复目录的 `zknowledge`；复制原构建回执后，使用退役前的冻结解释器及其已安装依赖运行 **138 项测试，全部通过，无跳过，38.075 秒**。完成后才删除活动源码和虚拟环境。

本次验证恢复源码与已有原生环境的组合。没有重新创建 Python 环境，没有验证新机器离线安装；纯 Python 依赖可能仍需包索引或另备缓存。恢复命令和平台限制见 [历史索引](HISTORY.md#python-基线恢复) 与归档中的 `RESTORE.md`。

## Rust 与宿主回归

已通过 workspace 格式检查、10 个包的 all-targets 检查、workspace Clippy `-D warnings`、metadata、Node 打包测试 7 项、ESLint 配置语法与 `git diff --check`。`rust/scripts/check.sh` 的逐包检查加入 `zg-storage` 和 `zg-knowledge`。

完整 workspace 在解除测试所需的本地 socket/子进程沙箱限制后，执行 `cargo test --locked --workspace --no-fail-fast`：**728 项通过，0 失败，10 项原有忽略**。忽略项包括需要模型资产的真实推理、手工刷新/性能测试、缺少 array_take 的已知原生路径，完整名称见机器记录。没有新增忽略项。

只读向量用例存在环境差异：在独立构建目录的沙箱内运行 KG+共享存储验证，累计 **30 项通过、1 项失败**；失败项在沙箱内单独复核仍失败。只读打开前后只有原 `embedding.index.3.proxima` 文件摘要发生变化。随后保持同一构建目录、库和用例参数，解除沙箱后复核 **1 项通过**；完整 workspace 中该用例也通过。原生库文件摘要与之前组件验收一致，Rust 运行代码和测试断言未变。当前证据不足以确定沙箱机制或其他上下文因素的具体原因，不将其泛化为所有环境下的保证；R1 只读/重开探针继续覆盖此边界。

完整检查各阶段已分步完成。第一次脚本运行遇到全局 ccache 不可写，改用临时缓存后继续；第二次因离线 ONNX Runtime 未配置而链接失败，指定本机已有库后解决；第三次测试被沙箱 socket 权限阻断，解除限制后完整 workspace 全部执行完成。没有宣称原样 `check.sh` 在沙箱内单次通过，也没有通过忽略测试或改动断言消除失败。

详细结果、日志摘要和归档验证见 [机器记录](archive/python-retirement-validation.json)。原始日志保存在本机忽略目录 `.artifacts/retirement/`。本轮没有新增模型运行时、公开检索、数据合并、Windows 支持或 graphdb binding；未提交、推送。
