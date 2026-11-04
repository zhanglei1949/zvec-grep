import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import test from "node:test";
import {
  loadG4Suite,
  g4CallPlan,
  g4Request,
  coverage,
  scopeViolations,
  validateRange,
  evidenceContribution,
} from "../core/g4.mjs";
import {
  parseGraphResponse,
  visibleRanges,
  scoreSearch,
  collectNodes,
  scoreExplore,
  scoreBudgetBoundary,
} from "../engines/zg/g4-parse.mjs";
import { PublicMcp } from "../engines/zg/g4-rpc.mjs";
import { aggregateG4 } from "../engines/zg/g4-report.mjs";
import { sha256, writeJson, readJson } from "../core/io.mjs";
import { repositorySlug } from "../core/corpus.mjs";
import { loadSuite } from "../core/suite.mjs";

const suite = await loadG4Suite();

test("v9 changes only visible-byte scoring and retains v8 inputs, calls and budgets", async () => {
  const previous = await loadG4Suite("g4-code-v8"),
    next = await loadG4Suite("g4-code-v9");
  assert.deepEqual(next.inputs, previous.inputs);
  assert.equal(next.protocol.evidence_scoring, "visible-bytes-v2");
  const { id, evidence_scoring, ...rest } = next.protocol;
  assert.deepEqual({ ...rest, id: previous.protocol.id }, previous.protocol);
  const calls = g4CallPlan(next.tasks, next.protocol);
  assert.equal(calls.length, 458);
  assert.deepEqual(calls, g4CallPlan(previous.tasks, previous.protocol));
  for (const call of calls) {
    const task = next.tasks.find((t) => t.task_id === call.task_id);
    assert.deepEqual(
      g4Request(task, call, "/frozen", next.protocol),
      g4Request(task, call, "/frozen", previous.protocol),
    );
  }
});

test("v9 credits only proven numbered empty-line newlines; old scoring stays unchanged", () => {
  for (const source of ["a\n\nb", "a\r\n\r\nb", "\n\nx", "中\n\n🙂"]) {
    const bytes = Buffer.from(source),
      sources = new Map([["src/a.rs", bytes]]);
    const lines = source
      .split(/\r?\n/)
      .map((s, i) => `${i + 1}\t${s}`)
      .join("\n");
    const parsed = parseGraphResponse(
      text(
        `freshness: fresh\n#1 matchedBy=fts src/a.rs:1-${source.split(/\r?\n/).length}\nsource:\n${lines}`,
      ),
      "augment",
    );
    const required = [
      { path: "src/a.rs", sha256: sha256(bytes), start: 0, end: bytes.length },
    ];
    assert.equal(
      coverage(required, visibleRanges(parsed, sources, true)).complete,
      1,
    );
    assert.ok(
      coverage(required, visibleRanges(parsed, sources)).covered_bytes <
        bytes.length,
    );
  }
  for (const [source, lines, covered] of [
    [
      "a\n\nb",
      [
        { line: 1, text: "a" },
        { line: 2, text: "" },
      ],
      2,
    ],
    [
      "a\n\nb",
      [
        { line: 1, text: "a" },
        { line: 3, text: "b" },
      ],
      2,
    ],
    [
      "a\n \nb",
      [
        { line: 1, text: "a" },
        { line: 2, text: "" },
        { line: 3, text: "b" },
      ],
      3,
    ],
    [
      "abcdef\n\nb",
      [
        { line: 1, text: "abc" },
        { line: 2, text: "" },
        { line: 3, text: "b" },
      ],
      5,
    ],
    ["a\n", [{ line: 1, text: "a" }], 1],
  ]) {
    const bytes = Buffer.from(source),
      sources = new Map([["src/a.rs", bytes]]);
    const parsed = {
      items: [{ path: "src/a.rs", source_lines: lines, unnumbered_source: [] }],
    };
    assert.equal(
      coverage(
        [
          {
            path: "src/a.rs",
            sha256: sha256(bytes),
            start: 0,
            end: bytes.length,
          },
        ],
        visibleRanges(parsed, sources, true),
      ).covered_bytes,
      covered,
    );
  }
  const parsed = parseGraphResponse(
    text(
      "freshness: fresh\n#1 matchedBy=fts src/a.rs:1-3\nsource:\n1\ta\n...\n3\tb",
    ),
    "augment",
  );
  const bytes = Buffer.from("a\n\nb");
  assert.equal(
    coverage(
      [{ path: "src/a.rs", sha256: sha256(bytes), start: 0, end: 4 }],
      visibleRanges(parsed, new Map([["src/a.rs", bytes]]), true),
    ).covered_bytes,
    2,
  );
});

