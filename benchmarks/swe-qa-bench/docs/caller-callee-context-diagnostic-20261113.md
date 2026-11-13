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
