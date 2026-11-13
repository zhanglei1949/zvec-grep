# Caller/Callee 模型上下文诊断 v1

本轮只诊断工具进入上下文和自然选择，不重跑全量，也不强制调用。
旧的 300-trial 报告和提示词快照保留不变。

## 冻结设定

- 两组：`relationships` 和 `agent`（search + callers + callees）。
- 三题：`matplotlib-37`、`sympy-38`、`xarray-32`，每题每组 2 次，共 12 trial。
- 选题用于复查已知符号、跨文件调用追踪和此前的空关系返回，不用于估计全题集收益。
- OpenCode 1.18.4、custom-openai/glm-5.2、既有 endpoint、采样和评审协议；local Potion、graph 索引、相同原生工具和子代理、禁用网页工具。
- 每个 run 最多 2 个任务 job，每题 trial 串行；两组固定相同新候选及 harness SHA。
- 低分、未调用、空结果、模型请求缺工具或缺指导均保留，不因此重跑。沿用异常最多额外重试 2 次的基础设施策略。
- 正式阶段选择 `experiment_phase=diagnostic`；此模式只生成上述三题、每题 2 次。

两组从下句开始使用完全一致的关系指导：

> For a known symbol's direct call relationships, use callers for incoming call sites or callees for outgoing call sites. A known symbol does not require a preceding discovery query.

两组共享精确查找、图能力边界、源码核对、停止条件及“不强制调用”的后续指导。
差异只保留可用工具列表和入口发现方式：关系组用原生 Grep/rg，组合组可用 search。
新快照见 `fixtures/caller-callee-guidance-v2.json`；旧快照不覆盖。

## 记录实际请求

诊断模式启用 `--record-model-requests`。每个 trial 的容器内运行 Node 透明转发器，
只改 OpenCode 的 provider baseURL，将原请求字节转发到既有 upstream；模型、messages、
tools、tool_choice、采样参数和响应流不改写。不记录鉴权头或其他 HTTP 头。

`agent/model-requests.jsonl` 记录每次实际 HTTP 请求的完整 JSON body、原字节 SHA-256、
时间及响应状态/传输错误。它覆盖主 Agent、子代理和后台请求；不使用 MCP 预检列表冒充
模型收到的工具列表。原始 messages 保留 role、系统/全局指导、任务、历史消息和工具返回。
`model-recorder.json` 保存本地转发地址和 upstream，正常结束标记验证日志已完成。

`task-report.json` 的 `trials[].model_requests` 逐请求保存：

- 实际工具名称、缺失/额外 zg 工具、tool_choice。
- 完整冻结指导是否出现、出现的消息 role、messages 和 body 校验和。
- 根据历史 tool_call ID 能明确关联的 session ID；不能关联的不推断。
- 请求/响应状态、记录文件位置和校验和。
- 在进入 MCP 前因参数 JSON 无效而产生的 zg 尝试，单独列出，不伪装成成功执行。

后台无工具请求单独保留。若全部请求都没有工具，则标记上下文问题；不当作“自主不用”。
有原生工具但缺 zg 工具或指导的请求也是诊断发现，不触发重跑。

## 验证与解释规则

先通过本地固定版本 OpenCode + 模拟 endpoint 验证转发前后 JSON 一致，
主 Agent/general/explore 的工具与指导均被记录，且记录不含鉴权值；再执行 Actions 小样本。
Rust 工具可用性、图预检与上下文记录分开验证。

完成后按以下顺序分析，而非从零调用反推原因：

1. 实际请求是否有预期两/三个 zg 工具；首次工具请求和后续请求是否一致。
2. 统一指导是否进入实际 messages，处于什么 role；是否发生动态加载缺失。
3. 在上下文可见的前提下，原生定位符号后是否切换关系查询，以及调用前后的实际动作。
4. 未调用是入口未找到、原生阅读已经完成，还是没有明确证据解释；后者保留未知。
5. 空返回、参数错误和执行错误分别报告。调用行为关联评分/资源仅作描述，不作因果解释。

原始大体积产物保存在忽略目录和 Actions artifacts。协议、可复用采集/分析代码、
新指导快照和精简结论进入 Git。候选/harness SHA、run ID 及结果在执行后补记。

## 执行版本

