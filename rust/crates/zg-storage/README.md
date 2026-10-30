# zg-storage

供 engine 与 knowledge 共用的底层工具，不管理 Schema、模型、发布、目录所有权或进程级初始化。

| 功能 | 启用方式 |
| --- | --- |
| SHA-256、原子文件替换、目录同步 | 默认可用，不依赖 zvec |
| collection 打开/创建、标量字段、批量 fetch、写响应计数校验 | 显式 zvec feature，含对应运行/构建依赖 |

engine 显式启用 zvec；knowledge 的正常库只使用通用工具，共存测试通过测试依赖启用 zvec。调用方保留自己的错误映射和写入策略。

```bash
cargo check --manifest-path rust/Cargo.toml --locked -p zg-storage --no-default-features --lib -j 14
cargo test --manifest-path rust/Cargo.toml --locked -p zg-storage --features zvec --all-targets -j 14
```

readonly_collection 与 write_statuses 原生测试 target 要求 zvec feature。前者验证 Flat/HNSW 只读生命周期不改文件；后者验证真实混合逐项结果、补写、过滤和重开，接收退役 KG 投影组件中的通用断言。feature 关闭时这些用例未运行，不能计为通过。

原生测试使用与 engine 一致的运行库环境；[模块收敛报告](../../../knowledge/MODULE_CONSOLIDATION_REPORT.md) 记录实际版本和结果。文件持久化的平台分支保持原样，本轮不增加 Windows 支持结论。
