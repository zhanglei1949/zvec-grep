# G2.0 接入与真实原生共存验收

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-23。范围：修复宿主 binding 接入测试，验证实际 graphdb 与 zvec 同进程使用，并完成 G2.1 之前的基础能力探针。

本轮授权的接入与共存目标已通过：宿主不再有原先 8 处 API 编译错误，13 项 binding 测试和 5 项 Schema 探针实际运行通过。另用上游官方工具准备无源码符号链接的独立源码包，重新构建原生库，运行上游测试，再用该库复跑宿主用例。

这不表示整个 G2 已完成。生产图适配器、事实入库、STATE/POLICY/HEAD、联合发布和 SDK 恢复核对均未实现；本轮没有修改上游 binding 或 engine 行为，也没有新增依赖、提交或推送。

## 1. 版本与环境

| 项目 | 实际值 |
| --- | --- |
| graphdb gitlink | `3bf0d532ce928c02782c76d42cc289b1e49e8df4` |
| 隔离源码身份 | `3bf0d532ce928c02782c76d42cc289b1e49e8df4-snapshot-2c1668d2bdafd9d7` |
| 原生 ABI | `1.3`（`0x10003`） |
| 原生构建 | Release；Cypher、IPC、AP、TP、file、durable TP 已启用 |
| Rust / Cargo | `1.98.0` |
| 系统 / 架构 | macOS 15.1.1 / arm64 |
| 原生工具链 | Apple Clang 16.0.0；CMake 4.0.1 |
| zvec Rust binding | workspace 原有 `zvec-rust=0.7.2` |
| 实际 zvec 原生版本 | `v0.0.0-g1ab7975`，不据此推断未验证的完整源码提交 |
| 编译并发 | `-j 14`、原生构建 `CMAKE_BUILD_PARALLEL_LEVEL=14` |

固定的嵌套依赖：

| 依赖 | 提交 |
| --- | --- |
| kuzu | `89f0263cc7a1fd9c396d2c4953747a013556a7f9` |
| nanoarrow | `a579fbf5d192e85b6249935e117de7d02a6dc4e9` |
| parallel-hashmap | `2ec799017610ef831f4dc29c21fb3cce7e4a19b9` |

源码包包含 964 个原生文件、10,641,534 字节。准备工具先核对嵌套依赖提交和 tracked 修改，再输出逐文件 SHA-256；本轮重新核对全部摘要，并检查整个包没有符号链接。最终宿主日志里的 BuildInfo 包含该 snapshot 身份，证明复跑使用的是这份重新构建的原生库。

原工作区三处绝对源码软链没有被删除或替换。本轮验证的是计划允许的独立源码包路线，尚未执行全新递归 Git 检出。开发验证中使用 `GRAPHDB_NATIVE_DIR` 指向临时构建产物；没有把本机路径写进 Cargo 依赖或生产配置。

## 2. 实现变化

- [graphdb_binding.rs](../rust/crates/zg-knowledge/tests/graphdb_binding.rs)：消费 DDL 返回的 ExecutionSummary；移除不存在的 Database::close，改为关闭连接并释放全部数据库拥有者。原 6 项接入测试扩为 13 项。
- [graphdb_schema.rs](../rust/crates/zg-knowledge/tests/graphdb_schema.rs)：新增 5 项真实能力探针。它们位于测试目录，不引入生产 Schema 或公开图 API；替代原计划中生产模块内的 probe 文件。
- 图与正文使用同一组测试文档 ID。图库只存 ID、修订和引用关系；正文与预计算测试向量由 zvec 保存。临时 collection 只是原生接入夹具，不是生产 KG 独立索引方案。

## 3. 真实共存流程

同一个测试进程中同时持有实际 graphdb TP 数据库与实际 zvec collection，执行以下检查：

1. zvec 写入两份 UTF-8 正文、修订及四维测试向量；逐项核对写入状态与汇总计数。
2. graphdb 参数化写入两个文档身份和一条带来源修订的关系，并提交。
3. 新显式 READ ONLY 事务读出关系目标 ID 与修订，再按该 ID 从 zvec 取回正文、修订和向量，核对一致。
4. zvec 实际执行向量查询和全文查询；已知向量命中预期目标，全文命中两份正文，错误修订过滤返回空。
5. flush/checkpoint，关闭连接、释放数据库拥有者并关闭 collection。
6. 反转打开顺序，以只读方式重开两个底座，重复身份、正文、向量、全文和修订过滤检查。

