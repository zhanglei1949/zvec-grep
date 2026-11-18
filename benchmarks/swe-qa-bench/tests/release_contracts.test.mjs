import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ARM_TOOLS, pruneGuidance, pruneManagedGuidance, startArmProxy } from "../zg_bench/release_mcp_proxy.mjs";
import { repeatedToolError, verifyReleaseRequest } from "../zg_bench/release_request_contract.mjs";

test("arm pruning deletes whole rules and preserves the published combined text", () => {
  const text = "intro\n- use zvec_grep_search first\n- use zvec_grep_callers or zvec_grep_callees\n- read source\n";
  assert.equal(pruneGuidance(text, "agent"), text);
  assert.equal(pruneGuidance(text, "search"), "intro\n- use zvec_grep_search first\n- read source\n");
  assert.equal(pruneGuidance(text, "relationships"), "intro\n- use zvec_grep_callers or zvec_grep_callees\n- read source\n");
  const before = "user-owned\n<!-- ZVEC_GREP_START -->\n", after = "<!-- ZVEC_GREP_END -->\nkeep this\n";
  assert.equal(pruneManagedGuidance(before + text + after, "search"), before + pruneGuidance(text, "search") + after);
  assert.throws(() => pruneManagedGuidance("unmanaged guidance", "search"));
});

for (const group of Object.keys(ARM_TOOLS)) for (const sse of [false, true]) {
  test(`real HTTP proxy ${group} ${sse ? "SSE" : "JSON"}: exact schemas, arguments and call results`, async () => {
    const directory = mkdtempSync(join(tmpdir(), "zg-release-proxy-"));
    const received = [];
    const instructions = "core\n- zvec_grep_search\n- zvec_grep_callers zvec_grep_callees\n";
    const tools = ARM_TOOLS.agent.map((name) => ({ name, description: "published", inputSchema: { type: "object", properties: { globs: { anyOf: [{ $ref: "#/$defs/Glob" }, { type: "null" }] } }, $defs: { Glob: { anyOf: [{ type: "string" }, { type: "array", items: { type: "string" } }] } } } }));
    const callResult = { content: [{ type: "text", text: "save lib.rs:4\ncall at lib.rs:6:50\n" }], isError: false };
    const upstream = http.createServer(async (request, response) => {
      const chunks = []; for await (const chunk of request) chunks.push(chunk);
      const rpc = JSON.parse(Buffer.concat(chunks)); received.push(rpc);
      const result = rpc.method === "initialize" ? { instructions, protocolVersion: "2025-11-25" } : rpc.method === "tools/list" ? { tools } : callResult;
      const body = JSON.stringify({ jsonrpc: "2.0", id: rpc.id, result });
      response.writeHead(200, { "content-type": sse ? "text/event-stream" : "application/json", "mcp-session-id": "fixture-session" });
      response.end(sse ? "event: message\ndata: " + body + "\n\n" : body);
    });
    await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
    const log = join(directory, "calls.jsonl");
    const proxy = await startArmProxy(`http://127.0.0.1:${upstream.address().port}/mcp`, log, group);
    const invoke = async (method, params = {}) => {
      const reply = await fetch(proxy.url, { method: "POST", headers: { "content-type": "application/json", "mcp-session-id": "fixture-session" }, body: JSON.stringify({ jsonrpc: "2.0", id: 42, method, params }) });
      assert.equal(reply.headers.get("mcp-session-id"), "fixture-session");
      const body = await reply.text();
      return JSON.parse(sse ? body.split("\n").find((line) => line.startsWith("data:")).slice(5) : body);
    };
    try {
      assert.equal((await invoke("initialize")).result.instructions, pruneGuidance(instructions, group));
      assert.deepEqual((await invoke("tools/list")).result.tools, tools.filter((tool) => ARM_TOOLS[group].includes(tool.name)));
      for (const globs of ["lib.rs", ["*.rs", "!test.rs"], [{ pattern: "*.RS", caseInsensitive: true }], null]) {
        const params = { name: ARM_TOOLS[group][0], arguments: { root: "/app", symbol: "::save", globs, limit: 7, preview: "full" } };
        assert.deepEqual((await invoke("tools/call", params)).result, callResult);
        assert.deepEqual(received.at(-1).params, params);
      }
      const count = received.length;
      const hidden = ARM_TOOLS.agent.find((name) => !ARM_TOOLS[group].includes(name)) || "zvec_grep_status";
      const rejected = await fetch(proxy.url, { method: "POST", body: JSON.stringify({ jsonrpc: "2.0", id: 43, method: "tools/call", params: { name: hidden, arguments: {} } }) });
      assert.equal((await rejected.json()).error.code, -32601);
      assert.equal(received.length, count);
      const events = readFileSync(log, "utf8").trim().split("\n").map(JSON.parse);
      const raw = events.find((event) => event.type === "upstream_response");
      assert.ok(Buffer.from(raw.body_base64, "base64").toString().includes(instructions.split("\n")[0]));
    } finally {
      await proxy.close(); await new Promise((resolve) => upstream.close(resolve)); rmSync(directory, { recursive: true });
    }
  });
}

