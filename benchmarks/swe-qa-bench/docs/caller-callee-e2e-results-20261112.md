# Caller/Callee E2E 三组评测结果

本轮不足以确认 caller/callee 带来稳定的端到端提升。组合组相对 Search 组平均评分 +0.82 分，描述性重采样区间跨过 0；总输入 token +6.19%、工具次数 +7.63%、Agent 耗时 +1.35%。未缓存输入减少 14.81%，金额变化未知。

关系工具只在组合组 9/100 个 trial 中被采用；关系组 100 个 trial 均未调用它们。独立预检和固定 OpenCode 的实际模型请求验证了工具暴露、可调用性和指导加载，因此未调用是本轮自主选择下的观测结果。三组正式运行全部成功，以下数据经过 300-trial 原始证据审计。

正式样本：20 题 × 5 次 × 3 组，共 300 个 trial。主表保留全部正式样本，不按分差、token 涨幅或工具调用行为筛选。

| 组别 | Trial | 平均评分 | 平均题内评分标准差 | 输入 token / trial | 输出 token / trial | 工具调用 / trial | Agent 秒 / trial |
|---|---:|---:|---:|---:|---:|---:|---:|
| 关系组 | 100 | 80.06 | 9.90 | 124,943.48 | 1,487.11 | 9.51 | 38.41 |
| Search 组 | 100 | 80.39 | 8.58 | 84,516.65 | 1,180.57 | 4.85 | 30.87 |
| 组合组 | 100 | 81.21 | 6.93 | 89,746.04 | 1,238.76 | 5.22 | 31.29 |

输入 token 包含缓存读取及写入；Agent 秒不含安装、索引和评审。资源统计覆盖数据库中持久化的主会话和递归子会话，不覆盖未持久化的后台标题等请求，不能作为完整 provider 账单。评审仅评价主 Agent 最终答案。评分是 GLM-5.2 自评的五维总分（5–100），并非人工正确率。自定义 provider 未提供的费用为未知，不按零费用处理。

## 组间差异

| 比较（前者 − 后者） | 评分差 | 输入 token 变化 | 输出 token 变化 | 工具次数变化 | Agent 耗时变化 | 逐题胜 / 平 / 负 |
|---|---:|---:|---:|---:|---:|---|
| 组合组 − Search 组 | +0.82 | +6.19% | +4.93% | +7.63% | +1.35% | 12 / 0 / 8 |
| 关系组 − Search 组 | -0.33 | +47.83% | +25.97% | +96.08% | +24.42% | 11 / 0 / 9 |
| 组合组 − 关系组 | +1.15 | -28.17% | -16.70% | -45.11% | -18.54% | 7 / 2 / 11 |

主要比较为组合组相对 Search 组；其余比较为补充。负的资源变化表示消耗降低。上述差异衡量工具集合和对应指导的整体效果，不将自然调用工具的 trial 视为随机处理组。

补充按题重采样：20 个配对题均分，10,000 次 bootstrap，seed=42，评分差的描述性 95% 百分位区间为 [-0.95, +2.77]。这不是预注册的显著性检验，也不证明固定选题具有随机代表性；不用于筛选或重跑 trial。

## zg 实际调用

| 组别 | 工具 | 使用 trial / 100 | 调用 | 成功 | 失败 | 未知状态 | 空返回 | 非空返回 | 返回内容未知 |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 关系组 | search | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 关系组 | callers | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 关系组 | callees | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| Search 组 | search | 90 | 119 | 119 | 0 | 0 | 0 | 119 | 0 |
| Search 组 | callers | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| Search 组 | callees | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 组合组 | search | 90 | 114 | 114 | 0 | 0 | 0 | 114 | 0 |
| 组合组 | callers | 8 | 11 | 11 | 0 | 0 | 1 | 10 | 0 |
| 组合组 | callees | 1 | 1 | 1 | 0 | 0 | 1 | 0 | 0 |

统计涵盖主 Agent 和递归子代理。成功表示工具执行完成；非空关系返回不代表目标符号的关系正确或充分。补充返回分类依据候选格式中的排名条目、`No matches.`、`No resolved call sites.` 和 `call at`；保留原采集器的未知分类，原始响应可复核。

| 组别 | 主会话工具调用 | 子会话工具调用 | 含可识别 shell zg 命令的 trial |
|---|---:|---:|---:|
| 关系组 | 938 | 13 | 0 |
| Search 组 | 477 | 8 | 0 |
| 组合组 | 522 | 0 | 0 |

