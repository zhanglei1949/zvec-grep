# 给定函数能力检查与 Xarray 单步重放，2026-11-16

执行前冻结的诊断协议。产品 Rust 源码保持 `85cb58345dac16ac62d85ae99365869cebcc8304`。
不修改产品代码，不跑全量，不重新回答或评分三题。

第一阶段为 retrieval-only：沿用 matplotlib:37、sympy:38、xarray:32 的固定仓库提交，
完整仓库、`local/potion-code-16m-v2`、graph=true、bindings 0.5.0、原 suite ignore。
17 个人工给定且先经固定源文件 AST 验证的函数，双向查询，共 34 次 MCP tools/call；
limit=20，globs 限定定义文件。所有空结果、错误、截断都保留，不为获得正结果增加查询。
原始请求、响应、工具面、安装指导、索引状态和固定源文件都输出到制品。

人工复核将分别记录：位置是否对应真实调用、是否与题目相关、是否提供此前分叉上下文没有的位置。
同名不能证明绑定正确；抽象方法、动态分派及字段传播不可按静态图空结果推断不存在。
这是给定正确函数的工具能力诊断，不能算自主 E2E 收益。

第二阶段只选最新 relationships Xarray 实际 trial `xarray-32__wp7vyRX`，
来源 Actions run `43824775591` 的 request id=16（第 15 个带工具请求）。
其上一条结果为 common.py:655–684，包含第 664 行 `data.coords.update(results)`；
原本下一步为 rg 查找 coordinates.py 的 `def update`。
整个原始请求捕获文件 SHA256 为
`2b151a1e8c17c2f988335619f31a6c8359c8c7591f39888c0577d5ea80247033`，
单条请求 body SHA256 为
`a497d31eae098b2c2dd854cd5ca7748a9fced2ecc20d1630c93012b11cac8c75`。

2×2 条件：baseline 原文、ready 只补图就绪事实、directive 只补明确调用指令、both 两者都补。
ready 说明 /app 已构建图且已通过就绪检查，无需重建或额外授权；
directive 明确指定下一步调用 `zvec_grep_zvec_grep_callees`，
参数 root=/app、symbol=Coordinates.update、globs=xarray/core/coordinates.py、limit=20。
因此 directive 同时提供正确目标，是显式指令依从性诊断。
附加文字只追加到首条 system 消息，其他消息、工具 schema、模型参数都不变；
baseline 恢复原始序列化字节并验证 SHA256。

每条件 5 次，按 repeat 轮转 baseline → ready → directive → both，共 20 请求。
GLM-5.2、temperature=0、seed=42、max_tokens=32000、thinking=true、reasoning_effort=high、
tool_choice=auto、stream=true 都保持原始值。不强制 API tool_choice，不进行结果导向重试。
每请求仅生成一个 assistant completion；执行其发出的只读工具动作后立即停止，不将结果回灌模型。
工具列表保留原文，写入/子代理等非只读动作记录但不执行。
原生工具使用受限的诊断执行器；不是从 OpenCode 数据库恢复完整会话。
这一差异发生在模型决策之后，原始请求直接重放给同一模型端点。

分母包括全部预定请求；HTTP、流中断和工具错误独立报告，不能混作“无调用”。
报告每条件关系工具选择数/5、目标及参数是否正确、调用是否返回有效位置、下一步工具和 usage；
缓存 token 另列。单步 token 不代表完整答题节省，5 次同 seed 不是独立大样本。
所有批量数据保存在 Git 外，并给出 Actions run 与本地绝对路径及 SHA256 清单。

执行：在隔离评测分支运行现有 SWE-QA Bench workflow，phase=oracle-capability，固定 candidate；
完成并审查结果后再 phase=branch-replay，`diagnostic_capability_run` 指向前者成功的 run。
回放作业验证前阶段仓库/候选/embedding/binding/ignore 身份相同。

执行记录：首轮单步重放 20/20 HTTP 200，但 runner 缺少 ripgrep 导致 18 个动作执行失败，
另 2 个动作使用 `head -20`，被受限执行器记录为不支持。原始模型决策及错误全部保留。
补充 `branch-execute` 阶段只下载已保存的 20 个 completion、核对请求摘要、
在同版本 /app 源码执行这些只读动作；不提供模型密钥，也不产生额外模型请求。
执行器增加 `head -20` 支持并保留原生匹配参数；这些修复不影响已经取得的工具选择数据。
