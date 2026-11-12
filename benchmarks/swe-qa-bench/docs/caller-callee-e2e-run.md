# Caller/Callee E2E execution record

Date: 2026-11-12 (Asia/Shanghai). Protocol: caller-callee-e2e-v1.

## Frozen design

Three separately dispatched arms: relationships, search, agent (combined).
Smoke: requests:16 once per arm. Full: the locked 20 tasks five times per arm.
Native tools remain available; no required tool calls or call-rate-based reruns.
GLM-5.2 and pinned OpenCode 1.18.4; local Potion embeddings and graph indexing.
The workflow limits each arm to two task jobs, with serial trials and judging.

## Local validation

- CLI/MCP library tests: 184 + 73 passed; strict Clippy and Rust formatting passed.
- Benchmark tests with real OpenCode against a local fake provider passed,
  including the experiment tools in main and explore subagent requests.
- Independent development fixture: real search/callers/callees MCP calls passed
  for all three toolsets. Disabled tools are also rejected by wire tests.
- Real-model development calibration (isolated fixture, explicit working
  directory): relationships called callers/callees once each; search called
  search once; combined called all three once each. All calls succeeded.
  This is calibration evidence, not benchmark performance evidence.
- No task-specific guidance or formal-evaluation-score prompt tuning was used.
  Earlier development harness runs revealed incorrect working-directory
  selection; their evidence is retained separately from the corrected run.
- macOS rejected the temporary npm-installed executable with AMFI error -423
  (ad-hoc/unknown certificate chain). The native build runs successfully.
  Linux Actions must independently verify the installed candidate before any
  model trial; the workflow enforces that prerequisite.

Local raw evidence: `/private/tmp/zg-caller-callee-native-preflight` and
`/private/tmp/zg-caller-callee-calibration-v3`. Earlier calibration evidence is
retained in the same directory prefix without `-v3` and with `-v2`.
These temporary paths are host-local, not durable artifact storage.

## Remote execution

Frozen candidate and harness: `f6b7c00269645bbb02f74ba07460957a773abb68`.

The three smoke runs were submitted together:

