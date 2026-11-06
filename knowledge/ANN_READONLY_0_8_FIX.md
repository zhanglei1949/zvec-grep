# zvec 0.8.0 只读摘要与下载器超时修复

验证平台：macOS arm64。旧版记录见 [ANN 只读摘要漂移](ANN_READONLY_DIGEST_DRIFT.md)，本轮结果见 [KG 实施记录](KG_CONSISTENCY_IMPLEMENTATION.md#验证记录2026-11-05)。

## ANN 文件为什么会变

workspace 已使用 zvec-rust **0.8.0**，旧的原生补丁构建记录仍对应 0.7.2。此前测试加载的是未修复的 0.8.0 预编译库。

`Index::Close` 对只读实例跳过显式 flush，但仍执行 streamer cleanup；Flat/HNSW 的关闭流程会刷新元数据。`Index::Open` 没有把 `read_only` 传给底层映射，元数据更新因而能写进 `.proxima` 文件。

沿用已有的 [原生补丁](../rust/patches/zvec-readonly-mapping.patch)：只读映射使用私有页和只读文件描述符，关闭强制回写；写入实例的参数保持原样。SDK 接入方式、Rust FFI 和查询结果契约不变。

[0.8.0 构建清单](../rust/patches/zvec-readonly-mapping-0.8.json) 固定以下来源，并检查源码和补丁摘要：

| 来源 | 固定提交 |
| --- | --- |
| zvec-rust 0.8.0 | `16836107cd08bb42ec4e5a4052919735346f7783` |
| 其 vendor/zvec | `8397071d815cd842be2068339e00a94e69f069d1` |
| SIMDe v0.8.2 | `71fd833d9666141edcd1d3c109a80e228303d8d7` |

原库的 Flat、HNSW 两项只读测试均在关闭阶段失败。修复库的两项测试通过，覆盖 Cosine/IP/L2、两个读者、拒绝写入、跨秒 close 和 Drop；检查完整文件内容及修改时间。ANN 相关的 8 个测试源文件在本轮未改动。

## 下载器测试为什么超时

原测试使用真实时钟，让正常的备用来源也在 20 ms 内响应。整套运行中，线程或磁盘调度可能耗完这个期限，导致主、备来源一起超时。

现在用服务器门控确认请求／首字节已到达，再推进暂停的 Tokio 时钟。**生产超时和测试中的 20 ms／50 ms 参数均未调整**；保留来源、请求数和错误内容断言，增加正文超过响应头期限仍能下载、文件内容正确、无残留临时文件的检查。下载器 19 项定向测试通过。

## 完整回归发现的并发写入问题

第一次 workspace 回归中，原来的 17 项失败全部通过，但借用 writer 的并发查询有一项失败：查询等待模型期间，索引器先修改了正文，结束时的版本校验因此拒绝结果。

写入器现在在第一次修改前停止接收新借用，并等已有查询结束。解析和向量计算仍可提前执行；修改、失败标记和最终持久化都经过同一入口。查询前后的版本校验保留，6 项并发集成测试的原有断言全部通过。

最终 workspace 全 targets、图 feature 回归为 **1477 通过、0 失败、14 个原有忽略项**。原 17 项失败全部通过；workspace Clippy `-D warnings`、格式检查和普通 CLI 构建、启动均通过。CLI 未设置 DYLD 路径覆盖，实际加载的库与修复 SDK 摘要相同。命令和逐项结果见 [机器记录](reports/ann-readonly-0.8/verification.json)。

## 构建与使用

初始化清单对应源码及嵌套子模块后运行 [构建脚本](../rust/scripts/build-zvec-readonly.py)。例如本轮使用：

```sh
python3 rust/scripts/build-zvec-readonly.py \
  --source /tmp/zg-zvec-sdk-0.8-source \
  --build /tmp/zg-zvec-sdk-0.8-build \
  --simde-source /tmp/zg-zvec-sdk-0.8-build/_deps/simde-src \
  --oss-mirror --jobs 8
```

脚本输出 `ZVEC_LIB_DIR`、`ZVEC_INCLUDE_DIR`，并写入 `lib/zg-zvec-native.json`。普通 Cargo 构建、测试与打包使用这两个 SDK 变量。Cargo 暂存的库须与清单中的运行库 SHA-256 一致。

修复 SDK 已导出到项目的忽略目录，可在仓库根目录选择它：

```sh
export ZVEC_LIB_DIR="$PWD/rust/target/zvec-readonly-0.8-aarch64-apple-darwin/lib"
export ZVEC_INCLUDE_DIR="$PWD/rust/target/zvec-readonly-0.8-aarch64-apple-darwin/include"
```

运行库 SHA-256 为 `685dda969ca0813b074cf887259ca5a1db24158323d2eb4b23c20811200ff5be`。验证中未设置临时 DYLD 覆盖，Cargo 暂存库与制品一致。未发布新的上游预编译包，后续构建须继续选择修复库；清理 target 后需重新构建或导出 SDK。
