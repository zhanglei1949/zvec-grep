import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, writeFile, readFile, chmod, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { forwardArguments, publicTool, observation, contractHash, redact } from "../zg_bench/swe_qa/choice_proxy.mjs";
const SEARCH = "zvec_grep_search", EXPLORE = "zvec_grep_explore";

test("four query choices, budgets and empty boundaries survive unchanged", () => {
  for (const graph of [undefined, { mode: "off" }, { mode: "augment", seeds: 2, expansion: { direction: "incoming", max_hops: 1, boundary: { sources: [] } } }, { mode: "constrain", scope: { anchor: { by: "name", name: "f" } } }]) {
    const args = { root: "/app", query: "401", ...(graph ? { graph } : {}), ...(graph?.mode === "augment" ? { graphExecution: { budget: { nodes: 50 } } } : {}) };
    const copy = structuredClone(args), output = forwardArguments(SEARCH, args);
    assert.deepEqual(output, { ...args, autoUpdate: false, freshness: "eventual" });
    assert.deepEqual(args, copy);
    assert.equal(Object.hasOwn(output, "graph"), graph !== undefined);
  }
  const explore = { root: "/app", request: { domain: "code", cursor: "opaque", operation: { operation: "neighbors" } }, execution: { budget: { nodes: 50 } } };
  assert.deepEqual(forwardArguments(EXPLORE, explore), explore);
  // Product validation handles invalid graph input; the proxy does not repair it.
  assert.deepEqual(forwardArguments(SEARCH, { root: "/app", graph: { mode: "bad" } }).graph, { mode: "bad" });
  for (const args of [{ root: "/other" }, { root: "/app", autoUpdate: true }, { root: "/app", apiKey: "secret" }])
    assert.throws(() => forwardArguments(SEARCH, args));
  assert.throws(() => forwardArguments("zvec_grep_index", { root: "/app" }));
});

test("real catalog retains product graph examples and descriptions", async () => {
  const catalog = JSON.parse(await readFile(new URL("./fixtures/mcp-agent-tools-graphdb.json", import.meta.url), "utf8"));
  const before = structuredClone(catalog);
  for (const source of catalog.tools) {
    const tool = publicTool(source);
    assert.equal(tool.description, source.description);
    assert.deepEqual(tool.annotations, source.annotations);
    assert.deepEqual(tool.inputSchema.examples, source.inputSchema.examples);
    if (tool.name === SEARCH) {
      assert.ok(tool.inputSchema.properties.graph);
      assert.ok(tool.inputSchema.properties.graphExecution);
      assert.equal(tool.inputSchema.$defs.GraphQueryMode.oneOf.length, 3);
    }
    for (const key of ["autoUpdate", "freshness", "apiKey"]) assert.equal(Object.hasOwn(tool.inputSchema.properties, key), false);
  }
  assert.deepEqual(catalog, before);
});

test("Explore prefers structured content and preserves partial, ambiguity and continuation", () => {
  const value = { view: { generation: "g" }, evidence: [1], locations: [{ status: "ambiguous" }], nodes: [],
    execution: { status: "partial", stop_reason: "graph_budget", usage: { nodes: 1 } }, next_cursor: "next" };
  const reply = { result: { structuredContent: value, content: [{ type: "text", text: "not another result" }] } };
  const result = observation(EXPLORE, reply);
  assert.equal(result.evidence, 1);
  assert.equal(result.status, "partial");
  assert.equal(result.next_cursor, "next");
  assert.equal(result.locations[0].status, "ambiguous");
  delete reply.result.structuredContent;
  reply.result.content[0].text = JSON.stringify(value);
  assert.deepEqual(observation(EXPLORE, reply), result);
  reply.result.content[0].text = "bad JSON";
  assert.equal(observation(EXPLORE, reply).status, "invalid");
  assert.equal(observation(EXPLORE, { error: { code: -32602 } }).status, "error");
  assert.equal(observation(SEARCH, { result: { content: [{ type: "text", text: "ordinary" }] } }).status, "absent");
});

test("product degradation permits null view and execution and keeps its cause", () => {
  const value = { status: "degraded", view: null, seeds: [], skipped: [], expansions: [],
    nodes: [], relations: [], evidence: [], diagnostics: ["graph deadline exceeded"], execution: null };
  const reply = v => ({ result: { content: [{ type: "text", text: "ordinary\nGraph\n```json\n" + JSON.stringify(v) + "\n```" }] } });
  const observed = observation(SEARCH, reply(value));
  assert.equal(observed.status, "degraded");
  assert.equal(observed.view, null);
  assert.deepEqual(observed.diagnostics, value.diagnostics);
  assert.equal(observed.usage, null);
  for (const status of ["complete", "partial"]) {
    assert.equal(observation(SEARCH, reply({ ...value, status })).status, "invalid");
    assert.equal(observation(EXPLORE, { result: { structuredContent: { ...value, execution: { status } } } }).status, "invalid");
  }
});

