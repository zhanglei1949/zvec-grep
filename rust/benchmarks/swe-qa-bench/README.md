<p align="right">
  English | <a href="./README_CN.md">中文</a>
</p>

# SWE-QA benchmark

## Three-arm Agent experiment

The independent **`sweqa20-zg-agent-choice-v1`** protocol reuses the pinned
SWE-QA20 questions, references and 11 repository revisions. It adds no evaluation
questions and preserves both earlier protocols and their output formats.

| Arm | Available zg tools | Query policy |
| --- | --- | --- |
| `zg-off` | search | The existing proxy injects `graph.mode=off`. |
| `zg-augment` | search | The existing proxy injects five seeds, outgoing Calls, 1–2 hops and the original budgets. |
| `zg-agent-choice` | search + explore | The Agent chooses ordinary search, Augment, Constrain or Explore using the real product guidance and parameter schemas. |

All arms retain native reading tools. Choosing ordinary search or native reads
alone is valid. The autonomous proxy preserves graph parameters, explicit scopes
and execution budgets; it freezes only index lifecycle and credentials. Fixed
arms retain their original proxy and instructions. Each repository has one
canonical index per experiment; all arms copy and verify it before and after
queries. Profile names are excluded from index identity.

The configuration remains OpenCode 1.18.4, Qwen3.7-max with thinking disabled,
the existing embedding and independent GLM-5.2 Judge. Trials run serially with
a 1,800-second limit and no automatic retries. Smoke is **5 × 3 × 1 = 15**
sessions; full is **20 × 3 × 3 = 180**. Same-question arms run consecutively,
rotating order by question and repetition.

### macOS offline acceptance

**Implementation and offline acceptance are complete on macOS. Real Agent,
model and Docker runs remain for Linux.** From this directory, with the native
Rust prerequisites configured as described in [`rust/README.md`](../../README.md):

```sh
uv sync --frozen
LITELLM_LOCAL_MODEL_COST_MAP=True HF_HUB_OFFLINE=1 \
  uv run --frozen --no-sync python -m unittest discover -s tests -v
node --test tests/augment_proxy.test.mjs tests/choice_proxy.test.mjs
cargo test --manifest-path ../../Cargo.toml -p zg-transport-mcp \
  --features knowledge-graph-graphdb \
  --test swe_qa_tool_catalog --test swe_qa_agent_tool_catalog
```

Tests use synthetic trial logs and mock usage, answers and Judge responses;
these are plumbing fixtures, not additional benchmark data. They cover mode
omission, all four query choices, errors, ambiguity, empty/partial/degraded
results, pagination, cancellation, request pairing, snapshot drift and missing
usage/scores. The Mac dry-run test builds a test tarball with a Linux ELF header
and exercises the real Linux x64 package validator without launching it.

`tests/fixtures/mcp-agent-tools-graphdb.json` captures real `initialize`
instructions and complete `tools/list`. Rust checks freshness; Python validates
the public examples against JSON Schema. The old search-only fixture remains.
For intentional contract changes, prefix the Cargo command with
`ZG_BENCH_UPDATE_MCP_FIXTURES=1`, review both fixtures, then rerun normally.

### Linux execution

Build a Linux x64 GraphDB-enabled candidate in `rust/`:

```sh
cargo build --locked --release -p zg --features knowledge-graph-graphdb
npm run pack:local -- --no-build
```

From this benchmark directory, use the locked dependencies, Docker and Node.js,
then export `DASHSCOPE_API_KEY`, `ZVEC_GREP_API_KEY` and `GLM_API_KEY` as for the
fixed Augment protocol. Keep the package, endpoint configuration and benchmark
sources unchanged between smoke and full runs.

