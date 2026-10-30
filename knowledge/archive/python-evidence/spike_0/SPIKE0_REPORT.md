> 历史 Python 验收记录；执行命令需要先恢复基线。原始文档和源码保存在 [Python 基线归档](../../python-baseline.zip)，本可阅读副本仅更新归档引用。

# Spike-0：底座组合实测记录

状态：完整套件六项检查全部通过（`complete_suite=true`，最近运行 `20261020T134436Z-5f6bd37a`）。zvec 以未修改源码构建并通过；graphdb 仅在显式兼容变体下通过，原始固定提交在本机 Apple Clang 16/libc++ 工具链构建失败，因此 `unmodified_source_builds=false`，本轮不能记为原始提交组合在此工具链直接通过。graphdb 顶层许可证缺失（`license_gaps=[graphdb]`）仍为发布前待澄清项。

## 环境与固定输入

| 项目 | 本次值 |
| --- | --- |
| 日期 | 2026-10-20 |
| 系统 | macOS 15.1.1，arm64 |
| 资源 | 14 个逻辑 CPU，24 GiB 物理内存 |
| Python | CPython 3.13.3，项目 `.venv` |
| 编译器 | Apple Clang 16.0.0（clang-1600.0.26.6） |
| CMake / Ninja | 3.31.6 / 1.11.1.4，来自哈希锁定的构建工具包 |
| 编译并行度 | 顶层 14；上游按 ProcessorCount 发起的 ExternalProject 也限制到 14；不是全部进程的统一资源硬限制 |
| 向量输入 | 两个预计算的 16 维向量，不调用 LLM 或 Embedding 服务 |

| 依赖 | 固定提交 | 根项目许可证 |
| --- | --- | --- |
| graphdb | `9d6e966ef0ae214cff8d5018efe1dafb37bc2a99` | 此提交未声明顶层项目许可证 |
| zvec | `a4347e31f1b8e216cffba59285427a3b78551229` | Apache-2.0 |
| py-tree-sitter v0.25.2 | `e2a5b21449c30c6a4fb49a55567a4699c3271f10` | MIT |
| tree-sitter-python v0.25.0 | `293fdc02038ee2bf0e2e206711b69c90ac0d413f` | MIT |
| tree-sitter-cpp v0.23.4 | `f41e1a044c8a84ea9fa8577fdd2eab92ec96de02` | MIT |
| SIMDe v0.8.2（构建头文件，非 Python 包） | `71fd833d9666141edcd1d3c109a80e228303d8d7` | MIT |

来源、完整提交和排除项见 [依赖清单](dependencies.json)。此处为五个 Python 包源码及 SIMDe 构建源码的许可证记录，未据此宣称已完成所有传递依赖的发布许可审计；graphdb 的第三方通知不能代替其项目许可证。

## 构建与验证方法

五个包从固定源码构建 wheel 后安装到同一虚拟环境，不使用包索引中同名原生包替代。graphdb 保留默认 Cypher、IPC 与持久 TP 能力；zvec 关闭工具与 C binding，保留本轮所测的全文和向量能力。

Kuzu 稀疏检出所需解析器与 runtime，Arrow 稀疏检出 cpp 子树。Arrow 的 `testing`、`cpp/submodules/parquet-testing` 为显式排除的测试数据 submodule；根 gitlink 保持原样，构建检查只允许这两项未初始化。初次尝试的无条件递归拉取已停止，随后改用明确排除配置。这是下载范围调整，不是版本替换。

SIMDe 已浅获取固定提交，通过 CMake 标准的 `FETCHCONTENT_SOURCE_DIR_SIMDE` 使用本地源码；初次完整历史拉取由本次试验主动停止，不计为原生编译错误。Arrow 后续依赖使用上游提供的 OSS 镜像与哈希校验。本次为减少嵌套构建争用，暂让主 Ninja 等待 Arrow 编译完成后继续，保留已编译对象；构建耗时不作为性能对比结果。

实际命令及重跑方法见 [操作说明（归档）](../../python-baseline.zip)。构建回执记录源码提交、递归 submodule、参数、wheel 哈希及安装结果。运行入口核对安装包对应的 wheel 哈希，再启动隔离的原生进程；失败、超时、缺少结果和依赖未验证分别保留。

zvec 的 `_zvec` 扩展按路径 `-force_load` 一组 `core_*` 静态库（`libcore_knn_flat.a` 等 13 个），却未在 CMake 中声明对它们的 target 依赖；只构建 `_zvec` 会因缺少这些 `.a` 而链接失败，且因无依赖边，与它们并列构建时还存在“先链 `_zvec` 后归档 core 库”的竞态。构建脚本显式将这 13 个 `core_*_static` 目标排在 `_zvec` 之前一并下发（scikit-build 按列表顺序逐个构建），保证从零检出可复现链接，不修改 zvec 源码。已用“同时删除 `libcore_knn_flat.a` 与 `_zvec.so` 后单次重建”验证该顺序修复。

## 实测结果

| 检查 | 当前结果 |
| --- | --- |
| 未修改 graphdb 源码构建 | 失败：`src/io/reader.cc:231` 调用浮点 `std::from_chars`，Apple Clang 16 配套 libc++（MacOSX15.2.sdk）显式删除该重载 |
| graphdb 兼容副本构建 | 通过；独立源码副本应用 [CSV fallback 补丁（归档）](../../python-baseline.zip)，原始 checkout 未改动 |
| graphdb 持久化及浮点 CSV | 通过；写入进程正常关闭后，另一进程重开核对点边及三行 DOUBLE/FLOAT 数据 |
| zvec 未修改源码构建与全文/向量 | 通过；wheel `zvec-0.6.1.dev7-cp313-cp313-macosx_15_0_arm64.whl`，逐项插入状态、flush、全文词项与固定向量命中，另一进程重开后重复查询 |
| Python / C++ grammar | 两项均通过；实际解析函数、校验 UTF-8 字节位置并检测错误输入 |
| 同进程完整组合（两种导入顺序） | 通过；graphdb、zvec、绑定与两种 grammar 在同一子进程导入并各自执行原生操作 |

兼容补丁在 libc++ 下跳过浮点 CSV 的 `from_chars` 优化，沿用已有 `ParseScalarTyped`；不引入另一套解析库。只验证本轮十进制、指数及符号样例，不据此声称全部 CSV 行为或性能等价。运行报告显式记录 `source_variant`、补丁哈希及 `unmodified_source_builds=false`。

脚本故障分类的三个单元检查、Ruff 检查及格式检查通过。这些脚本检查与原生运行结果分别报告。

## 结论边界与后续

Spike-0 的六项检查已全部通过，R1（固定 commit 能否一起构建跑通）在 graphdb 兼容变体下得到肯定回答，zvec 以未修改源码通过。两项已知保留须带入下一阶段：其一，原始 graphdb 提交在 Apple Clang 16/libc++ 工具链存在浮点 `std::from_chars` 构建缺口，进入正式实现前须决定上游修复、更换工具链（如提供浮点 `from_chars` 的 libc++/编译器）或长期维护该兼容补丁，并把它作为已知偏差记录；其二，graphdb 顶层许可证缺失是发布前必须澄清的阻塞项，第三方通知不能替代项目许可证。

本轮不验证崩溃恢复、并发隔离、提交可见性上限、规模天花板、知识有效视图、增量更新协议或模型质量。正常关闭后的跨进程重开不等于故障恢复。

Spike-0 完成后继续 Spike-1/2。只有 R1–R5 依照交付计划的 profile 和判定门完成，才能放行正式骨架；本轮结果不能单独作为阶段 A 的 Go 结论。
