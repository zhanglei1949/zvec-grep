// Release-only arm control. No schema, argument or result normalization.
import http from "node:http";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";

export const ARM_TOOLS = {
  search: ["zvec_grep_search"],
  relationships: ["zvec_grep_callees", "zvec_grep_callers"],
  agent: ["zvec_grep_callees", "zvec_grep_callers", "zvec_grep_search"],
};
const hash = (text) => createHash("sha256").update(text).digest("hex");
const START = "<!-- ZVEC_GREP_START -->", END = "<!-- ZVEC_GREP_END -->";
// Published 0.3.1 search-only rules that do not spell out the tool name.
const SEARCH_ONLY = [
  "- Preserve the question's concepts, relationships, and constraints",
  "- `query` creates one primary hybrid FTS-plus-vector group",
  "- For a fused mixed search, use arguments such as",
  "- Search results include bounded source snippets by default.",
  "- If semantic retrieval remains irrelevant, fall back",
  "- Use returned search evidence without a status preflight.",
  "- When an index is missing, search can ask the user",
];

export function pruneGuidance(text, group) {
  const allowed = ARM_TOOLS[group];
  if (!allowed || typeof text !== "string") throw new Error("Invalid arm guidance");
  if (group === "agent") return text;
  const hidden = ARM_TOOLS.agent.filter((name) => !allowed.includes(name));
  // Each published rule is a single line; delete whole rules, never rewrite them.
  return text.split("\n").filter((line) => !hidden.some((name) => line.includes(name)) &&
    !(group === "relationships" && SEARCH_ONLY.some((prefix) => line.trimStart().startsWith(prefix)))).join("\n");
}

export function pruneManagedGuidance(text, group) {
  const start = text.indexOf(START), end = text.indexOf(END);
  if (start < 0 || end < start || text.indexOf(START, start + START.length) >= 0)
    throw new Error("Expected exactly one managed guidance block");
  return text.slice(0, start) + pruneGuidance(text.slice(start, end), group) + text.slice(end);
}

export function rpcMessages(text, contentType) {
  if (!contentType.includes("text/event-stream")) return text ? [JSON.parse(text)] : [];
  return text.split(/\r?\n\r?\n/).flatMap((block) => {
    const data = block.split(/\r?\n/).filter((line) => line.startsWith("data:")).map((line) => line.slice(5).trimStart()).join("\n");
    return data ? [JSON.parse(data)] : [];
  });
}

export async function startArmProxy(upstreamUrl, logPath, group) {
  if (!ARM_TOOLS[group]) throw new Error("Unknown experiment arm");
  const upstream = new URL(upstreamUrl);
  if (upstream.protocol !== "http:" || !["127.0.0.1", "localhost"].includes(upstream.hostname))
    throw new Error("MCP upstream must be loopback HTTP");
  const record = (value) => appendFileSync(logPath, JSON.stringify({ time: Date.now(), ...value }) + "\n");
  const pending = new Set();
  let sequence = 0;
  const server = http.createServer(async (request, response) => {
    const sequenceId = ++sequence, chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const bytes = Buffer.concat(chunks);
    let rpc;
    try { rpc = JSON.parse(bytes.toString()); } catch { /* MCP GET/DELETE. */ }
    record({ type: "request", sequence: sequenceId, method: request.method, body: bytes.toString() });
    if (rpc?.method === "tools/call" && !ARM_TOOLS[group].includes(rpc.params?.name)) {
      const body = JSON.stringify({ jsonrpc: "2.0", id: rpc.id, error: { code: -32601, message: "Tool unavailable in this experiment arm" } });
      record({ type: "rejected_call", sequence: sequenceId, name: rpc.params?.name });
      response.writeHead(200, { "content-type": "application/json" }).end(body);
      return;
    }
    const target = new URL(upstream);
    target.search = new URL(request.url, upstream).search;
    const headers = { ...request.headers, host: target.host, "accept-encoding": "identity" };
    delete headers.connection; delete headers["transfer-encoding"];
    if (request.method === "POST") headers["content-length"] = bytes.length;
    else delete headers["content-length"];
    const forwarded = http.request(target, { method: request.method, headers }, (reply) => {
      const replyHeaders = { ...reply.headers };
      delete replyHeaders["content-length"]; delete replyHeaders["transfer-encoding"]; delete replyHeaders.connection;
      if (request.method !== "POST") { response.writeHead(reply.statusCode, replyHeaders); reply.pipe(response); return; }
      const parts = [];
      reply.on("data", (part) => parts.push(part));
      reply.on("end", () => {
        const raw = Buffer.concat(parts);
        record({ type: "upstream_response", sequence: sequenceId, status: reply.statusCode,
          content_type: reply.headers["content-type"], body_base64: raw.toString("base64"), sha256: hash(raw) });
        let body = raw;
        try {
          const rewrite = (value) => {
            if (rpc?.method === "tools/list" && value.result?.tools)
              value.result.tools = value.result.tools.filter((tool) => ARM_TOOLS[group].includes(tool.name));
            if (["initialize", "server/discover"].includes(rpc?.method) && typeof value.result?.instructions === "string")
              value.result.instructions = pruneGuidance(value.result.instructions, group);
            return value;
          };
          if (["initialize", "server/discover", "tools/list"].includes(rpc?.method)) {
            const type = String(reply.headers["content-type"] || "");
            if (type.includes("text/event-stream")) {
              body = Buffer.from(raw.toString().split(/\r?\n\r?\n/).map((block) => {
                const values = rpcMessages(block + "\n\n", type);
                if (!values.length) return block;
                const other = block.split(/\r?\n/).filter((line) => !line.startsWith("data:"));
                return [...other, "data: " + JSON.stringify(rewrite(values[0]))].join("\n");
              }).join("\n\n"));
            } else if (raw.length) body = Buffer.from(JSON.stringify(rewrite(JSON.parse(raw))));
          }
          record({ type: "response", sequence: sequenceId, status: reply.statusCode,
            body_base64: body.toString("base64"), sha256: hash(body) });
          response.writeHead(reply.statusCode, replyHeaders).end(body);
        } catch (error) {
          record({ type: "protocol_error", sequence: sequenceId, error: error.message });
          response.writeHead(502).end("MCP arm proxy protocol error");
        }
      });
      reply.on("error", (error) => { record({ type: "transport_error", sequence: sequenceId, error: error.message }); response.destroy(error); });
    });
    pending.add(forwarded);
    forwarded.on("close", () => pending.delete(forwarded));
    forwarded.on("error", (error) => { record({ type: "transport_error", sequence: sequenceId, error: error.message }); if (!response.headersSent) response.writeHead(502); response.end(); });
    response.on("close", () => { if (!response.writableFinished) forwarded.destroy(); });
    forwarded.end(bytes);
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  record({ type: "started", group, upstream: upstream.href });
  return { url: `http://127.0.0.1:${server.address().port}/mcp`, async close() {
    await new Promise((resolve) => { server.close(resolve); server.closeAllConnections(); for (const request of pending) request.destroy(); });
    record({ type: "closed" });
  } };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [upstream, group, log, closed, guidanceFile] = process.argv.slice(2);
  const original = readFileSync(guidanceFile, "utf8");
  writeFileSync(guidanceFile + ".release-original", original);
  writeFileSync(guidanceFile, pruneManagedGuidance(original, group));
  const proxy = await startArmProxy(upstream, log, group);
  process.stdout.write(JSON.stringify({ url: proxy.url, group }) + "\n");
  process.once("SIGTERM", async () => { await proxy.close(); writeFileSync(closed, "closed\n"); });
}