test("Locate candidates count as results while an anchor alone does not fill neighbors", () => {
  for (const [status, candidates, empty] of [["unique", [{ id: "n" }], false], ["ambiguous", [{ id: "n" }, { id: "m" }], false], ["not_found", [], true]]) {
    const value = { view: { generation: "g" }, nodes: [], relations: [], paths: [], references: [], evidence: [],
      locations: [{ status, candidates }], execution: { status: "complete", stop_reason: null, usage: {} } };
    const reply = { result: { structuredContent: value, content: [{ type: "text", text: "stale text" }] } };
    assert.equal(observation(EXPLORE, reply, "locate").empty, empty);
    assert.equal(observation(EXPLORE, { result: { content: [{ type: "text", text: JSON.stringify(value) }] } }, "locate").empty, empty);
    assert.equal(observation(EXPLORE, reply, "neighbors").empty, true);
  }
});

test("contract hashing is order independent and credentials are redacted", () => {
  assert.equal(contractHash({ a: 1, b: 2 }), contractHash({ b: 2, a: 1 }));
  assert.deepEqual(redact({ apiKey: "secret", nested: { authorization: "Bearer secret" }, cursor: "opaque" }),
    { apiKey: "[REDACTED]", nested: { authorization: "[REDACTED]" }, cursor: "opaque" });
});

