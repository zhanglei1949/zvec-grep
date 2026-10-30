# ANN 只读打开期间文件摘要漂移

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

记录日期：2026-10-24；修复验收：2026-10-26。状态：**固定版本原生补丁已在本地验证通过**。适用当前 macOS arm64 修复制品；原 0.7.2 预编译包仍有缺陷，必须显式采用补丁库。以下保留原始失败，再记录修复，不代表上游已发布新包。

## 现象与证据边界

- 用例：`zg-knowledge/tests/native_projection.rs` 的 `vector_projection_missing_item_repair_and_same_dimension_space_rejection`。
- 原断言：关闭可写 ProjectionIndex 后记录整个目录的文件摘要，再以只读方式执行 open → fetch → close，要求摘要完全一致。保留该断言，不忽略 ANN 文件、不增加通过重试。
- 用户提供的另一会话交接报告称：2026-10-23 17:10–17:18 的全 targets 运行出现一次失败，只有 `embedding.index.3.proxima` 的摘要变化；命令未指定 `--test-threads=1`。原始失败日志及前后完整摘要未随交接提供，因此对这次历史事件仅保留外部会话报告；本轮独立复现见下文。报告来源摘要与命令见 [观察记录](archive/pre-g4-validation/README.md#g2-1-followup "包内：knowledge/reports/g2.1-followup/ann-observation.json")。
- [G2.0 验收记录](G2_0_ACCEPTANCE_REPORT.md) 已保留历史受限环境摘要差异；当轮向量只读测试通过，不撤销历史记录。
- [G2.1 原始完整回归日志](archive/pre-g4-validation/README.md#g2-1 "包内：knowledge/reports/g2.1/full-regression.log") 中，该用例和完整串行回归均通过。它与另一会话的失败不是同一次执行，不能互相否定，也不能据此归因于测试并发或沙箱。
- **2026-10-24 本轮已独立复现**：完整回归在沙箱外使用隔离测试状态和 `--test-threads=1`，结果为 546 通过、1 失败、10 忽略，在同一断言失败。逐项比较前后摘要，唯一变化为 `embedding.index.3.proxima`，从 `8287687c…d328` 变为 `c0a48aae…ac20`。完整日志和全部前后文件摘要分别保存在 [失败日志](archive/pre-g4-validation/README.md#g2-1-followup "包内：knowledge/reports/g2.1-followup/full-regression.log") 与 [复现记录](archive/pre-g4-validation/README.md#g2-1-followup "包内：knowledge/reports/g2.1-followup/ann-reproduction.json")。这证明该现象至少不依赖默认测试线程并发或沙箱；zvec 内部线程与可写关闭后的任务仍需定位。

修复前已确认文件字节变化，当时尚无证据证明向量内容、查询结果或图数据损坏；也没有证据证明只读 open 本身就是写入方。当时待区分可写关闭后的延迟写入、只读句柄生命周期和并发等窗口；下文保留原定位方案，实际根因与关闭结论见最后一节。

## 对 G2 的影响

这个用例使用 KG 独立 ProjectionIndex，与正式 zg IndexStore 的 Schema 和生命周期不同；两者依赖同一 zvec 底座。不能把组件报告直接当成宿主已经发生同一故障，也不能忽略它对宿主只读契约的影响。

graphdb 的只读事务验证不因这条 zvec 报告失效。G2.2 的 graph-only 路径和后续联合 ReadSession 接入仍须验证宿主 zvec 只读打开是否会写文件；在问题关闭或支持条件明确验证前，不宣称该前提已得到完整证明。此项不以修改 G2.1 业务逻辑或放宽原断言收口。

## 修复前定位方案（保留）

在仓库根目录、按既有说明准备原生依赖后运行：

```bash
cargo test --manifest-path rust/Cargo.toml --locked --offline \
  -p zg-knowledge --features graphdb --test native_projection -j 14 \
  vector_projection_missing_item_repair_and_same_dimension_space_rejection \
  -- --exact --nocapture
```

分别记录单用例、native_projection 全目标、完整串行回归及默认测试线程数下的结果。每次使用新临时目录与独立进程；保留失败，不以循环重试后的通过替代失败记录。

定位时在 flush、可写 close、只读 open、fetch、只读 close 的各边界记录文件摘要、长度和时间，并记录原生版本、平台、线程数、仍存活的拥有者及后台任务。必要时跟踪实际写入方；诊断只包含元数据，不输出正文、向量或凭据。

关闭条件：得到可复现窗口及写入原因，修复或明确并验证受支持的生命周期条件；原摘要断言以及宿主 IndexStore 的对应只读验证在这些条件下通过。单次通过、推测环境原因或跳过文件均不满足关闭条件。

## 2026-10-26 根因、修复与验收

根因在 zvec 原生读取生命周期。`Index::close` 虽对只读实例跳过显式 flush，仍调用 streamer cleanup；Flat/HNSW 的 close 会刷新带秒级更新时间的元数据。原 `Index::open` 未把 read_only 传递为底层私有映射，MMapFileStorage 写脏页及关闭时的元数据刷新最终修改文件。若读者在同一秒内打开并关闭，更新时间相同，原摘要用例可能通过。

已在真实测试里分别核对 open、fetch/query、close；原库只有 close 阶段失败。增加 1.1 秒跨时钟刻度检查后可稳定复现。禁用 mmap 的对照仍失败，未采用该配置绕过。用与 SDK 对应的固定提交 `1ab7975dfc2d2160054bafff614831b7099cd930` 重新构建无补丁库，同样复现两个共享层用例失败。

[原生补丁](../rust/patches/zvec-readonly-mapping.patch) 将只读映射设为 COW/private，并关闭强制回写；文件描述符随之只读。只读 cleanup 的内部元数据变化留在私有页；可写打开的原有 COW/强制持久化规则保持原样。没有复制 collection、修改文件权限、屏蔽 ANN 文件或增加宿主 FFI。

[构建入口](../rust/scripts/build-zvec-readonly.py) 检查固定 Git 提交、源码前后哈希、补丁哈希和嵌套子模块版本；构建后生成 native 制品及词典摘要。继续复用 workspace 的 zvec-rust 0.7.2，通过 SDK 支持的 `ZVEC_LIB_DIR`/`ZVEC_INCLUDE_DIR` 接入。没有在 KG 中重新固定一份 zvec 源码。构建与打包都须采用这份修复制品；只改测试进程的加载路径不算交付。默认远端预编译资产尚未修复，本轮没有提交、推送或发布上游。

| 验证 | 实际结果 |
| --- | --- |
| 原预编译库分阶段回归 | open/fetch 不改文件，跨秒 close 失败 |
| 同提交源码无补丁库 | Flat/HNSW 两项共享只读回归失败 |
| 同提交补丁库 | Flat/HNSW × Cosine/IP/L2，查询、拒绝写入、两个读者、显式 close、Drop 全部通过；核对所有文件内容和 mtime |
| KG 原失败用例 | 保留完整目录摘要断言，新增 open/fetch 阶段及跨秒 close 检查，通过 |
| 正式宿主 IndexStore | FTS、ANN、canonical evidence、close/Drop 均不改文件内容或 mtime |
| 三组件完整回归 | **557 通过、0 失败、10 个原有手工用例忽略**，沙箱外串行执行；通过的是正常 SDK 环境变量选中的补丁库，其 SHA 与 staged 制品一致，不依赖临时 DYLD override |
| Clippy | 三组件 all targets、图 feature、`-D warnings` 通过 |

前后对照日志、原生加载路径、40 项源码/补丁摘要与最新命令见 [机器记录](archive/pre-g4-validation/README.md#ann-readonly-fix "包内：knowledge/reports/ann-readonly-fix/verification.json")。旧 546/1/10 记录保持冻结。当前结论只覆盖上述源码补丁及制品，未验证 Linux/Windows、断电或其他版本底座；COW 私有脏页的资源成本留后续规模测试。

打包前须核对 staged runtime 与 `zg-zvec-native.json` 的 SHA-256。后续复核曾发现共享 `target/debug` 里的库与补丁摘要不一致；在 SDK 覆盖环境下重新构建后，已恢复一致并再次确认动态加载的是指定补丁库。共享 target 不能作为固定原生版本的依据；该复核日志另列在 [G2.2 机器记录](archive/pre-g4-validation/README.md#g2-2-build-commit "包内：knowledge/reports/g2.2-build-commit/verification.json")。
