<p align="right">
  <a href="./README.md">English</a> | 中文
</p>

# SWE-QA benchmark

## 普通 zg／知识图谱 Augment 对照

独立协议 `sweqa20-zg-augment-agent-v1` 使用本目录的 Rust 框架，不改变下文
`baseline / zvec-grep` 的既有协议。两个新 profile 都使用 zg：`zg-off` 关闭图扩展，
`zg-augment` 固定使用 5 个种子、Calls 出边、1～2 跳和当前默认图预算。
本轮不开放 explore 或 Constrain，也不区分图结构与增加上下文量各自的贡献。

固定 OpenCode 1.18.4、DashScope `qwen3.7-max`（`enable_thinking=false`）、
Qwen3.7 Text Embedding 和独立 GLM-5.2 Judge（temperature=0）。题目、答案及
11 个仓库的提交沿用冻结的 SWE-QA20。smoke 为既有 5 题×两组×一次；正式运行
为 20 题×两组×三次，共 120 个独立会话。每次最多 1,800 秒，单并发，配对相邻、
交替执行，不自动重试。现有的 Claude 每次 4 美元限制不适用于这个 OpenCode 协议。

### 准备与执行

运行主机需要 Docker、Node.js 和本目录锁定的 Python 依赖。**候选包必须是 Linux
x64 Rust 本地包**，不能使用 macOS 包、TypeScript 包或仅含安装器的发布 meta 包。
在 Linux x64 构建环境的 `rust/` 中打包：

```sh
cargo build --locked --release -p zg --features knowledge-graph-graphdb
npm run pack:local -- --no-build
```

将生成的 `.tgz` 传到评测主机，然后从本 README 所在目录执行：

```sh
uv sync --frozen
source .venv/bin/activate
export DASHSCOPE_API_KEY="your-answer-model-key"
export ZVEC_GREP_API_KEY="your-embedding-key"
export GLM_API_KEY="your-judge-key"

python -m zg_bench.swe_qa.augment run \
  --package /absolute/path/to/linux-x64-candidate.tgz \
  --output /absolute/path/to/NEW_SMOKE --tier smoke --dry-run
```

`--dry-run` 验证资产和包内校验和，冻结 manifest、源码差异和调用矩阵，并生成命令；
不启动 Docker 或模型。所有输出目录必须不存在，dry-run 和实际运行使用不同目录。

```sh
python -m zg_bench.swe_qa.augment run \
  --package /absolute/path/to/linux-x64-candidate.tgz \
  --output /absolute/path/to/SMOKE --tier smoke
python -m zg_bench.swe_qa.augment collect /absolute/path/to/SMOKE
python -m zg_bench.swe_qa.augment judge /absolute/path/to/SMOKE
python -m zg_bench.swe_qa.augment report /absolute/path/to/SMOKE

python -m zg_bench.swe_qa.augment run \
  --package /absolute/path/to/linux-x64-candidate.tgz \
  --output /absolute/path/to/FULL --tier full --smoke /absolute/path/to/SMOKE
python -m zg_bench.swe_qa.augment collect /absolute/path/to/FULL
python -m zg_bench.swe_qa.augment judge /absolute/path/to/FULL
python -m zg_bench.swe_qa.augment report /absolute/path/to/FULL
```

正式运行要求 smoke 的完整性、评分和 usage 核对均通过，并使用相同候选包、配置及
评测源码。远程模型权重无法冻结，报告保留这一限制。Agent trial 不可覆盖或自动重跑；
重新实验使用新目录。Judge 单次失败记录 pending，不伪装为低分或成功。

### 数据隔离与计量

- 每仓库第一次准备时构建一次文本／向量／图索引，将其作为本轮规范副本；后续 trial
  复制同一副本。校验关闭重开后的图查询、查询前后存储摘要及两组 seed 身份。
- 两组使用同一个 MCP 代理，只有搜索工具可见，schema、描述和指引一致；模式和
  `autoUpdate=false / freshness=eventual` 由代理注入。搜索响应、错误、partial 和
  取消消息透传。直接通过 shell 调用 zg、访问索引存储或调用其他 zg 工具会使完整性失败。
- 参考答案只供主机侧 Judge 使用，不挂载给 Agent，不进入索引。禁止评测 Agent 委派子任务。
- 本地 HTTP usage relay 对两组同样启用，流式请求明确要求 `include_usage=true`。
  它只保存服务端 usage、请求序号和模型标识，不保存密钥、请求内容或模型正文；正文
  字节原样流向 OpenCode。服务端缓存包含在 prompt_tokens 中，只计一次。
