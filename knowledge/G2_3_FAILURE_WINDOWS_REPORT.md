# G2.3 故障注入窗口覆盖与残留

日期：2026-10-27。承接 [批次与恢复](G2_2_BATCH_RECOVERY_REPORT.md)、[原生控制](G2_2_NATIVE_CONTROL_REPORT.md)、[恢复与租约](G2_3_RECOVERY_LEASES_REPORT.md)、[保留与清理](G2_3_RETENTION_REPORT.md)。本轮对应 [G2 计划](G2_IMPLEMENTATION_PLAN.md) 第 9 节的故障注入矩阵，补齐可确定性注入的窗口，并按第 735 行诚实标注无法在开发环境制造的原生残留窗口。**G2 整体仍未完成**：规模/成本测量与 G2.4 收口待推进；尚无公开图查询。

本轮不新增依赖，不改 Schema/ID/规则指纹/默认搜索行为，不提交或推送。

## 1. 九类故障注入窗口 → 覆盖映射

计划第 733 行列举的九类窗口，逐项对应到实际测试（均核对磁盘实态与重开结果，不只检查返回错误）：

| # | 窗口 | 覆盖测试 | 状态 |
| --- | --- | --- | --- |
| 1 | 栅栏前后 | `graph_publication_*`（service.rs 写前栅栏）、`text_recovery.rs`（dirty STATE 修复） | 已覆盖 |
| 2 | zvec 文件替换中 | `engine_storage.rs::incremental_failure_removes_all_old_searchable_content`、`storage_crash.rs`（子进程自杀 + 手改 zvec 记录） | 已覆盖 |
| 3 | 文本 checkpoint 与 stable 之间 | `graph_recovery_leases.rs`、`text_recovery.rs`（Mutating→Stable 显式修复，实读确认） | 已覆盖 |
| 4 | 候选 DDL / 分批导入 | `retention_tests.rs::graph_retention_discards_known_partial_schema_and_preserves_unknown`、`graph_indexing.rs::graph_recovery_classifies_partial_import...`、`import_tests.rs`、`control_tests.rs::active_native_import_cancel_and_deadline_roll_back` | 已覆盖 |
| 5 | 根与回执事务中 / 提交响应 | `control_tests.rs::unknown_response_quarantines_despite_cancel_then_reopen_finds_receipt`（真实提交成功后适配层 Unknown→隔离→重开找回回执）、`committed_receipt_survives_process_exit_without_checkpoint`（确认提交后 exit(29)）、`commit_build_cancellation_before_write_leaves_no_partial_root`（真实原生写前协作取消）、`graph_resume_late_stop_preserves_commit_through_host_recheck`（真实提交确认后注入宿主/原生晚到停止） | 提交前后边界及响应丢失已覆盖；原生事务中途见 §3 残留 |
| 6 | 只读确认 | `commit_build` 写后新只读会话确认（build.rs:224）、`graph_explicit_recovery_inspection_is_readonly...`（Inspect 全程文件摘要不变）、`control_tests` 重开 `inspect_build` | 已覆盖 |
| 7 | HEAD 文件 / 目录持久化 | `zg-storage::filesystem` 测试覆盖写前拒绝、完整临时文件、原子替换与旧读者；`graph_recovery_selects_only_matching_committed_candidate_and_rejects_stale_base` 只覆盖 HEAD 比较/竞争拒绝 | **部分覆盖**：未在 HEAD rename 后、父目录 fsync 中/后注入文件系统故障或 SIGKILL；实现与 CAS 测试不等价于该故障验证 |
| 8 | 宿主 manifest 切换后响应 | `graph_manifest_response_loss_after_switch_recovers_selected_effect`：`after_manifest` 在真实 `publish_build` 成功后、`confirm_manifest_selection` 前注入适配层响应丢失，核对新 manifest/HEAD/原生回执并显式 Reconcile；`graph_cancel_after_prepared_head_keeps_previous_manifest_selected` 仅补充切换前取消 | 切换后响应丢失已覆盖；`before_manifest` 用例不再作为该窗口证据 |
| 9 | 清理阶段 | `retention_tests.rs`（已知 pre-root 可清理 / 未知保留）、`graph_state/discard/tests.rs`（原子移出、符号链接拒绝、HEAD 保护、可续作） | 已覆盖 |