该用例证明同进程链接/加载、两个原生库同时存活、交替调用及关闭重开可用，不证明两个原生调用同时执行的线程安全，也不承诺跨存储原子事务或完整图发布。

## 4. 事务、生命周期与错误证据

| 行为 | 证据 |
| --- | --- |
| 空闲连接 DDL | 返回 Committed，连接不遗留事务 |
| 显式只读视图 | 旧视图在写入提交后仍读到旧计数；另一连接的新 READ ONLY 事务读到新计数 |
| 回滚与事务失败 | 显式 rollback 无效果；同事务重复主键使其 rollback-only，commit 失败后没有早先部分效果 |
| 持久化与只读重开 | 关闭重开数据保留；只读写入返回结构化 ReadOnly 错误，重新可写打开计数未变 |
| 目录所有权 | 释放 Database facade 后，clone/Connection 仍保有目录；最后连接关闭后才能重新打开 |
| 取消 | 已请求的原生 CancelToken 导致 Cancelled，随后独立查询可正常执行；未验证运行中取消时序 |
| 资源限制 | max_results 超限返回 ResourceExhausted、ResultRows 限额和 RolledBack |
| 准入期限 | AP 被占用时返回 AdmissionTimeout、NotStarted；不把 transaction_timeout 当作执行期限 |
| 结果转换边界 | 写入已提交后 query_one 基数检查失败，错误仍为 Result 阶段、Committed；读取确认两条业务记录实际存在，不能盲目重发 |

以上是实际原生返回。执行器既有 unknown/cancel 纯规则测试不计为原生提交未知或恢复证据。生产适配器的错误映射与期限桥接仍属于后续接入。

## 5. Schema 与写入探针

| 计划问题 | 实测结论与边界 |
| --- | --- |
| STRING 主键 | 参数化 UTF-8、引号与 NUL 字符串可写读；本轮测试的复合主键形式被拒绝 |
| REL 与目标版本 | STRING 边属性、同端点多边、反向查询通过；REL 作为边端点被拒绝 |
| 列类型 | INT64、UINT64（含 u64::MAX）、BOOL、DOUBLE、typed null、STRING[]、STRUCT 的实际 DDL＋参数化写读通过 |
| 二级索引与过滤 | 本轮测试的 CREATE INDEX 形式被拒绝；128 行非主键属性过滤实测 scan_predicate_candidates=128，返回正确一行；不外推到大图 |
| 维护 DDL | 本轮 ALTER、DROP、MERGE、CREATE OR REPLACE 形式均被拒绝，outcome=RolledBack；DELETE→CREATE 后 rollback 保留原值 |
| 初始化与业务写入 | 空闲连接完成 DDL 后立即写入可见；失败 DDL 不遗留事务、不破坏既有表数据 |
| 批量写入 | 128 行参数化单事务与 import_batches 都写读通过；bulk 中一项主键冲突使本次 import 整体回滚，无前一项部分效果 |
| 原生共存 | 见第 3 节，不以 BuildInfo/open 代替实际操作 |
| Scope/Binding/Diagnostic 规模 | 未运行；按计划允许顺延至 G2.1，不计为已通过 |

探针中的两种 128 行写法只记录单次探索耗时，受缓存与并行测试影响，不构成性能对比或选择 bulk 的依据。首版正式写路径与完整物理 Schema 在 G2.1 再定；原生 LIST/STRUCT 可用也不取消配置/证据的版本化契约。被拒绝的 DDL 结论仅指固定提交上实际测试的形式。

## 6. 执行结果与记录