- 同时核对 OpenCode 原始 step_finish 和 Harbor 0.18 的计数。Harbor 输入包含缓存读取、
  不包含缓存写入；核对时补足写入，主报告使用服务端总数。缓存明细未提供时标 N/A。
- 总工具调用包含失败调用和批量条目，另列 zg 搜索／原生调用；Agent 墙钟包含模型等待、
  搜索和文件读取，不含安装、索引、usage relay 启动、存储审计及 Judge。

### 报告解释与离线验证

`manifest.json`、`matrix.json`、`commands.json`、`outcomes.json`、`jobs/` 和
`collection.json` 保存运行身份及证据；`scores/` 保存逐次盲化评分。每份有效答案沿用
五维各 1～20 分的评分规则；无最终答案另记完成计入质量分 0，未评分答案保持 N/A。

`report/report.json` 和 `report/report.md` 给出逐题及总表：回答总分、正确性、完整性、
输入／输出／总 token、总工具／zg 调用及耗时。先对每题的三次运行取均值，再按题等权
汇总；百分比变化也是题级变化的等权平均，不是总数之比。零分母为 N/A，绝对差保留。
95% 区间采用固定种子的 10,000 次按题配对 bootstrap；重复运行不当作独立问题。
报告单列 task 均值的耗时中位数／p95、失败、usage 完整性、图错误、退化和工具使用率。

质量区间包含零时结论为不确定；效率不能抵消质量下降。准备失败、丢失证据或模式错误
不能产生有效收益结论；失败 trial 的已观测开销保留，缺失 usage 不填零。verifier 的
正 reward 只表示环境检查通过，不能当作答案正确。验收不要求正向结果。

`report/review.md` 为变化最大的最多三道改善题和三道退化题生成盲化源码复核包，
`review-key.json` 单独保留对应关系。复核人应对关键结论、引用记录源码路径／行号和
“支持／矛盾／证据不足”，保存独立复核记录，不改写答案或分数；报告不会自动宣称已完成人工复核。

采集和报告均只读已有试验输入，分别生成新文件，不再次调用回答模型。离线重算报告时
将同一 manifest、collection、候选包和原始证据及 scores 复制到新目录，再执行 report；
证据哈希变化会被拒绝。检查命令：

```sh
python -m unittest discover -s tests -v
node --test tests/augment_proxy.test.mjs
```

## 原有 baseline / zvec-grep 评测

此 benchmark 用于衡量 `zvec-grep` 对 Agent 回答代码仓库级软件工程问题的能力有何影响。发布结果中，两个 profile 使用相同的 Claude Code Agent、Claude Opus 5 模型、任务 prompt、代码仓库 commit、环境和限制：

- **Baseline：** Claude Code 使用其标准工具。
- **zvec-grep：** 同一个 Agent 获得准备好的代码仓库索引，并通过 MCP 使用 zvec-grep。

索引构建单独测量，不计入 Agent 执行耗时。

## Benchmark 定义

