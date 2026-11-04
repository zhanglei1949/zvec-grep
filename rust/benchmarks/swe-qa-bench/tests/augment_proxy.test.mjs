import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, writeFile, readFile, chmod, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import http from "node:http";
import { createInterface } from "node:readline";
import { publicTool, forwardArguments, graphObservation } from "../zg_bench/swe_qa/augment_proxy.mjs";
import { UsageReader, relay } from "../zg_bench/swe_qa/augment_usage.mjs";

const config = { profile: "zg-augment", graph: { mode: "augment", seeds: 5 },
  graph_execution: { request_timeout_ms: 30000 }, instructions: "same instructions" };

test("both profiles publish the same schema without managed arguments", () => {
  const tool = { name: "zvec_grep_search", description: "same description", inputSchema: {
    properties: { root: {}, graph: {}, graphExecution: {}, autoUpdate: {}, freshness: {}, apiKey: {} },
    required: ["root", "graph"],
  } };
  const exposed = publicTool(tool);
  assert.deepEqual(Object.keys(exposed.inputSchema.properties), ["root"]);
  assert.deepEqual(exposed.inputSchema.required, ["root"]);
  assert.equal(exposed.description, tool.description);
  assert.ok(tool.inputSchema.properties.graph);
  for (const profile of ["zg-off", "zg-augment"])
    assert.throws(() => forwardArguments({ root: "/app", graph: { mode: "off" } }, { ...config, profile }));
  const off = forwardArguments({ root: "/app", query: "question", limit: 7 }, { ...config, profile: "zg-off" });
  assert.deepEqual(off.graph, { mode: "off" });
  assert.equal(off.graphExecution, undefined);
  assert.equal(off.limit, 7);
  assert.equal(off.autoUpdate, false);
  assert.equal(off.freshness, "eventual");
});

test("graph observation preserves product error and partial distinctions", () => {
  assert.equal(graphObservation({ result: { isError: true } }).status, "error");
  const graph = { status: "partial", evidence: [1], view: {} };
  const response = { result: { content: [{ type: "text", text: `ordinary\nGraph\n\`\`\`json\n${JSON.stringify(graph)}\n\`\`\`` }] } };
  assert.deepEqual(graphObservation(response), { status: "partial", evidence: 1, view: {} });
  response.result.content[0].text += "truncated";
  assert.equal(graphObservation(response).status, "invalid");
});

test("provider usage parser handles split SSE without counting duplicate usage", () => {
  const reader = new UsageReader();
  reader.feed('data: {"choices":[{"delta":{"content":"answer"}}]}\n\ndata: {"us');
  reader.feed('age":{"prompt_tokens":40,"completion_tokens":5}}\n\n');
  reader.feed('data: {"usage":{"prompt_tokens":40,"completion_tokens":5}}\n\ndata: [DONE]\n');
  assert.deepEqual(reader.end(), { prompt_tokens: 40, completion_tokens: 5 });
});

test("usage relay preserves provider bytes and records no authorization or prompt", { timeout: 10000 }, async () => {
  const root = await mkdtemp(join(tmpdir(), "augment-usage-"));
  let upstream, proxy;
  try {
    const payload = 'data: {"usage":{"prompt_tokens":40,"completion_tokens":5}}\n\ndata: [DONE]\n';
    upstream = http.createServer((request, response) => {
      assert.equal(request.headers.authorization, "Bearer secret-test");
      assert.equal(request.url, "/v1/chat/completions");
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.end(payload);
    });
    upstream.listen(0, "127.0.0.1");
    await new Promise((resolve_) => upstream.once("listening", resolve_));
    const log = join(root, "usage.jsonl");
    proxy = relay({ upstream: `http://127.0.0.1:${upstream.address().port}/v1`, log, port: 0 });
    await new Promise((resolve_) => proxy.once("listening", resolve_));
    const response = await fetch(`http://127.0.0.1:${proxy.address().port}/chat/completions`, {
      method: "POST", headers: { authorization: "Bearer secret-test", "content-type": "application/json" },
      body: JSON.stringify({ model: "qwen3.7-max", messages: [{ role: "user", content: "private prompt" }], stream: true }),
    });
    assert.equal(await response.text(), payload);
    const text = await readFile(log, "utf8");
    assert.ok(!text.includes("secret-test"));
    assert.ok(!text.includes("private prompt"));
    const usage = JSON.parse(text.trim().split("\n")[1]);
    assert.equal(usage.usage.prompt_tokens, 40);
  } finally {
    for (const server of [proxy, upstream]) if (server) {
      server.closeAllConnections();
      await new Promise((resolve_) => server.close(resolve_));
    }
    await rm(root, { recursive: true });
  }
});

