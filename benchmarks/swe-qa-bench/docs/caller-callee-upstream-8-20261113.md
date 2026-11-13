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

## v2 完整结果（2026-11-13）

修复版 direct [43386788153](https://github.com/zhanglei1949/zvec-grep/actions/runs/43386788153)、flow [43386815338](https://github.com/zhanglei1949/zvec-grep/actions/runs/43386815338) 均 attempt 1 成功。8/8 题、32/32 trial、16/16 任务报告完整，零失败、零缺失、零 Harbor 额外重试。26 个零调用样本保留；不强制调用，不按评分或调用情况重跑。

冻结 harness `426d339a62e9d81b0af9ef4810b563108bdf60fd`，candidate `bfe5a0acefed7f1a4f46161a3c5da9fd3fdaec54`，实际安装包 SHA256 `8b67777ef3780f9cf6a95fd6c3a4c18e904aba2300b0cbbf9ce9c7418685e082`。指导仍为原来的 direct/flow 快照，未更改图就绪提示。索引 ignore SHA256 为 `c2b720c90c315d1f8b446b977d0df4a9784f1e05ac2707ee6f2ca5723b43a17a`。同仓库任务的索引构建身份一致；这里核对的是构建参数和身份，不声称分别构建的底层索引文件逐字节相同。

### 实际上下文与调用

原始 HTTP 记录共 201 次 Agent 侧请求，全部 200 且完整结束：169 次携带工具，32 次为无工具的后台标题请求。judge 请求不计入这 201 次。169/169 个携带工具的请求都实际包含 callers/callees（OpenCode 名称 zvec_grep_zvec_grep_callers / zvec_grep_zvec_grep_callees）及九项原生工具 bash/edit/glob/grep/read/skill/task/todowrite/write；没有 search 或网页工具。两项工具 description、安装 AGENTS 与 MCP instructions 均与冻结版本一致，两层指导都实际进入 system 消息；tool_choice 全为 auto。

请求均保持 glm-5.2、temperature=0、seed=42、enable_thinking=true、reasoning_effort=high、max_tokens=32000，未传 response_format。每道题同变体两次重复的首个请求逐字段相同；只把 flow 定位句替换成 direct 后，同题四个首请求也逐字段相同。后续工具选择和输出仍有波动。递归会话导出完整；本轮 32 个 trial 都只有根会话，没有真实子代理样本。

| 变体 | 调用过关系工具 | callers 次数 | callees 次数 | 零调用 | 平均 Judge |
|---|---:|---:|---:|---:|---:|
| direct | 3/16（18.75%） | 3 | 0 | 13 | 88.5625 |
| flow | 3/16（18.75%） | 3 | 0 | 13 | 89.6250 |

全部 6 次实际 MCP 调用均成功并返回非空已解析调用点；没有 MCP 失败、空返回、未知状态、执行前 invalid-tool 参数错误，未识别到 shell zg CLI 绕行。callees 在模型上下文中可用，但本轮没有被选择，因此本轮没有新增真实 callees 执行证据。

### 逐题结果

每格两次 trial 的均分；采用率含全部 trial。

| 任务 | direct Judge | flow Judge | direct 采用率 | flow 采用率 |
|---|---:|---:|---:|---:|
| reflex-46 | 78.00 | 76.50 | 0/2 | 0/2 |
| scikit-learn-39 | 85.50 | 90.00 | 0/2 | 0/2 |
| sympy-37 | 63.00 | 71.00 | 1/2 | 1/2 |
| django-42 | 91.50 | 92.50 | 0/2 | 0/2 |
| conan-17 | 100.00 | 100.00 | 0/2 | 0/2 |
| flask-43 | 100.00 | 98.00 | 0/2 | 0/2 |
| requests-45 | 91.00 | 91.50 | 2/2 | 2/2 |
| matplotlib-45 | 99.50 | 97.50 | 0/2 | 0/2 |

### 资源与解释

| 每 trial 均值 | direct | flow |
|---|---:|---:|
| Input tokens（含缓存） | 65887.38 | 56281.94 |
| Output tokens | 738.31 | 721.56 |
| 全部工具次数 | 6.625 | 5.625 |
| Agent 秒 | 22.91 | 21.33 |

flow 平均评分 +1.0625，总 input tokens -14.58%、output -2.27%、工具次数 -15.09%、Agent 耗时 -6.87%。两版关系工具采用率和调用数完全相同；这些均值差异不能解释为更多关系工具调用带来的收益。资源沿用完整 session-tree 口径，后台标题未进入持久会话，不计入表中 token。费用缺失，不能用含缓存的总输入 token 直接推算费用。

索引准备单列：direct 的 index_duration_seconds 总和 265.12 秒，flow 为 241.29 秒；各 8 次构建、8 次 seed 恢复。安装/恢复详情保留在 setup，不加进 Agent 秒。

**自然语言上游题能够触发 callers，但采用集中在两个任务，flow 没有提高采用率。** requests:45 的两版四个 trial 都先 grep TooManyRedirects，再 read 找到 resolve_redirects，随后 callers(resolve_redirects) 获得 Session.send 在 sessions.py:723/740 的调用点，再 read 核对。这里能观察到“原生发现入口 → 关系查询 → 源码验证”的链路。

sympy:37 两版各有一个 trial 调用 callers(_get_func_order)，返回 canonical_odes（systems.py:1069）和 _classify_linear_system（systems.py:1413），Agent 再读取这两处。`sympy-37__zZdxYrU`（direct，86 分）和 `sympy-37__mvgu4zH`（flow，96 分）都在调用之前已经通过 grep/read 找到了 _get_func_order；关系工具补充了消费者位置，不能把它记成“工具发现了入口”。

同题未调用的 `sympy-37__zpe2wAb`（direct，40）和 `sympy-37__5EQg8K9`（flow，46）沿 classify_sysode 的内联 max_order 循环解释，这条源码路径真实存在，但未覆盖参考答案指定的 _get_func_order 主链。入口选择差异在关系查询前已经出现；这组分差不能作为 callers 的因果增益。也不能把低分零调用一律解释成 Agent 已经获得了正确且充分的答案。

其余六题两版共 24 个 trial 都走原生路径。例如 `scikit-learn-39__pPgKM8c` 只 grep _BaseRidge 然后 read _ridge.py 的 fit 逻辑，95 分；`django-42__dCcLnhr` 读取 clean 和 Field.run_validators 两处实现，96 分；`flask-43__9DYNmqD` 通过两次 grep 加一次 read tag.py 解释字典分派，100 分；`matplotlib-45__vuj5jr6` 用 bash/grep/read 追踪 callable 对象、方法别名和 cycler 状态，99 分。它们说明原生读取能覆盖不少任务，但轨迹不能唯一说明模型的内部动机。

六次查询都是补充“谁调用已知符号”的入边，选择 callers 的方向正确；callees 则未被使用。没有证据支持只把定位句拓宽成 control/data flow 就能让这批题更常调用关系工具。上一轮自拟符号题为 direct 6/6、flow 5/6，本轮上游题两版都为 3/16；两套问题不同，只能描述采用差异，不能把自拟题调用率外推为真实任务收益。

本轮只有 relationships 组，没有无工具或 search 组，所以不能据此估计 caller/callee 本身对答案质量的因果提升。GLM 自评、八题定向选择和每题仅两次重复限制了结论。

### 全部 v2 trial

C/E 为实际 callers/callees 次数。所有评分和零调用均保留。

| 变体 | Trial | Judge | C/E | Input tokens | Agent 秒 | 原生工具 |
|---|---|---:|---:|---:|---:|---|
| direct | `conan-17__xqXXm4t` | 100 | 0/0 | 39502 | 18.64 | grep×3, read×1 |
| direct | `conan-17__iw4jNaB` | 100 | 0/0 | 39328 | 17.44 | grep×3, read×1 |
| direct | `django-42__dCcLnhr` | 96 | 0/0 | 41929 | 16.56 | grep×3, read×2 |
| direct | `django-42__AyXG2mm` | 87 | 0/0 | 45576 | 16.74 | grep×2, read×3 |
| direct | `flask-43__JVC4NiQ` | 100 | 0/0 | 46200 | 19.82 | grep×3, read×3 |
| direct | `flask-43__9DYNmqD` | 100 | 0/0 | 28927 | 11.46 | grep×2, read×1 |
| direct | `matplotlib-45__ZnZ9n75` | 100 | 0/0 | 177810 | 49.71 | bash×3, grep×6, read×10 |
| direct | `matplotlib-45__vuj5jr6` | 99 | 0/0 | 256555 | 65.98 | bash×7, grep×4, read×10 |
| direct | `reflex-46__UgjDfRz` | 82 | 0/0 | 27627 | 14.33 | bash×1, grep×2 |
| direct | `reflex-46__GD53HtU` | 74 | 0/0 | 21142 | 10.42 | grep×2 |
| direct | `requests-45__gJo99yq` | 92 | 1/0 | 46809 | 23.39 | grep×1, read×5 |
| direct | `requests-45__ZPLAUiU` | 90 | 1/0 | 46765 | 20.40 | grep×2, read×3 |
| direct | `scikit-learn-39__pPgKM8c` | 95 | 0/0 | 26955 | 15.02 | grep×1, read×1 |
| direct | `scikit-learn-39__4vaN2Np` | 76 | 0/0 | 26955 | 13.28 | grep×1, read×1 |
| direct | `sympy-37__zZdxYrU` | 86 | 1/0 | 95834 | 28.98 | grep×4, read×5 |
| direct | `sympy-37__zpe2wAb` | 40 | 0/0 | 86284 | 24.35 | grep×4, read×3 |
| flow | `conan-17__vU7jdRb` | 100 | 0/0 | 39482 | 17.25 | grep×3, read×1 |
| flow | `conan-17__6KFtLbn` | 100 | 0/0 | 39430 | 20.40 | grep×2, read×1 |
| flow | `django-42__HbfLvBV` | 93 | 0/0 | 33093 | 15.45 | grep×2, read×2 |
| flow | `django-42__dMBUiWX` | 92 | 0/0 | 30158 | 18.02 | grep×2, read×2 |
| flow | `flask-43__8VLaHF3` | 96 | 0/0 | 29073 | 12.63 | grep×2, read×1 |
| flow | `flask-43__np6e2Yv` | 100 | 0/0 | 29073 | 11.80 | grep×2, read×1 |
| flow | `matplotlib-45__LNUVaTL` | 100 | 0/0 | 159179 | 47.52 | bash×1, grep×7, read×5 |
| flow | `matplotlib-45__ZudYtvX` | 95 | 0/0 | 119383 | 40.51 | bash×1, grep×5, read×5 |
| flow | `reflex-46__myq9iDm` | 79 | 0/0 | 39432 | 14.46 | bash×1, grep×2, read×2 |
| flow | `reflex-46__peoedb9` | 74 | 0/0 | 40394 | 14.04 | bash×1, grep×2, read×1 |
| flow | `requests-45__vN7N2ja` | 90 | 1/0 | 46594 | 17.49 | grep×1, read×4 |
| flow | `requests-45__QjjfFYM` | 93 | 1/0 | 46529 | 17.97 | grep×1, read×4 |
| flow | `scikit-learn-39__bRtHTRv` | 87 | 0/0 | 73190 | 27.47 | grep×2, read×4 |
| flow | `scikit-learn-39__oaY3JJ6` | 93 | 0/0 | 27972 | 16.22 | grep×1, read×1 |
| flow | `sympy-37__5EQg8K9` | 46 | 0/0 | 64809 | 21.80 | grep×2, bash×1, read×3 |
| flow | `sympy-37__mvgu4zH` | 96 | 1/0 | 82720 | 28.31 | grep×4, read×5 |

### v1 保留记录与额外开销（不进入主比较）

首轮两版各完成 7/8 题，共 28 个 Agent trial；Django 的 4 个名义 trial 均在索引准备失败，各尝试 3 次，共 12 次失败的准备、8 次额外异常重试。失败 trial 没有 model-requests.jsonl，工具使用记为未知/未运行，不当作零调用样本。失败 setup 的 index_duration_seconds 未记录，保留为未知，原始日志仍有 CLI 输出。

首轮已完成样本：direct 2/14 trial 调用关系工具，共 2 次 callers；flow 2/14，共 5 次 callers，其中一次 No matches 后按建议更正 scope 再查。两版 callees 均 0。首轮真实 HTTP 共 175 次（147 次带工具），上下文检查无缺失。首轮 28 个已完成 Agent trial 的 token/时间是额外开销，不能从 v2 指标中消失，也不与 v2 混合成扩大样本。

| v1 单独记录 | direct | flow |
|---|---:|---:|
| 已完成 trial 平均 Judge | 87.57 | 88.71 |
| Input tokens 总和 | 862752.00 | 756757.00 |
| Output tokens 总和 | 9895.00 | 9679.00 |
| Agent 秒总和 | 337.02 | 297.54 |

### 证据与复现

[精简证据清单](caller-callee-upstream-8-evidence-20261113.json) 包含主结果 16 份任务报告 SHA256、32 个 trial 的 192 个原始文件 SHA256、逐次关系查询参数/结果、上下文、索引身份，以及独立 v1 的 28 个已完成 trial 和 4 个 setup 失败记录。file_sha256 中 result.json 位于 trial 根目录，其余文件位于 agent 目录。两个 cohort 的 raw_requests 分别以各自 runs 目录为根。

下载 v2 原始证据（仅下载，不调模型）：

```sh
gh run download 43386788153 -R zhanglei1949/zvec-grep --pattern 'caller-callee-*' --dir /tmp/zg-upstream/v2/artifacts/direct
gh run download 43386815338 -R zhanglei1949/zvec-grep --pattern 'caller-callee-*' --dir /tmp/zg-upstream/v2/artifacts/flow
```

v1 同样下载 43384378065、43384409264 到独立的 /tmp/zg-upstream/v1/artifacts/direct|flow，原始失败与 .retry-history 都包含在 evidence artifacts 中。切到各自冻结 harness 后，使用 `python -m zg_bench.tool_experiment aggregate --reports <variant-dir> --group relationships --phase upstream-8 --task-set caller-callee --output <output-dir>` 离线汇总；v2 每版完整，v1 会明确报 Django 缺失。不要混合不同变体、harness 或协议。

验证：本地 220 项 Python 检查通过（1 项 Docker 合同未启用），包含固定 OpenCode 的真实主/子代理模拟服务请求；59 项 Node 检查、actionlint、diff 检查通过。Actions 两版验证、真实 MCP 预检和 secret scan 均通过。运行期间没有改提示词、题目或候选代码；唯一修正是经证实的 PNG 附件排除，并明确升级协议、保留首轮所有记录。