```sh
python -m zg_bench.swe_qa.choice run \
  --package /absolute/path/to/linux-x64-candidate.tgz \
  --output /absolute/path/to/CHOICE_DRY_RUN --tier full --dry-run

python -m zg_bench.swe_qa.choice run \
  --package /absolute/path/to/linux-x64-candidate.tgz \
  --output /absolute/path/to/CHOICE_SMOKE --tier smoke
python -m zg_bench.swe_qa.choice collect /absolute/path/to/CHOICE_SMOKE
python -m zg_bench.swe_qa.choice judge /absolute/path/to/CHOICE_SMOKE
python -m zg_bench.swe_qa.choice report /absolute/path/to/CHOICE_SMOKE

python -m zg_bench.swe_qa.choice run \
  --package /absolute/path/to/linux-x64-candidate.tgz \
  --output /absolute/path/to/CHOICE_FULL --tier full \
  --smoke /absolute/path/to/CHOICE_SMOKE
python -m zg_bench.swe_qa.choice collect /absolute/path/to/CHOICE_FULL
python -m zg_bench.swe_qa.choice judge /absolute/path/to/CHOICE_FULL
python -m zg_bench.swe_qa.choice report /absolute/path/to/CHOICE_FULL
```

Every output directory must be new; dry-run and real runs use separate paths.
Full execution requires this protocol's own smoke report to pass integrity,
scoring and usage checks. Old protocol directories are rejected. `collect` and
`report` use saved evidence only; `judge` calls the remote independent model.

The report includes all three prespecified comparisons: **choice − off**
(primary), augment − off and choice − augment. It reuses the original blind
five-dimension Judge, task means, paired bootstrap and cost accounting. Missing
answers score zero; unscored answers stay N/A; intervals containing zero remain
uncertain. Raw/forwarded MCP arguments, versions, evidence, stop reasons,
cursors, errors and index audits are retained with credentials redacted from
proxy logs. Parameter errors remain tool errors; corrupt evidence is reported
separately as an integrity failure. Tool-use distributions are **behavior
statistics, not selection accuracy**. The autonomous arm changes both available
tools and guidance, so its overall effect cannot be attributed to routing alone.
Remote model weights also remain outside the freeze.

## Earlier fixed Augment experiment