test("wire bridge forwards errors, content, cancellation and logs actual injection", { timeout: 10000 }, async () => {
  const root = await mkdtemp(join(tmpdir(), "augment-proxy-"));
  let child;
  try {
    const backend = join(root, "zg");
    await writeFile(backend, `#!${process.execPath}
import { createInterface } from "node:readline";
const send = (x) => process.stdout.write(JSON.stringify(x) + "\\n");
createInterface({ input: process.stdin }).on("line", (line) => {
 const m = JSON.parse(line);
 if (m.method === "notifications/cancelled") { send({jsonrpc:"2.0",method:"cancel_seen",params:m.params}); return; }
 if (m.method === "initialize") send({jsonrpc:"2.0",id:m.id,result:{instructions:"backend"}});
 if (m.method === "tools/list") send({jsonrpc:"2.0",id:m.id,result:{tools:[{name:"zvec_grep_search",description:"same",inputSchema:{properties:{root:{},query:{},graph:{},graphExecution:{}}}},{name:"zvec_grep_explore"}]}});
 if (m.method === "tools/call") {
  if (m.params.arguments.fts) {
   const fence = String.fromCharCode(96).repeat(3);
   const text = "\\nGraph\\n" + fence + "json\\n" + JSON.stringify({status:"complete",evidence:[],view:{id:"fixture"}}) + "\\n" + fence;
   send({jsonrpc:"2.0",id:m.id,result:{content:[{type:"text",text}]}});
  } else send({jsonrpc:"2.0",id:m.id,result:{isError:true,content:[{type:"text",text:"budget exceeded"}],structuredContent:{original:true}}});
 }
});
`);
    await chmod(backend, 0o755);
    const log = join(root, "events.jsonl");
    const path = join(root, "config.json");
    await writeFile(path, JSON.stringify({ ...config, log }));
    child = spawn(process.execPath, [resolve("zg_bench/swe_qa/augment_proxy.mjs"), path], {
      env: { ...process.env, PATH: `${root}:${process.env.PATH}` }, stdio: ["pipe", "pipe", "pipe"],
    });
    const received = [];
    const waiters = [];
    createInterface({ input: child.stdout }).on("line", (line) => {
      received.push(JSON.parse(line));
      waiters.splice(0).forEach((resolve_) => resolve_());
    });
    const exchange = async (message, match) => {
      child.stdin.write(JSON.stringify(message) + "\n");
      while (!received.some(match)) await new Promise((resolve_) => waiters.push(resolve_));
      return received.find(match);
    };
    const init = await exchange({ id: 1, method: "initialize" }, (m) => m.id === 1);
    assert.equal(init.result.instructions, config.instructions);
    const tools = await exchange({ id: 2, method: "tools/list" }, (m) => m.id === 2);
    assert.equal(tools.result.tools.length, 1);
    const result = await exchange({ id: 3, method: "tools/call", params: { name: "zvec_grep_search", arguments: { root: "/app", query: "question" } } }, (m) => m.id === 3);
    assert.deepEqual(result.result, { isError: true, content: [{ type: "text", text: "budget exceeded" }], structuredContent: { original: true } });
    await exchange({ method: "notifications/cancelled", params: { requestId: 3 } }, (m) => m.method === "cancel_seen");
    const denied = await exchange({ id: 4, method: "tools/call", params: { name: "zvec_grep_explore", arguments: {} } }, (m) => m.id === 4);
    assert.equal(denied.error.code, -32602);
    const events = (await readFile(log, "utf8")).trim().split("\n").map(JSON.parse);
    const request = events.find((event) => event.event === "request").request;
    assert.deepEqual(request.params.arguments.graph, config.graph);
    assert.equal(request.params.arguments.autoUpdate, false);
    assert.equal(events.find((event) => event.event === "response").graph.status, "error");
    const probe = spawn(process.execPath, [resolve("zg_bench/swe_qa/augment_proxy.mjs"), path, "--probe"], {
      env: { ...process.env, PATH: `${root}:${process.env.PATH}` }, stdio: ["pipe", "pipe", "pipe"],
    });
    probe.stdin.end();
    let output = "";
    probe.stdout.on("data", (chunk) => { output += chunk; });
    const exit = await new Promise((resolve_) => probe.once("close", resolve_));
    assert.equal(exit, 0);
    assert.equal(JSON.parse(output).ready, true);
  } finally {
    if (child) {
      child.stdin.end();
      child.kill("SIGTERM");
      await new Promise((resolve_) => child.once("close", resolve_));
    }
    await rm(root, { recursive: true });
  }
});
