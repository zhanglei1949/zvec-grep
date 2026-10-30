# G4 代码图评测结果

原始证据已迁入[归档](archive/g4-validation/README.md)。以下保留 2026-10-29 的实验结论；本次整理未重新执行检索实验。

日期：2026-10-29。执行已完成，整体验证未全部通过。只评估本地 G3.0～G3.3 的 Rust／Python 能力，没有修改产品 Rust／原生源码、协议、依赖锁、ready 条件或预算，也未修改语雀。

## 结果

两仓完成 458 次正式评分调用。固定冒烟、构建和重开检查单列保存；另有 1 次只读引用目标身份定位，不计正文证据或检索耗时。来源、模型、文本／向量／图及发布状态的前后摘要一致。

| 项目 | ripgrep | requests |
| --- | --- | --- |
| 冻结 commit | `91f5d22ef48bde459d90a8c7dbeab416e97d0f2b` | `46e939b5525d9c72b677340985582b04b128478a` |
| ready 来源 | 102/102 | 35/35；另有 1 个空文件按原规则排除 |
| 正式调用 | 224/224 | 234/234，含原 `requests:16` |
| 结构化契约核对 | 18/24 | 20/24 |
| Off／Augment 普通输出 | 60/60 配对一致 | 60/60 配对一致；旧题另 5 对也一致 |
| Constrain 可见内容越界 | 0/40 次处理臂调用 | 0/40 次处理臂调用 |

以下是固定开发题上的配对观察，不是普遍收益结论：

| 模式 | 题数／仓 | ripgrep | requests |
| --- | --- | --- | --- |
| Augment：必需证据完整覆盖率平均变化 | 12 | +11.1 个百分点 | +18.8 个百分点 |
| Augment：必需证据字节覆盖率平均变化 | 12 | +12.3 个百分点 | +30.8 个百分点 |
| Augment：附加耗时 P50 | 全部重复 | 309 ms | 190 ms |
| Constrain：完整覆盖率平均变化 | 8 | 0 | 0 |
| Constrain：字节覆盖率平均变化 | 8 | +19.5 个百分点 | 0 |
| Constrain：范围内文件 MRR@10 | 8 | Off 0.640 → 1.000 | Off 0.813 → 1.000 |
| Constrain：附加耗时 P50 | 全部重复 | 1,176 ms | 265 ms |

Augment 的普通排名保持不变，新增证据单独计分。Constrain 使用范围内文件及正文标签，不能用它的文件排序变化代替回答质量。requests 的长函数在默认预览下未完整展示，因此其范围题完整正文覆盖率仍为 0。

Augment 平均新增的已标注必需字节约为 41／535 B；另有 66／4,815 B 新字节未被部分标签判定，不能直接称为无关。已标注引用位置观察到 87 条正确目标、0 条错误目标；其余引用未穷举，不外推全图精确率。旧 `requests:16` 的普通文件 MRR 两臂均为 0，单独保留，不冒充 SWE-QA20 全集结果。

## 失败与边界

48 道结构化题中，38 道通过冻结契约条件，证据覆盖另列。其余 10 道分为：

- 5 道预算边界：`new` 多义定位明确报输出预算错误；4 道包含／导入列表明确返回 `partial/output_budget`。未把错误或未完成结果当作空结果，未提高预算。
- 4 道标注假设问题：`escape` 同时匹配函数名和文件限定名，冻结请求没有指定 function kind，实际返回多义符合现有接口。
- 1 道标注位置问题：`dict_class` 的契约检查落在参数声明上，不是调用位置，不能由此判定产品解析错误。

原始请求、标签和严格评分均未回改。详情见 [annotation-review.json](archive/g4-validation/annotation-review.json)。后两类须在新输入版本修正，不能算本轮产品缺陷或改判为通过。

zg 自身的 P2 冻结视图另执行了 14 个请求、各三次，共 42 次：33 次满足预期，存储摘要不变。保留了 `validate` 定位的输出预算拒绝，以及 `lookup::module_path` 的导入／两跳缺失。后者尚需独立排查，不能用外部仓的结果覆盖这个问题。见 [dogfood-report.json](archive/g4-validation/README.md "包内：knowledge/reports/g4/dogfood-report.json")。

macOS 上解包位置的可执行文件遭 SIGKILL，复签未稳定解决。正式运行显式使用原构建路径，并强制核对可执行文件与 zvec 库的 SHA-256 和包内载荷相同；没有据此宣布 npm 安装后启动验证通过。`run-1`～`run-4` 的准备／冒烟失败全部保留。

## 成本

同机 Release、CPU，单位为墙钟时间和进程峰值 RSS；不计入查询延迟。

| 仓库 | 新文本索引 | 附加图构建 | 图构建峰值 RSS | 图目录增量 |
| --- | --- | --- | --- | --- |
| ripgrep | 5.13 s | 47.60 s | 530.5 MiB | 17.0 MiB |
| requests | 2.91 s | 10.19 s | 446.0 MiB | 3.4 MiB |

公开 MCP index 图配置检查另耗时约 9.35／1.94 s，单独记录。查询共享会话、索引和系统缓存，重复样本不是独立问题，不作为冷盘或 SLA 测试。图入口执行上限沿用请求 30 s、图阶段 5 s；MCP 传输看门狗为 40 s，包含等待清理。Off 按现有接口不接收 graphExecution，因此本轮不作两臂超时边界公平性的结论；完成调用均远低于 30 s。

## 检查与复现

- 163 项评测框架测试、23 项图事实回归、既有 Rust／CLI／MCP／daemon 一致性测试通过。
- 新增脚本的语法、格式及 diff 检查通过；离线回放重算得到相同报告。
- 1,049 个基线文件中，1,043 个未改，6 个改动仅涉及计划、评测入口／说明和冻结快照格式排除；1,529 个原生源文件未改。见 [source-audit.json](archive/g4-validation/README.md "包内：knowledge/reports/g4/source-audit.json")。
- 原 v6 协议、20 个问题及标签保持原样。新入口及命令见 [评测 README](../benchmarks/zg-retrieval/README.md#g4-code-graph-profile-v7)。

主记录：[结构化报告](archive/g4-validation/README.md "包内：knowledge/reports/g4/run-5/report.json")、[公开请求与响应](archive/g4-validation/README.md "包内：knowledge/reports/g4/run-5/")、[验证汇总](archive/g4-validation/verification.json)、[执行规格](G4_VALIDATION_SET_DESIGN.md)。`report.json` 中全局完整性仍为 false，原因是保留的预算错误；只对已完成的配对给出上述局部观察。

后续先发布修正类型定位与调用位置标注的新输入版本，独立排查 zg 的相对导入缺失，并补充输出预算边界的用户体验验证。八语言、文档图、Debug 超时和 Agent 评测继续后置。