本 benchmark 使用 [`peng-weihan/SWE-QA-Bench`](https://github.com/peng-weihan/SWE-QA-Bench) 中固定的 20 个任务子集。Benchmark 输入锁定在此目录中：

- [`selection.json`](zg_bench/swe_qa/data/selection.json) 记录任务 ID、任务 slug、代码仓库 commit、资源哈希和 runner tier 成员关系。
- [`references.json`](zg_bench/swe_qa/data/references.json) 包含独立评审器使用的隔离参考答案，Agent 无法访问该文件。
- [`datasets/`](datasets/) 包含锁定的 Harbor 任务环境、prompt 和 verifier。
- [`swe-qa-bench.yaml`](suites/swe-qa-bench.yaml) 将本地数据集提供给 benchmark runner。

下文的验证命令会在模型运行前检查锁定的任务选择、代码仓库 commit、哈希以及参考答案的隔离状态。

## 发布测试配置

- **覆盖范围：** 20 个检索密集任务，覆盖 What、Where、How、Why、
  8 个意图和 11 个代码仓库。
- **Agent：** Claude Code `2.1.212`。
- **模型：** Claude Opus 5（`claude-opus-5`），high 推理强度。
- **Treatment Embedding：** Qwen3.7 Text Embedding
  （`qwen/qwen3.7-text-embedding`）。
- **Embedding Endpoint：** 使用下方本地配置中给出的 Qwen OpenAI-compatible
  endpoint。
- **运行次数：** 每个任务、每个 profile 独立运行 3 次。
- **预算：** 每个任务/profile 最多 USD 4.00。

Baseline 与 zvec-grep 除工具访问外保持完全相同。索引构建单独计时，参考答案对两个 profile 均不可见。

## 指标与报告

Job Summary 单元格使用 `baseline / zvec-grep / change`。每个任务的 baseline 和 zvec-grep 数值是对应 profile 三次 trial 的平均值。

| 指标 | 变化 | 含义 |
| --- | --- | --- |
| Judge | `zvec-grep - baseline` | 正值表示评审质量更高。 |
| `input_token`、`toolcall`、Agent 执行耗时 | `(zvec-grep - baseline) / baseline` | 负值表示资源用量更低。 |

如果 baseline 分母为零，对应的效率比较结果为 `N/A`。索引设置时间与 Agent 执行耗时分开统计。

在 Aggregate 行中：

- Judge 数值是所有任务的等权平均值。
- Baseline 和 zvec-grep 的效率数值是各任务 profile 平均值之和。
- 展示的效率变化是任务级变化的等权平均值，而不是聚合总和的比值。
- 结果为 `N/A` 的任务只会从受影响的 Aggregate 指标中排除。

## 本地配置

Harbor 使用 Docker 运行锁定的任务环境。为了得到可比较的结果，请保持主机平台、Claude Code 版本和模型服务配置一致。

安装以下前置依赖：

- [uv](https://docs.astral.sh/uv/)
- Docker Engine 或 Docker Desktop，并支持 Docker Compose v2
- Node.js 22 或更新版本以及 npm

验证 Docker Compose、安装锁定的依赖，并导出 Claude 与 Embedding 凭证：

```sh
docker compose version
npm ci

cd benchmarks/swe-qa-bench
uv sync --frozen
source .venv/bin/activate
export ANTHROPIC_API_KEY="your-anthropic-api-key"
export ZVEC_GREP_API_KEY="your-qwen-embedding-api-key"
export ZVEC_GREP_EMBEDDING_ENDPOINT="https://llm-67x4s810wr6kl2i4.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/embeddings"
```

先验证锁定资源，再运行 profile-aware 预检：

```sh
python -m zg_bench.swe_qa validate \
  --selection zg_bench/swe_qa/data/selection.json \
  --references zg_bench/swe_qa/data/references.json \
  --dataset datasets
```

```sh
zg-bench doctor \
  --agent claude-code \
  --model claude-opus-5 \
  --profile all \
  --embedding-model qwen/qwen3.7-text-embedding \
  --embedding-endpoint "$ZVEC_GREP_EMBEDDING_ENDPOINT" \
  --zvec-grep-package ../..
```

本地 package 路径依赖上文在代码仓库根目录运行的 `npm ci`。

## 本地 smoke test 和 dry run

使用以下命令查看锁定的任务选择：

```sh
zg-bench list tasks swe-qa-bench --tier smoke
zg-bench list tasks swe-qa-bench --tier full
```

首先 dry run 5 个任务、每个 profile 三次 trial 的配置：

```sh
zg-bench run swe-qa-bench \
  --tier smoke \
  --agent claude-code \
  --model claude-opus-5 \
  --profile all \
  --n-attempts 3 \
  --embedding-model qwen/qwen3.7-text-embedding \
  --embedding-endpoint "$ZVEC_GREP_EMBEDDING_ENDPOINT" \
  --zvec-grep-package ../.. \
  --dry-run
```

移除 `--dry-run` 即可执行 5 题 smoke run；将 `--tier smoke` 改为
`--tier full` 可运行发布测试对应的 20 题配对 Agent 协议。Runner 会为
Baseline 和 zvec-grep 同时固定 Claude Code `2.1.212`、Claude Opus 5、high
推理强度和每个 profile USD 4.00 预算。Harbor 轨迹和 verifier 输出写入
`runs/`；该命令不会自动重建发布结果中的 LLM Judge 与聚合报告。

## 诊断失败的运行

当 trial 记录异常时，`zg-bench` 会输出结构化的 Agent 或 zvec-grep 配置错误，并以非零状态退出。使用以下命令检查保存的 run：

```sh
zg-bench diagnose --latest
zg-bench diagnose <job-name>
```
