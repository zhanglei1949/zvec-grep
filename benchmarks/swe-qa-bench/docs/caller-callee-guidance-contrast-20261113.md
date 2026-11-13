# 关系组 direct/flow 24-trial 对照协议 v1

本轮只运行 relationships 组：6 题 × direct/flow × 2 次，共 24 个独立 trial。不会重跑 QA20 全量。实际模型请求、递归会话、工具参数与结果全部保存；零调用、空结果和低分保留，不要求调用工具。

## 预先固定的变量

候选代码固定为 `bfe5a0acefed7f1a4f46161a3c5da9fd3fdaec54`，包含现有 guidance variant 实现。本轮不改 Rust、工具描述、图索引就绪提示、原生工具、任务预算、采样与评审设置。Harness 绑定运行前的完整提交 SHA；两份 run 使用同一 candidate/harness。

唯一指导差异是已有的定位句：

- direct：`For a known symbol's direct call relationships, use callers for incoming call sites or callees for outgoing call sites.`
- flow：`For a known symbol, use callers for incoming call sites or callees for outgoing call sites, including to trace how control or data flows through call chains, locate where a symbol is invoked, or enumerate what a symbol invokes.`

冻结全文及工具描述见 `fixtures/caller-callee-guidance-contrast-v1.json`。两版均保留“no tool call is mandatory”和“Stop when evidence is sufficient”。安装指导和 MCP instructions 使用同一变体。实际 preflight surface 与快照不一致时停止；实际请求中缺工具或指导则记录为诊断结果，不因零调用重跑。

| 类别 | 固定问题 | 方向/目的 |
|---|---|---|
| 概念 | matplotlib:37 | 沿用上次 FontInfo 概念题，原问题与参考答案不变 |
| 概念 | sympy:38 | 沿用上次概念题，原问题与参考答案不变 |
| 概念 | xarray:32 | 沿用上次概念题，原问题与参考答案不变 |
| 符号 | requests:cc1 | 谁调用 resolve_redirects；沿用现有题 |
| 符号 | matplotlib:cc2 | _make_line 调用谁；嵌套调用与构造函数 |
| 符号 | sympy:cc2 | _get_func_order 调用谁；项目函数与内建函数 |

选择在看本轮输出前固定。符号题不会要求使用 MCP。新参考答案来自 pinned 源码逐行核对，并用 Python AST 独立检查调用表达式，未参考候选工具结果；仅 judge 可见。三道概念题从 QA20 原样复制，QA20 的冻结输入文件不变。定向题集现在共 16 题，selection revision 2 / references revision 3，既有 11 题不改；本轮仅选择表中六题。

## 执行与判读

两个 workflow run 分别选择 `task_set=caller-callee`、`experiment_group=relationships`、`experiment_phase=guidance-contrast`、`guidance_variant=direct|flow`。新阶段每 run 固定 6 题 × 2 次，最多两个任务并发，每题内串行。OpenCode 1.18.4、custom-openai/glm-5.2、既有百炼 endpoint、本地 Potion embedding、graph 索引及 ignore 文件一致。索引耗时单列，不计 Agent 耗时。每个 run 独立汇总，不把 direct/flow 混入现有 aggregate。

模型请求在 HTTP 边界采集完整 messages/tools 和请求参数；不记录认证头，上传前沿用 secret scan。smoke/full 也启用相同采集，并在 collect/report 中审计；其新报告协议为 caller-callee-e2e-v2，历史 v1 报告保留。

异常最多额外重试两次，原失败轨迹保留；没有调用、空结果、评分高低都不触发重跑。所有 trial 逐行报告，不作分差/token 筛选。

主要指标为关系工具采用率（调用过 callers 或 callees 的 trial / 全部 trial），同时分别报告 callers/callees 的次数、成功、失败、空结果和未知状态。按概念/符号题分层比较 direct/flow；每格分母都是 6。附带评分、token、工具次数与耗时，费用未知则保持未知。

先核验每个 Agent 请求实际包含的工具定义、AGENTS/MCP 指导及角色，再从完整轨迹描述发现符号后是否切换关系工具、方向与参数是否正确、原生工具能否已提供充分证据。调用与未调用行为的分层仅作描述，不解释为因果收益。6 题的小样本及 GLM 自评不能支持总体性能结论，也不能从外部轨迹唯一断言模型的内部动机。

运行 ID、冻结 harness、校验和、逐 trial 结果与原因分析在执行完成后追加。原始数据保存在忽略目录 `runs/guidance-contrast-20261113/` 和 Actions artifacts，不覆盖此前实验。

启动前校验：本地 Python 回归 220 项通过（2 项需额外环境而跳过）；另用固定 OpenCode 1.18.4 对本地模拟服务执行 8 项 OpenCode 检查，全部通过，包含主代理和递归子代理实际请求。共享 Node 检查 59 项通过；actionlint、diff 检查以及 QA20/定向题集输入与参考隔离校验通过。既有 11 条参考记录保持不变。仓库 admin/maintain 权限及 GLM_API_KEY 名称已确认；未配置 endpoint variable，按现有 settings 使用用户给定的百炼真实 endpoint。
