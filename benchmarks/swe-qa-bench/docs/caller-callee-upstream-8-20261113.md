# 八道上游关系题 direct/flow E2E 对照 v2

本轮运行此前从 SWE-QA-Bench 筛出的全部八道上游关系题，只使用 relationships 组，direct/flow 各重复两次，共 32 个独立 trial。它们与前一轮 24-trial 的三道 QA20 概念题和三道自拟符号题分别报告。

## 冻结协议

| 固定任务 | 关注点 |
|---|---|
| reflex:46 | 事件参数规格解析函数的调用位置 |
| scikit-learn:39 | Ridge 求解器选择与底层 helper 调用 |
| sympy:37 | _get_func_order / ode_order 数据流 |
| django:42 | ModelMultipleChoiceField 自定义验证器执行 |
| conan:17 | preset writer 参数到配置生成器的委托 |
| flask:43 | JSON 解码后递归 untag 与字节解码的动态调用 |
| requests:45 | TooManyRedirects 与重定向调用链 |
| matplotlib:45 | 连续 plot 的属性循环和样式状态传播 |

固定任务清单由 `upstream-8` scope 明确列出，并贯通 workflow matrix、collect 和 aggregate；不会加入自拟题、原 QA20 三题或其他题目。八道问题、锁定仓库 commit、参考答案均沿用当前 selection/references revision 3，原样使用，不根据本轮工具输出或评分改写。参考答案与 verification_notes 继续对答题 Agent 隔离；只有 reference_answer 传给 judge。

候选固定为 `bfe5a0acefed7f1a4f46161a3c5da9fd3fdaec54`，沿用前一轮候选和完整指导快照 `fixtures/caller-callee-guidance-contrast-v1.json`。两份 run 使用同一冻结 harness；唯一指导差异仍为 direct/flow 的关系定位句，同时作用于安装 AGENTS 和 MCP instructions。工具实现、description、图索引就绪提示、原生能力、任务预算和评审协议不改。

两版都只暴露 callers/callees，保留原生工具和子代理，禁用网页工具。OpenCode 1.18.4、custom-openai/glm-5.2、用户给定百炼 endpoint、local/potion-code-16m-v2、启用 graph 的索引和 suite ignore 文件均固定；索引准备耗时单列。每 run 最多两个任务 job 并发，每题内 trial 串行，两版并发 dispatch。

每个 trial 记录真实 HTTP messages/tools、工具 description、指导角色、采样参数、根及递归子会话、所有 zg 调用参数/结果/状态和可识别的 shell zg 绕行。后台标题请求与 judge 请求区分统计，认证头不落盘，上传前进行既有 secret scan。

Agent 自主选择，保留零调用、低分和空返回；不按调用或评分重跑。仅异常最多额外重试两次，保留失败尝试；执行前工具参数错误与真正 MCP 调用失败分开计数。运行中不调优提示词、实现或题目。不重跑原 QA20 全量，不与旧样本合并。

## 指标与证据

主要观察：调用过 callers/callees 的 trial / 全部 trial，分别报告两工具的调用、成功、失败、空结果和未知状态。逐题、逐 trial 报告评分、token、工具数和耗时；费用未返回则保持未知，不按分差或 token 涨幅排除任务。按调用行为划分仅作描述，不作因果解释。

先校验实际上下文与完整指导快照，再从轨迹说明概念/入口发现如何接到关系查询、是否正确选择方向、关系结果不足时是否核对源码，以及未调用时的原生路径。分别披露失败、缺失、重试、未调用和能力绕行。小样本和 GLM 自评限制结论，不能把调用率本身当作质量提升。

原始产物和本轮一次性脚本保存在忽略目录 `runs/upstream-8-20261113/` 与 Actions artifacts；运行 ID、harness SHA、结果和精简校验清单完成后补充到本文。原有 24-trial 报告和原始记录完整保留。

启动前校验：220 项 Python 回归通过（仅 Docker 合同项因未启用环境而跳过），包含锁定 OpenCode 1.18.4 对本地模拟服务的真实主/子代理请求检查。actionlint、diff 检查、八题矩阵、题目 hash、仓库 commit 和参考隔离校验通过。selection、references 和指导快照逐字节未改；仓库 admin/maintain 权限及 GLM_API_KEY Secret 名称已确认。

## v1 索引失败与 v2 修正

首轮 v1 的 direct [43384378065](https://github.com/zhanglei1949/zvec-grep/actions/runs/43384378065)、flow [43384409264](https://github.com/zhanglei1949/zvec-grep/actions/runs/43384409264) 使用 harness `d570e8546f75278bc9e1af87182b105907544164`。两版 Django 索引均因 `tests/mail/attachments/file_png.txt` 无法按文本提取而失败，Agent 尚未运行；沿用异常重试后仍失败。其他任务继续运行，原始产物保留在 `runs/upstream-8-20261113/` 及原 run，不按评分或调用率选择样本。

已从 pinned Django commit 下载该文件独立核实：144 字节、PNG 魔数 `89504e470d0a1a0a`，SHA256 `138b31cf5ec7b3109fe9a0262620b0b0b5a9d5975fd7c3fd3f5de81383db266a`。原 QA20 的 index.ignore 已包含此精确路径，关系题集的 ignore 文件此前遗漏。v2 只补入该文件路径，不跳过其他提取错误，不改问题、参考答案、指导、模型或候选代码。

ignore 摘要已在 index seed 身份和缓存 key 中，因此修复后不会复用不兼容索引。为保持全部任务和两版指导使用同一 harness/索引协议，重新 dispatch 本轮 8 题 × 2 次 × 2 版，作为独立 v2 队列；不与 v1 的部分结果拼接，不重跑 QA20 全量。v2 原始数据放在 `runs/upstream-8-png-20261113/`。这种重跑由索引准备故障触发，与未调用、空返回或低分无关。