MCP 暴露限制不构成 shell 隔离；shell 检测只覆盖可识别的直接 zg 命令，不能证明不存在任意包装或间接调用。

## 关系工具调用 trial 清单

以下 9 个 trial 均来自组合组主 Agent；其余 291 个 trial 未调用关系工具。清单只用于描述，不据此估计因果收益。

| Trial | 重复序号 | 评分 | 输入 token | Agent 秒 | callers / callees |
|---|---:|---:|---:|---:|---:|
| conan-19__rGJsnFr | 5 | 66 | 69,341 | 42.55 | 1 / 0 |
| matplotlib-37__cHdZNcH | 1 | 98 | 94,984 | 44.24 | 2 / 0 |
| matplotlib-37__Qszr57W | 2 | 99 | 164,211 | 41.60 | 1 / 0 |
| matplotlib-37__4ayuoVg | 4 | 94 | 135,457 | 55.54 | 1 / 0 |
| matplotlib-37__buDAxTB | 5 | 90 | 94,974 | 43.76 | 2 / 0 |
| sympy-38__eD6Yvt4 | 3 | 33 | 90,714 | 35.03 | 2 / 0 |
| xarray-32__zHcjW3Q | 3 | 91 | 83,149 | 36.54 | 1 / 0 |
| xarray-32__YuNDzgb | 5 | 91 | 80,194 | 41.36 | 0 / 1 |
| xarray-46__KsKYnUK | 1 | 96 | 178,641 | 42.71 | 1 / 0 |

全部调用与未调用 trial 的评分、资源和原始证据路径见 `full-audit/trial-inventory.json`；每个工具的调用/未调用描述性分层见 `full-audit/audit.json` 的 `descriptive_usage_strata`。

## 收益、退化与调用证据

下表均为组合组相对 Search 组的完整题均分差；用于说明异质性，不将调用行为视为随机处理。

| 题目 | 评分差 | 具体证据与限制 |
|---|---:|---|
| django-21 | +11.60 | 两组均未调用关系工具。最大的正向题均分差不能作为关系查询贡献的证据。 |
| sympy-38 | +10.20 | `sympy-38__eD6Yvt4` 的两次 callers 查询中，`_det` 返回矩阵入口的直接调用点及同名 Vector 符号；`MatrixDeterminant::det` 无已解析调用点。该 trial 33 分，组合题均分仍仅 25.60。 |
| matplotlib-37 | +1.20 | 4/5 个组合 trial 共调用 callers 6 次。`FontInfo` 返回 `TruetypeFonts::_get_info` 的调用点 `_mathtext.py:394:16`，提供了具体源码位置；小幅组间分差不等于单次查询的因果收益。 |
| xarray-32 | -2.00 | 不同 trial 分别查询 `Coordinates.from_xindex` 的 callers 和 callees；前者返回测试调用点，后者为空。两者均为 91 分，完整题均分由 94.80 降至 92.80。 |
| streamlink-43 | -8.00 | 两组均未调用关系工具。该退化完整保留，不能归因于关系查询，也未因低分剔除或重跑。 |

Conan 的调用证据也已保留：`conan-19__rGJsnFr` 查询 `DepsGraphBuilder._initialize_requires`，返回 `load_graph` 在 `graph_builder.py:44:14` 和 `60:28` 的调用位置；该 trial 66 分。工具返回非空调用点并不保证完整回答。

## 逐题评分与运行波动

每格为 5 次 trial 的平均分 ± 样本标准差。

