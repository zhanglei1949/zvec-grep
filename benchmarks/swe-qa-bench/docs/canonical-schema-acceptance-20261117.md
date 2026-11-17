# 规范关系 schema：真实链路最小验收

执行前冻结；目前仅完成本机无模型验证，尚未启动真实会话。

目标是确认 `/mcp/canonical` 能否使 OpenCode + GLM-5.2 的关系工具参数成功解析、到达 MCP 并返回可核实的信息。
这轮是显式给定目标的 E2E 链路诊断，不用于推断自然采用率或整体 SWE-QA 提升。

## 固定设置

- 沿用 Actions `43846728650` 的 Xarray:32 原题、源码提交 `40119bf35c53fc222934894d842cfc07b2e264bf`。
- OpenCode 1.18.4、GLM-5.2，temperature=0、seed=42、thinking=true、reasoning_effort=high、max_tokens=32000、tool_choice=auto。
- relationships 工具组，direct 基础指导、READY 信息和完整 JSON 示例均保持原样，包括旧格式字符串 `globs` 示例。
  新入口执行层仍接收这一格式。本轮不把示例同步改成规则数组，以便只改变模型可见的工具 schema。
- local Potion、bindings 0.5.0、完整仓库 graph 索引与原 ignore 文件不变；候选包固定为待执行快照的完整 SHA。
- 不修改 OpenCode/MaaS，不修复或重写模型输出、关系工具参数，不强制 API tool_choice。
- 旧入口完整指导与工具定义必须匹配上一轮原生快照；新入口只允许关系过滤 schema 改变。
  基线存于 `fixtures/caller-callee-canonical-baseline-20261117.json`，历史结果与旧 fixtures 不改写。

## 执行顺序

1. 构建当前候选，安装到隔离的 `/app`，建索引并核对就绪状态；实际安装 AGENTS.md。
2. 运行真实 OpenCode + 真 MCP + 模拟模型的预检。逐请求记录器在转发前确认 AGENTS.md、MCP instructions
   与 canonical schema 已进入实际模型请求；首个答题请求只允许 system/user。原字节转发，不改请求或响应。
3. 只启动第一个全新 GLM 会话。若回答未完成，或没有成功返回至少一条可独立核实的 Coordinates.update
   callee 关系，停止，不启动剩余四个会话。
4. 第一个会话通过后，再顺序启动四个新会话；后续遇到未完成会话也停止。任何失败或零调用均不补跑。

每个会话维持原 900 秒墙钟上限，新增一条预先声明的止损规则：连续三个不同 callID 的 `invalid` 动作，
且错误输入相同，即终止该会话。原事件、部分 SSE、数据库快照与已记录 usage 仍保存；未完成 usage 作为下界。
新增停止规则不进入模型上下文，也不作为与历史超时样本比较 token 改善的依据。

## 判定与分母

- 分别记录指导加载、实际 schema、模型选择、JSON 合法、MCP 到达、执行成功、非空、源码核实及回答完成。
- 函数定义、调用位置用固定源码 AST 核对；还需核对调用绑定。仅位置存在不算正确关系，例如 builtin len
  被误连到 StringAccessor::len。保留所有未验证、错误和正确边，至少一条正确关系才通过 pilot。
- 报告实际启动数与计划上限 5；未启动、未完成、没有 usage 与零调用分别列出，不把未知计为零。
- 对照上一轮五个会话，保留旧超时与缺失结果。比较输入/缓存/输出 tokens、完成率及错误模式；停止规则差异单列。
- 此链路诊断本身不运行 judge。若五个会话完成，回答评分需另用同一评分协议重评新旧保存答案，
  不沿用历史不存在的分数，也不把调用成功直接当成评分提升。

## 实现与证据

入口为 `python -m zg_bench.canonical_schema_e2e --output <目录>`，要求环境变量 `DIAGNOSTIC_CANDIDATE` 为完整 SHA。
候选、harness、包指纹、源码、ignore、实际文件/请求/原始 SSE/工具轨迹、答案、usage、源码核验均保存在运行 artifact。
单次运行的临时 workflow 模板保存在 Git 外，仅放入独立验收分支；不会修改主分支 workflow，也没有全量评测 job。

本机准备与预检证据：
`/Users/zhanglei/.codex/visualizations/2026/11/16/01a1e225-1c10-7bd1-ae5c-3941501311dd/canonical-schema-e2e-20261117/`。

当前本机没有 OpenCode 或 GLM 凭据，真实预检和会话需要已有 GitHub Actions 凭据。
准备好的代码尚未推送；实际运行后另存版本记录，不把准备阶段写成已完成 E2E。