test("v8 freezes corrected inputs and keeps v7 retrieval questions and budgets", async () => {
  const next = await loadG4Suite("g4-code-v8");
  assert.equal(g4CallPlan(next.tasks, next.protocol).length, 458);
  assert.equal(
    next.inputs.tasks.filter((t) => t.case_role === "normal").length,
    43,
  );
  assert.equal(
    next.inputs.tasks.filter((t) => t.case_role === "budget_boundary").length,
    5,
  );
  assert.deepEqual(next.protocol.execution, suite.protocol.execution);
  for (const task of next.inputs.tasks) {
    const old = suite.inputs.tasks.find((t) => t.task_id === task.task_id);
    assert.deepEqual(task.retrieval, old.retrieval);
  }
  const old = suite.inputs.tasks.find((t) => t.task_id === "requests:G4-24");
  const fixed = next.inputs.tasks.find((t) => t.task_id === old.task_id);
  assert.ok(
    fixed.contract.must_not_resolve[0].start >
      old.contract.must_not_resolve[0].start,
  );
  assert.equal(fixed.contract.must_not_resolve[0].quote, "dict_class");
  for (const id of [
    "ripgrep:G4-05",
    "ripgrep:G4-19",
    "ripgrep:G4-20",
    "ripgrep:G4-23",
  ]) {
    const op = next.inputs.tasks.find((t) => t.task_id === id).explore_request
      .operation;
    const locators = [
      op.anchor,
      op.start,
      op.target,
      op.selector?.owner,
    ].filter((x) => x?.name === "escape");
    assert.equal(locators.length, 1);
    assert.equal(locators[0].kind, "function");
  }
});

test("budget negatives require the declared reason and still validate partial evidence", () => {
  const task = {
    budget_expectation: {
      status: "product_error",
      reason: "complete result exceeds output budget",
    },
  };
  assert.equal(
    scoreBudgetBoundary(
      task,
      { status: "product_error", text: "timeout" },
      new Map(),
    ).passed,
    false,
  );
  assert.equal(
    scoreBudgetBoundary(
      task,
      { status: "product_error", text: task.budget_expectation.reason },
      new Map(),
    ).passed,
    true,
  );
  task.budget_expectation = { status: "partial", stop_reason: "output_budget" };
  task.contract = { locations: ["unique"] };
  const parsed = {
    status: "success",
    items: [],
    graph: {
      execution: { status: "partial", stop_reason: "output_budget" },
      locations: [{ status: "unique" }],
      evidence: [],
    },
  };
  assert.equal(scoreBudgetBoundary(task, parsed, new Map()).passed, true);
  parsed.graph.execution.status = "complete";
  assert.equal(scoreBudgetBoundary(task, parsed, new Map()).passed, false);
  parsed.graph.execution.status = "partial";
  parsed.graph.locations = [];
  assert.equal(scoreBudgetBoundary(task, parsed, new Map()).passed, false);
  parsed.graph.locations = [{ status: "unique" }];
  parsed.items = [{ path: "missing.rs", source_lines: [] }];
  assert.throws(
    () => scoreBudgetBoundary(task, parsed, new Map()),
    /unrecorded source/,
  );
});
const text = (value, isError = false) => ({
  isError,
  content: [{ type: "text", text: value }],
});
const body = Buffer.from("fn f() {}\nfn g() {}\n"),
  hash = sha256(body);
const range = {
  path: "src/a.rs",
  sha256: hash,
  start: 0,
  end: 9,
  quote: "fn f() {}",
};
const ordinary =
  "freshness: fresh\n#1 matchedBy=fts src/a.rs:1\nsource:\n1\tfn f() {}";

async function temporary(t) {
  const p = await mkdtemp(join(tmpdir(), "zg-g4-test-"));
  t.after(() => rm(p, { recursive: true, force: true }));
  return p;
}