| 题目 | 关系组 | Search 组 | 组合组 | 组合 − Search |
|---|---:|---:|---:|---:|
| astropy-38 | 71.40 ± 26.05 | 58.20 ± 28.58 | 58.80 ± 25.43 | +0.60 |
| conan-1 | 93.40 ± 4.16 | 92.60 ± 3.78 | 91.20 ± 2.59 | -1.40 |
| conan-19 | 94.00 ± 3.54 | 80.20 ± 12.34 | 82.60 ± 9.91 | +2.40 |
| conan-27 | 80.60 ± 9.45 | 82.60 ± 5.37 | 78.20 ± 8.61 | -4.40 |
| conan-39 | 45.40 ± 44.37 | 91.00 ± 7.55 | 95.00 ± 2.55 | +4.00 |
| django-21 | 93.00 ± 3.81 | 73.60 ± 14.03 | 85.20 ± 12.97 | +11.60 |
| django-32 | 93.20 ± 1.64 | 92.80 ± 2.49 | 94.60 ± 2.41 | +1.80 |
| matplotlib-37 | 94.40 ± 3.97 | 94.00 ± 2.55 | 95.20 ± 3.56 | +1.20 |
| pylint-10 | 34.40 ± 30.36 | 61.60 ± 17.37 | 62.60 ± 12.66 | +1.00 |
| pylint-14 | 82.40 ± 4.34 | 82.00 ± 7.87 | 82.40 ± 7.23 | +0.40 |
| pylint-9 | 89.80 ± 4.87 | 77.60 ± 15.19 | 79.40 ± 12.28 | +1.80 |
| reflex-6 | 87.60 ± 10.88 | 86.80 ± 2.17 | 87.60 ± 2.51 | +0.80 |
| requests-16 | 95.40 ± 3.21 | 97.80 ± 2.28 | 96.00 ± 4.24 | -1.80 |
| sqlfluff-2 | 86.40 ± 3.44 | 89.00 ± 4.85 | 88.00 ± 3.54 | -1.00 |
| streamlink-14 | 62.00 ± 20.35 | 56.60 ± 22.27 | 56.40 ± 8.99 | -0.20 |
| streamlink-43 | 92.20 ± 6.91 | 93.80 ± 5.22 | 85.80 ± 3.77 | -8.00 |
| sympy-26 | 90.60 ± 2.88 | 90.80 ± 3.27 | 88.60 ± 2.88 | -2.20 |
| sympy-38 | 27.00 ± 8.00 | 15.40 ± 7.27 | 25.60 ± 7.86 | +10.20 |
| xarray-32 | 93.20 ± 2.59 | 94.80 ± 3.42 | 92.80 ± 2.17 | -2.00 |
| xarray-46 | 94.80 ± 3.27 | 96.60 ± 3.71 | 98.20 ± 2.49 | +1.60 |

## 逐题资源差异：组合组相对 Search 组

斜线前后为 Search / 组合的每 trial 平均值。

| 题目 | 输入 token | 输入变化 | 输出 token 变化 | 工具次数 | Agent 秒 | Agent 耗时变化 |
|---|---:|---:|---:|---:|---:|---:|
| astropy-38 | 44,045 / 54,708 | +24.21% | +32.23% | 3.6 / 4.4 | 21.8 / 21.3 | -2.46% |
| conan-1 | 28,632 / 32,790 | +14.52% | -1.76% | 2.8 / 2.6 | 18.5 / 15.5 | -16.48% |
| conan-19 | 167,172 / 261,309 | +56.31% | +8.38% | 6.2 / 9.2 | 55.6 / 64.1 | +15.16% |
| conan-27 | 39,584 / 61,331 | +54.94% | -5.35% | 2.8 / 4.0 | 23.5 / 25.6 | +8.73% |
| conan-39 | 63,073 / 58,707 | -6.92% | -20.04% | 3.4 / 2.6 | 40.3 / 29.8 | -25.93% |
| django-21 | 79,646 / 70,029 | -12.07% | +9.99% | 3.0 / 3.6 | 28.6 / 29.1 | +1.58% |
| django-32 | 64,597 / 86,766 | +34.32% | +29.77% | 8.2 / 8.2 | 35.1 / 42.6 | +21.48% |
| matplotlib-37 | 127,088 / 124,391 | -2.12% | +29.17% | 7.0 / 9.2 | 43.8 / 46.2 | +5.49% |
| pylint-10 | 51,317 / 48,704 | -5.09% | -30.31% | 4.2 / 3.0 | 22.6 / 19.0 | -16.25% |
| pylint-14 | 8,571 / 9,981 | +16.45% | -4.65% | 0.0 / 0.0 | 9.9 / 11.0 | +10.86% |
| pylint-9 | 86,146 / 180,749 | +109.82% | +3.77% | 7.0 / 7.6 | 36.8 / 48.1 | +30.63% |
| reflex-6 | 93,781 / 91,488 | -2.45% | +12.54% | 5.8 / 5.8 | 26.7 / 28.0 | +5.23% |
| requests-16 | 77,143 / 64,638 | -16.21% | -4.33% | 4.0 / 2.8 | 28.1 / 25.5 | -9.26% |
| sqlfluff-2 | 72,933 / 76,344 | +4.68% | +15.48% | 4.8 / 2.8 | 25.5 / 27.6 | +8.35% |
| streamlink-14 | 40,705 / 49,893 | +22.57% | +26.39% | 1.6 / 1.8 | 22.2 / 25.2 | +13.38% |
| streamlink-43 | 58,443 / 58,955 | +0.88% | -12.44% | 5.2 / 4.2 | 23.2 / 21.0 | -9.48% |
| sympy-26 | 50,820 / 60,866 | +19.77% | -2.02% | 4.2 / 4.0 | 22.0 / 22.4 | +1.65% |
| sympy-38 | 85,585 / 97,507 | +13.93% | +23.38% | 3.0 / 8.8 | 25.4 / 32.4 | +27.60% |
| xarray-32 | 145,072 / 79,617 | -45.12% | -1.48% | 6.8 / 3.6 | 47.7 / 37.5 | -21.39% |
| xarray-46 | 305,981 / 226,146 | -26.09% | -4.40% | 13.4 / 16.2 | 60.1 / 54.0 | -10.08% |

