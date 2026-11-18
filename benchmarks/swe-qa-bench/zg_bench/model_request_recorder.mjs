// Trial-local OpenAI-compatible transport recorder. Headers are never persisted.
import http from "node:http";
import https from "node:https";
import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { execFile } from "node:child_process";

const [upstreamBase, output, closed, contractFile] = process.argv.slice(2);
const contract = contractFile ? JSON.parse(readFileSync(contractFile, "utf8")) : null;
const { repeatedToolError, verifyReleaseRequest } = contract?.mode === "release" ? await import("./release_request_contract.mjs") : {};
const responsesDirectory = join(dirname(output), "model-responses");
if (contract) mkdirSync(responsesDirectory, { recursive: true });
let firstTaskChecked = false;
let releaseStopped = false;
let taskRequestsForwarded = 0;
const normalize = (text) => text.split(/\r?\n/).map((line) => line.trim()).join("\n").trim();
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
  if (contract?.mode === "release") {
    const repeated = repeatedToolError(body.messages, 3, contract.error_guard_version !== 1);
    const budgetExceeded = body.tools?.length && Number.isSafeInteger(contract.max_task_requests) && taskRequestsForwarded >= contract.max_task_requests;
    if (releaseStopped || repeated || budgetExceeded) {
      releaseStopped = true;
      const stop = { stage: "agent", reason: budgetExceeded ? "request_budget_exhausted" : "repeated_tool_error", request_id: id, repeated };
      record({ type: "guard_stop", id, ...stop });
      writeFileSync(join(dirname(output), "guard-stop.json"), JSON.stringify(stop));
      response.writeHead(400, { "content-type": "application/json" }).end(JSON.stringify({ error: { message: "Repeated tool errors; trial stopped by benchmark protocol", type: "benchmark_guard" } }));
      // Enabled only inside a single-trial Harbor container, never local tests.
      if (contract.terminate_isolated_agent === true) execFile("pkill", ["-TERM", "-f", "[o]pencode.*run"], () => {});
      return;
    }
  }
  if (contract && body.tools?.length) {
    try {
      if (contract.mode === "release") {
        verifyReleaseRequest(body, contract, !firstTaskChecked);
      } else {
      const system = body.messages.filter((m) => m.role === "system").map((m) => m.content).join("\n");
      const block = /<server name="zvec_grep">([\s\S]*?)<\/server>/.exec(system)?.[1];
      if (!system.includes(contract.guidance.trim()) || !block || normalize(block) !== normalize(contract.instructions))
        throw new Error("Installed AGENTS.md or MCP instructions absent from actual system message");
      const names = body.tools.map((t) => t.function.name);
      if (!contract.tool_names.every((name) => names.includes(name))) throw new Error("Relationship tool absent");
      if (!firstTaskChecked && (body.messages.length !== 2 || body.messages[0].role !== "system" || body.messages[1].role !== "user"))
        throw new Error("First task request contains previous conversation history");
      if (body.tool_choice !== "auto") throw new Error("Expected voluntary API tool_choice=auto");
      }
      record({ type: "guidance_verified", id, first_task_request: !firstTaskChecked,
        guidance_sha256: createHash("sha256").update(contract.guidance).digest("hex"),
        instructions_sha256: createHash("sha256").update(contract.instructions).digest("hex"),
        message_count: body.messages.length });
      firstTaskChecked = true;
    } catch (error) {
      record({ type: "guidance_rejected", id, error: error.message });
      response.writeHead(400).end(error.message);
      return;
    }
  }
  const target = new URL(upstream.href.replace(/\/$/, "") + "/chat/completions");
  if (body.tools?.length) taskRequestsForwarded++;
  const headers = { ...request.headers, host: target.host };
  delete headers.connection;
  const transport = target.protocol === "https:" ? https : http;
  const forwarded = transport.request(target, { method: "POST", headers }, (reply) => {
    record({ type: "response", id, status: reply.statusCode, ...(contract ? { content_type: reply.headers["content-type"], content_encoding: reply.headers["content-encoding"] } : {}) });
    const responseChunks = [];
    if (contract) {
      reply.on("data", (chunk) => responseChunks.push(chunk));
      reply.on("close", () => writeFileSync(join(responsesDirectory, `${id}.raw`), Buffer.concat(responseChunks)));
    }
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
