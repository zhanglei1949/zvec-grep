# KG 一致性、查询与边界改进

本轮保持完整候选构建。图候选分批写入，只有完整提交、核验并被 zg 发布记录选中的结果可查询。文档抽取、语言扩展、JSON 路径和物理差量写入不在本轮范围。

## 身份与历史

- 物理 Schema / facts format 升至 **v8**。不提供 `legacy_v7` 自动迁移；未知或不兼容格式报错，保留旧库。
- 同格式重建继承实体身份、FactId、Episode、支持来源、更正和撤回。旧位置不能确认时保留待绑定提及。
- 完整文本重建若不构建图，须先证明没有受保护语义数据；不能把图谱历史随旧正文目录清掉。
- 当前事实必须为 `active`，且有至少一条有效 `Support`。支持来源未删除、版本已核验、绑定的 Episode 有效；授权过滤先于支持累计。`Correction` 只作更正证据。
- `created_at_ms`、`changed_at_ms` 由 KG 填写，输入中的时间不覆盖已存事实。构建记录固定操作时间，恢复沿用；追加支持或重放不刷新，更正／撤回才更新 `changed_at_ms`。未知旧创建时间可为空。
- `EPISODE_LINK` 使用显式稳定 ID，只允许同空间、非自连的 `REPLY_TO` / `RESULT_OF`。Episode 退出会使其支持失效。
- Rust / JSON 公共事实与查询字段使用 `relation_type`，原生列仍映射为 `predicate`。

## 发布与读取

| 标识 | 用途 |
| --- | --- |
| `generation` | 一次逻辑发布；查询引用和游标绑定它 |
| `storage_generation` | 正文、全文和向量索引所在目录 |
| `graph_build_id` | 一个图构建产物 |

zg manifest 中的 `publication` 选择数据组合，记录知识库 ID、正文版本、配置摘要、图产物及就绪状态。图 Root / Receipt 证明提交；图侧 HEAD 只是准备记录。graph-only 发布生成新版本并复用正文目录；无变化时保持原版本。

文本先发布、图后补齐，是两次发布。图未跟上正文时，旧图保留为历史继承来源，但不可查询。文本原地写入失败后仍受恢复栅栏保护，回退 manifest 不能恢复已改写的数据。普通检索也核对栅栏和已发布的正文版本；不一致就停止读取。借用活动 writer 时，查询前后都要核对。

`ReadSession` 并列持有正文与不透明 `GraphHandle`；`KgReadView` 保留同一租约。`NodeRef` / `ObjectRef` 校验知识库、generation、图产物和对象类型；Fact 使用 ObjectRef。首期不提供任意旧 generation 重开。

现有 zg 恢复／语义维护选项中的 `generation` 字段仍指物理目录，Rust 字段注释已说明；查询中的 `ViewStamp.generation` 指逻辑发布。

## 查询

- `Neighbors` 与 `RelationTypes` 支持 `page_size`（默认 64、最大 256）、`cursor` / `next_cursor`。实际可见的关系类型按方向返回，不给精确数量。
- 邻接读取使用稳定键续查，保留并行边、引用次数和自环。游标绑定版本、起点、操作、过滤与授权；版本或权限变化后失效。
- `relation_filters` 的每项为 `edge_type` + 可选 `relation_types`。结构边不接受业务关系名；关系名、来源和 kind 的显式空集不匹配任何记录。
- 空间、起点、kind、关系名、上下文和当前状态进入 graphdb 查询；事实只加载候选所需来源与 Episode。端点核验不展开全部关联事实。
- `boundary.kinds` 限制每跳到达的节点；`result_filter.kinds` 只筛输出，不剪中间路径。普通语义选择使用 `query.kind`。来源授权只接受 source IDs，不能把 kind 筛选当作授权。
- 页满或有可续查进度的预算停止返回 Partial；不能安全续查、超大单条结果、取消或超时则报错。Constrain 只接受完整范围。

输出及 KG 保留的数据有预算。graphdb 仍受原生行数／中间结果限制，但 LIMIT 不等于只扫描一页，排序可能先整理键。业务计数与底层扫描不能等同；底层精确扫描量当前未接入，不能按 page_size 推算。构建报告记录来源复用、实际重写行数、耗时和原生 segment 峰值；该峰值不代表进程 RSS。

## 模块与维护

`KgService` 保留四个查询入口：`open_view`、`explore`、`augment`、`resolve_scope`；四个维护入口：`build_candidate`、`validate_candidate`、`inspect_build`、`discard_candidate`。可选 query 未实现。维护接口不选择 zg 的整体发布版本。

KG 负责图候选、身份与来源维护、事实有效性和查询；zg 负责解析／模型调度、正文与索引、刷新、锁和联合发布。现有读取与恢复机制继续复用。

SQLite 的运行时写入、解析回写和读取路径已移除；普通文本索引不创建空 SQLite。完整代码图核验并发布后，在写锁内清理已停用的已识别 SQLite 文件；未知文件保留并报告清理延后。被发布记录选中或仍有租约的资源不可清理。

本轮只有逻辑增量，物理上仍重写完整候选。共享检索不拆公共包，不增加插件框架；物理增量和独立应用后续单独验证。

## 验证记录（2026-11-05）