对应验收矩阵条目：G2-A09（初建/重开/A→B→A、失败 attempt 不复用为成功、过期尝试拒绝）、G2-A11（候选失败不选中、根与不可变成功回执同事务、新只读确认）、G2-A15（取消与提交/选择竞争、Unknown 收到取消仍隔离且不删候选、已成功选择不改成未生效）、G2-A16（DDL 未建表也能记宿主诊断、unknown 不追加图库、确认失败不改成功回执、pending 崩溃后重开核对）、G2-A17（旧图读者与新构建共存、句柄未释放不能删目录、清理失败不撤销成功）均有上表测试支撑。

## 2. 本轮新增

- A1 宿主重放：`GraphRecoveryAction::Resume` + `Candidate::resume` + `resume_candidate`，测试 `graph_indexing.rs::graph_recovery_resumes_an_interrupted_import_with_fixed_identity`（中断候选→固定身份续写前缀→提交确认→幂等→可 Select；错误身份 `RESOURCE_BUSY`；不重复 Embedding）。
- A2 提交边界取消：`control_tests.rs::commit_build_cancellation_before_write_leaves_no_partial_root`（事实完整未提交时，写前取消→`Cancelled` 且 `inspect_build` 为 `None`，无半截 root/回执；后续未取消的 commit 仍恰好成功一次）。
- A1 提交后停止修复：`Candidate::resume` 拆为导入、提交与提交后确认；Candidate checkpoint 及宿主重检分别使用独立五秒确认的 context/token/QueryControl，同时保留并检查原 context 与原 QueryControl 的晚到停止。确认失败统一保留 `commitEffect=committed`，写入 `stage=confirmation_required` 并返回 `recovery_required`，不映射为普通 Cancelled/Deadline。`graph_resume_late_stop_preserves_commit_through_host_recheck` 在真实 `commit_build` 已写并确认根/回执后、宿主重检前分别注入 Cancelled/Deadline，返回 committed＋晚到停止诊断，固定 operation/attempt、不自动 Select、不修复 mutating STATE、不重试 Unknown，幂等且不重复 Embedding。
- A2 manifest 切换后响应：`graph_manifest_response_loss_after_switch_recovers_selected_effect` 使用测试专用 `after_manifest` 注入适配层响应丢失。注入时真实 manifest 已切换，HEAD 与原生回执均已提交；Inspect/Reconcile 恢复宿主诊断，不重做选择。
- HEAD 持久化仍只部分覆盖：当前没有可确定性命中 HEAD 原子 rename 与父目录 fsync 之间的原生/文件系统 fault hook，不以 `atomic_write` 实现审查或 CAS 竞争测试冒充故障注入。

## 3. 残留原生窗口（未验证，不伪造）

按计划第 735 行，以下两个窗口位于原生事务中途，开发环境无原生 fault-injection hook 无法确定性制造，**列为残留、不计为通过**：

- R1 SIGKILL/崩溃精确命中原生 root/receipt 提交事务中途。现有测试或在提交成功后退出（`committed_receipt_survives_process_exit`），或在提交成功后注入未知回包（`unknown_response_quarantines`，其注释明确 "not a native mid-commit crash simulation"）；均未在事务写入中途终止。
- R2 取消/期限恰在 `commit_build` 原生写入期间到达。`commit_build` 在写前做协作 `context.check()`（本轮新增测试覆盖该边界），但原生 root/receipt 事务是不带生产者回调的阻塞 C++ 调用，无法像 `import_batches`（`GatedSource` 门控）那样在事务中途注入停止。

缓解（已实现并测试，但不替代对 R1/R2 的直接中断验证）：`commit_build` 写后用**新的只读会话**确认 root/receipt（build.rs:224，不被后到取消覆盖）；重开时 `inspect_build`/`ensure_no_build_commit` 从持久 db 实态判定"已提交/未提交"，`commit_build` 对已存在证明幂等；恢复入口（Inspect/Reconcile/Select/Discard/Resume）据实态而非时间戳分类，未知则 `recovery_required` 隔离、不删候选。因此即便 R1/R2 发生，重开核对能从持久状态收敛结果——但该收敛路径针对的是"提交后"与"提交前"，**事务中途崩溃的实际磁盘半状态未被直接制造与核对**。

## 4. A1/A2 修复验收与验证

### 4.1 原生环境与可复现命令

原生环境沿用 G2.0 固定 graphdb 提交 `3bf0d532...` 与 [ANN 只读修复制品](ANN_READONLY_DIGEST_DRIFT.md)。实际 dyld 加载路径为 `/private/tmp/zg-ann-fix/native-baseline/lib/libzvec_c_api.dylib`，SHA-256 为 `50e243d8a941419c30bf11b4e5b4e8852275584109d647d86b3fa4129d805912`，与修复制品一致，不是仍有 ANN 只读缺陷的旧预编译包。运行环境为 macOS arm64、Rust 1.98.0、编译并发 14；所有 Cargo 命令串行执行。