| 执行项 | 结果 | 记录 |
| --- | --- | --- |
| 原工作区原生库上的宿主接入测试 | 13 通过，0 失败/忽略 | 初次验证；以隔离源码复跑作为最终记录 |
| 隔离源码包上游 query/persistence/ownership | 28 通过，0 失败/忽略 | [上游日志](archive/pre-g4-validation/README.md#g2-0 "包内：knowledge/reports/g2.0/upstream-tests.log") |
| 隔离原生库上的 zg-knowledge 全 targets、graphdb feature | 50 通过，0 失败/忽略，包含 13 binding＋5 Schema | [宿主日志](archive/pre-g4-validation/README.md#g2-0 "包内：knowledge/reports/g2.0/host-tests.log") |
| zg-knowledge 默认 feature 全 targets | 32 通过，0 失败/忽略；图用例按 feature 未编译 | [默认日志](archive/pre-g4-validation/README.md#g2-0 "包内：knowledge/reports/g2.0/default-tests.log") |
| graphdb feature 全 targets Clippy | `-D warnings` 通过 | [Clippy 日志](archive/pre-g4-validation/README.md#g2-0 "包内：knowledge/reports/g2.0/clippy.log") |
| 组件 cargo fmt、git diff --check | 通过 | 格式与文档回读检查 |

[机器记录](archive/pre-g4-validation/README.md#g2-0 "包内：knowledge/reports/g2.0/verification.json") 保存原生清单、文件摘要、命令、测试计数、探针输出与未覆盖事项。日志里的执行器 panic 是既有 `panics_release_resources_and_worker_remains_available` 故意触发并验证的行为，该用例通过。

本轮默认和 graphdb feature 两次只读向量组件测试均通过。历史受限环境文件摘要差异的根因未定位，本轮通过不撤销已有历史记录。

早期尝试也保留边界：源码包首次从 `/tmp` 路径构建时，CMake 对比其真实 `/private/tmp` 路径导致 source ownership 检查失败；改用规范化绝对路径和独立 target 后通过，没有改源码。Clippy 首次调用因 cargo-clippy 软链旁缺少 driver 失败，使用实际 1.98 工具链目录后发现并修复了测试头注释中 CMake 缺反引号的一处 lint，随后测试与检查全部复跑通过。

## 7. 复现方式

以下在仓库根目录执行。依赖需要已取得且 Rust 缓存可用；不假定全新环境已经具备离线依赖。

```bash
# 使用所选上游工具导出完整的独立源码包，不改原工作区软链。
g2_dir="$(mktemp -d /tmp/zg-g2.0.XXXXXX)"
g2_dir="$(cd "$g2_dir" && pwd -P)"
python3 rust/thirdparty/graphdb/rust/tools/prepare_source_package.py "$g2_dir/source"
CARGO_TARGET_DIR="$g2_dir/target" CMAKE_BUILD_PARALLEL_LEVEL=14 \
  cargo test --manifest-path "$g2_dir/source/Cargo.toml" --locked --offline \
  -p graphdb --test query --test persistence --test ownership -j 14

# 从本次 Cargo 输出中读取其原生目录，避免在生产配置中固定开发机路径。
g2_native="$(python3 - "$g2_dir/target" <<'PY'
from pathlib import Path
import sys
values = []
for output in Path(sys.argv[1]).glob("debug/build/graphdb-sys-*/output"):
    for line in output.read_text().splitlines():
        if line.startswith("cargo:native_dir="):
            values.append(line.split("=", 1)[1])
assert len(values) == 1, values
print(values[0])
PY
)"
GRAPHDB_NATIVE_DIR="$g2_native" CMAKE_BUILD_PARALLEL_LEVEL=14 \
  cargo test --manifest-path rust/Cargo.toml --locked --offline \
  -p zg-knowledge --features graphdb --all-targets -j 14 -- --nocapture
cargo test --manifest-path rust/Cargo.toml --locked --offline \
  -p zg-knowledge --no-default-features --all-targets -j 14
GRAPHDB_NATIVE_DIR="$g2_native" \
  cargo clippy --manifest-path rust/Cargo.toml --locked --offline \
  -p zg-knowledge --features graphdb --all-targets -j 14 -- -D warnings
cargo fmt --manifest-path rust/Cargo.toml -p zg-knowledge -- --check
git diff --check
```

如果本机 cargo-clippy 软链旁缺少 clippy-driver，先将 `rustup which cargo --toolchain 1.98.0` 所在目录加入 PATH。已有原生构建也可直接运行宿主两个测试 target，但不能把一次缓存构建当作独立源码复现。

## 8. 未覆盖与下一步

未制造真实 CommitOutcomeUnknown、提交响应丢失或进程终止后的 SDK 回执核对；未实现运行中取消与宿主 deadline 桥接；未验证跨平台、规模门及生产图发布。当前源码包不等于注册表发布，也不解决许可证/分发门。

下一步可进入 G2.1 的同次事实绑定、Schema 细化与事实往返；发布/恢复实现不得把这些未覆盖窗口算作通过。G2.1 还需完成三张事实表体量与合成 ID 验证，G2.2/G2.3 再兑现候选发布和恢复契约。
