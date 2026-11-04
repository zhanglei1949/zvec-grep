import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { createServer } from "node:net";
import { run, writeJson, sha256, fileHash } from "../../core/io.mjs";
import { PublicMcp } from "./g4-rpc.mjs";
import { diskInventory } from "./g4-run.mjs";
import { loadG4Suite } from "../../core/g4.mjs";

const repo = process.cwd(),
  base = resolve(process.argv[2]),
  root = join(base, "workspace"),
  home = join(base, "runtime");
const input = join(
    repo,
    "benchmarks/zg-retrieval/data/m7-six-languages-v1.json",
  ),
  spec = JSON.parse(await readFile(input, "utf8"));
const parserSuite = process.argv[5] ? await loadG4Suite(process.argv[5]) : null;
const binary = resolve(process.argv[3] ?? join(repo, "rust/target/release/zg")),
  api = resolve(
    process.argv[4] ?? join(repo, "rust/target/debug/examples/m7_api"),
  );
await mkdir(base);
await mkdir(root);
await mkdir(home);
await writeJson(join(base, "frozen.json"), {
  input_sha256: await fileHash(input),
  generator_sha256: await fileHash(
    join(repo, "benchmarks/zg-retrieval/tools/m7-labels.py"),
  ),
  binary_sha256: await fileHash(binary),
  api_sha256: await fileHash(api),
  parser_suite: parserSuite?.identity ?? null,
  frozen_before_candidate: true,
});
for (const s of spec.sources) {
  assert.equal(sha256(s.text), s.sha256);
  await mkdir(join(root, s.path, ".."), { recursive: true });
  await writeFile(join(root, s.path), s.text);
}
const net = createServer();
await new Promise((r) => net.listen(0, "127.0.0.1", r));
const port = net.address().port;
await new Promise((r) => net.close(r));
await writeJson(join(home, "config.json"), {
  version: 1,
  server: { host: "127.0.0.1", port },
});
const env = {
  ...process.env,
  ZVEC_GREP_HOME: home,
  ZVEC_GREP_WORKSPACE_REGISTRY: join(home, "registry.json"),
  ZVEC_GREP_AUTHORIZATION_KEY_FILE: join(home, "authorization.key"),
  NO_COLOR: "1",
  FORCE_COLOR: "0",
};
const apiEnv = { ...env };
delete env.DYLD_LIBRARY_PATH;
delete env.LD_LIBRARY_PATH;
const cli = (args) => run(binary, args, { cwd: root, env }),
  rust = (args) => run(api, args, { cwd: root, env: apiEnv });
const graph = join(base, "graph.json");
await writeJson(graph, spec.graph);
await writeJson(
  join(base, "index.json"),
  await cli([
    "--index",
    root,
    "--mode",
    "direct",
    "--embedding",
    "local/potion-code-16m-v2",
    "--device",
    "cpu",
    "--model-cache",
    "/private/tmp/zg-next/model-cache",
    "--graph-options",
    graph,
    "--glob",
    "!**/go.mod",
    "--glob",
    "!compile_commands.json",
  ]),
);
const cases = JSON.parse(
  await readFile(
    join(
      repo,
      "rust/crates/zg-knowledge/tests/fixtures/semantic-source-changes-v1.json",
    ),
    "utf8",
  ),
).cases;
async function apply(name, input) {
  const p = join(base, `semantic-${name}.json`);
  await writeJson(p, input);
  await writeJson(
    join(base, `apply-${name}.json`),
    await rust(["apply", root, p]),
  );
}
await apply("initial", cases.find((c) => c.name === "initial").input);
await apply(
  "correction",
  cases.find((c) => c.name === "explicit-correction").input,
);
const manifest = JSON.parse(
    await readFile(join(root, ".zvec-grep/manifest.json"), "utf8"),
  ),
  storage = join(
    root,
    ".zvec-grep/generations",
    manifest.storageGeneration,
    "storage",
  ),
  before = await diskInventory(storage);
