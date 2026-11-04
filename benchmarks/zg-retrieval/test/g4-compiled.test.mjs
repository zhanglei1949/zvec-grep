import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import test from "node:test";
import { loadG4Suite, g4CallPlan, g4Request, coverage } from "../core/g4.mjs";
import {
  COMPILED_PROFILE,
  COMPILED_REVISION,
  compiledFixtures,
  compiledCorpusManifest,
  compiledAnnotations,
  materializeCompiledCorpus,
  unpackCompiledSources,
} from "../core/g4-compiled.mjs";
import { objectHash, readJson, run, sha256, writeJson } from "../core/io.mjs";
import { repositorySlug } from "../core/corpus.mjs";
import { aggregateG4 } from "../engines/zg/g4-report.mjs";
import { collectReferenceIdentities } from "../engines/zg/g4-run.mjs";
import {
  parseGraphResponse,
  visibleRanges,
  judgeRelations,
} from "../engines/zg/g4-parse.mjs";
import {
  boundaryReference,
  normalizeBoundaryGraph,
  verifyDefaultSchema,
} from "../engines/zg/g4-compiled-boundaries.mjs";

const suite = await loadG4Suite(COMPILED_PROFILE);

test("new known-site identities use deduplicated unscored public lookups", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "zg-compiled-identities-"));
  t.after(() => rm(directory, { recursive: true }));
  const annotations = compiledAnnotations(suite.inputs, "google/uuid");
  const a = annotations.find((a) => a.target);
  const reference = {
    kind: a.kind,
    status: "resolved",
    target: "target",
    evidence: {
      source_id: a.site.path,
      raw_digest: a.site.sha256,
      decoded_digest: a.site.sha256,
      span: { start: a.expression_context.start, end: a.site.end },
    },
  };
  const graph = { status: "complete", nodes: [], references: [reference] };
  await writeJson(join(directory, "raw.json"), {
    content: [
      {
        type: "text",
        text: `freshness: fresh\nNo matches.\n\nGraph\n\`\`\`json\n${JSON.stringify(graph)}\n\`\`\``,
      },
    ],
  });
  const calls = [];
  const client = {
    request: async (method, request) => {
      calls.push({ method, request });
      return {
        content: [
          { type: "text", text: "identity response retained without scoring" },
        ],
      };
    },
  };
  const task = suite.tasks.find((t) => t.repository === "google/uuid");
  const record = {
    task_id: task.task_id,
    family: "augment",
    arm: "augment",
    raw_path: "raw.json",
  };
  await collectReferenceIdentities(
    client,
    directory,
    "/frozen",
    [task],
    [record, record],
    { graph: { build_id: "build" } },
    annotations,
  );
  assert.equal(calls.length, 1);
  assert.equal(calls[0].request.name, "zvec_grep_explore");
  assert.deepEqual(calls[0].request.arguments.request.operation.target, {
    by: "node",
    node: { graph_build_id: "build", node_id: "target" },
  });
  const reads = await readJson(join(directory, "reference-identities.json"));
  assert.equal(reads.length, 1);
  assert.match(reads[0].purpose, /no body evidence credit/);
});

test("frozen calls and explicit type relations reuse known-site target scoring", () => {
  const annotations = suite.inputs.repositories.flatMap((repo) =>
    compiledAnnotations(suite.inputs, repo.repository),
  );
  assert.equal(annotations.length, 19);
  for (const a of annotations.filter((a) => a.target)) {
    const ref = {
      kind: a.kind,
      status: "resolved",
      target: "target",
      evidence: {
        source_id: a.site.path,
        raw_digest: a.site.sha256,
        decoded_digest: a.site.sha256,
        span: { start: a.expression_context.start, end: a.site.end },
      },
    };
    const nodes = new Map([
      [
        "target",
        { source_id: a.target.path, name: a.target.name, span: a.target.span },
      ],
    ]);
    assert.equal(
      judgeRelations({ references: [ref] }, nodes, [], annotations)
        .known_correct,
      1,
    );
    nodes.get("target").name = "wrong target";
    assert.equal(
      judgeRelations({ references: [ref] }, nodes, [], annotations).known_wrong,
      1,
    );
    ref.evidence.raw_digest = "0".repeat(64);
    assert.throws(
      () => judgeRelations({ references: [ref] }, nodes, [], annotations),
      /digest mismatch/,
    );
  }
});