test("request contract requires both guidance surfaces and exact published schema", () => {
  const schema = { type: "object", properties: { globs: { anyOf: [{ type: "string" }, { type: "array" }] } } };
  const contract = { guidance: "AGENTS release guidance", instructions: "MCP release instructions", tool_names: ["zvec_grep_zvec_grep_callers"], schemas: { zvec_grep_zvec_grep_callers: schema } };
  const body = { model: "glm-5.2", temperature: 0, seed: 42, max_tokens: 32000, enable_thinking: true, reasoning_effort: "high", tool_choice: "auto",
    messages: [{ role: "system", content: 'AGENTS release guidance\n<server name="zvec_grep">MCP release instructions</server>' }, { role: "user", content: "question" }],
    tools: [{ function: { name: "zvec_grep_zvec_grep_callers", parameters: schema } }] };
  verifyReleaseRequest(body, contract, true);
  const qwen = structuredClone(body); qwen.model = "qwen3.8-max";
  assert.throws(() => verifyReleaseRequest(qwen, contract, true), /generation settings/);
  verifyReleaseRequest(qwen, { ...contract, model: "qwen3.8-max" }, true);
  const changedSampling = { ...qwen, temperature: 0.6 };
  assert.throws(() => verifyReleaseRequest(changedSampling, { ...contract, model: "qwen3.8-max" }, true), /generation settings/);
  const bad = structuredClone(body); bad.messages[0].content = "AGENTS release guidance";
  assert.throws(() => verifyReleaseRequest(bad, contract, true), /instructions absent/);
  const hidden = structuredClone(body); hidden.tools.push({ function: { name: "zvec_grep_zvec_grep_search", parameters: {} } });
  assert.throws(() => verifyReleaseRequest(hidden, contract, false), /tool set differs/);
  const changed = structuredClone(body); changed.tools[0].function.parameters.properties.globs = { type: "string" };
  assert.throws(() => verifyReleaseRequest(changed, contract, true), /schema differs/);
  const history = structuredClone(body); history.messages.push({ role: "assistant", content: "old" });
  assert.throws(() => verifyReleaseRequest(history, contract, true), /previous conversation/);
});

test("guard stops the third identical error, resets after progress, and retains distinct errors", () => {
  function failed(id, name = "invalid", error = "JSON Parse error", args = "{}") {
    return [{ role: "assistant", tool_calls: [{ id, function: { name, arguments: args } }] }, { role: "tool", tool_call_id: id, content: error }];
  }
  const first = [...failed("a"), ...failed("b")];
  assert.equal(repeatedToolError(first), null);
  assert.equal(repeatedToolError([...first, ...failed("c")]).count, 3);
  const progress = failed("success", "read", "source code");
  assert.equal(repeatedToolError([...first, ...progress, ...failed("c")]), null);
  assert.equal(repeatedToolError([...first, ...failed("c", "invalid", "different error")]), null);
  const argumentError = 'failed to deserialize parameters: unknown field `value`, expected one of `root`, `fts`, `globs`';
  const wrapped = '{"value":"{\\"fts\\": normalize, \\"globs\\": lib.rs}"}';
  const actualShape = [...failed("x", "zvec_grep_zvec_grep_search", argumentError, wrapped),
    ...failed("y", "zvec_grep_zvec_grep_search", argumentError, wrapped),
    ...failed("z", "zvec_grep_zvec_grep_search", argumentError, wrapped)];
  assert.equal(repeatedToolError(actualShape).count, 3);
  const wrappedErrors = actualShape.map((message) => message.role === "tool" ? { ...message, content: "The arguments provided to the tool are invalid: " + argumentError } : message);
  assert.equal(repeatedToolError(wrappedErrors).count, 3);
  assert.equal(repeatedToolError(wrappedErrors, 3, false), null); // Original GLM chain detector.
  assert.equal(repeatedToolError(actualShape, 3, false).count, 3);
});