const named = (name, source_id) => ({
    by: "name",
    name,
    ...(source_id ? { source_id } : {}),
  }),
  req = (operation) => ({ domain: "code", operation });
const sem = (history) => ({
  domain: "semantic",
  operation: {
    operation: "semantic",
    query: {
      space_id: "workspace",
      context: "project-a",
      predicate: "owner",
      history,
    },
  },
});
const normalize = (v) => {
  assert.equal(v.execution.status, "complete", JSON.stringify(v.execution));
  const c = structuredClone(v);
  delete c.execution.qualification_micros;
  delete c.execution.query_micros;
  delete c.execution.usage.output_bytes;
  return c;
};
const requests = [],
  targets = new Map();
for (const [i, c] of spec.calls.entries())
  requests.push({
    name: `target-${i}`,
    request: req({
      operation: "locate",
      target: named(c.target, c.target_source),
    }),
    check: (v) => {
      assert.equal(v.locations[0].status, "unique");
      assert.equal(v.locations[0].candidates.length, 1);
      targets.set(
        JSON.stringify([c.target, c.target_source]),
        v.locations[0].candidates[0].node.node_id,
      );
    },
  });
const steps = {
  go: "GoStep",
  java: "javaStep",
  c: "cStep",
  cpp: "cppStep",
  rust: "RustStep",
  python: "PythonStep",
};
for (const p of spec.paths) {
  requests.push({
    name: `${p.language}-path`,
    request: req({
      operation: "shortest_path",
      start: named(p.anchor),
      target: named(p.target),
      traversal: { relations: ["calls"], min_hops: 2, max_hops: 2 },
    }),
    check: (v) => {
      assert.equal(v.paths.length, 1);
      assert.equal(v.paths[0].relations.length, 2);
    },
  });
  requests.push({
    name: `${p.language}-callers`,
    request: req({
      operation: "neighbors",
      anchor: named(p.target),
      relations: ["calls"],
      direction: "incoming",
    }),
    check: (v) => {
      assert.equal(v.relations.length, 1);
      assert.ok(v.nodes.some((n) => n.name === steps[p.language]));
    },
  });
  requests.push({
    name: `${p.language}-expand`,
    request: req({
      operation: "expand",
      anchor: named(p.anchor),
      traversal: { relations: ["calls"], min_hops: 1, max_hops: 2 },
    }),
    check: (v) => {
      assert.equal(v.relations.length, 2);
      for (const name of [steps[p.language], p.target])
        assert.ok(v.nodes.some((n) => n.name === name));
    },
  });
}
for (const t of spec.type_relations)
  requests.push({
    name: `${t.anchor}-types`,
    request: req({
      operation: "neighbors",
      anchor: named(t.anchor),
      relations: t.relations,
      direction: "outgoing",
    }),
    check: (v) => {
      assert.equal(v.relations.length, t.targets.length);
      for (const n of t.targets) assert.ok(v.nodes.some((x) => x.name === n));
    },
  });
for (const [i, c] of spec.calls.entries())
  requests.push({
    name: `call-${i}`,
    request: req({
      operation: "references",
      selector: {
        by: "owner",
        owner: {
          by: "position",
          source_id: c.source,
          byte_offset: c.span.start,
          kind: c.source.endsWith(".java") ? "method" : "function",
        },
      },
    }),
    check: (v) => {
      const r = v.references.find(
        (r) =>
          r.kind === "calls" &&
          r.evidence.span.start === c.span.start &&
          r.evidence.span.end === c.span.end,
      );
      assert.ok(r);
      assert.equal(r.status, "resolved");
      assert.equal(
        r.target,
        targets.get(JSON.stringify([c.target, c.target_source])),
      );
      assert.ok(
        v.evidence.some(
          (e) => e.reference.reference_id === r.id && e.content === c.text,
        ),
      );
    },
  });
