# Caller/Callee guidance 对照实验与 symbol-anchored 功能题集运行说明

本文档说明如何运行 caller/callee guidance 变体对照（`direct` vs `flow`），以及独立的 16 题诊断集（十一道上游题与五道 symbol-anchored 题）。开发校准另用 dev fixture。题集和变体分别选择，QA20 冻结输入保持不变。

## 背景与目标

300-trial 正式实验（见 [caller-callee-e2e-results-20261112.md](caller-callee-e2e-results-20261112.md)）显示：关系组 0/100、组合组仅 9/100 调用 caller/callee。工具暴露和可调用性已有预检证据；概念题与 symbol-only 接口的匹配程度可能影响采用率，但现有结果尚不能确认低采用率的原因。本轮分别改变指导和题集以检验这一假说：

- **guidance 变体对照**：把 caller/callee 的定位从「直接调用关系」拓宽到「调用链上的控制/数据流追踪」，检验概念流追踪题的采用率是否变化。这是待验证假说，不是确定结论。
- **symbol-anchored 功能题集**：直接给确切 symbol 的「谁调用 X / X 调用谁」题，覆盖 exact hit、scope 限定、空返回、未命中建议等路径，用于验证 caller/callee 功能本身能否被正确驱动。这是「考到功能」的主杠杆。

## guidance 变体定义

唯一差异是 caller/callee 的定位句，其余逐字一致，且保留 `no tool call is mandatory` / `Stop when evidence is sufficient` 等非强制约束。

- `direct`（C0，默认）：`For a known symbol's direct call relationships, use callers for incoming call sites or callees for outgoing call sites.`
- `flow`（T1）：`For a known symbol, use callers for incoming call sites or callees for outgoing call sites, including to trace how control or data flows through call chains, locate where a symbol is invoked, or enumerate what a symbol invokes.`

`flow` 只拓宽语义定位，不加强制调用措辞；是否提升采用率需实验验证。Search 组无 caller/callee 句，变体对其无影响。

## symbol-anchored 功能题集

dev fixture（`fixtures/caller-callee-dev/`）扩充后的符号与考察点：

- `normalize`：callers 命中（`save` 与 `storage::save` 两个调用点）。
- `save`（顶层）：callers 命中（`import_record`、`export_record`）；callees 命中（`normalize`）。
- `import_record`：callees 命中（`save`）。
- `storage::flush`：scope 限定 callees（`storage::save`）。
- `orphan_leaf`：callers 空返回（无已解析调用点），验证「empty ≠ absence」。
- `WrongScope::normalize`：未命中 → 两级建议回退。

该题集仅供开发校准与功能验证，`revision_policy` 仍为「最多两轮、只修歧义/误用、不以评分或调用率优化」。

## 在另一台机器上运行

当前执行的 [24-trial 关系组对照协议](caller-callee-guidance-contrast-20261113.md) 使用新增 `guidance-contrast` 阶段；每个变体仅六题 × 两次。以下 smoke/full 为保留的其他入口，所有实验阶段均采集实际模型请求。

### 题集与阶段

| task_set | smoke | full | diagnostic |
|---|---|---|---|
| `qa20`（默认） | requests-16 × 1 | 20 题 × 5 | 三题 × 2，仅 direct |
| `caller-callee` | requests-cc1、requests-45、sympy-cc1，各 1 次 | 16 题 × 5 | 不支持（另有 guidance-contrast） |

`caller-callee` 必须选择 relationships/search/agent 之一。它是定向诊断样本，不能据其评分推断 SWE-QA 总体收益。冻结资产可先离线验证：

```sh
uv run --project benchmarks/swe-qa-bench python -m zg_bench.swe_qa validate-cc \
  --selection benchmarks/swe-qa-bench/zg_bench/swe_qa/data/selection-caller-callee.json \
  --references benchmarks/swe-qa-bench/zg_bench/swe_qa/data/references-caller-callee.json \
  --dataset benchmarks/swe-qa-bench/datasets-caller-callee
```

本地运行该题集时用 `zg-bench run caller-callee`；collect/aggregate 均追加 `--task-set caller-callee`。开发 fixture 不属于这 16 题，也不进入正式评分。

任务矩阵的 `smoke` 读取当前 selection 的 `gate.auto_tasks`，`all-full` 读取其全部任务，两者分别为三题和十六题。新增 `guidance-contrast` scope 只读取冻结的六题清单。

### E2E 索引协议（v1）

QA20 和 16 题集共用现有 E2E runner。每个任务 checkout selection 锁定的仓库 commit，在仓库根目录执行：

