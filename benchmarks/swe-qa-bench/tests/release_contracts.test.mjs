import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
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
});
