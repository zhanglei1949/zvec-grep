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

Corrected run IDs and full-run results will be appended after validation.
No formal performance result is available yet.