候选及 harness：`7e271bab1b2730e0bda4f6d1ec81ef0430f768f1`。
关系组：[43348122347](https://github.com/zhanglei1949/zvec-grep/actions/runs/43348122347)；
组合组：[43348127237](https://github.com/zhanglei1949/zvec-grep/actions/runs/43348127237)。
两组均为 diagnostic 阶段，各 6 个 trial，均在首次 attempt 成功结束。

最初的 43347851994、43347856782 因误传短 SHA，在 checkout 阶段失败，
没有构建候选或运行模型；已使用完整 SHA 重新提交，不计入诊断样本。
原始失败日志及本轮产物保存在忽略目录 `runs/context-diagnostic-20261113/`。

本地验证：Rust CLI 184、MCP 73 项通过；Python 200 项完成（1 项默认 Docker 跳过）；
固定 OpenCode 的真实 HTTP 测试验证主/子代理请求完整转发、工具/指导实际记录，
鉴权值未落盘；actionlint、Rust 格式及 diff 检查通过。

## 结果：工具已经送达，关系组仍未采用

12/12 个 trial 的原始请求已审计，无上下文缺失、模型阶段失败或 Harbor 重试。
125 次 Agent 侧实际 API 请求均返回 HTTP 200，响应流完整结束；其中 113 次带工具请求均验证了：

- 关系组包含真实 `zvec_grep_zvec_grep_callers`、`zvec_grep_zvec_grep_callees`，组合组另外包含 search。
- 两组统一后的全局指导位于实际 `system` 消息；不是只检查安装文件。
- MCP 服务 instructions 也进入 `system` 消息，实际工具 description 与预检导出一致。
- 所有实际请求保留 model、messages、tools、采样参数；temperature=0、seed=42 等设置一致。

其余 12 次无工具请求逐一确认为标题生成，不误报为主 Agent 缺工具。
独立评审沿用原协议，不属于 OpenCode 请求记录器覆盖的这 125 次请求。
本轮没有实际子代理调用；主/子代理采集能力由固定 SDK 的模拟 endpoint 测试验证，
不将测试证据冒充本轮实际子会话。

| 组别 | Trial | 全部请求 | 带工具请求 | Search 执行 | Callers 执行 | Callees 执行 | 上下文问题 |
|---|---:|---:|---:|---:|---:|---:|---:|
| 关系组 | 6 | 78 | 72 | 0 | 0 | 0 | 0 |
| 组合组 | 6 | 47 | 41 | 8 | 1 | 0 | 0 |

关系组原生工具合计：grep 44、read 53、bash 2；组合组：grep 18、read 33。
唯一的关系查询成功返回非空结果，没有无效参数的 zg 尝试，也没有可识别的 shell zg 调用。
本轮没有因未调用、空结果或评分而重跑；旧的短 SHA checkout 失败发生在模型运行前。

### 逐 trial 上下文与行为

每行的带工具请求全部含有预期 zg 工具和 `system` 指导。

| 组别 | Trial | 全部 / 带工具请求 | Grep / Read / Bash | Search / Callers / Callees |
|---|---|---:|---:|---:|
| 关系组 | matplotlib-37__8BXAqH3 | 9 / 8 | 6 / 6 / 0 | 0 / 0 / 0 |
| 关系组 | matplotlib-37__wfJ2kMf | 15 / 14 | 4 / 9 / 0 | 0 / 0 / 0 |
| 关系组 | sympy-38__W8yXfkZ | 10 / 9 | 7 / 8 / 0 | 0 / 0 / 0 |
| 关系组 | sympy-38__dbS8cNp | 7 / 6 | 4 / 3 / 1 | 0 / 0 / 0 |
| 关系组 | xarray-32__sZ9tFKT | 12 / 11 | 7 / 9 / 1 | 0 / 0 / 0 |
| 关系组 | xarray-32__xT7VGTp | 25 / 24 | 16 / 18 / 0 | 0 / 0 / 0 |
| 组合组 | matplotlib-37__yzz3Uxy | 6 / 5 | 2 / 2 / 0 | 1 / 1 / 0 |
| 组合组 | matplotlib-37__SYfhbmV | 6 / 5 | 3 / 2 / 0 | 1 / 0 / 0 |
| 组合组 | sympy-38__mm2s5Rh | 11 / 10 | 4 / 9 / 0 | 2 / 0 / 0 |
| 组合组 | sympy-38__fkjWVLM | 9 / 8 | 4 / 6 / 0 | 2 / 0 / 0 |
| 组合组 | xarray-32__AW9gprE | 6 / 5 | 3 / 5 / 0 | 1 / 0 / 0 |
| 组合组 | xarray-32__Dee2WLc | 9 / 8 | 2 / 9 / 0 | 1 / 0 / 0 |

### 对“为什么调用/不调用”的解释

**可排除的原因：** 在本轮 12 个 trial 中，不能再解释为 zg 工具未暴露、全局指导未加载、
MCP instructions 丢失或模型接口请求失败。实际请求与后续轨迹均已保存，可逐请求验证。
这项结论只适用于有请求记录的新样本，不能补证旧的 100 个关系组 trial 的每次 HTTP 请求。

**已观察到的未调用路线：** 关系组通常沿“grep 定位符号 → read 定义 → grep 查用法 → read 下游”
完成问题，没有切换图查询。Matplotlib 的 `wfJ2kMf` 在找到 FontInfo 后仍通过字段用法和
backend 源码追踪传播；Xarray 的 `xT7VGTp` 使用了 16 次 grep、18 次 read，仍未查询关系。
这既不是调用失败后放弃，也不是工具名/参数错误造成的零成功次数。

**已观察到的调用路线：** 组合组 Matplotlib `yzz3Uxy` 首轮并行 grep 与 search，随后读取
`_mathtext.py`，再并行 grep 字段用法和 `callers(FontInfo)`。该查询返回
`TruetypeFonts::_get_info` 在 `_mathtext.py:394:16` 的调用点，耗时约 226ms，
随后继续阅读 PostScript backend。

同组同题的 `SYfhbmV` 也经过 grep + search 和同一源码阅读阶段，但随后继续 grep 字段用法，
没有调用关系工具。两次 trial 的首次 API 请求 JSON 内容逐字段一致，包含 messages、tools
及采样设置；行为仍不同。因此，Search 入口不是采用关系工具的充分条件，固定 seed 也不能
当作本次服务输出完全确定的保证。

**与轨迹一致、尚未证明因果的解释：** 原生检索和阅读是一条完整的替代路线；指导保留了
“精确查找足够时用 Grep”和“证据充分就停止”，没有额外要求图查询。题目又涉及字段传播、
性能开销和解释性数据流，超出了工具所提供的直接 call-site 边界，模型可能将源码阅读视为
充分证据。上述因素和入口发现后的路线选择都值得关注，但不能据此断言模型内部的唯一动机。

统一关系指导后，零调用仍在关系组复现，所以此前“关系组提示词较弱”的解释应降格为假设，
不能作为单一根因。本轮确认的是**能力已进入上下文，而自然采用仍然不足**；它不证明关系
查询没有价值，也不测量强制使用后的收益。没有追加全量运行或强制调用组。

## 证据与复现

[精简请求证据清单](caller-callee-context-evidence-20261113.json) 保存 6 份报告的校验和，
12 个 trial 首次实际工具列表、消息/请求文件 SHA-256、调用参数/返回及轨迹位置。
完整原始提示词位于每个 trial 的 `agent/model-requests.jsonl` 中 `type=request` 的 `body.messages`，
完整实际工具列表和描述位于 `body.tools`；不是用配置快照替代实际记录。

本地完整产物：`benchmarks/swe-qa-bench/runs/context-diagnostic-20261113/`。
`summary/report.json` 包含逐 trial 请求审计，`raw/` 包含 HTTP JSONL、原始轨迹和递归会话，
`audit.json` 保留独立逐请求复核结果。远端原始产物可从上述两个 run 下载：

```sh
gh run download 43348122347 --repo zhanglei1949/zvec-grep \
  --pattern 'caller-callee-evidence-diagnostic-*' --dir /tmp/zg-diagnostic/relationships
gh run download 43348127237 --repo zhanglei1949/zvec-grep \
  --pattern 'caller-callee-evidence-diagnostic-*' --dir /tmp/zg-diagnostic/agent
```

如需重现这套小样本，在候选/harness 7e271ba 上以 `experiment_phase=diagnostic` 分别 dispatch
relationships 和 agent；该模式固定三题、每题 2 次。不要选择 full。原有 300-trial 结果不与
这 12 个诊断样本合并，评分仅作描述，不以此小样本重估端到端增益。