本次完整回归使用当前 graphdb 原生构建和**带只读补丁的 zvec-rust 0.8.0 SDK**，平台为 macOS arm64，测试并发数为 4。未放宽 ANN 文件断言，也未增加忽略项。

| 检查 | 结果 |
| --- | --- |
| workspace，全 targets，启用 graphdb，失败后继续运行 | **1477 通过、0 失败、14 个原有忽略项** |
| 原来的失败项 | **16 项 ANN 摘要检查和 1 项下载器超时用例全部通过** |
| 原生只读回归 | Flat/HNSW 两项通过，含全部距离类型、跨秒 close、Drop、内容及 mtime 检查 |
| 下载器 | 19 项通过；改用受控时钟，生产超时和原测试期限不变 |
| 借用 writer 的并发查询 | 6 项通过；写入前停止新借用并等已有查询结束，保留版本校验 |
| Clippy | workspace all targets、图 feature、`-D warnings` 通过 |
| 普通 CLI | 构建及直接启动通过；未设置 DYLD 路径覆盖，实际加载库与 SDK 摘要一致 |
| 格式与空白 | workspace rustfmt、`git diff --check` 通过 |

ANN 根因是 0.8.0 预编译库在只读 close 时回写元数据。已按 SDK 对应源码应用原生补丁，并补齐版本匹配检查。第一次修复后整套运行得到 1475 通过、1 失败、14 忽略；唯一失败是 writer 借用查询期间正文被修改，现已修复并完成上述整套回归。

14 个忽略项保持原样，包含需模型／插件环境的测试、手工用例、Linux 启动检查及已有的 macOS 原生内核限制。未验证其他平台。

构建须选择修复 SDK，上游默认预编译包尚未替换。使用方式见 [0.8.0 修复记录](ANN_READONLY_0_8_FIX.md)；完整命令、运行库摘要、原 17 项逐项结果和 9 个未修改测试源文件的摘要见 [机器记录](reports/ann-readonly-0.8/verification.json)。

### 修复前记录（保留）

此前使用 zvec-rust 0.8.0 的原预编译库，测试并发数为 2。以下为当时的结果，不替代上面的最终验证。

| 检查 | 结果 |
| --- | --- |
| KG 全 targets，启用 graphdb | **148 通过**，含事实时间、支持有效性、分页、授权和原生存储回归 |
| engine 完整单测，沙箱外 | **638 通过、17 失败、12 忽略**，失败详情见下文 |
| 工作区／发布记录 | 45 通过 |
| 读租约与缓存 | 原 15 项通过；补充普通检索恢复检查后，缓存测试组 15 项通过，1 个原有性能用例忽略（两组有重叠） |
| 语义继承与重建保护 | 6 通过，含旧格式阻断、跨物理目录继承、阻止文本重建丢弃历史 |
| Augment | 最终 8 项通过，含活动 writer 不可绕过恢复检查 |
| Constrain | 最终 5 项通过，范围必须完整，不降级成普通检索 |
| SQLite 清理与文本栅栏定向检查 | 各 1 项通过 |
| CLI、MCP、daemon、协议 | 317 通过；恢复检查和 writer 调整后，daemon 124 项再次通过 |
| Rust API Schema | 3 通过，最终检查未开启快照更新 |
| 不启用图 feature 的 engine | all targets 编译通过 |
| Clippy | KG、engine、CLI、MCP、daemon、协议 all targets，图 feature，`-D warnings` 通过 |
| 格式与空白 | workspace rustfmt、`git diff --check` 通过 |

**修复前 engine 完整回归未全绿。** 当时最后一次在沙箱外运行，结果为 638 通过、17 失败、12 忽略：

- **16 项只读文件断言失败**。逐项差异均为 ANN 的 `embedding.index.*.proxima`。当时没有修改或更换原生 zvec 库，也没有放宽这些断言；只读文件不变的契约尚未验证通过。历史背景见 [ANN 只读摘要漂移](ANN_READONLY_DIGEST_DRIFT.md)，其中旧补丁的结论不自动适用于当时的 0.8.0 制品。
- 下载器的 `response_header_timeout_falls_back_without_total_download_deadline` 在整套运行中两路来源均超时；单独重跑通过。该用例的响应头期限为 20 ms。当时未改下载器，整套失败与单跑通过的记录均保留。

首次沙箱运行的 618 通过、35 失败、12 忽略也保留。其中 10 项沙箱 HTTP 失败在外部重跑下载器组时通过；10 项旧发布／租约／SQLite 断言已修正，并在最终完整单测中通过。其余 15 项为当次 ANN 摘要失败。

修复前的完整日志与逐项差异为 `/tmp/kg-engine-all-final.log`、`/tmp/kg-engine-all-final-summary.json`；首次日志为 `/tmp/kg-engine-full-final.log`，下载器单跑为 `/tmp/kg-downloader-timeout-isolated.log`。当时的 KG、接口和重建保护日志分别为 `/tmp/kg-times-consistency.log`、`/tmp/kg-surfaces-verified.log`、`/tmp/kg-semantic-sources-verified.log`；普通检索、writer、daemon 和 lint 日志为 `/tmp/kg-text-read-guard.log`、`/tmp/kg-augment-guard-fixed.log`、`/tmp/kg-daemon-writer-guard.log`、`/tmp/kg-writer-guard-clippy.log`。该轮完整命令启用了 all targets，但因单测失败停止；接口契约的 3 项测试已另行通过。