```sh
zg --index --embedding local/potion-code-16m-v2 --graph \
  --ignore-file .zg-bench-index.ignore
```

`.zg-bench-index.ignore` 由各 suite 的 `index_ignore_file` 上传：QA20 使用 `index.ignore`，16 题集使用 `index-caller-callee.ignore`。后者排除上传的 ignore 文件本身，以及经核实实际为 PNG 的 Django `tests/mail/attachments/file_png.txt`；该精确路径在八题实验 v2 修正中补入，之前冻结的 24-trial 仍使用其原版本。没有语言或目录白名单。参考答案、审计备注和本地 runs 证据不上传到任务仓库。

| 项目 | 本轮约定 |
|---|---|
| 文件范围 | 固定仓库中候选 zg 支持的类型，遵循候选默认扫描与 ignore 规则；不限 Python |
| 大小限制 | runner 不传覆盖参数；当前候选源码对代码默认 1MiB、数据 16MiB、其他可索引文本 256MiB；二进制、不支持类型和超限文件跳过 |
| 图与 embedding | 三组均启用 `--graph`，均使用 `local/potion-code-16m-v2` |
| 组间固定项 | 仓库 commit、候选包 SHA、embedding、suite ignore 文件及其 SHA；指导变体不改变索引参数 |
| 原始证据 | 保留 setup 中的候选身份、index stdout/stderr/status、ignore 摘要和 index seed 身份；跳过文件以候选输出为准 |

大小默认值来自当前候选源码的 `default_file_size_limit`，由冻结候选版本绑定；不能假定其他候选采用同一默认值。先前“仅 Python、单文件 2MiB”的审计与本轮协议不同。若要复现旧审计，应显式对齐过滤条件、大小限制和候选版本，作为单独协议运行，结果分别报告。

预检构建用于检查功能边界。正式 smoke 仍须使用实际冻结候选包，重新确认空边、误边和截断等观察；这些观察不作为评分参考答案。

### 参考答案修订（v2）

2026-11-13 从未发布的本地初稿中移出了 requests:cc1、matplotlib:cc1、sympy:cc1 的候选工具行为描述，保留在 `verification_notes`，并在 references 文件记录 `reference_revision=2`。三题的问题、源码事实和 pinned commit 保持不变；其他八题不改。judge 仅接收 `reference_answer`，不接收 `verification_notes`。

这是评分输入修订；后续各组须使用同一 references 版本和 harness commit。此次没有重评分已保存答复，也没有覆盖历史运行产物或已发布结果。

### 路径 A · 本地 dev 校准（最轻量，无需 Docker）

适合先验证「caller/callee 能否被驱动」。需要已构建的 `zg` 二进制；带 `--opencode` 时需 OpenCode 1.18.4 与 `GLM_API_KEY`。

```sh
uv sync --project benchmarks/swe-qa-bench --frozen

# direct（默认）：仅验证三组 MCP 工具暴露/可调用，无需模型
uv run --project benchmarks/swe-qa-bench python -m zg_bench.local_tool_preflight \
  --binary /path/to/zg --output /tmp/zg-preflight-direct

# flow：加环境变量即可切换变体（install 与 daemon 均继承）
ZVEC_GREP_GUIDANCE_VARIANT=flow \
uv run --project benchmarks/swe-qa-bench python -m zg_bench.local_tool_preflight \
  --binary /path/to/zg --output /tmp/zg-preflight-flow

# 带 --opencode：每组在 dev 题集上跑一次真实模型会话，导出 session-snapshot.json 供核对 caller/callee 调用
ZVEC_GREP_GUIDANCE_VARIANT=flow \
uv run --project benchmarks/swe-qa-bench python -m zg_bench.local_tool_preflight \
  --binary /path/to/zg --opencode /path/to/opencode --output /tmp/zg-preflight-flow-model
```

### 路径 B · 本地 e2e 单臂（需 Docker + GLM_API_KEY + local embedding + candidate 包）

镜像 CI 命令，先打通 smoke（requests-16 × 1）。`--guidance-variant` 必须与 `--mcp-toolset` 同用。

```sh
export GLM_API_KEY=<your-key>
uv run --project benchmarks/swe-qa-bench zg-bench run swe-qa-bench \
  --tier full --task requests-16 --agent opencode --model custom-openai/glm-5.2 \
  --profile zvec-grep --mcp-toolset agent --guidance-variant flow --record-model-requests \
  --n-attempts 1 --max-retries 2 \
  --embedding-model local/potion-code-16m-v2 \
  --zvec-grep-package /path/to/candidate.tgz \
  --jobs-dir runs/local/exp --job-name experiment-agent-flow-zvec-grep

uv run --project benchmarks/swe-qa-bench python -m zg_bench.tool_experiment collect \
  --runs-dir runs/local/exp --task requests-16 --trials 1 \
  --group agent --phase smoke \
  --candidate <candidate-sha> --harness <harness-sha> --run-url <local-note> \
  --output runs/local/report/requests-16
```