test("wire audit uses real catalog, preserves choices, pages, errors and cancellation", { timeout: 15000 }, async () => {
  const root = await mkdtemp(join(tmpdir(), "choice-wire-"));
  const fixture = resolve("tests/fixtures/mcp-agent-tools-graphdb.json");
  const backend = join(root, "zg"), log = join(root, "wire.jsonl");
  let child;
  try {
    await writeFile(backend, `#!${process.execPath}
import {readFileSync} from 'node:fs';import {createInterface} from 'node:readline';
const c=JSON.parse(readFileSync(process.env.CHOICE_TEST_CATALOG,'utf8'));
const send=x=>process.stdout.write(JSON.stringify(x)+'\\n');let wait;
createInterface({input:process.stdin}).on('line',line=>{
 const m=JSON.parse(line);
 if(m.method==='initialize') send({id:m.id,result:{instructions:c.instructions}});
 if(m.method==='tools/list') send({id:m.id,result:{tools:c.tools}});
 if(m.method==='notifications/cancelled') send({id:wait,error:{code:-32800,message:'cancelled'}});
 if(m.method==='tools/call') {
  const a=m.params.arguments;
  if(a.query==='wait'){wait=m.id;return;}
  if(a.query==='error'){send({id:m.id,error:{code:-32602,message:process.env.DASHSCOPE_API_KEY}});return;}
  if(m.params.name==='zvec_grep_explore'){
   const v={view:{generation:'g'},nodes:[],relations:[],paths:[],references:[],evidence:[1],locations:[],execution:{status:a.request.cursor?'complete':'partial',stop_reason:a.request.cursor?null:'graph_budget',usage:{nodes:1}}};
   if(a.request.operation.operation==='locate'){v.evidence=[];v.locations=[{status:'unique',candidates:[{id:'n'}]}];v.execution.status='complete';}
   if(!a.request.cursor)v.next_cursor='page-2';
   send({id:m.id,result:{structuredContent:v,content:[{type:'text',text:JSON.stringify(v)}]}});
  }else send({id:m.id,result:{content:[{type:'text',text:JSON.stringify(a)}]}});
 }
});
`);
    await chmod(backend, 0o755);
    const config = join(root, "config.json");
    await writeFile(config, JSON.stringify({ profile: "zg-agent-choice", instructions: "readonly fixture", log }));
    child = spawn(process.execPath, [resolve("zg_bench/swe_qa/choice_proxy.mjs"), config], {
      env: { ...process.env, PATH: `${root}:${process.env.PATH}`, CHOICE_TEST_CATALOG: fixture, DASHSCOPE_API_KEY: "fixture-secret-123" },
      stdio: ["pipe", "pipe", "pipe"],
    });
    const received = [], waiters = [];
    createInterface({ input: child.stdout }).on("line", line => { received.push(JSON.parse(line)); waiters.splice(0).forEach(f => f()); });
    const exchange = async (message, id = message.id) => {
      child.stdin.write(JSON.stringify(message) + "\n");
      while (!received.some(r => r.id === id)) await new Promise(resolve => waiters.push(resolve));
      return received.find(r => r.id === id);
    };
    const init = await exchange({ id: 1, method: "initialize" });
    assert.ok(init.result.instructions.endsWith("readonly fixture"));
    const source = JSON.parse(await readFile(fixture, "utf8"));
    assert.ok(init.result.instructions.startsWith(source.instructions));
    const tools = await exchange({ id: 2, method: "tools/list" });
    assert.equal(tools.result.tools.length, 2);
    for (const [index, graph] of [undefined, { mode: "off" }, { mode: "augment", seeds: 1 }, { mode: "constrain", scope: { anchor: { by: "name", name: "f" } } }].entries()) {
      const args = { root: "/app", query: "implementation", ...(graph ? { graph } : {}) };
      const result = await exchange({ id: index + 3, method: "tools/call", params: { name: SEARCH, arguments: args } });
      assert.deepEqual(JSON.parse(result.result.content[0].text), { ...args, autoUpdate: false, freshness: "eventual" });
    }
    const args = { root: "/app", request: { domain: "code", operation: { operation: "neighbors" } }, execution: { budget: { nodes: 1 } } };
    const first = await exchange({ id: 8, method: "tools/call", params: { name: EXPLORE, arguments: args } });
    const cursor = first.result.structuredContent.next_cursor;
    const second = await exchange({ id: 9, method: "tools/call", params: { name: EXPLORE, arguments: { ...args, request: { ...args.request, cursor } } } });
    assert.equal(second.result.structuredContent.execution.status, "complete");
    const error = await exchange({ id: 10, method: "tools/call", params: { name: SEARCH, arguments: { root: "/app", query: "error" } } });
    assert.equal(error.error.message, "fixture-secret-123");
    const rejected = await exchange({ id: 11, method: "tools/call", params: { name: SEARCH, arguments: { root: "/app", apiKey: "never-log-this" } } });
    assert.equal(rejected.error.code, -32602);
    child.stdin.write(JSON.stringify({ id: 12, method: "tools/call", params: { name: SEARCH, arguments: { root: "/app", query: "wait" } } }) + "\n");
    const cancelled = await exchange({ method: "notifications/cancelled", params: { requestId: 12 } }, 12);
    assert.equal(cancelled.error.code, -32800);
    await exchange({ id: 13, method: "tools/call", params: { name: EXPLORE, arguments: {
      root: "/app", request: { domain: "code", operation: { operation: "locate", target: { by: "name", name: "f" } } },
    } } });
    child.stdin.end();
    assert.equal(await new Promise(resolve => child.once("close", resolve)), 0);
    const text = await readFile(log, "utf8");
    assert.ok(!text.includes("never-log-this"));
    assert.ok(!text.includes("fixture-secret-123"));
    const events = text.trim().split("\n").map(JSON.parse);
    const forwardedExplore = events.find(e => e.event === "request" && e.forwarded.id === 8);
    assert.deepEqual(forwardedExplore.original.params.arguments, forwardedExplore.forwarded.params.arguments);
    assert.equal(events.find(e => e.event === "response" && e.request.id === 8).graph.evidence, 1);
    assert.equal(events.filter(e => e.event === "response").length, 9);
    assert.equal(events.find(e => e.event === "response" && e.request.id === 13).graph.empty, false);
    assert.ok(events.some(e => e.event === "cancel" && e.request_id === 12));
    // Catalog-only preflight does not issue retrieval calls.
    const probeLog = join(root, "probe.jsonl");
    await writeFile(config, JSON.stringify({ profile: "zg-agent-choice", instructions: "readonly fixture", log: probeLog }));
    const probe = spawn(process.execPath, [resolve("zg_bench/swe_qa/choice_proxy.mjs"), config, "--probe"], {
      env: { ...process.env, PATH: `${root}:${process.env.PATH}`, CHOICE_TEST_CATALOG: fixture }, stdio: ["pipe", "pipe", "pipe"],
    });
    let output = ""; probe.stdout.on("data", c => { output += c; }); probe.stdin.end();
    assert.equal(await new Promise(resolve => probe.once("close", resolve)), 0);
    assert.equal(JSON.parse(output).ready, true);
    assert.ok(!(await readFile(probeLog, "utf8")).includes('"event":"request"'));
  } finally {
    if (child && child.exitCode === null) child.kill("SIGTERM");
    await rm(root, { recursive: true });
  }
});