for (const [i, c] of spec.negative_cases.entries()) {
  const text = spec.sources.find((s) => s.path === c.source).text.trimStart(),
    a = text.lastIndexOf(c.text),
    offset = Buffer.byteLength(text.slice(0, a));
  assert.ok(a >= 0);
  requests.push({
    name: `unknown-${i}`,
    request: req({
      operation: "references",
      selector: { by: "source", source_id: c.source },
    }),
    check: (v) => {
      assert.ok(
        v.references.some(
          (r) =>
            r.kind === "calls" &&
            r.status === c.status &&
            r.evidence.span.start === offset,
        ),
      );
    },
  });
}
for (const c of spec.deferred_languages)
  requests.push({
    name: `deferred-${c.symbol}`,
    request: req({ operation: "locate", target: named(c.symbol, c.source) }),
    check: (v) => assert.equal(v.locations[0].status, "not_found"),
  });
const facts = (v) =>
  v.semantic.objects
    .filter((o) => o.record.type === "fact")
    .map((o) => [o.record.value.identity.id, o.record.value.state]);
requests.push(
  {
    name: "semantic-current",
    request: sem(false),
    check: (v) => assert.deepEqual(facts(v), [["f2", "active"]]),
  },
  {
    name: "semantic-history",
    request: sem(true),
    check: (v) =>
      assert.deepEqual(facts(v), [
        ["f1", "superseded"],
        ["f2", "active"],
      ]),
  },
);
const client = new PublicMcp(
    [
      binary,
      "--server",
      "--stdio",
      "--listen",
      `127.0.0.1:${port}`,
      "--mcp-toolset",
      "full",
    ],
    { cwd: root, env },
  ),
  results = [];
