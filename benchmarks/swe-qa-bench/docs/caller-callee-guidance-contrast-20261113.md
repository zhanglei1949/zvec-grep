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

## 执行结果（2026-11-13）

两个 run 均 attempt 1 成功：direct [43378735002](https://github.com/zhanglei1949/zvec-grep/actions/runs/43378735002)，flow [43378755997](https://github.com/zhanglei1949/zvec-grep/actions/runs/43378755997)。24/24 trial、12/12 任务报告齐全，零失败、零缺失、零 Harbor 额外重试；未重跑全量、未强制调用、未改变图索引就绪提示。

冻结 harness：`405b6dd162774662a925ae8be6949ac3a65eb968`；candidate：`bfe5a0acefed7f1a4f46161a3c5da9fd3fdaec54`。两个独立下载的候选包 SHA256 均为 `8b67777ef3780f9cf6a95fd6c3a4c18e904aba2300b0cbbf9ce9c7418685e082`。同仓库任务的 index seed 身份一致；全部使用 graph 和同一 ignore 摘要。

### 实际模型上下文

重新从原始 model-requests.jsonl 审计到 204 次 Agent 侧 HTTP 请求，全部 HTTP 200 并有 response_end；其中 180 次携带工具，24 次是没有工具的后台标题请求。独立 judge 请求不计入这 204 次。

180/180 个携带工具的请求均包含两项 MCP 工具 `zvec_grep_zvec_grep_callers` / `zvec_grep_zvec_grep_callees`，以及 bash、edit、glob、grep、read、skill、task、todowrite、write 九项原生工具；没有 search 或网页工具。两项工具的 description 与冻结快照一致，安装 AGENTS 指导和 MCP instructions 均实际进入 system 消息。未发现工具/指导缺失、额外 zg 工具或变体串用。

204 次请求参数均为 glm-5.2、temperature=0、seed=42、enable_thinking=true、reasoning_effort=high、max_tokens=32000，未传 response_format。每道题同变体两次运行的首个 Agent 请求 JSON 逐字段相同；把 flow 定位句还原为 direct 后，同题跨变体的首个请求也逐字段相同。后续行为仍存在差异，不能假设 seed 相同就有确定输出。

递归导出完整，但本轮每个 trial 实际只有根会话，没有真实子代理样本。主/子代理的模拟服务合同验证已通过；本轮不能提供真实子代理请求的新增证据。以上实际上下文结论仅适用于本轮 24 个 trial，不能倒推旧 300-trial 未归档的 HTTP 上下文。

### 调用与质量

采用率分母包含所有 trial；下表调用次数只计真正进入 MCP 的调用。参数生成失败另计。

| 变体 | 概念题采用率 | 符号题采用率 | 总采用率 | callers 次数 | callees 次数 | 零调用 trial |
|---|---:|---:|---:|---:|---:|---:|
| direct | 0/6 | 6/6 | 6/12 | 2 | 4 | 6 |
| flow | 0/6 | 5/6 | 5/12 | 2 | 3 | 7 |

11 次实际 MCP 调用全部成功、均返回非空已解析调用点：无 MCP 执行失败、无空返回、无未知状态。另有 5 次执行前的 invalid-tool JSON 解析错误（direct 2，flow 3），均为 globs 字符串未加引号/未形成合法数组；Agent 在同一 trial 内自行纠正。它们是参数生成错误，不是 MCP 服务错误，也不是额外 trial。所有 13 个零调用样本完整保留；未识别到 shell 调用 zg CLI 的能力绕行。

| 每 trial 均值 | direct | flow |
|---|---:|---:|
| Judge | 80.50 | 82.25 |
| 总 input tokens（含缓存） | 109161.08 | 147681.00 |
| Output tokens | 1755.50 | 1394.25 |
| 全部工具次数（含 invalid） | 9.00 | 9.83 |
| Agent 秒 | 36.46 | 38.03 |

flow 比 direct 平均评分 +1.75，但概念题评分是 68.50 vs 70.67，符号题是 96.00 vs 90.33；不能把该小样本差异解释为工具采用的因果收益。总输入 token +35.29%、输出 -20.58%、全部工具次数 +9.26%、Agent 耗时 +4.30%。资源沿用 session-tree 口径；后台标题未进入持久会话，不计入表中 token。费用未返回，不能由含缓存的总 token 直接推算费用。

索引构建耗时单列：direct 的 index_duration_seconds 总和 279.137 秒，flow 256.068 秒；每版各 6 次构建、6 次 seed 恢复，恢复与安装开销仍保留在 setup 原始记录，不加进 Agent 秒。

### 为什么调用或不调用

**最强的观察是任务形态差异。** 本轮明确给出符号的 12 个 trial 中有 11 个调用；三道概念题的 12 个 trial 全部未调用。直接关系题能够驱动模型使用正确方向的 callers/callees，否定了“关系组一概拿不到工具或一概不会使用”的解释。概念题需要先把概念、字段传播或性能问题转换成具体符号和关系查询，轨迹里没有发生这一步切换。这支持“入口发现和关系查询的衔接/任务匹配是低采用率的重要因素”的判断，尚不能确认模型唯一的内部动机。

具体证据：

- `requests-cc1__v622qcZ`（direct）：首个动作直接 callers(resolve_redirects, globs=[src/requests/**])，无需发现步骤。4 个 requests 符号 trial 都成功找到 Session.send 的两处调用点（sessions.py:723、740）。输出还含测试调用点，Agent 按题目 src/ 范围筛选。
- `matplotlib-cc2__XYfbGbV`（direct）：先 callees(_process_plot_var_args._make_line)，后 read 核对第 334 行起源码，补出工具未列出的 Line2D 构造调用及嵌套调用求值顺序，评分 100。其余两次实际 callees 查询走相同方向。
- `matplotlib-cc2__jMYiXP9`（flow，唯一未调用的符号样本）：grep 找 def _make_line，read 函数体后直接回答，评分 99。对题目给出的文件和这个短函数，原生读取已经给出完整证据。相同首请求的另一次 flow trial 使用了 callees，说明单次选择仍有波动。
- `matplotlib-37__VDEwDaU`（direct）：grep FontInfo/postscript_name → read _mathtext.py → grep info 字段 → read textpath/PDF/PS backend；6 次 grep、5 次 read，评分 95。它沿字段引用探索，未切换到直接调用边。
- `xarray-32__6gTNeT6`（flow）：即使已找到 from_variables/from_xindex/assign_coords，仍用 22 次 grep、23 次 read 分析变量构造和性能；没有关系查询。广义 flow 措辞没有改变该 trial 的探索路线。
- `sympy-38__sirsKsQ`（direct）：沿 _eval_determinant/Determinant.doit 追踪，未找到参考主链 minorEntry → minor → _minor，评分 13；另外三个同题样本也没有找到 minorEntry。零调用不能一律解释成“原生工具已经答对、因此无需调用”，入口误识别时也可能一直沿原生路径走下去。

**flow 定位句没有提高本轮采用率。** 概念题两版都是 0/6；符号题 flow 反而少 1 个调用。因此现有证据不支持仅把“direct relationships”拓宽到“control/data flow”就能解决采用问题。题型间不是同一道问题的随机改写，仓库与难度也不完全配对；不能把题型差异当成严格的单因素因果效应。图就绪提示保持原样，本轮没有检验它的作用。

**调用成功不等于证据完整或答案正确。** _make_line 的 callees 只返回两个项目方法，未列出源码中的 Line2D 构造调用；Agent 通过源码补齐。_get_func_order 的 callees 只返回 ode_order，不包含 Python 内建 max。`sympy-cc2__cVNDuT3` 虽也 read 源码，仍错误声称 max 不是函数调用，评分 62；`sympy-cc2__mfdc2Jq` 正确区分 ode_order/max，评分 96。图结果是已解析边集合，不代表所有 Python 调用表达式；这些轨迹与过度依赖结果集合的风险一致，但不能单独归因于图工具。整个运行期间没有修复或调优这些问题。

### 全部 trial 清单

C/E 为实际 callers/callees 次数；invalid 为执行前参数错误。无删样或评分过滤。

| 变体 | 任务 | Trial | Judge | C/E | invalid | 原生工具 |
|---|---|---|---:|---:|---:|---|
| direct | matplotlib-37 | `matplotlib-37__VDEwDaU` | 95 | 0/0 | 0 | grep×6, read×5 |
| direct | matplotlib-37 | `matplotlib-37__LKuJoAE` | 94 | 0/0 | 0 | grep×5, read×11 |
| direct | matplotlib-cc2 | `matplotlib-cc2__XYfbGbV` | 100 | 0/1 | 0 | read×1 |
| direct | matplotlib-cc2 | `matplotlib-cc2__Np89Ziu` | 100 | 0/1 | 0 | read×1 |
| direct | requests-cc1 | `requests-cc1__nTMeomg` | 97 | 1/0 | 1 | — |
| direct | requests-cc1 | `requests-cc1__v622qcZ` | 94 | 1/0 | 0 | — |
| direct | sympy-38 | `sympy-38__sirsKsQ` | 13 | 0/0 | 0 | grep×6, read×5 |
| direct | sympy-38 | `sympy-38__HunuVKH` | 31 | 0/0 | 0 | grep×5, read×4 |
| direct | sympy-cc2 | `sympy-cc2__cVNDuT3` | 62 | 0/1 | 0 | read×1 |
| direct | sympy-cc2 | `sympy-cc2__4ov7myt` | 89 | 0/1 | 1 | read×1 |
| direct | xarray-32 | `xarray-32__59T25nZ` | 100 | 0/0 | 0 | grep×8, read×16, glob×1 |
| direct | xarray-32 | `xarray-32__zeY79rv` | 91 | 0/0 | 0 | grep×11, read×13 |
| flow | matplotlib-37 | `matplotlib-37__hpwgTdJ` | 94 | 0/0 | 0 | grep×5, read×4 |
| flow | matplotlib-37 | `matplotlib-37__aeBDxKU` | 94 | 0/0 | 0 | grep×8, read×8 |
| flow | matplotlib-cc2 | `matplotlib-cc2__jMYiXP9` | 99 | 0/0 | 0 | grep×1, read×1 |
| flow | matplotlib-cc2 | `matplotlib-cc2__UVsGvYH` | 98 | 0/1 | 0 | read×1 |
| flow | requests-cc1 | `requests-cc1__FhcU3Ub` | 98 | 1/0 | 1 | — |
| flow | requests-cc1 | `requests-cc1__8maPKKF` | 99 | 1/0 | 1 | — |
| flow | sympy-38 | `sympy-38__hskx9SE` | 23 | 0/0 | 0 | grep×6, read×6 |
| flow | sympy-38 | `sympy-38__K97tydZ` | 27 | 0/0 | 0 | bash×1, grep×5, read×3 |
| flow | sympy-cc2 | `sympy-cc2__VytLAVx` | 86 | 0/1 | 0 | read×1 |
| flow | sympy-cc2 | `sympy-cc2__mfdc2Jq` | 96 | 0/1 | 1 | read×1 |
| flow | xarray-32 | `xarray-32__SLewd52` | 78 | 0/0 | 0 | grep×5, read×8 |
| flow | xarray-32 | `xarray-32__6gTNeT6` | 95 | 0/0 | 0 | grep×22, read×23, glob×1 |

### 证据与复核

小型校验清单：[caller-callee-guidance-contrast-evidence-20261113.json](caller-callee-guidance-contrast-evidence-20261113.json)，含 12 份任务报告 SHA256、24 个 trial 的原始请求/工具/递归会话/轨迹/setup/result 文件 SHA256、逐次关系查询参数与结果、执行前错误、上下文及索引身份。file_sha256 中 result.json 位于 trial 根目录，其他文件位于 agent 目录；raw_requests 给出相对路径。大体积原始文件和本轮一次性脚本保存在忽略目录 `runs/guidance-contrast-20261113/`。

可独立下载 Actions 原始证据（仅下载，不触发测试）：

```sh
gh run download 43378735002 -R zhanglei1949/zvec-grep --pattern 'caller-callee-*' --dir /tmp/zg-contrast/artifacts/direct
gh run download 43378755997 -R zhanglei1949/zvec-grep --pattern 'caller-callee-*' --dir /tmp/zg-contrast/artifacts/flow
```

切到冻结 harness 后，可对每版下载目录运行 `python -m zg_bench.tool_experiment aggregate --reports <variant-dir> --group relationships --phase guidance-contrast --task-set caller-callee --output <output-dir>`，仅离线汇总，不重调模型或 judge。不要把两版送入同一次 aggregate；身份校验会拒绝混合指导版本。

小样本、固定问题、模型输出波动和 GLM 自评限制了结论。这里确认了真实上下文、自然采用和具体错误路径；没有建立 caller/callee 对一般 E2E 质量的因果提升。