The independent `sweqa20-zg-augment-agent-v1` protocol compares **zg-off** with
**zg-augment** using OpenCode 1.18.4 and Qwen3.7-max. It retains the existing
baseline/zvec-grep protocol. Run `python -m zg_bench.swe_qa.augment --help` for the
local entry point; see the [protocol, commands and measurement definitions](README_CN.md#普通-zg知识图谱-augment-对照).
It requires a Linux x64 Rust local package with the GraphDB feature, a successful
10-trial smoke experiment before the 120-trial full run, and independent GLM-5.2
scoring. A benchmark-only MCP bridge fixes the graph treatment; a usage relay
reconciles cache-inclusive provider usage with OpenCode and Harbor. Raw evidence,
failure states, task-paired bootstrap intervals and blind review packets are retained.

macOS can run the offline checks before Linux execution. From this directory:

```sh
uv sync --frozen
uv run --frozen --no-sync python -m unittest discover -s tests -v
node --test tests/augment_proxy.test.mjs
cargo test --manifest-path ../../Cargo.toml -p zg-transport-mcp \
  --features knowledge-graph-graphdb --test swe_qa_tool_catalog
```

These checks use no Docker, model calls or API keys. Both arms expose the same
search-only description and schema; examples using hidden arguments are removed.
The fixture in `tests/fixtures/mcp-search-graphdb.json` comes from real product
`tools/list`, with a Rust freshness check and Python JSON Schema validation.
For an intentional contract update, prefix the Cargo command with
`ZG_BENCH_UPDATE_MCP_FIXTURES=1`, review the fixture, then rerun the offline checks.
Proxy changes require a new smoke run under the existing frozen-input rules.

This benchmark measures how `zvec-grep` affects an agent's ability to answer
repository-level software-engineering questions. The published comparison uses
the same Claude Code agent, Claude Opus 5 model, task prompt, repository commit,
environment, and limits for both profiles:

- **Baseline:** Claude Code uses its standard tools.
- **zvec-grep:** the same agent receives a prepared repository index and uses
  zvec-grep through MCP.

Index construction is measured separately and is not included in agent wall
time.

## Benchmark definition

This benchmark uses a pinned 20-task subset of
[`peng-weihan/SWE-QA-Bench`](https://github.com/peng-weihan/SWE-QA-Bench).
The benchmark inputs are locked in this directory:

- [`selection.json`](zg_bench/swe_qa/data/selection.json) records task IDs,
  task slugs, repository commits, asset hashes, and runner tier membership.
- [`references.json`](zg_bench/swe_qa/data/references.json) contains the
  isolated references used by the independent judge; agents cannot access it.
- [`datasets/`](datasets/) contains the
  pinned Harbor task environments, prompts, and verifiers.
- [`swe-qa-bench.yaml`](suites/swe-qa-bench.yaml) exposes the
  local dataset to the benchmark runner.

The validation command below checks the locked selection, repository commits,
hashes, and reference isolation before model-backed runs.

## Published protocol

- **Coverage:** 20 retrieval-intensive tasks spanning What, Where, How, and
  Why, 8 intentions, and 11 repositories.
- **Agent:** Claude Code `2.1.212`.
- **Model:** Claude Opus 5 (`claude-opus-5`) at high reasoning effort.
- **Treatment embedding:** Qwen3.7 Text Embedding
  (`qwen/qwen3.7-text-embedding`).
- **Embedding endpoint:** the OpenAI-compatible Qwen endpoint shown in the
  local setup below.
- **Trials:** three independent runs per task and profile.
- **Budget:** USD 4.00 per task/profile run.

Baseline and zvec-grep run with identical settings. Index construction is
measured separately, and the reference answers remain isolated from both
profiles.

## Metrics and reporting

Job Summary cells use `baseline / zvec-grep / change`. Each task's baseline and
zvec-grep values are profile means across the three trials.

| Metric | Change | Interpretation |
| --- | --- | --- |
| Judge | `zvec-grep - baseline` | Positive means higher judged quality. |
| `input_token`, `toolcall`, agent wall time | `(zvec-grep - baseline) / baseline` | Negative means lower resource usage. |

A zero baseline denominator produces `N/A` for that efficiency comparison.
Index setup time is retained separately from agent wall time.

In the Aggregate row:

- Judge values are equal-weight means across tasks.
- Baseline and zvec-grep efficiency values are sums of the per-task profile
  means.
- The displayed efficiency change is the equal-weight mean of task-level
  changes, not a ratio of the aggregate sums.
- An `N/A` task is excluded only from the affected Aggregate metric.

## Local setup

Harbor runs the pinned task environments in Docker. Use the same host platform,
Claude Code version, and provider configuration for comparable results.

Install these prerequisites:

- [uv](https://docs.astral.sh/uv/)
- Docker Engine or Docker Desktop with Docker Compose v2
- Node.js 22 or newer and npm

Verify Docker Compose, install the locked dependencies, and export the Claude
and embedding credentials:

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

Validate the pinned assets, then run the profile-aware preflight:

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

The local package path requires the repository-root `npm ci` shown above.

## Local smoke and dry run

Inspect the locked task selections with:

```sh
zg-bench list tasks swe-qa-bench --tier smoke
zg-bench list tasks swe-qa-bench --tier full
```

Start with a dry run of the five-task, three-trial-per-profile configuration:

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

Remove `--dry-run` to execute the five-task smoke run. To run the published
20-task paired-agent protocol, change `--tier smoke` to `--tier full`. The
runner pins Claude Code `2.1.212`, Claude Opus 5, high reasoning effort, and the
USD 4.00 per-profile budget for both Baseline and zvec-grep. Harbor trajectories
and verifier output are written to `runs/`; the command does not automatically
recreate the published LLM Judge and aggregate report.

## Diagnose a failed run

When a trial records an exception, `zg-bench` prints the structured agent or
zvec-grep setup error and exits non-zero. Inspect a saved run with:

```sh
zg-bench diagnose --latest
zg-bench diagnose <job-name>
```