## 缓存输入拆分

| 组别 | 未缓存输入 token / trial | 缓存读取 token / trial | 缓存写入 token / trial |
|---|---:|---:|---:|
| 关系组 | 9,925.24 | 115,018.24 | 0.00 |
| Search 组 | 12,972.33 | 71,544.32 | 0.00 |
| 组合组 | 11,051.64 | 78,694.40 | 0.00 |

组合组相对 Search 组的未缓存输入变化为 -14.81%，与总输入 token 的 +6.19% 不同。缓存定价和 provider 完整费用未知，不能从总输入变化直接推断金额变化。

## 索引与准备开销

| 组别 | 索引执行次数 | 索引秒合计 | seed 恢复次数 | seed 恢复秒合计 | 准备秒合计 |
|---|---:|---:|---:|---:|---:|
| 关系组 | 15 | 506.63 | 85 | 25.83 | 895.05 |
| Search 组 | 19 | 597.54 | 81 | 24.46 | 988.62 |
| 组合组 | 19 | 592.04 | 81 | 23.23 | 958.02 |

## 完成、失败与重试

独立审计核对 60 个任务 job、300 个最终 trial：全部有完整执行、评分及递归会话证据，最终异常为零、缺失为零。

| 组别 | 完成任务 | 完成 trial | Harbor 额外重试 |
|---|---:|---:|---:|
| 关系组 | 20 | 100 | 0 |
| Search 组 | 20 | 100 | 0 |
| 组合组 | 20 | 100 | 0 |

上表统计当前正式 cohort，历史失败及取消轮另行保留，不混入 300 个正式样本。历史调试总费用不可得；其不完整用量不能视为零。

## 版本与证据

候选：`f6b7c00269645bbb02f74ba07460957a773abb68`；harness：`c308c2ae182a9b25ca5ffb5a180258453275a490`。OpenCode `1.18.4`，`custom-openai/glm-5.2`，local Potion embedding，启用 graph。各组提示词以实际 surface SHA-256 固定：

执行 endpoint 为 `https://llm-67x4s810wr6kl2i4.cn-beijing.maas.aliyuncs.com/compatible-mode/v1`。请求固定 temperature=0、seed=42、enable_thinking=true、reasoning_effort=high、max_tokens=32000，不传 response_format。评审使用相同 GLM endpoint 和既有自评协议；密钥仅通过 Secret 注入。

