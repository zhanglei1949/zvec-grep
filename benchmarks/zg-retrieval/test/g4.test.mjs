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
} from "../engines/zg/g4-parse.mjs";
import { PublicMcp } from "../engines/zg/g4-rpc.mjs";
import { aggregateG4 } from "../engines/zg/g4-report.mjs";
import { sha256, writeJson, readJson } from "../core/io.mjs";
import { repositorySlug } from "../core/corpus.mjs";
import { loadSuite } from "../core/suite.mjs";

const suite = await loadG4Suite();
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
  const dir = await temporary(t);
  await writeJson(join(dir, "suite.json"), {
    identity: suite.identity,
    protocol: suite.protocol,
    inputs: suite.inputs,
  });
  for (const repo of suite.inputs.repositories) {
    const p = join(dir, repositorySlug(repo.repository));
    await mkdir(p);
    const tasks = suite.tasks.filter((x) => x.repository === repo.repository);
    await writeJson(join(p, "run.json"), {
      commit: repo.commit,
      suite: suite.identity,
      status: "preparation_failed",
      issues: ["graph not ready"],
    });
    await writeJson(
      join(p, "calls.json"),
      g4CallPlan(tasks, suite.protocol).map((c) => ({
        ...c,
        status: "blocked",
        request: g4Request(
          tasks.find((x) => x.task_id === c.task_id),
          c,
          "<unavailable>",
          suite.protocol,
        ),
      })),
    );
  }
  const a = await aggregateG4(dir),
    b = await aggregateG4(dir);
  assert.deepEqual(a, b);
  assert.equal(a.integrity_passed, false);
  assert.equal(
    a.repositories.reduce((n, r) => n + r.blocked_calls, 0),
    458,
  );
  const repo = suite.inputs.repositories[0],
    p = join(dir, repositorySlug(repo.repository), "calls.json"),
    rows = await readJson(p);
  rows.pop();
  await writeJson(p, rows);
  await assert.rejects(aggregateG4(dir), /missing call matrix/);
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