在仓库根目录先设置：

```bash
export GRAPHDB_NATIVE_DIR=/private/tmp/zg-g2.0-dlx1w26k/target/debug/build/graphdb-sys-a304d1d6c8cb014e/out/native/build
export ZVEC_LIB_DIR=/tmp/zg-ann-fix/native-baseline/lib
export DYLD_LIBRARY_PATH=/tmp/zg-ann-fix/native-baseline/lib
```

完整原生回归实际执行：

```bash
rustup run 1.98.0 cargo test --manifest-path rust/Cargo.toml \
  --locked --offline \
  -p zg-engine -p zg-knowledge -p zg-storage \
  -p zg-daemon -p zg-daemon-protocol \
  --features zg-engine/knowledge-graph-graphdb,zg-storage/zvec \
  --all-targets -j 14 -- --test-threads=1
```

默认 feature 与静态检查使用同一 Rust 版本及并发参数：

```bash
rustup run 1.98.0 cargo test --manifest-path rust/Cargo.toml \
  --locked --offline -p zg-engine --no-default-features --lib graph_ \
  -j 14 -- --test-threads=1

cargo clippy --manifest-path rust/Cargo.toml --locked --offline \
  -p zg-engine -p zg-daemon --no-default-features --all-targets \
  -j 14 -- -D warnings

cargo clippy --manifest-path rust/Cargo.toml --locked --offline \
  -p zg-engine -p zg-knowledge -p zg-storage \
  -p zg-daemon -p zg-daemon-protocol \
  --features zg-engine/knowledge-graph-graphdb,zg-storage/zvec \
  --all-targets -j 14 -- -D warnings

cargo fmt --manifest-path rust/Cargo.toml --all -- --check
git diff --check
```

当前机器的 rustup `clippy`/`fmt` 代理入口异常，实际检查分别使用 Rust 1.98.0 toolchain 内的 `cargo-clippy clippy` 与 `cargo-fmt fmt` 执行上述相同参数，不以此改变检查范围或降低 lint 级别。

### 4.2 实际结果

| 执行 | 实际结果 |
| --- | --- |
| `graph_resume_late_stop_preserves_commit_through_host_recheck` 定向回归 | 1 通过 / 0 失败；同一用例分别覆盖 Cancelled、Deadline，均完成真实提交、宿主重检、根/回执核对、固定身份、幂等及 Embedding 调用数核对 |
| `graph_manifest_response_loss_after_switch_recovers_selected_effect` 定向回归 | 1 通过 / 0 失败；注入时新 manifest、HEAD 与原生回执均已生效，显式 Inspect/Reconcile 收敛 |
| `commit_build_cancellation_before_write_leaves_no_partial_root` | 在完整原生回归中通过；提交前取消不产生根/回执，后续未取消提交恰好成功一次 |
| 完整五包真实 graphdb＋zvec 回归 | **673 通过 / 0 失败 / 10 既有忽略**；较模块收敛基线 671 多本轮两项测试。engine 537（lib 478、各集成 target 59）、KG 45、storage 6、daemon 79、protocol 6 |
| 默认 feature 图相关测试 | 16 通过 / 0 失败 / 0 忽略 |
| 默认 feature Clippy（engine、daemon） | 通过，`-D warnings` |
| 图 feature 五包 Clippy | 通过，`-D warnings`；`Candidate::resume` 已拆分，无新增 lint 豁免 |
| 全 workspace rustfmt 检查、`git diff --check` | 均通过 |

未验证 Linux/Windows、断电、其他底座版本。完整回归日志保存在本次执行环境的 `/tmp/zg-module-consolidation/a1-a2-full-native.log`，命令退出码为 0，耗时 456.829 秒。

## 5. 结论

九类窗口中，manifest 切换后的适配层响应丢失已有直接测试；HEAD 文件/目录持久化仅部分覆盖，原子 rename 后父目录同步及中途崩溃仍未直接验证。两个原生提交事务中途窗口（R1 崩溃、R2 事务中途取消）继续列为残留：已验证其前后的映射、独立只读确认、幂等重提交与重开收敛，未验证事务中途实际半状态。报告明确区分真实原生导入/提交边界、宿主或文件系统注入、适配层响应丢失；不得以协作边界、提交成功后退出或成功后 Unknown 冒充原生中途恢复门通过。G2 故障验收在这些窗口及 HEAD 持久化直接验证补齐前不宣布完整。