test("a known unresolved callee cannot acquire a guessed target or match an outer call", () => {
  const annotations = compiledAnnotations(suite.inputs, "google/uuid");
  const a = annotations.find((a) => a.resolution_expectation === "unresolved");
  const ref = {
    kind: "calls",
    status: "resolved",
    target: "guessed",
    evidence: {
      source_id: a.site.path,
      raw_digest: a.site.sha256,
      decoded_digest: a.site.sha256,
      span: { start: a.expression_context.start, end: a.site.end },
    },
  };
  assert.equal(
    judgeRelations({ references: [ref] }, new Map(), [], annotations)
      .known_wrong,
    1,
  );
  ref.evidence.span.start--;
  const result = judgeRelations(
    { references: [ref] },
    new Map(),
    [],
    annotations,
  );
  assert.equal(result.unjudged, 1);
  assert.equal(result.known_wrong, 0);
});

test("compiled profile retains the frozen 800 requests and v9 defaults without a legacy extra row", async () => {
  const before = await loadG4Suite("g4-code-v9");
  assert.equal(suite.tasks.length, 48);
  assert.equal(g4CallPlan(suite.tasks, suite.protocol).length, 800);
  assert.equal(
    suite.tasks.some((t) => t.legacy),
    false,
  );
  for (const field of [
    "model",
    "device",
    "retrieval",
    "limit",
    "repetitions",
    "quality_repetition",
    "request",
    "execution",
    "evidence_scoring",
  ])
    assert.deepEqual(suite.protocol[field], before.protocol[field]);
  const frozen = await readJson(join(compiledFixtures, "call-plan.json"));
  assert.equal(
    frozen.inputs_file_sha256,
    sha256(
      await readFile(new URL("../data/g4-compiled-v1.json", import.meta.url)),
    ),
  );
});

test("archive materialization preserves every exact main source, generated config and independent boundary", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "zg-compiled-materialize-"));
  t.after(() => rm(directory, { recursive: true }));
  const sources = await unpackCompiledSources(join(directory, "archive"));
  for (const repo of suite.inputs.repositories) {
    const root = join(directory, repositorySlug(repo.repository));
    await materializeCompiledCorpus(repo, root, sources);
    const manifest = await compiledCorpusManifest(root, repo);
    const expected = [
      ...repo.selection.files,
      ...repo.selection.configuration_files,
    ];
    if (repo.selection.generated_configuration)
      expected.push("compile_commands.json");
    assert.deepEqual(
      manifest.entries.map((e) => e.path).sort(),
      expected.sort(),
    );
    for (const boundary of suite.inputs.boundary_cases.filter(
      (b) => b.repository === repo.repository,
    )) {
      const workspace = join(directory, boundary.case_id.replace(":", "-"));
      await materializeCompiledCorpus(repo, workspace, sources, boundary);
      const snapshot = await compiledCorpusManifest(workspace, repo, boundary);
      assert.deepEqual(
        snapshot.entries.map((e) => e.path).sort(),
        [...boundary.workspace_files].sort(),
      );
      assert.equal(
        snapshot.entries.some((e) => e.path === "compile_commands.json"),
        false,
      );
    }
    assert.ok(
      !manifest.entries.some(
        (e) => e.path.includes("answers") || e.path.includes("provenance"),
      ),
    );
  }
});

test("changed source and accidental answers inside the indexed tree are rejected", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "zg-compiled-drift-"));
  t.after(() => rm(directory, { recursive: true }));
  const sources = await unpackCompiledSources(join(directory, "archive"));
  const repo = suite.inputs.repositories[0],
    root = join(directory, "workspace");
  await materializeCompiledCorpus(repo, root, sources);
  await writeFile(join(root, "answers.json"), "{}");
  await assert.rejects(
    compiledCorpusManifest(root, repo),
    /unexpected indexed-tree file/,
  );
  await rm(join(root, "answers.json"));
  await writeFile(
    join(sources, repositorySlug(repo.repository), "uuid.go"),
    "changed",
  );
  await assert.rejects(
    materializeCompiledCorpus(repo, join(directory, "changed"), sources),
    /source drift/,
  );
});

