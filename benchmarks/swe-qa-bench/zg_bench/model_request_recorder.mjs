// Trial-local OpenAI-compatible transport recorder. Headers are never persisted.
import http from "node:http";
import https from "node:https";
import { appendFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";

const [upstreamBase, output, closed] = process.argv.slice(2);
const upstream = new URL(upstreamBase);
if (!["http:", "https:"].includes(upstream.protocol) || upstream.username || upstream.password || upstream.search || upstream.hash)
  throw new Error("Upstream must be an HTTP(S) base URL without credentials, query or fragment");
const record = (event) => appendFileSync(output, JSON.stringify({ time: Date.now(), ...event }) + "\n");
let sequence = 0;
const pending = new Set();
const server = http.createServer(async (request, response) => {
  const id = ++sequence;
  if (request.method !== "POST" || request.url !== "/v1/chat/completions") {
    record({ type: "rejected", id, method: request.method, path: request.url });
    response.writeHead(400).end("Expected POST /v1/chat/completions");
    return;
  }
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  const bytes = Buffer.concat(chunks);
  let body;
  try {
    body = JSON.parse(bytes.toString("utf8"));
  } catch (error) {
    record({ type: "invalid_json", id, error: error.message });
    response.writeHead(400).end("Invalid JSON request");
    return;
  }
  record({ type: "request", id, path: request.url, body_sha256: createHash("sha256").update(bytes).digest("hex"), body });
  const target = new URL(upstream.href.replace(/\/$/, "") + "/chat/completions");
  const headers = { ...request.headers, host: target.host };
  delete headers.connection;
  const transport = target.protocol === "https:" ? https : http;
  const forwarded = transport.request(target, { method: "POST", headers }, (reply) => {
    record({ type: "response", id, status: reply.statusCode });
    response.writeHead(reply.statusCode, reply.headers);
    reply.pipe(response);
    reply.on("end", () => record({ type: "response_end", id }));
    reply.on("error", (error) => {
      record({ type: "response_error", id, error: error.message });
      response.destroy(error);
    });
  });
  pending.add(forwarded);
  forwarded.on("close", () => pending.delete(forwarded));
  forwarded.on("error", (error) => {
    record({ type: "upstream_error", id, error: error.message });
    if (!response.headersSent) response.writeHead(502);
    response.end("Upstream request failed");
  });
  response.on("close", () => {
    if (!response.writableFinished) forwarded.destroy(new Error("Client disconnected"));
  });
  forwarded.end(bytes);
});
server.listen(0, "127.0.0.1", () => {
  record({ type: "started", upstream: upstream.href });
  process.stdout.write(JSON.stringify({ baseURL: `http://127.0.0.1:${server.address().port}/v1`, upstream: upstream.href }) + "\n");
});
let stopping = false;
process.on("exit", (code) => {
  if (stopping && code === 0) {
    record({ type: "closed" });
    writeFileSync(closed, "closed\n");
  }
});
process.on("SIGTERM", () => {
  stopping = true;
  server.close();
  server.closeAllConnections();
  for (const request of pending) request.destroy(new Error("Recorder shutdown"));
});