- 关系组：[Actions 43232140196](https://github.com/zhanglei1949/zvec-grep/actions/runs/43232140196)；surface `24821aa43f0fc9173f112c40abe4aeaeb10546257879c75fbfaf5108bd5b2a81`。
- Search 组：[Actions 43232146223](https://github.com/zhanglei1949/zvec-grep/actions/runs/43232146223)；surface `00f4244c62fb6d44936b6e407d65a7601053ece91b333c8b14c6dd6285c5ec22`。
- 组合组：[Actions 43232152271](https://github.com/zhanglei1949/zvec-grep/actions/runs/43232152271)；surface `2e46faba186de9c137ee4e100ccbe247ab42c450c5b1476079e1a852419ac865`。

[冻结指导全文、工具描述与 schema](../fixtures/caller-callee-guidance.json) 与实际运行导出一致；各组策略摘要见 [协议](../README_CN.md#callercallee-三组-e2e-实验v1)。

候选包 SHA-256：`780e1e9fa8a328311d0604b229a577453a33caffe09ada4f97323ddbd128f9f7`。
共同 OpenCode 配置 SHA-256：`f52a7b94de67caf69613a5f2b09ee87ac2cb95f8282fdec6b9ab938cb70f4bab`。
索引 ignore SHA-256：`1c590c6f45e68bb88a78e0cc1aa2235360bc6070536eb1e0dcba2f686cc96cc9`。

本地证据根目录：`benchmarks/swe-qa-bench/runs/caller-callee-20261112/cohort-fits-c308c2a/`（Git 忽略）。`full-report/report.json` 保留逐 trial 评分、资源、完整调用参数/响应/时间/会话及原始证据路径；`full-report/report.md` 列出所有调用和未调用 trial；`full-evidence/` 保留原始会话与配置；`full-audit/audit.json` 保留独立核验结果和逐报告校验和；各组 `*-guidance.json` 保留实际指导全文、工具描述和 schema。远端对应每题的 `caller-callee-task-full-*`、`caller-callee-evidence-full-*` artifacts。

[精简证据清单](caller-callee-e2e-evidence-20261112.json) 固定全部 60 份任务报告的 SHA-256、run URL，以及合并报告和 300-trial 清单的校验和。原始调用均保留开始/结束时间、参数、返回文本、状态、会话 ID 和证据位置；主 Agent 与递归子代理分别核算，关联评分和资源。

在冻结 harness 的 checkout 中，可下载并重新合并原始任务报告，不需要重跑模型或评审：

```sh
evidence_root=benchmarks/swe-qa-bench/runs/reproduce-caller-callee-20261112
for spec in relationships:43232140196 search:43232146223 agent:43232152271; do
  group=${spec%%:*}
  run_id=${spec#*:}
  gh run download "$run_id" --repo zhanglei1949/zvec-grep \
    --pattern 'caller-callee-task-full-*' --dir "$evidence_root/full/$group"
  gh run download "$run_id" --repo zhanglei1949/zvec-grep \
    --pattern 'caller-callee-evidence-full-*' --dir "$evidence_root/full-evidence/$group"
done
uv run --project benchmarks/swe-qa-bench python -m zg_bench.tool_experiment aggregate \
  --reports "$evidence_root/full" --group all --phase full \
  --output "$evidence_root/full-report"
```

补充 bootstrap 的重现细节：按 task slug 排序的 20 个配对题均分差，使用 Python `random.Random(42).choices(..., k=20)` 重采样 10,000 次，取 `statistics.quantiles(..., n=40, method="inclusive")` 的首尾分位数。该补充分析在完整两组数据上定义，不参与运行选择。

复现：在上述 harness 使用 `swe-qa-bench.yml`，分别传 `experiment_group=relationships|search|agent`、`experiment_phase=full`、`candidate_ref=上述候选SHA`、`model=glm-5.2`、`embedding=local`、`scope=all-full`；每组 2 个任务 job 并发，每题 5 次串行，异常最多额外重试 2 次。密钥通过 Actions Secret 注入。

限制：固定 20 题和单一模型、自评评分、相同采样 seed、有限重复及自然工具选择，限制结论的外推和因果解释。预检和冒烟不进入正式样本。历史失败、取消及索引输入修正见 [执行记录](caller-callee-e2e-run.md)，不同版本的旧轮不混入本报告。

## 附录：旧筛选口径的敏感性分析

仅此附录剔除题均分差严格超出 ±10 分，或输入 token 变化严格超出 ±100% 的题；控制组输入为零且处理组非零时剔除。主表和主结论保留这些样本。

| 比较 | 保留题数 | 筛选后评分差 | 筛选后输入 token 变化 | 剔除题目 |
|---|---:|---:|---:|---|
| 组合组 − Search 组 | 17 | -0.42 | +0.53% | django-21, pylint-9, sympy-38 |
| 关系组 − Search 组 | 12 | -0.78 | +26.37% | astropy-38, conan-19, conan-39, django-21, pylint-10, pylint-9, streamlink-14, sympy-38 |
| 组合组 − 关系组 | 15 | -1.36 | -26.33% | astropy-38, conan-19, conan-39, pylint-10, pylint-9 |