test("v7 freezes two corpora and the 48/24/16 matrix without changing v6", async () => {
  assert.equal(suite.inputs.tasks.length, 48);
  assert.equal(g4CallPlan(suite.tasks, suite.protocol).length, 458);
  const v6 = await loadSuite();
  assert.equal(v6.lock.tasks.length, 20);
  assert.deepEqual(v6.protocol.modes, ["hybrid", "fts", "vector"]);
});
test("paired calls alternate arms and retain five samples and the fifth quality result", () => {
  const task = suite.tasks.find((t) => t.retrieval?.constrain),
    calls = g4CallPlan([task], suite.protocol);
  for (const family of ["augment", "constrain"]) {
    const c = calls.filter((x) => x.family === family);
    assert.equal(c.length, 10);
    assert.notEqual(c[0].arm, c[2].arm);
    assert.equal(c.filter((x) => x.quality_observation).length, 2);
    assert.deepEqual([...new Set(c.map((x) => x.repetition))], [1, 2, 3, 4, 5]);
  }
});
test("retrieval route and graph modes are independent and deadlines are not widened", () => {
  const task = suite.tasks.find((t) => t.retrieval?.constrain);
  for (const call of g4CallPlan([task], suite.protocol)) {
    const request = g4Request(task, call, "/corpus", suite.protocol);
    assert.equal(request.arguments.root, "/corpus");
    if (call.arm === "off") assert.equal(request.arguments.graph, undefined);
    else if (call.arm !== "explore") {
      assert.equal(request.arguments.query, task.retrieval.query);
      assert.equal(request.arguments.graphExecution.graph_timeout_ms, 5000);
    }
    assert.equal(request.arguments.preview, undefined);
  }
});
test("range validation rejects escaping paths and invalid UTF-8 lengths", () => {
  validateRange(range);
  assert.throws(() => validateRange({ ...range, path: "../source" }));
  assert.throws(() => validateRange({ ...range, quote: "🙂" }));
});
test("coverage unions overlapping bytes, requires matching digest and does not count range declarations", () => {
  assert.equal(
    coverage(
      [range],
      [
        { ...range, end: 4 },
        { ...range, start: 3 },
      ],
    ).complete_fraction,
    1,
  );
  assert.equal(coverage([range], [{ ...range, end: 4 }]).complete_fraction, 0);
  assert.equal(
    coverage([range], [{ ...range, sha256: "0".repeat(64) }]).covered_bytes,
    0,
  );
  assert.equal(coverage([range], []).complete, 0);
});
test("scope validation checks whole intervals including intermediate gaps", () => {
  assert.equal(scopeViolations([range], [range]).length, 0);
  assert.equal(scopeViolations([{ ...range, end: 15 }], [range]).length, 1);
  assert.equal(
    scopeViolations(
      [range],
      [
        { ...range, end: 3 },
        { ...range, start: 4 },
      ],
    ).length,
    1,
  );
});
test("public search parser separates ordinary output and graph JSON, also for empty results", () => {
  const graph = { status: "complete", evidence: [], scope: null };
  const parsed = parseGraphResponse(
    text(`${ordinary}\n\nGraph\n\`\`\`json\n${JSON.stringify(graph)}\n\`\`\``),
    "augment",
  );
  assert.equal(parsed.items.length, 1);
  assert.equal(parsed.ordinary_text, ordinary);
  assert.deepEqual(parsed.graph, graph);
  assert.equal(
    parseGraphResponse(
      text(
        `freshness: fresh\nNo matches.\n\nGraph\n\`\`\`json\n${JSON.stringify(graph)}\n\`\`\``,
      ),
      "constrain",
    ).items.length,
    0,
  );
});
test("truncated output and unknown completion cannot masquerade as successful empty results", () => {
  assert.throws(() =>
    parseGraphResponse(text(`${ordinary}\nGraph\n\`\`\`json\n{`), "augment"),
  );
  assert.throws(() =>
    parseGraphResponse(
      text('{"view":{},"locations":[],"evidence":[],"execution":{}}'),
      "explore",
    ),
  );
  assert.equal(
    parseGraphResponse(text("deadline", true), "explore").status,
    "product_error",
  );
});
test("visible evidence needs actual matching bytes and ignores retrieved pointers", () => {
  const sources = new Map([[range.path, body]]),
    parsed = parseGraphResponse(text(ordinary), "augment");
  assert.equal(coverage([range], visibleRanges(parsed, sources)).complete, 1);
  parsed.graph = {
    evidence: [
      {
        status: "retrieved",
        reference: { source_id: range.path, span: range },
      },
    ],
  };
  assert.equal(visibleRanges(parsed, sources).length, 1);
  parsed.items = [];
  assert.equal(visibleRanges(parsed, sources).length, 0);
  parsed.graph = {
    evidence: [
      {
        status: "verified",
        reference: {
          source_id: range.path,
          raw_digest: hash,
          decoded_digest: hash,
          span: { start: 0, end: 9 },
        },
        content: "fn f() {}",
      },
    ],
  };
  assert.equal(visibleRanges(parsed, sources).length, 1);
  parsed.graph.evidence[0].content = "wrong";
  assert.throws(() => visibleRanges(parsed, sources));
});
test("ordinary source corruption and unrecorded files fail evidence verification", () => {
  const parsed = parseGraphResponse(text(ordinary), "augment");
  assert.throws(() => visibleRanges(parsed, new Map()));
  assert.throws(() =>
    visibleRanges(parsed, new Map([[range.path, Buffer.from("fn h() {}")]])),
  );
});
test("Constrain requires complete exact scope and reports leaked visible text", () => {
  const task = {
    retrieval: { scope_required_evidence: [range], allowed_regions: [range] },
  };
  const parsed = parseGraphResponse(text(ordinary), "constrain");
  parsed.graph = {
    scope: { complete: true, exact_filter: true },
    evidence: [],
  };
  assert.ok(
    scoreSearch(
      task,
      { family: "constrain", arm: "constrain" },
      parsed,
      new Map([[range.path, body]]),
    ).valid_scope,
  );
  task.retrieval.allowed_regions = [{ ...range, start: 1 }];
  assert.ok(
    !scoreSearch(
      task,
      { family: "constrain", arm: "constrain" },
      parsed,
      new Map([[range.path, body]]),
    ).valid_scope,
  );
});
test("blocked matrix can be replayed but cannot yield an all-green report", async (t) => {
  for (const profile of ["g4-code-v7", "g4-code-v8", "g4-code-v9"]) {
    const selected = await loadG4Suite(profile);
    const dir = await temporary(t);
    await writeJson(join(dir, "suite.json"), {
      identity: selected.identity,
      protocol: selected.protocol,
      inputs: selected.inputs,
    });
    for (const repo of selected.inputs.repositories) {
      const p = join(dir, repositorySlug(repo.repository));
      await mkdir(p);
      const tasks = selected.tasks.filter(
        (x) => x.repository === repo.repository,
      );
      await writeJson(join(p, "run.json"), {
        commit: repo.commit,
        suite: selected.identity,
        status: "preparation_failed",
        issues: ["graph not ready"],
      });
      await writeJson(
        join(p, "calls.json"),
        g4CallPlan(tasks, selected.protocol).map((c) => ({
          ...c,
          status: "blocked",
          request: g4Request(
            tasks.find((x) => x.task_id === c.task_id),
            c,
            "<unavailable>",
            selected.protocol,
          ),
        })),
      );
    }
    const a = await aggregateG4(dir, profile),
      b = await aggregateG4(dir, profile);
    assert.deepEqual(a, b);
    assert.equal(a.integrity_passed, false);
    if (profile !== "g4-code-v7") {
      assert.equal(
        a.repositories.reduce((n, r) => n + r.budget_boundary.total, 0),
        5,
      );
      assert.equal(
        a.repositories.reduce((n, r) => n + r.budget_boundary.passed, 0),
        0,
      );
    }
    assert.equal(
      a.repositories.reduce((n, r) => n + r.blocked_calls, 0),
      458,
    );
    const repo = selected.inputs.repositories[0],
      p = join(dir, repositorySlug(repo.repository), "calls.json"),
      rows = await readJson(p);
    rows.pop();
    await writeJson(p, rows);
    await assert.rejects(aggregateG4(dir, profile), /missing call matrix/);
  }
});
test("raw stdio client speaks public JSON-RPC and closes its owned child", async (t) => {
  const dir = await temporary(t),
    script = join(dir, "server.cjs");
  await writeFile(
    script,
    `require('readline').createInterface({input:process.stdin}).on('line',s=>{const m=JSON.parse(s);if(m.id)console.log(JSON.stringify({jsonrpc:'2.0',id:m.id,result:{method:m.method}}));});`,
  );
  const rpc = new PublicMcp([process.execPath, script], {});
  t.after(() => rpc.close());
  assert.equal((await rpc.initialize()).method, "initialize");
  assert.equal((await rpc.request("tools/list", {})).method, "tools/list");
  await rpc.close();
});

