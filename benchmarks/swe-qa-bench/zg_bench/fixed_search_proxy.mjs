// Benchmark-only MCP adapter. Both candidates receive the same search controls.
import http from "node:http";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";

export const SEARCH_POLICY = { preview: "short", limit: 20 };

export function constrainTool(tool) {
  if (tool.name !== "zvec_grep_search") return tool;
  const fixed = structuredClone(tool);
  const properties = fixed.inputSchema.properties;
  Object.assign(properties.preview, { enum: ["short"], default: "short", const: "short" });
  properties.preview.description += " Fixed by this benchmark to short.";
  Object.assign(properties.limit, { minimum: 20, maximum: 20, default: 20, const: 20 });
  properties.limit.description += " Fixed by this benchmark to 20.";
  fixed.description = fixed.description.replace(
    'set preview: "full" to return each complete stored entity without changing retrieval or ranking',
    'search uses preview: "short" and limit: 20 fixed by this benchmark',
  );
  return fixed;
}

function messages(text, contentType) {
  if (contentType.includes("text/event-stream")) {
    return text.split(/\r?\n/).filter((line) => line.startsWith("data:")).map((line) => {
      try { return JSON.parse(line.slice(5).trim()); } catch { return null; }
    }).filter(Boolean);
  }
  try { return [JSON.parse(text)]; } catch { return []; }
}

export async function startProxy(upstreamUrl, logPath, { instructions } = {}) {
  const upstream = new URL(upstreamUrl);
  if (upstream.protocol !== "http:" || !["127.0.0.1", "localhost"].includes(upstream.hostname)) {
    throw new Error("Fixed search proxy requires a loopback HTTP MCP upstream");
  }
  const record = (event) => appendFileSync(logPath, JSON.stringify({ time: Date.now(), ...event }) + "\n");
  const pending = new Set();
  let sequence = 0;
  const server = http.createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    let bytes = Buffer.concat(chunks);
    let rpc;
    try { rpc = JSON.parse(bytes.toString()); } catch { /* GET, DELETE and empty bodies. */ }
    let callSequence;
    if (rpc?.method === "tools/call") {
      callSequence = ++sequence;
      const requested = structuredClone(rpc.params.arguments || {});
      const effective = rpc.params.name === "zvec_grep_search" ? { ...requested, ...SEARCH_POLICY } : requested;
      record({ type: "call", sequence: callSequence, rpc_id: rpc.id, name: rpc.params.name,
        requested_arguments: requested, effective_arguments: effective,
        normalized_fields: Object.keys(SEARCH_POLICY).filter((key) => requested[key] !== effective[key]) });
      rpc.params.arguments = effective;
      bytes = Buffer.from(JSON.stringify(rpc));
    }
    const target = new URL(upstream);
    target.search = new URL(request.url, upstream).search;
    const headers = { ...request.headers, host: target.host, "accept-encoding": "identity" };
    delete headers.connection;
    delete headers["transfer-encoding"];
    if (request.method === "POST") headers["content-length"] = bytes.length;
    else delete headers["content-length"];
    const forwarded = http.request(target, { method: request.method, headers }, (reply) => {
      const replyHeaders = { ...reply.headers };
      delete replyHeaders["content-length"];
      delete replyHeaders["transfer-encoding"];
      delete replyHeaders.connection;
      if (request.method !== "POST") {
        response.writeHead(reply.statusCode, replyHeaders);
        reply.pipe(response);
        return;
      }
      const parts = [];
      reply.on("data", (part) => parts.push(part));
      reply.on("end", () => {
        let body = Buffer.concat(parts).toString();
        const contentType = String(reply.headers["content-type"] || "");
        const parsed = messages(body, contentType);
        if (rpc?.method === "tools/list" || (instructions !== undefined && ["initialize", "server/discover"].includes(rpc?.method))) {
          const rewrite = (value) => {
            if (value.result?.tools) value.result.tools = value.result.tools.map(constrainTool);
            if (instructions !== undefined && typeof value.result?.instructions === "string") {
              record({ type: "instructions_override", method: rpc.method,
                original_sha256: createHash("sha256").update(value.result.instructions).digest("hex"),
                effective_sha256: createHash("sha256").update(instructions).digest("hex") });
              value.result.instructions = instructions;
            }
            return value;
          };
          if (contentType.includes("text/event-stream")) {
            body = body.split(/\r?\n/).map((line) => {
              if (!line.startsWith("data:") || !line.slice(5).trim()) return line;
              try { return "data: " + JSON.stringify(rewrite(JSON.parse(line.slice(5)))); }
              catch { return line; }
            }).join("\n");
          } else if (parsed.length) body = JSON.stringify(rewrite(parsed[0]));
        }
        if (callSequence !== undefined) {
          const result = parsed.find((value) => value.id === rpc.id);
          record({ type: "result", sequence: callSequence, rpc_id: rpc.id,
            http_status: reply.statusCode, result: result?.result, error: result?.error,
            body_sha256: createHash("sha256").update(body).digest("hex") });
        }
        response.writeHead(reply.statusCode, replyHeaders);
        response.end(body);
      });
      reply.on("error", (error) => {
        record({ type: "response_error", sequence: callSequence, error: error.message });
        response.destroy(error);
      });
    });
    pending.add(forwarded);
    forwarded.on("close", () => pending.delete(forwarded));
    forwarded.on("error", (error) => {
      record({ type: "upstream_error", sequence: callSequence, error: error.message });
      if (!response.headersSent) response.writeHead(502);
      response.end("MCP upstream request failed");
    });
    response.on("close", () => { if (!response.writableFinished) forwarded.destroy(); });
    forwarded.end(bytes);
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  record({ type: "started", upstream: upstream.href, policy: SEARCH_POLICY });
  return {
    url: `http://127.0.0.1:${server.address().port}/mcp`,
    async close() {
      await new Promise((resolve) => {
        server.close(resolve);
        server.closeAllConnections();
        for (const request of pending) request.destroy();
      });
      record({ type: "closed" });
    },
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [upstream, log, closed, instructionsFile] = process.argv.slice(2);
  const proxy = await startProxy(upstream, log, instructionsFile ? { instructions: readFileSync(instructionsFile, "utf8") } : {});
  process.stdout.write(JSON.stringify({ url: proxy.url, policy: SEARCH_POLICY }) + "\n");
  process.once("SIGTERM", async () => { await proxy.close(); writeFileSync(closed, "closed\n"); });
}