for (const bounded of [false, true]) {
test(`recorder preserves raw SSE and enforces ${bounded ? "request budget" : "repeated-error stop"} before the provider`, async () => {
  const directory = mkdtempSync(join(tmpdir(), "zg-release-recorder-"));
  let forwarded = 0;
  const raw = 'data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"zvec_grep_zvec_grep_callees","arguments":"{\\\"globs\\\": lib.rs}"}}]}}]}\n\ndata: [DONE]\n\n';
  const upstream = http.createServer(async (request, response) => {
    for await (const chunk of request) { /* Consume without logging credentials. */ }
    forwarded++;
    response.writeHead(200, { "content-type": "text/event-stream" });
    response.write(raw.slice(0, 30)); response.end(raw.slice(30));
  });
  await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
  const schema = { type: "object", properties: { root: { type: "string" } } };
  const contract = { mode: "release", terminate_isolated_agent: false, guidance: "release guidance", instructions: "MCP rules",
    tool_names: ["zvec_grep_zvec_grep_callees"], schemas: { zvec_grep_zvec_grep_callees: schema }, ...(bounded ? { max_task_requests: 1 } : {}) };
  const contractFile = join(directory, "contract.json"), output = join(directory, "requests.jsonl"), closed = join(directory, "closed");
  writeFileSync(contractFile, JSON.stringify(contract));
  const child = spawn(process.execPath, [fileURLToPath(new URL("../zg_bench/model_request_recorder.mjs", import.meta.url)),
    `http://127.0.0.1:${upstream.address().port}/v1`, output, closed, contractFile]);
  let stderr = ""; child.stderr.on("data", (chunk) => { stderr += chunk; });
  const exit = new Promise((resolve) => child.once("exit", resolve));
  try {
    const ready = await new Promise((resolve, reject) => { child.stdout.once("data", (chunk) => resolve(JSON.parse(chunk))); child.once("error", reject); child.once("exit", () => reject(Error(stderr))); });
    const body = { model: "glm-5.2", temperature: 0, seed: 42, max_tokens: 32000, enable_thinking: true, reasoning_effort: "high", tool_choice: "auto",
      messages: [{ role: "system", content: 'release guidance\n<server name="zvec_grep">MCP rules</server>' }, { role: "user", content: "question" }],
      tools: [{ function: { name: "zvec_grep_zvec_grep_callees", parameters: schema } }] };
    const send = (value) => fetch(ready.baseURL + "/chat/completions", { method: "POST", headers: { "content-type": "application/json", authorization: "Bearer fake-secret-never-recorded" }, body: JSON.stringify(value) });
    assert.equal(await (await send(body)).text(), raw);
    const loop = structuredClone(body);
    for (let index = 0; !bounded && index < 3; index++) loop.messages.push(
      { role: "assistant", tool_calls: [{ id: String(index), function: { name: "zvec_grep_zvec_grep_callees", arguments: '{"value":"bad arguments"}' } }] },
      { role: "tool", tool_call_id: String(index), content: 'failed to deserialize parameters: unknown field `value`, expected one of `root`, `symbol`, `globs`' });
    assert.equal((await send(loop)).status, 400);
    assert.equal(forwarded, 1);
  } finally {
    child.kill("SIGTERM"); await exit;
    await new Promise((resolve) => upstream.close(resolve));
  }
  assert.equal(readFileSync(join(directory, "model-responses/1.raw"), "utf8"), raw);
  const recorded = readFileSync(output, "utf8");
  assert.ok(!recorded.includes("fake-secret-never-recorded"));
  assert.ok(recorded.includes('"type":"guard_stop"'));
  assert.ok(recorded.includes('"first_task_request":true'));
  assert.equal(JSON.parse(readFileSync(join(directory, "guard-stop.json"))).reason, bounded ? "request_budget_exhausted" : "repeated_tool_error");
  assert.equal(readFileSync(closed, "utf8"), "closed\n");
  rmSync(directory, { recursive: true });
});
}