test("exact source paths preserve C/C++ extension case without a Rust/Python suffix filter", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "zg-compiled-case-"));
  t.after(() => rm(directory, { recursive: true }));
  const paths = ["upper/A.C", "lower/a.c", "upper/Types.H", "lower/types.h"];
  const bytes = "// 中\r\n";
  const repo = {
    repository: "test/case",
    files: paths.map((path) => ({
      path,
      bytes: Buffer.byteLength(bytes),
      sha256: sha256(bytes),
    })),
    selection: { files: paths, configuration_files: [] },
  };
  const sources = join(directory, "sources");
  for (const path of paths) {
    const full = join(sources, "test__case", path);
    await mkdir(join(full, ".."), { recursive: true });
    await writeFile(full, bytes);
  }
  const root = join(directory, "workspace");
  await materializeCompiledCorpus(repo, root, sources);
  assert.deepEqual(
    (await compiledCorpusManifest(root, repo)).entries
      .map((e) => e.path)
      .sort(),
    paths.sort(),
  );
});

test("v9 visible-byte scoring applies to new paths, Unicode, CRLF, gaps and truncation", () => {
  for (const path of [
    "src/a.go",
    "src/A.java",
    "src/a.c",
    "src/a.cc",
    "upper/A.C",
    "upper/A.H",
  ]) {
    for (const newline of ["\n", "\r\n"]) {
      const source = Buffer.from(["中", "", "emoji🙂"].join(newline));
      const sources = new Map([[path, source]]);
      const parse = (lines) =>
        parseGraphResponse(
          {
            content: [
              {
                type: "text",
                text: `freshness: fresh\n#1 matchedBy=fts ${path}:1-3\nsource:\n${lines}`,
              },
            ],
          },
          "augment",
        );
      const required = [
        { path, sha256: sha256(source), start: 0, end: source.length },
      ];
      assert.equal(
        coverage(
          required,
          visibleRanges(parse("1\t中\n2\t\n3\temoji🙂"), sources, true),
        ).complete,
        1,
      );
      assert.equal(
        coverage(
          required,
          visibleRanges(parse("1\t中\n...\n3\temoji"), sources, true),
        ).complete,
        0,
      );
      assert.ok(
        coverage(required, visibleRanges(parse("1\t中\n2\t"), sources, true))
          .covered_bytes < source.length,
      );
    }
  }
});

test("schema defaults and response normalization retain all non-timing budget counts", () => {
  const budget = {
    nodes: 2000,
    edges: 10000,
    candidates: 5000,
    mapping_units: 5000,
    output_bytes: 65536,
    filter_bytes: 262144,
  };
  const tools = {
    tools: [
      {
        name: "zvec_grep_explore",
        inputSchema: {
          $defs: {
            BudgetLimits: {
              properties: Object.fromEntries(
                Object.entries(budget).map(([k, v]) => [k, { default: v }]),
              ),
            },
            ExecutionOptions: {
              properties: {
                budget: { default: budget },
                request_timeout_ms: { default: 30000 },
                graph_timeout_ms: { default: 5000 },
              },
            },
          },
        },
      },
    ],
  };
  verifyDefaultSchema(tools, suite.protocol, budget);
  const altered = structuredClone(tools);
  altered.tools[0].inputSchema.$defs.ExecutionOptions.properties.graph_timeout_ms.default = 6000;
  assert.throws(
    () => verifyDefaultSchema(altered, suite.protocol, budget),
    /timeout changed/,
  );
  const graph = {
    execution: {
      status: "complete",
      qualification_micros: 1,
      query_micros: 2,
      usage: { nodes: 3, output_bytes: 4 },
    },
  };
  assert.deepEqual(normalizeBoundaryGraph(graph), {
    execution: { status: "complete", usage: { nodes: 3 } },
  });
  graph.execution.status = "partial";
  assert.throws(() => normalizeBoundaryGraph(graph));
});

test("boundary matching requires the frozen callee, real abstention and no fabricated target", () => {
  const boundary = suite.inputs.boundary_cases.find(
    (b) => b.case_id === "go:B-01",
  );
  const annotation = suite.inputs.reference_annotations.find(
    (r) =>
      r.repository === boundary.repository &&
      r.site.quote === "UnmarshalBinary",
  );
  const reference = {
    kind: "calls",
    status: "unresolved",
    target: null,
    evidence: {
      source_id: annotation.site.path,
      span: {
        start: annotation.expression_context.start,
        end: annotation.expression_context.end - 3,
      },
    },
  };
  assert.equal(
    boundaryReference({ references: [reference] }, boundary, suite.inputs),
    reference,
  );
  assert.throws(
    () => boundaryReference({ references: [] }, boundary, suite.inputs),
    /missing or ambiguous/,
  );
  assert.throws(
    () =>
      boundaryReference(
        { references: [{ ...reference, target: "guessed" }] },
        boundary,
        suite.inputs,
      ),
    /invented/,
  );
  assert.throws(() =>
    boundaryReference(
      { references: [{ ...reference, status: "resolved" }] },
      boundary,
      suite.inputs,
    ),
  );
});

