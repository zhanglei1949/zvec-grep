# 新 AGENTS.md 与 MCP 指导的全新 OpenCode 会话验证

执行前冻结协议。诊断目标为新指导的真实加载和明确调用指令的依从性，不估计自主采用率或整体增益。

- E2E：固定 Xarray:32 原题、仓库提交 `40119bf35c53fc222934894d842cfc07b2e264bf`，relationships 工具组。
- 产品 Rust 固定 `85cb58345dac16ac62d85ae99365869cebcc8304`，不改产品代码。
- OpenCode 1.18.4、GLM-5.2、temperature=0、seed=42、thinking=true、reasoning_effort=high、max_tokens=32000。
- 完整仓库及既有 graph/local Potion/bindings 0.5.0/index.ignore 协议不变。
- 5 个独立新会话，每次隔离 config/data/state/cache，不传 --continue 或 --session；不带任何历史 assistant/tool/reasoning 消息。
- 每个会话安装 AGENTS.md 后，把上一轮 READY 与 DIRECTIVE 文字写入 ZVEC_GREP 指导块内并读回核对。
  明确指定 Coordinates.update、定义文件和查询参数，因此是给定目标的指令诊断。
- MCP instructions 同步增加对应文字。使用 benchmark HTTP 适配器修改真实 initialize 返回的 instructions；
  MCP 使用原生工具名，AGENTS.md 使用 OpenCode 工具名，归一化 namespace 后全文一致。
  关系工具描述/schema/执行参数及真实 MCP 调用不变；工具列表含既有原生工具，API tool_choice=auto。
- 模型请求由 OpenCode 自行组装。记录器只转发原始 body 字节，先验证 AGENTS.md 与 MCP 指导均进入 system。
  首个答题请求必须只有 system/user 两条消息。后台标题请求单独保留，不当作答题请求。
- 先运行一次真实 OpenCode → 真 MCP → 本地模拟模型的合同检查；该检查不调用真实模型。
  合同失败则不启动真实会话。5 个真实会话允许完整答题，不从历史请求重放，不使用 API 强制工具选择。
- 无评分/judge 请求、无全量评测、无按调用结果补跑。失败和零调用均保留，以 5 个预定会话为分母。

保留修改前后 AGENTS.md、实时 MCP 工具面、请求原始 body/摘要、模型原始响应、OpenCode stdout、
完整会话/调用快照、关系工具参数与实际返回、最终答案、usage、索引和安装日志。
源码状态在会话结束后核对。批量数据保存在 Git 外并提供 SHA256 清单与 Actions 编号。

报告：首请求加载证据；第一个实际动作；是否发出指定关系查询；是否成功、非空且含正确相关位置；
5 个会话的完整答题 token/耗时单列，不与之前单步 token 混算，不据此声称评分提高或 token 节省。
