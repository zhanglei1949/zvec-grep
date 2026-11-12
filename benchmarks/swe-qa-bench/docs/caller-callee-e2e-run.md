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

Run IDs, immutable commits, artifact locations and results will be appended
after dispatch and completion. No formal performance result is available yet.