### 路径 C · GitHub Actions dispatch（推荐，免本地 Docker）

前置：fork 默认分支存在可 dispatch 的 workflow，`--ref` 指向包含本轮 harness 改动的版本；fork 配置 secret `GLM_API_KEY` 与 variable `SWE_QA_MODEL_BASE_URL`（填你自己百炼 workspace 的 OpenAI 兼容真实地址，例如 `https://<your-workspace>.cn-beijing.maas.aliyuncs.com/compatible-mode/v1`，不要用任何临时代理地址）；`embedding=local` 无需 embedding 凭证；触发者需 fork 的 admin/maintain 角色。详见 [run-in-fork.md](run-in-fork.md)。

```sh
# C0（direct）与 T1（flow）分别 dispatch；三组 relationships/search/agent 各自独立 run
gh workflow run swe-qa-bench.yml --repo OWNER/zvec-grep --ref main \
  -f candidate_ref=<含本轮Rust改动的sha或分支> \
  -f experiment_group=agent -f experiment_phase=full \
  -f guidance_variant=flow \
  -f model=glm-5.2 -f embedding=local -f scope=all-full
```

上述命令默认 `task_set=qa20`；运行 16 题集时追加 `-f task_set=caller-callee`。`diagnostic` 会把实际 surface 与冻结的 v2（direct）快照比对，因此 **flow 变体可用 smoke/full/guidance-contrast，不能用旧 diagnostic**；workflow 在启动任务前拒绝此组合。

## C0 / T1 对照协议

- `guidance_variant` 已纳入 aggregate 身份元组：`direct` 与 `flow` 报告不可混合，否则 aggregate 报「incompatible experiment reports」。
- 对同一 `task_set`、同一 `experiment_group` 分别跑 `guidance_variant=direct` 与 `flow`，各自 `aggregate` 得到独立 `report.json`，再比较两份报告的 `usage_rates`（caller/callee use_rate）与 judge/token。比较指导时固定题集；比较题集时固定指导，分别报告各题集结果。
- 主要观察指标：关系工具 use_rate、attempted/succeeded/empty/empty_unknown；QA20 另观察概念流追踪题（sympy-38/astropy-38/xarray-46/matplotlib-37）的采用率变化。调用行为分层仅作描述，不作因果解释。

```sh
# 下载某 cohort 的 task artifacts 后离线合并（不调模型）
uv run --project benchmarks/swe-qa-bench python -m zg_bench.tool_experiment aggregate \
  --reports /path/to/downloaded-task-artifacts --group all --phase full --output /path/to/report
```

## 中立性保障

- `flow` 是 opt-in 实验变量，默认 `direct` 与现行正式评测逐字节一致；未设置 `ZVEC_GREP_GUIDANCE_VARIANT` 时行为不变。
- 不为提升调用率/评分而加强制调用措辞；低/零调用保留，不按调用率重跑（遵守 dev fixture `revision_policy` 与 README「do not force tool use」）。
- symbol-anchored 题集独立于正式 20 题，不互相污染；同模型/采样/seed/judge，graph 索引强制开启并校验。

## 离线校验

从仓库根目录运行 Python 回归和共享 Node 校验：

```sh
uv run --project benchmarks/swe-qa-bench python -m unittest discover \
  -s benchmarks/swe-qa-bench/tests
node --test benchmarks/zg-retrieval/test/rust-package-cache.test.mjs \
  .github/scripts/bench-comment.test.mjs \
  .github/scripts/retrieval-pr-comment.test.mjs \
  benchmarks/zg-retrieval/test/workflow.test.mjs
```

从 `rust/` 运行候选侧校验：

```sh
cargo test --locked --offline -p zg-cli
cargo test --locked --offline -p zg-transport-mcp relationship
```

完整的 OpenCode/Docker 合同校验需要额外环境，见 SWE-QA Offline workflow。离线检查验证执行链路与输入约束；采用率和答案质量仍需真实模型 E2E 评测。

`candidate_ref` 必须包含 guidance 变体实现；探针会核对实际 MCP instructions 与 AGENTS.md，变体未生效时在模型运行前失败。模型 endpoint 使用所配置百炼 workspace 的真实地址。