test("partial first source lines earn only the unique printed suffix bytes", () => {
  const body = Buffer.from("        return host\n");
  const parsed = parseGraphResponse(
    text(
      "freshness: fresh\n#1 matchedBy=fts src/a.rs:1\nsource:\n1\t   return host",
    ),
    "augment",
  );
  const ranges = visibleRanges(parsed, new Map([["src/a.rs", body]]));
  assert.equal(ranges[0].start, 5);
  assert.equal(ranges[0].end, 19);
});
test("duplicate required ranges are scored once", () => {
  assert.equal(coverage([range, range], [range]).required, 1);
});

test("node identity ignores JSON member order and traversal distance", () => {
  const node = {
    node: { node_id: "n", graph_build_id: "b" },
    source_id: "src/a.rs",
    kind: "function",
    name: "f",
    qualified_name: "f",
    span: { start: 0, end: 9 },
    distance: 1,
  };
  const map = collectNodes({ graph: { nodes: [node] } });
  const reordered = {
    ...Object.fromEntries(Object.entries(node).reverse()),
    distance: null,
    span: { end: 9, start: 0 },
  };
  assert.equal(collectNodes({ graph: { nodes: [reordered] } }, map).size, 1);
});
test("path scoring follows public NodeRef objects rather than string IDs", () => {
  const nodes = ["a", "b", "c"].map((name) => ({
    node: { node_id: name, graph_build_id: "b" },
    name,
  }));
  const parsed = {
    status: "success",
    items: [],
    graph: {
      execution: { status: "complete" },
      locations: [
        { status: "unique", candidates: [] },
        { status: "unique", candidates: [] },
      ],
      nodes,
      relations: [],
      references: [],
      evidence: [],
      paths: [{ nodes: nodes.map((n) => n.node), relations: ["ab", "bc"] }],
    },
  };
  const task = {
    truth: { nodes: [], edges: [], required_evidence: [] },
    contract: {
      locations: ["unique", "unique"],
      path_hops: 2,
      path_names: ["a", "b", "c"],
    },
  };
  assert.ok(
    scoreExplore(
      task,
      parsed,
      new Map(),
      new Map(nodes.map((n) => [n.node.node_id, n])),
    ).passed,
  );
});
test("Constrain checks public symbol metadata as well as source bytes", () => {
  const parsed = parseGraphResponse(
    text(ordinary.replace("source:", "symbol: function f\nsource:")),
    "constrain",
  );
  parsed.graph = {
    scope: { complete: true, exact_filter: true },
    evidence: [],
  };
  const task = {
      retrieval: { scope_required_evidence: [range], allowed_regions: [range] },
    },
    sources = new Map([[range.path, body]]);
  assert.ok(
    scoreSearch(
      task,
      { family: "constrain", arm: "constrain" },
      parsed,
      sources,
    ).valid_scope,
  );
  parsed.items[0].metadata = ["symbol: function g"];
  assert.ok(
    !scoreSearch(
      task,
      { family: "constrain", arm: "constrain" },
      parsed,
      sources,
    ).valid_scope,
  );
});

test("additional evidence subtracts baseline bytes and leaves incomplete gold unjudged", () => {
  const result = evidenceContribution(
    [range],
    [{ ...range, end: 3 }],
    [{ ...range, end: 15 }],
  );
  assert.deepEqual(result, {
    additional_bytes: 12,
    known_required_additional_bytes: 6,
    unjudged_additional_bytes: 6,
  });
});
