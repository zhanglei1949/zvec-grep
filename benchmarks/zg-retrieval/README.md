# ZG Retrieval-only benchmark

The workflow measures **zg-hybrid, zg-fts and zg-vector** on four independent suites. The original SWE-QA20 suite uses 20 unchanged questions and 11 pinned repositories from [Actions run 35206585943](https://github.com/Cuiyus/zvec-grep/actions/runs/35206585943). The exploratory suites contain 20 original BEIR queries across four datasets, ten DuRetrieval queries and 20 original Quarry queries across eight languages; see the [pilot design](expansion/README.md). Each mode calls the Rust public MCP search endpoint directly with its default response presentation. No answering agent, query rewriting, subquery generation or LLM judge participates. See the [SWE-QA20 design](../../docs/zg-retrieval-only-sweqa20-design.md) for its frozen protocol and scoring details.

## Run CI and read the result

The [Retrieval-only workflow](../../.github/workflows/retrieval-only.yml) supports manual runs and reuse by the [PR version comparison](../../.github/workflows/retrieval-comparison.yml). For a manual single-version run, select **Retrieval-only → Run workflow**. Use GitHub's workflow-ref selector to choose the benchmark harness (`main` for the latest merged harness), then set `candidate_ref` to the branch, tag or commit containing the `rust/` workspace to compile. It defaults to `main`. The `embedding` choice defaults to `local`, which preserves each suite's configured local model. Select `remote` to run every suite with `qwen/qwen3.7-text-embedding`; this requires the repository secret `QWEN_EMBEDDING_API_KEY` and variable `QWEN_EMBEDDING_ENDPOINT`. Every run includes all three modes.

PR creation, reopening and new commits automatically compare the frozen PR base and head through all four suites. An authorized `@zg-bench` PR comment requests the same comparison and posts its table. Both versions use the same harness and local models, with separate fresh indexes; neither receives remote embedding credentials. e2e is skipped by default. Each metric cell is base / head / delta (head minus base), on a 0–1 scale. Missing or invalid results make the comparison incomplete rather than zero. The default-branch [result publisher](../../.github/workflows/retrieval-pr-comment.yml) creates or updates one English PR comment with the table and run link, including for fork PRs. It skips cancelled or outdated runs, keeps benchmark execution read-only, and becomes active after its workflow is merged into the default branch. The `retrieval-comparison` artifact and Actions summary also retain the result; `retrieval-base-*` and `retrieval-head-*` retain the underlying reports. See [benchmark setup](../README.md).

For a manual paired comparison without a PR comment, select **Retrieval-only PR → Run workflow** and supply full `baseline_sha` / `candidate_sha` commit identities and their `baseline_repository` / `candidate_repository` (`owner/name`). This uses the same local-only comparison and does not run e2e. Manual runs require maintain/admin access.

The workflow freezes the selected harness ref to a full commit, checks out the candidate separately, resolves it to another full commit SHA, and builds `candidate/rust/`. The packed native package cache is keyed by operating system, architecture and that exact candidate commit. A hit skips Rust compilation and packaging. On a miss, a second Cargo cache can reuse registry data and `rust/target` objects before `npm run pack:local` creates the candidate tarball. Node.js version is not part of candidate selection, package-cache identity or report identity.

Manual and comment runs require the original actor and current re-run actor to have the repository **maintain or admin** role; execution jobs check both, including partial re-runs. Automatic PR runs instead require the supported PR event and local embedding, with read-only permissions and no model secrets. These in-workflow checks apply to the checked-in workflow. Contributors who can modify the workflow or its local authorization action on another branch can bypass that check; a permission boundary against those contributors requires repository- or organization-level Actions execution policies.

The **Retrieval results** job publishes one page with a suite overview and a separate three-mode table for SWE-QA20, BEIR, DuRetrieval and Quarry. All four suite jobs also publish their own aggregate and per-question tables as soon as each job finishes. In the SWE-QA20 section, the three rows are:

| Arm | Public MCP search input | Presentation |
| --- | --- | --- |
| zg-hybrid | `query: originalQuestion` | Rust MCP default |
| zg-fts | `fts: [originalQuestion]` | Rust MCP default |
| zg-vector | `vector: [originalQuestion]` | Rust MCP default |

Every request also uses `limit: 10`, `autoUpdate: false`, `freshness: eventual` and `preferSymbol: false`. FTS and vector requests omit `query` and the other route. The benchmark does not send a `preview` field because the Rust MCP schema does not expose it. The product retains its native chunking, aggregation, ranking and bounded response presentation.

| Column | Meaning / aggregation |
| --- | --- |
| File Hit@1 | Mean of each question/mode's five binary rank-1 observations |
| File Hit@5 | Mean of each question/mode's five binary Top-5 observations |
| File Hit@10 | Mean of each question/mode's five binary Top-10 observations |
| File MRR@10 | Mean of five `1 / first matching native rank` observations per question/mode; Top-10 misses are zero |
| nDCG@10 | Mean of five binary discounted-gain observations per question/mode, then repository macro average across 11 repositories |
| Stable Top 10 | Count of question/mode cases whose five ordered public result locations match exactly |
| Mean output (KiB) | Mean public MCP text UTF-8 bytes / 1024, using successful fifth calls only; not model tokens |
| Avg RT / P50 RT (ms) | Mean and median of all successful MCP search calls, including five repetitions; excludes indexing |

Each repository uses one fresh index and one MCP session. Modes run in the fixed order **hybrid → fts → vector**; each original question is called five consecutive times within each mode. All five calls supply quality and stability observations; only the fifth supplies output size. A complete run contains **300 calls and 60 question/mode quality summaries**, with **20 questions, 20 output samples and 100 latency samples per mode** when all calls succeed. Repetitions are not independent questions.

Isolated public-response format failures still fail CI, but the overview displays diagnostic scores from validated questions with explicit question/repository coverage and a failed-task table. Invalid evidence is excluded, never counted as a miss or a zero. Protocol, identity, missing-call and other integrity errors still withhold all aggregates. Product failures with valid evidence retain zero quality credit and fail operational integrity; failed calls are excluded from output and latency measurements. Quality scores have no arbitrary pass threshold. Fixed mode order and shared runtime/model caches mean latency is an observation under this protocol, not a controlled comparison of cold-start or mode execution speed.

## Dataset and scoring

Frozen inputs are `data/source.lock.json`, `data/queries.jsonl`, `configs/protocol.json`, and `gold/files-v1.json`. The questions and 39 relevant-file targets are unchanged. Targets are unique accepted paths projected from AI-reviewed source annotations; bridge-only paths do not earn credit. They are partial positives, not independent human ground truth or complete answer evidence. The [metric rationale](../../docs/zg-retrieval-metric-review.md) explains how to interpret the scores and their limits.

All five quality metrics use the same file labels and normalized-path matching. Native ranks are preserved: repeated file chunks consume positions and are never collapsed or renumbered. Source declarations, outlines and response length cannot alter these quality scores. File Hit/RR/MRR use contract `sweqa-file-hit-rr-v1`.

`metrics/ndcg.mjs` uses first-target rank, binary gain and target-count IDCG. It is checked against a byte-identical pinned Python scoring reference, with its MIT attribution preserved in [`test/fixtures/ndcg-reference/`](test/fixtures/ndcg-reference/). The reference is used only by unit tests. The benchmark runs only ZG.

Questions, labels and reports remain outside indexed source checkouts. Indexing applies the frozen code-extension and size policy while retaining ZG's native ignore rules. Repository indexes are never cached; model downloads and compiled candidate packages may be cached. Corpus and model inventories plus the Rust CLI's public aggregate index status are checked before and after retrieval. Model identity includes artifact contents but excludes only the runtime-generated `.zvec-grep-artifacts-<24hex>.complete` cache marker, whose machine-specific timestamps do not identify model weights.

Rust candidates are built through the [shared package Action](../../.github/actions/rust-candidate-package/action.yml); its [manifest utility](../shared/rust-package-cache.mjs) binds each tarball to the exact source commit and package bytes. SWE-QA uses the same packaging path while retaining its own evaluation protocol.

## Reports and artifacts

- `retrieval-results`: the unified `summary.md` and machine-readable `summary.json`.
- `retrieval-zg-report`: the SWE-QA20 suite conclusion (`summary.json` and `summary.md`), validated `report.json`, `report.md` and per-call `scores.jsonl`.
- `retrieval-zg-evidence`: raw public requests/responses, installation evidence, corpus/model inventories and public index status for all 11 repositories.
- `retrieval-beir-report`, `retrieval-duretrieval-report` and `retrieval-quarry-report`: independent pilot summaries with BEIR dataset and Quarry language coverage and scores.
- `retrieval-beir-evidence`, `retrieval-duretrieval-evidence` and `retrieval-quarry-evidence`: pilot public requests/responses and index status.

Evidence retention is 14 days. After a shared candidate build, SWE-QA20, BEIR, DuRetrieval and Quarry run as four independent suite jobs. Each suite publishes its own aggregate metrics and a per-question table even if an individual question fails. The final results job publishes the combined page. Missing artifacts and failed upstream jobs remain explicit in the overview.

SWE-QA20 ZG reports use **schema 7**, with `preview: "mcp-default"`, three `modes`, and one quality row per question/mode. Its section uses the **schema 5** overview with fixed `zg-hybrid`, `zg-fts`, `zg-vector` rows; the combined page uses schema 1. The SWE-QA20 overview records coverage, failed task IDs/modes/reasons, the frozen harness commit and selected candidate ref/commit. Quality rows retain all five public result lists and measurement observations so validators can recompute the case mean, ranking stability, output size and latency. The representative fifth-call `file_retrieval` and `ndcg` fields remain for evidence; `quality_mean` supplies headline quality.

The protocol ID is `sweqa20-zg-rust-three-modes-mcp-default-v7`. The comparator accepts schema 7 reports with matching protocol and frozen inputs, Rust MCP default presentation and all three modes. Reports from other protocol versions require their matching scorer checkout. Replaying saved evidence is not a new retrieval run.

## Code structure

```text
zg-retrieval/
  *.mjs              Stable command-line entrypoints
  core/              Frozen suite, corpus, I/O and shared response scoring
  metrics/           File ranking, nDCG and operational measurements
  engines/zg/        Runner, public response parser, snapshots and evidence audit
  reports/           Validation, same-protocol comparisons and CI rendering
  configs/           Frozen protocol
  data/              Frozen questions and repository/source lock
  gold/              Frozen relevance labels and annotation provenance
  test/              Contract tests and pinned scoring reference
```

The runner captures public evidence; the aggregator reparses and audits it; metric functions score normalized results; report validators recompute quality and measurements before comparison or CI rendering. Metrics do not depend on engine implementations. Response presentation behavior remains covered by the product's MCP contract tests, rather than duplicate retrieval benchmark arms.

## Run locally

Use Node.js, Python 3, npm, Git, Rust and a platform supported by the packed product. Python is used only for the stdlib scoring oracle. The first retrieval run needs network access for repository checkouts, Rust dependencies and the embedding model. Run from the repository root:

```sh
node --test benchmarks/zg-retrieval/test/*.test.mjs
retrieval_work="$(mktemp -d)"
mkdir -p "$retrieval_work/package"
(cd rust && npm ci && npm run pack:local)
cp rust/dist/npm/*.tgz "$retrieval_work/package/"

node benchmarks/zg-retrieval/run.mjs \
  --package "$retrieval_work/package" \
  --output "$retrieval_work/results" \
  --corpus "$retrieval_work/corpus" \
  --model-cache "$retrieval_work/model-cache" \
  --candidate-commit "$(git rev-parse HEAD)"

node benchmarks/zg-retrieval/report.mjs "$retrieval_work/results"
```

Local commands use the configured local models by default. To reproduce the
remote workflow choice, set `RETRIEVAL_EMBEDDING=remote` together with
`ZVEC_GREP_API_KEY` and an HTTPS `ZVEC_GREP_ENDPOINT` before running the suite
and report commands.

`--package` accepts a tarball or a directory containing exactly one `.tgz`. The runner installs it in an isolated consumer. Use a new `--output` directory and fresh corpus checkout: output is never overwritten and an existing index is rejected. An external model-download cache may be reused.

Add `--repository reflex-dev/reflex` or `--tasks reflex:6` for an explicitly labeled subset smoke run; all three modes still execute. Standalone `report.mjs` requires all 20 questions, and CI rejects subset reports. Modes and MCP presentation are fixed by the protocol, without selection flags.

## Offline replay and comparisons

Download `retrieval-zg-evidence` into one directory, then recompute:

```sh
node benchmarks/zg-retrieval/report.mjs /absolute/path/to/downloaded-shards
node benchmarks/zg-retrieval/compare.mjs baseline/report.json candidate/report.json NEW_OUTPUT_DIR
node benchmarks/zg-retrieval/ci-report.mjs \
  --zg /absolute/path/to/report.json \
  --output /absolute/path/to/overview
```

An isolated task/mode failure can produce a diagnostic partial overview while the command and CI still fail. Incompatible or globally invalid evidence produces no scores. Candidate source selection happens through `candidate_ref`; the harness comes from the workflow ref selected for that run.

## Corpus size and remote embedding load

Query counts are small, but every run builds fresh corpus indexes before searching:

| Suite | Indexed corpus |
| --- | --- |
| SWE-QA20 | Source files from 11 pinned repositories |
| BEIR | 75,128 unique documents across SciFact (5,183), NFCorpus (3,633), ArguAna (8,674) and FiQA (57,638) |
| DuRetrieval | 100,001 passages |
| Quarry | 20 pinned repository preimages, filtered by each task's language |

ArguAna needs a separate leave-one-out index for each of its three selected queries, so BEIR processes 92,473 document files across its six indexes. Source files can yield multiple embedding entities. Model downloads and compiled packages may be cached; corpus indexes and their document embeddings are not reused between runs or versions. Each of the 70 queries is also searched in three modes with five calls per mode.

Manual remote runs start the four suite jobs independently. The Qwen text backend's default indexing policy starts at eight concurrent batches and can grow to twelve; Qwen3.7 text embedding accepts up to twenty inputs per batch. Separate runs for different candidates can overlap, with no account-wide token budget in this workflow. Large fresh indexes and overlapping batches can therefore exhaust a shared remote token rate limit even when there are few test queries. PR and comment version comparisons deliberately use local embedding.

For Model Studio, `Allocated quota exceeded` indicates a token rate limit; [the provider documents TPM/TPS limits and account-wide aggregation](https://www.alibabacloud.com/help/en/model-studio/rate-limit). Remote capacity planning needs token-aware pacing across suite jobs and runs, in addition to limiting per-process concurrency. Reducing test query counts alone does not remove the indexing load.

## Limits

These public development questions support regression diagnosis, not broad generalization claims. A file hit does not establish that its returned snippet answers the question. Reports do not claim complete candidate/fusion histories or embedding inputs; a miss alone cannot identify the responsible stage. Answer correctness and agent token use require separate evaluations.

## G4 code graph profile (v7)

### Corrected input version (v8)

`--protocol g4-code-v8` selects `data/g4-code-v2.json`. Generate it before
candidate execution with `python3 benchmarks/zg-retrieval/tools/g4-labels-v2.py
--sources /absolute/path/to/frozen-checkouts`. Checkout directory names are
`BurntSushi__ripgrep` and `psf__requests`; commits and source bytes are checked.
The generator refuses to overwrite frozen input. The v1 generator and input
remain unchanged, and v7 replay still uses them.

v8 retains all 48 structured tasks and all 458 calls, the same retrieval pairs,
scoring formulas, source/model revisions and default budgets. Four `escape`
locators specify function kind; the `dict_class` negative uses an independent
AST `Call.func` span. The 43 normal contracts and five predeclared budget
negatives are reported separately. Only the exact declared output-budget error
or partial reason satisfies a negative; transport failures, invalid evidence,
missing calls, snapshot drift and scope leakage still fail integrity. v1 failures
are not rescored. Compare candidates using the same v2 identity, never v1 vs v2.

Use the run and replay commands below with `g4-code-v8` in place of `g4-code-v7`.

### Visible evidence scoring (v9)

`--protocol g4-code-v9` retains the v2 input, all 458 calls, five repetitions,
model, ranking and default budgets from v8. Its `visible-bytes-v2` scorer also
credits the newline of a numbered empty source line when the frozen source
matches and the next displayed source line is consecutive. Omitted, truncated
or unlocatable bytes receive no credit. v7/v8 replay retains its original scoring.

Score calibration and product changes are reported separately: compare product
candidates under the same v9 identity. A higher score from recalculating an old
response is not a retrieval gain. See the [current acceptance matrix](../../knowledge/S2_ACCEPTANCE_MATRIX.md)
for the frozen baseline, missing evidence and per-question comparisons.

The original v6 SWE-QA20 protocol and commands above are unchanged. Select
`--protocol g4-code-v7` explicitly for the Rust/Python graph experiment. It uses
one frozen ripgrep revision and the existing requests revision, 48 structured
explore questions, 24 Off/Augment pairs and 16 Off/Constrain pairs. The original
`requests:16` question is an additional, separately reported file-retrieval row.
There is no answering agent, document graph or claim about eight-language support.

Build `zg` with `knowledge-graph-graphdb` in Release and package that binary using
`npm run pack:local -- --no-build`. The package must contain the matching patched
zvec read-only runtime described in the Rust README. Use fresh output and corpus
directories, and keep them outside the indexed checkout:

```sh
node benchmarks/zg-retrieval/run.mjs --protocol g4-code-v7 \
  --package /absolute/path/to/candidate.tgz \
  --output /absolute/path/to/NEW_RESULTS \
  --corpus /absolute/path/to/NEW_CORPUS \
  --model-cache /absolute/path/to/model-cache \
  --candidate-commit FULL_CANDIDATE_COMMIT

node benchmarks/zg-retrieval/report.mjs --protocol g4-code-v7 \
  /absolute/path/to/NEW_RESULTS
```

`--source-cache /absolute/path/to/frozen-checkouts` can clone the two pinned Git
checkouts locally; it never copies their indexes. Each run builds a fresh text
index, publishes a graph-only candidate, verifies reopening, and exercises the
public MCP index graph configuration before freezing the query view. CLI build
costs, the additional MCP maintenance check and query latency are reported
separately. Exact process RSS collection uses the system `time` command; if the
host forbids its counters, the measurement failure remains visible.

For local macOS hosts where an extracted Mach-O is killed while the original
build path runs, `--binary /absolute/path/to/zg --native-library-dir /path/to/lib`
can execute the original build files. Both executable and runtime library must
match the package payload SHA-256 exactly. The execution-path limitation is
recorded; this does not validate launching the installed package. It does not
change signatures, product code, budgets or labels.

Source facts and expected parser abstention are separate in `data/g4-code-v1.json`.
The annotation generator uses CPython AST and an independent Rust lexical scan,
with explicitly source-reviewed call pairs; it never reads zg graph output.
These are partial development labels without independent human review. Additional
unlabelled relationships are unknown, not false positives. Changing a frozen
question, label or source requires a new input version.

Quality is computed from public text only, including its visible Graph JSON
section. Source filenames, ranges, outlines and retrieved-evidence pointers do
not earn body evidence credit. Verified bytes must match captured source files;
ordinary source lines must match those files exactly. Off/Augment ordinary output
must agree. Constrain uses its own scoped gold and checks every visible source
interval against the independently annotated boundary.

All requests, responses, errors, view stamps, source/model/storage inventories and
raw hashes are retained. Missing calls, drift, invalid evidence and scope leakage
fail integrity. A repository that cannot prepare retains every planned call as
blocked; offline replay cannot turn it into a successful empty experiment.
Runtime errors and low quality are reported rather than retried away. Results
apply to the frozen questions and existing Rust/Python capabilities only.

The frozen G4 experiment is summarized in [the validation report](../../knowledge/G4_VALIDATION_REPORT.md). Its [evidence archive](../../knowledge/archive/g4-validation/README.md) includes restoration and offline replay instructions; use a new output directory for subsequent experiments.

## M7 六语言收敛

用户决定本阶段跳过 JS／TS 代码图。`tools/m7-labels.py` 从冻结四语言源码夹具与新增 Rust／Python 源码声明生成 `data/m7-six-languages-v1.json`，不读取候选结果。JS／TS 仅验证已有文本检索及代码图能力边界。C++ 固定 grammar 的原默认参数失败样本作为独立拒绝用例，原 v1 失败不改写。

使用相同原生环境构建 `zg` 的图 feature Release，使用 `cargo build --manifest-path rust/Cargo.toml --release -p zg-engine --features knowledge-graph-graphdb --example m7_api` 构建正式 helper。运行 `node benchmarks/zg-retrieval/engines/zg/m7-run.mjs NEW_OUTPUT_DIRECTORY [ZG_BINARY] [API_HELPER]`；二进制默认原 Release 路径，helper 默认 Debug example 路径，可显式选择安装包与固定 helper。CLI／daemon／MCP 清除 `DYLD_LIBRARY_PATH`／`LD_LIBRARY_PATH`，确保加载自己的运行库；Rust helper 保留原生 SDK 环境。模型缓存路径在脚本中显式固定，按本机已核验缓存调整。输出必须是新目录，脚本隔离 registry、认证文件及 daemon 端口。默认预算不变；比较完整响应，仅归一化计时和受其数字宽度影响的输出字节计数。

R3 修复默认参数语法后，可在 `m7-run.mjs NEW_OUTPUT_DIRECTORY ZG_BINARY API_HELPER` 后显式追加 `g4-compiled-v2`。runner 按冻结源码 SHA 匹配既有 cpp:B-02，复用该协议预先声明的 ready_locate 预期；题集与原默认 v1 的 not_ready 重放行为不变。当前六语言兼容运行使用这一显式参数。

## 四语言真实查询题集：R0／R1

`configs/g4-compiled-v1.json` 与 `data/g4-compiled-v1.json` 冻结 Go／Java／C／C++ 各 12 道自然语言题，共 48 道、32 个 Constrain 配对；另有 8 组独立边界。英文问题、中文标准答案、源码原文和字节见 [answers](gold/g4-compiled-v1-answers.md)。标注是 agent 对固定源码的审阅，未人工复核，不来自产品响应或真实用户日志。

[冻结来源 fixtures](fixtures/g4-compiled/README.md)提供离线原样来源包和重生成方法。运行 `python3 -m unittest discover -s benchmarks/zg-retrieval/test -p test_g4_compiled_labels.py -v` 核对来源重生成与篡改拒绝。生成器独占创建新输出，已有版本不能覆盖。

v1 保留原 Java 配置重复与 C／C++ 来源未就绪的拒绝行为；v2 修订 Java 根，沿用同一题集、标准答案、模型、800 请求和默认预算。C／C++ 最小 grammar 修复及后续继承／正文修复已交付。当前质量及残留见 [S2 验收矩阵](../../knowledge/S2_ACCEPTANCE_MATRIX.md)。旧 v7／v8／v9 的评分行为保持，构建成本继续暂缓。

R2 使用同一 run 入口的 `--protocol g4-compiled-v1 --prepare-only` 验证准备流程，沿用 package／binary／native-library-dir／model-cache／output／corpus 参数。此选项保留完整 800 条阻塞，报告应明确未通过，不生成质量收益。首次索引即使用冻结 GraphOptions，避免 C 头文件先以缺省语言抽取；旧协议保持原顺序。

边界脚本为 `engines/zg/g4-compiled-boundaries.mjs`，参数为 `--output NEW_DIR --binary FIXED_BINARY --api-helper FIXED_HELPER --native-library-dir PINNED_LIB_DIR --model-cache PINNED_MODEL_CACHE`。复用 ../../rust/crates/zg-engine/examples/m7_api.rs 对应的已验收 helper，不添加产品接口。冻结的调用矩阵和来源摘要保存在 fixtures/g4-compiled。

### g4-compiled-v2 配置修订与解析修复

v2 仅修订 Java 根，并单列 C++ B-02 解析修复成功定位契约。run/report 使用 `--protocol g4-compiled-v2`，其他参数沿用上面的 v1 命令。边界 runner 同样接收该协议，并要求 `--binary-sha256` 与 `--api-sha256` 固定候选。准备、边界和正式质量调用分别保存，使用新输出目录；冻结输入与标准答案不能被候选结果覆盖。