test("new protocol run/replay preserve all 800 blocked calls with unavailable quality", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "zg-compiled-blocked-"));
  t.after(() => rm(directory, { recursive: true }));
  await writeJson(join(directory, "suite.json"), {
    identity: suite.identity,
    inputs: suite.inputs,
    protocol: suite.protocol,
  });
  for (const repo of suite.inputs.repositories) {
    const output = join(directory, repositorySlug(repo.repository));
    const tasks = suite.tasks.filter((x) => x.repository === repo.repository);
    await writeJson(join(output, "run.json"), {
      commit: repo.commit,
      suite: suite.identity,
      status: "preparation_failed",
      integrity_verified: false,
      issues: ["source preparation blocked"],
    });
    await writeJson(
      join(output, "calls.json"),
      g4CallPlan(tasks, suite.protocol).map((call) => ({
        ...call,
        status: "blocked",
        reason: "source preparation blocked",
        request: g4Request(
          tasks.find((x) => x.task_id === call.task_id),
          call,
          "<unavailable>",
          suite.protocol,
        ),
      })),
    );
  }
  const report = await aggregateG4(directory, COMPILED_PROFILE);
  assert.equal(report.integrity_passed, false);
  assert.equal(report.correctness_passed, false);
  assert.equal(
    report.repositories.reduce((n, r) => n + r.blocked_calls, 0),
    800,
  );
  assert.equal(
    report.repositories.some((r) => r.budget_boundary),
    false,
  );
  assert.ok(
    report.repositories.every((r) =>
      r.quality.every(
        (q) =>
          q.usable_pairs === 0 && q.arms.every((a) => a.quality_samples === 0),
      ),
    ),
  );
  const markdown = await readFile(join(directory, "report.md"), "utf8");
  assert.ok(!markdown.includes("旧 requests"));
  assert.equal(
    report.limits.some((x) => x.includes("Two public")),
    false,
  );
  const before = objectHash(report);
  await assert.rejects(
    run(process.execPath, [
      "benchmarks/zg-retrieval/report.mjs",
      "--protocol",
      COMPILED_PROFILE,
      directory,
    ]),
    (error) =>
      error.result.code === 1 &&
      !error.result.stderr.includes("unknown protocol"),
  );
  assert.equal(
    objectHash(await readJson(join(directory, "report.json"))),
    before,
  );
  await assert.rejects(
    run(process.execPath, [
      "benchmarks/zg-retrieval/run.mjs",
      "--protocol",
      COMPILED_PROFILE,
    ]),
    (error) =>
      error.result.code === 1 &&
      error.result.stderr.includes("G4 requires --package"),
  );
});

test("v2 changes only invalid Java root configuration and declares the grammar repair separately", async () => {
  const revision = await loadG4Suite(COMPILED_REVISION);
  assert.deepEqual(revision.tasks, suite.tasks);
  for (const key of [
    "reference_annotations",
    "relation_annotations",
    "boundary_cases",
  ])
    assert.deepEqual(revision.inputs[key], suite.inputs[key]);
  assert.deepEqual(
    g4CallPlan(revision.tasks, revision.protocol),
    g4CallPlan(suite.tasks, suite.protocol),
  );
  for (const call of g4CallPlan(revision.tasks, revision.protocol)) {
    const task = revision.tasks.find((t) => t.task_id === call.task_id);
    assert.deepEqual(
      g4Request(task, call, "/frozen", revision.protocol),
      g4Request(task, call, "/frozen", suite.protocol),
    );
  }
  const java = revision.inputs.repositories.find((r) => r.language === "java");
  assert.equal(java.selection.graph.module_roots.length, 1);
  assert.equal(java.selection.graph.module_roots[0].root, "tools/java_api/src");
  assert.equal(
    suite.inputs.repositories.find((r) => r.language === "java").selection.graph
      .module_roots.length,
    2,
  );
  assert.equal(suite.inputs.boundary_cases.at(-1).expected, "not_ready");
  assert.deepEqual(revision.protocol.parser_conformance, {
    "cpp:B-02": "ready_locate",
  });
});