try {
  await client.initialize();
  for (const item of requests) {
    const p = join(base, `request-${item.name}.json`);
    await writeJson(p, item.request);
    const rustApi = JSON.parse((await rust(["explore", root, p])).stdout),
      direct = JSON.parse(
        (
          await cli([
            "--explore",
            "--root",
            root,
            "--mode",
            "direct",
            "--request",
            p,
          ])
        ).stdout,
      ),
      daemon = JSON.parse(
        (
          await cli([
            "--explore",
            "--root",
            root,
            "--mode",
            "server",
            "--request",
            p,
          ])
        ).stdout,
      ),
      raw = await client.request("tools/call", {
        name: "zvec_grep_explore",
        arguments: { root, request: item.request },
      });
    assert.equal(Boolean(raw.isError), false, JSON.stringify(raw));
    const mcp = JSON.parse(raw.content[0].text);
    await writeJson(join(base, `response-${item.name}.json`), {
      rustApi,
      direct,
      daemon,
      mcp,
      raw,
    });
    for (const v of [direct, daemon, mcp])
      assert.deepEqual(normalize(v), normalize(rustApi));
    item.check(direct);
    assert.ok(direct.evidence.every((e) => e.status === "verified"));
    results.push({ name: item.name, passed: true });
  }
} finally {
  await client.close();
  await cli(["--server", "off"]);
  await writeFile(join(base, "mcp-stderr.log"), client.stderr);
}
const contexts = [];
for (const c of [...spec.paths, ...spec.deferred_languages]) {
  const values = {};
  for (const mode of c.anchor ? ["off", "augment", "constrain"] : ["off"]) {
    const graph =
      mode === "off"
        ? { mode }
        : {
            mode,
            ...(mode === "augment"
              ? {
                  seeds: 3,
                  expansion: { relations: ["calls"], min_hops: 1, max_hops: 2 },
                }
              : {
                  scope: {
                    anchor: named(c.anchor),
                    traversal: {
                      relations: ["calls"],
                      min_hops: 2,
                      max_hops: 2,
                    },
                    region: "full",
                  },
                }),
          };
    const p = join(base, `context-${c.marker}-${mode}.json`);
    await writeJson(p, { query: c.marker, graph });
    values[mode] = JSON.parse((await rust(["context", root, p])).stdout);
    await writeJson(
      join(base, `context-response-${c.marker}-${mode}.json`),
      values[mode],
    );
    assert.ok(values[mode].items.length > 0);
  }
  if (c.anchor) {
    assert.deepEqual(values.off.items, values.augment.items);
    assert.ok(
      values.constrain.items.every((i) => i.content.includes(c.target)),
    );
  }
  contexts.push({ marker: c.marker, modes: Object.keys(values), passed: true });
}
const correction = structuredClone(
  cases.find((c) => c.name === "explicit-correction").input,
);
correction.sources = [
  { change: "delete", source: correction.sources[0].source },
];
correction.fact_changes = [];
await apply("withdraw", correction);
const p = join(base, "after-withdraw.json");
await writeJson(p, sem(false));
const final = JSON.parse((await rust(["explore", root, p])).stdout);
await writeJson(join(base, "withdraw-response.json"), final);
assert.deepEqual(facts(final), []);
const after = await diskInventory(storage);
assert.deepEqual(after, before);
const parserChecks = [];
for (const c of spec.unsupported_sources) {
  let expected = c.expected;
  if (parserSuite) {
    const matches = parserSuite.inputs.boundary_cases.filter((b) =>
      b.evidence.some((e) => e.sha256 === c.sha256),
    );
    assert.equal(matches.length, 1, "parser boundary source must match");
    assert.equal(matches[0].expected, c.expected);
    expected =
      parserSuite.protocol.parser_conformance?.[matches[0].case_id] ??
      c.expected;
  }
  const workspace = join(base, c.name);
  await mkdir(join(workspace, "cpp"), { recursive: true });
  await writeFile(join(workspace, c.path), c.text);
  const options = join(base, `${c.name}-graph.json`);
  await writeJson(options, {
    scope: "code",
    module_roots: [
      {
        root: "cpp",
        language: "cpp",
        namespace: "unsupported",
        module_prefix: [],
        layout: "flat",
      },
    ],
  });
  const result = await cli([
    "--index",
    workspace,
    "--mode",
    "direct",
    "--embedding",
    "local/potion-code-16m-v2",
    "--device",
    "cpu",
    "--model-cache",
    "/private/tmp/zg-next/model-cache",
    "--graph-options",
    options,
  ]);
  await writeJson(join(base, `${c.name}-index.json`), result);
  if (expected === "not_ready") assert.ok(result.stdout.includes("NotReady"));
  else assert.equal(expected, "ready_locate");
  const request = join(base, `${c.name}-request.json`);
  await writeJson(
    request,
    req({ operation: "locate", target: named("ReadScalarLiteral") }),
  );
  if (expected === "ready_locate") {
    const value = JSON.parse(
      (
        await cli([
          "--explore",
          "--root",
          workspace,
          "--mode",
          "direct",
          "--request",
          request,
        ])
      ).stdout,
    );
    assert.equal(value.execution.status, "complete");
    assert.equal(value.locations[0].status, "unique");
    assert.equal(value.locations[0].candidates.length, 1);
    assert.ok(value.evidence.every((e) => e.status === "verified"));
    await writeJson(join(base, `${c.name}-ready.json`), value);
    parserChecks.push({ name: c.name, expected, passed: true });
    continue;
  }
  let error;
  try {
    await cli([
      "--explore",
      "--root",
      workspace,
      "--mode",
      "direct",
      "--request",
      request,
    ]);
  } catch (e) {
    error = e.result;
  }
  assert.ok(error);
  assert.equal(error.code, 1);
  assert.ok(error.stderr.includes("code graph domain is not_ready"));
  await writeJson(join(base, `${c.name}-rejection.json`), error);
  parserChecks.push({ name: c.name, expected, passed: true });
}
await writeJson(join(base, "report.json"), {
  passed: true,
  scope: spec.scope,
  public_cases: results.length,
  public_calls: results.length * 4,
  contexts,
  parser_checks: parserChecks,
  results,
  semantic_withdrawal_never_restores_predecessor: true,
  text_and_vectors_unchanged: true,
  before,
  after,
});
console.log(
  `${results.length} cases across four entrypoints and ${contexts.length} retrieval cases passed.`,
);