| Arm | Run | Phase |
|---|---|---|
| relationships | [43216597212](https://github.com/zhanglei1949/zvec-grep/actions/runs/43216597212) | smoke |
| search | [43216599265](https://github.com/zhanglei1949/zvec-grep/actions/runs/43216599265) | smoke |
| combined (`agent`) | [43216591261](https://github.com/zhanglei1949/zvec-grep/actions/runs/43216591261) | smoke |

All three initial smoke runs failed during setup because the task image has no
`python3`, required by the first surface probe. They did not produce model
answers. The probe now uses the Node runtime already installed for OpenCode;
native live MCP validation passes with that implementation. A regression test
checks the task setup command does not require Python. The Rust candidate stays
at the SHA above; corrected smoke runs use a new harness commit.

## Successful smoke and formal dispatch

Candidate: `f6b7c00269645bbb02f74ba07460957a773abb68`.
Answering harness: `fc35997b0f73706a03011f2eb54e3f60002b41e2`.

| Arm | Successful smoke | Formal run |
|---|---|---|
| relationships | [43218303325](https://github.com/zhanglei1949/zvec-grep/actions/runs/43218303325) | [43219034725](https://github.com/zhanglei1949/zvec-grep/actions/runs/43219034725) |
| search | [43218302615](https://github.com/zhanglei1949/zvec-grep/actions/runs/43218302615) | [43219041723](https://github.com/zhanglei1949/zvec-grep/actions/runs/43219041723) |
| combined | [43218296749](https://github.com/zhanglei1949/zvec-grep/actions/runs/43218296749) | [43219049082](https://github.com/zhanglei1949/zvec-grep/actions/runs/43219049082) |

All three smoke reports and their checksums validated before formal dispatch.
The one-task smoke is a pipeline check, not performance evidence:

| Arm | Judge | Input tokens | Agent seconds | Search / callers / callees calls |
|---|---:|---:|---:|---|
| relationships | 73 | 27098 | 16.92 | 0 / 0 / 0 |
| search | 95 | 32229 | 23.33 | 1 / 0 / 0 |
| combined | 100 | 86108 | 33.51 | 1 / 0 / 0 |

The formal Search and combined runs initially failed an offline fake-MCP
exposure check before any model trials. Only their failed prerequisite jobs
were resumed, preserving the frozen harness and all failure logs. The fake
server's optional SSE GET returned 501; a concurrent change (`5eb4367`) adds
405 handling and connection diagnostics. Real OpenCode was observed issuing
three such GET probes in the local seven-case contract test.

Another operation cancelled the relationships formal run. Its original
artifacts contain five completed trials for reflex:6 and five for sqlfluff:2,
all with tool-call audits. These ten completed answering trials must be
preserved rather than rerun. They have not produced a complete independent
task report, and are not included in an aggregate score.

Current coordination is pending because the cancellation and concurrent commit
were not issued by this execution flow. Search and combined formal runs remain
active. No complete 300-trial performance result is available yet.

Host-local evidence and status manifests are under the ignored directory
`benchmarks/swe-qa-bench/runs/caller-callee-20261112/`: `smoke-report/`,
`smoke-evidence/`, `failed-smoke-v1/`, `full-cancelled-evidence/`,
`run-manifest.json`, and the per-phase status snapshots. The cancelled trial
inventory is `full-cancelled-evidence/relationships-trial-inventory.json`.

## MCP fixture correction after the first full dispatch

Full runs 43219041723 (search) and 43219049082 (combined) failed the offline
tool-exposure contract before starting model trials. Relationship run
43219034725 passed validation and began trials, then was cancelled to avoid
mixing cohorts. Those runs are not a completed three-arm evaluation.

The stateless mock MCP server now explicitly returns HTTP 405 for its optional
GET stream instead of BaseHTTPRequestHandler's default 501. Missing-tool
assertions retain the original strength and now include the arm, MCP request
history and OpenCode connection logs. No production prompts were changed.

Harness `5eb4367a962befe6e40c2f445b5b92ee620155a2` passed Linux run
[43220157190](https://github.com/zhanglei1949/zvec-grep/actions/runs/43220157190):
196 tests (one Docker opt-in skip), three additional eight-test OpenCode
contract passes, and a separate Docker shutdown check. This checks the observed
transport defect; finite repetition does not prove the absence of all races.

New smoke runs: relationships 43220714175, search 43220704577, combined
43220709656. Their records and any ensuing full runs are isolated under the
ignored `runs/caller-callee-20261112/cohort-5eb4367` directory. The Rust candidate
remains `f6b7c00269645bbb02f74ba07460957a773abb68`.

## Corrected cohort: smoke passed, full evaluation dispatched

All three corrected smoke workflows completed successfully. Downloaded reports
passed checksum, candidate/harness identity, expected-trial and tool-surface
checks; the combined smoke report has `complete: true` and no missing tasks.
Each arm ran one requests:16 trial. Relationships made no zg calls; search and
combined each called search once, with no callers/callees calls. These are
natural-use smoke observations, not estimates of full-benchmark effectiveness.

The following full runs were submitted together, each for 20 tasks × 5 trials:

| Arm | Full run |
|---|---|
| relationships | [43221552875](https://github.com/zhanglei1949/zvec-grep/actions/runs/43221552875) |
| search | [43221559801](https://github.com/zhanglei1949/zvec-grep/actions/runs/43221559801) |
| combined (`agent`) | [43221566623](https://github.com/zhanglei1949/zvec-grep/actions/runs/43221566623) |

All use candidate `f6b7c00269645bbb02f74ba07460957a773abb68` and harness
`5eb4367a962befe6e40c2f445b5b92ee620155a2`. Full outcomes remain pending.
The ignored cohort directory contains `run-manifest.json`, `smoke-report/`,
downloaded smoke call evidence, and live `full-status.json`; full evidence and
the combined report will be written there after completion.

## Further contract failure: full cohort stopped

Full run 43221566623 failed its offline contract in the relationships subcase.
The retained diagnostic showed `initialize`, `notifications/initialized` and
GET, but no `tools/list`; the first model request had no MCP tools. Thus the
405-only correction and three passing repetitions were insufficient evidence
that the intermittent connection problem was fixed. Runs 43221552875 and
43221559801 were cancelled; this cohort cannot support a complete comparison.

The mock now uses HTTP/1.1, matching the real endpoint's persistent transport,
instead of BaseHTTPRequestHandler's HTTP/1.0 connection-close default.
Commit `71c9fb1` also preserves runtime-file logs on assertion failure. Local
eight-test contracts passed. Linux run
[43222269382](https://github.com/zhanglei1949/zvec-grep/actions/runs/43222269382)
adds ten mandatory contract repetitions after the complete suite. Its outcome
must be checked before treating this further correction as validated or
starting another model cohort. Prompts and the Rust candidate are unchanged.

Linux run 43222269382 subsequently completed successfully. Its downloaded log
contains all ten mandatory eight-test contract completions, with no failed
tests, as well as the complete suite and Docker checks. The persistent HTTP
fixture is therefore validated by these repeated checks; this finite test does
not establish that every possible OpenCode connection race is absent. The next
cohort must still pass its own three end-to-end smoke workflows before full
dispatch. Raw Linux verification log: `/private/tmp/zg-mcp-http11-linux-ten.log`.

## HTTP/1.1 cohort: current full evaluation

All three smoke runs passed under frozen harness
`8065d623210aa322997d4076bbe0e5df6511ff4e`: relationships 43223495006,
search 43223501519, combined 43223501130. The downloaded aggregate report is
complete with no missing tasks, and evidence checksums/identities passed.
For this one-trial smoke, callers/callees were unused in all arms; search was
called once in search and combined. No prompts were changed based on these
observations.

| Arm | Current full run | Trials planned |
|---|---|---:|
| relationships | [43224241293](https://github.com/zhanglei1949/zvec-grep/actions/runs/43224241293) | 100 |
| search | [43224248511](https://github.com/zhanglei1949/zvec-grep/actions/runs/43224248511) | 100 |
| combined | [43224255136](https://github.com/zhanglei1949/zvec-grep/actions/runs/43224255136) | 100 |

Each current full workflow passed validation and entered its two concurrent
task jobs. Outcomes remain pending. Candidate stays at
`f6b7c00269645bbb02f74ba07460957a773abb68`. Cohort artifacts and the live
manifest are isolated in ignored `runs/caller-callee-20261112/cohort-http11-8065d62`.

## Observed infrastructure failure and recovery policy

Combined job 131204012639 (`matplotlib-37`, run 43224255136) failed before
Harbor created its job directory: the hosted runner's Docker daemon was not
running. No model trial was produced. Its log is retained as
`/private/tmp/zg-matplotlib-combined-failure.log`; this is missing evidence, not
a zero score. A direct single-job rerun request was refused with HTTP 403
because the overall workflow was still running. Other tasks continue.

The one-run recovery watcher waits for that workflow's terminal state, verifies
the original harness and attempt, then requests only this failed job once.
It does not repeat successful trials or select by score. Recovered evidence must
still contain five complete trials with the same candidate/harness identity.

Future experiment summaries now download this run's complete reports from all
attempts. A regression covers nineteen complete task reports from attempt 1
plus the one recovered task from attempt 2; taking only attempt 2 is correctly
rejected as incomplete. The currently frozen workflow retains its earlier
summary code, so this cohort's final offline aggregation will explicitly use
the valid complete reports across attempts and retain the selection manifest.

## Astropy index-input correction: supersedes the HTTP/1.1 full cohort

The Search Astropy job 131204030559 exhausted its two retries per trial:
all five logical trials failed during index setup, before agent execution.
The retained Harbor summary records ten retries. The index reported 36 failed
files, including binary FITS fixtures whose ASCII headers were recognized as
text. Combined Astropy job 131204012646 also failed; these are setup failures,
not zero-quality answers. Original logs and Search artifacts are retained in
`cohort-http11-8065d62/failed-astropy-search` and the original Actions runs.

The shared index ignore file now excludes `astropy/**/*.fits`. This removes
all 51 FITS fixtures from the index, including the binary files that failed;
the repository files remain available to native agent tools. Against the
locked Astropy commit `0a041d38fbd0014298d4cf340d9f891c4522093f`, the unchanged
native candidate completed graph-enabled local Potion indexing: 1,686 files,
26,414 entities, zero failures, in 16.036 seconds. Evidence is retained at
`/private/tmp/zg-astropy-index-check/index.log`. The 13 existing adapter tests
also pass, including ignore-file upload and index-cache identity checks.

Because index input changed, the three full runs above are superseded rather
than combined with corrected trials. The Docker-only recovery watcher is
stopped. A new cohort must freeze the corrected harness, pass all three smoke
runs, and then dispatch three full runs against that same harness. Candidate,
prompts, questions, sampling and judging remain unchanged. No performance or
tool-use outcome was used to select this correction.

## Corrected cohort: frozen harness c308c2a

Harness `c308c2ae182a9b25ca5ffb5a180258453275a490` passed all three smoke
workflows: relationships 43231380126, search 43231386551, combined 43231393034.
Independent local audit verified all three raw call exports, recursive usage
totals, tool surfaces and their recomputed checksums. Relationship tools were
unused; search was called once in Search and once in combined. These smoke
observations did not change guidance and are excluded from formal results.

| Arm | Corrected full run | Planned trials |
|---|---|---:|
| relationships | [43232140196](https://github.com/zhanglei1949/zvec-grep/actions/runs/43232140196) | 100 |
| search | [43232146223](https://github.com/zhanglei1949/zvec-grep/actions/runs/43232146223) | 100 |
| combined | [43232152271](https://github.com/zhanglei1949/zvec-grep/actions/runs/43232152271) | 100 |

These runs were dispatched consecutively without waiting for another arm.
Candidate remains `f6b7c00269645bbb02f74ba07460957a773abb68`; all use the
same corrected harness. Outcomes are pending. Evidence, run manifest and
incremental audit are isolated in ignored
`runs/caller-callee-20261112/cohort-fits-c308c2a`; smoke audit is
`smoke-audit/audit.json`. Prior full runs were cancelled and retained separately.
