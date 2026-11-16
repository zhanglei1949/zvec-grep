# 完整 JSON 参数示例：五个全新 OpenCode 会话对照

执行前冻结。与 [上一轮真实文件加载诊断](caller-callee-fresh-guidance-20261116.md)
（Actions `43843188341`、harness `822fa1c6`）比较，唯一模型可见变化是把明确调用指令中的
裸参数子句替换为完整 JSON 示例，保留 globs：

```json
{
  "root": "/app",
  "symbol": "Coordinates.update",
  "globs": "xarray/core/coordinates.py",
  "limit": 20
}
```

指令前后文字、READY 信息、基础指导、工具描述/schema、参数执行策略全部不变。
AGENTS.md 指导块与 MCP initialize instructions 同步修改，归一化工具名前缀后全文一致。
旧重放 DIRECTIVE 保持原样，旧数据不修改或重新评分。

沿用 fresh-guidance-e2e：同一 Xarray:32 原题及源码提交
`40119bf35c53fc222934894d842cfc07b2e264bf`，relationships 组，Rust candidate
`85cb58345dac16ac62d85ae99365869cebcc8304`，OpenCode 1.18.4、GLM-5.2、
temperature=0、seed=42、thinking=true、reasoning_effort=high、max_tokens=32000、tool_choice=auto。
完整仓库的索引参数、local Potion、bindings 0.5.0、ignore 均沿用上一轮。

先运行相同的真实 OpenCode/真 MCP/本地模拟模型预检，不调用真实模型；通过后执行 5 个新会话。
每个会话独立 config/data/state/cache，开始前 session=0；首个答题请求仅 system/user。
逐请求校验新 AGENTS.md 和 MCP instructions 确实进入 system，再原字节转发，不修改模型输出或参数。
允许完整答题，保留全部失败，不按调用结果补跑；没有 judge 或全量评测。

主要比较：5 个会话中的模型选择意图、合法 JSON、到达 MCP、成功且非空、返回正确相关新增位置。
错误调用与 OpenCode invalid 分开记录。对 Coordinates.update 的返回逐条对照固定源码；
非空不直接视为有效。输入/缓存/输出 tokens、答题请求阶段耗时及最终答案保留，
小样本且给定目标，不据此推断自主采用率或质量提升。

保留实际 AGENTS.md、MCP 指导、首请求、完整 HTTP 请求/原始响应、OpenCode 调用和 MCP 请求/返回、
会话状态、答案、usage、包/源码/索引身份；批量数据在 Git 外，附 SHA256 清单和 Actions 编号。
