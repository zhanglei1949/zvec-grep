// Use the Node runtime already provisioned for OpenCode in every task image.
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

const [url, group, fixture] = process.argv.slice(2);
const headers = {
  "Content-Type": "application/json",
  Accept: "application/json, text/event-stream",
};
async function request(method, params, id) {
  const payload = { jsonrpc: "2.0", method, params };
  if (id !== undefined) payload.id = id;
  const response = await fetch(url, {
    method: "POST",
    headers,
    body: JSON.stringify(payload),
    signal: AbortSignal.timeout(60_000),
  });
  if (!response.ok) throw new Error(`MCP ${method}: HTTP ${response.status}`);
  const session = response.headers.get("Mcp-Session-Id");
  if (session) headers["Mcp-Session-Id"] = session;
  const text = await response.text();
  if (!text) return {};
  const value = response.headers
    .get("content-type")
    ?.includes("text/event-stream")
    ? text
        .split(/\r?\n/)
        .filter((line) => line.startsWith("data:") && line.slice(5).trim())
        .map((line) => JSON.parse(line.slice(5)))
        .find((message) => message.id === id)
    : JSON.parse(text);
  if (!value) throw new Error(`Missing MCP response for ${method}`);
  if (value.error) throw new Error(JSON.stringify(value.error));
  return value.result;
}

const initialized = await request(
  "initialize",
  {
    protocolVersion: "2025-03-26",
    capabilities: {},
    clientInfo: { name: "zg-bench-probe", version: "1" },
  },
  1,
);
headers["MCP-Protocol-Version"] = initialized.protocolVersion;
await request("notifications/initialized", {});
const { tools } = await request("tools/list", {}, 2);
const expected = {
  search: ["zvec_grep_search"],
  relationships: ["zvec_grep_callees", "zvec_grep_callers"],
  agent: ["zvec_grep_callees", "zvec_grep_callers", "zvec_grep_search"],
}[group];
if (
  !expected ||
  JSON.stringify(tools.map((tool) => tool.name).sort()) !==
    JSON.stringify(expected)
)
  throw new Error("MCP tool list differs from experiment arm");
const configDirectory = process.env.OPENCODE_CONFIG
  ? dirname(process.env.OPENCODE_CONFIG)
  : join(process.env.XDG_CONFIG_HOME || join(homedir(), ".config"), "opencode");
const guidance = await readFile(join(configDirectory, "AGENTS.md"), "utf8");
const surface = { instructions: initialized.instructions, tools, guidance };
surface.sha256 = createHash("sha256")
  .update(JSON.stringify(surface))
  .digest("hex");
if (fixture) {
  surface.preflight_calls = [];
  for (const [index, name] of expected.entries()) {
    const args = name.endsWith("search")
      ? { root: fixture, fts: "normalize" }
      : {
          root: fixture,
          symbol: name.endsWith("callers") ? "normalize" : "import_record",
        };
    const result = await request(
      "tools/call",
      { name, arguments: args },
      index + 3,
    );
    const content = JSON.stringify(result);
    if (
      result.isError ||
      (name.endsWith("search")
        ? !content.includes("normalize")
        : !content.includes("save") || !content.includes("call at"))
    )
      throw new Error(`Fixture query failed: ${content}`);
    surface.preflight_calls.push({ name, arguments: args, result });
  }
}
console.log(JSON.stringify(surface, null, 2));
