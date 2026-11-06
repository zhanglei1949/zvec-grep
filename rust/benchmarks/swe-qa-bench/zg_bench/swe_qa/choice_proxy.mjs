// Autonomous arm: preserve graph choices, freeze index lifecycle, and audit the wire.
import { spawn } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";
import { performance } from "node:perf_hooks";
import { createHash } from "node:crypto";
import { graphObservation, SEARCH } from "./augment_proxy.mjs";

export const EXPLORE = "zvec_grep_explore";
const controlled = ["autoUpdate", "freshness", "apiKey"];
const sensitive = /^(api[_-]?key|authorization|password|secret|access[_-]?token|refresh[_-]?token)$/i;
export function redact(value) {
  if (Array.isArray(value)) return value.map(redact);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, sensitive.test(k) ? "[REDACTED]" : redact(v)]));
}

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(Object.keys(value).sort().map(k => [k, canonical(value[k])]));
}
export const contractHash = value => createHash("sha256").update(JSON.stringify(canonical(value))).digest("hex");

export function publicTool(tool) {
  const output = structuredClone(tool);
  for (const key of controlled) delete output.inputSchema.properties[key];
  output.inputSchema.required = (output.inputSchema.required ?? []).filter(k => !controlled.includes(k));
  if (output.inputSchema.examples) {
    output.inputSchema.examples = output.inputSchema.examples.filter(e => !controlled.some(k => Object.hasOwn(e, k)));
    if (!output.inputSchema.examples.length) delete output.inputSchema.examples;
  }
  // Retain definitions reachable from the published fields, including graph definitions.
  const defs = output.inputSchema.$defs;
  if (defs) {
    const pending = [], reachable = new Set();
    const collect = value => {
      if (!value || typeof value !== "object") return;
      if (typeof value.$ref === "string" && value.$ref.startsWith("#/$defs/"))
        pending.push(value.$ref.slice(8).split("/")[0].replaceAll("~1", "/").replaceAll("~0", "~"));
      for (const [key, child] of Object.entries(value)) if (key !== "$defs") collect(child);
    };
    collect(output.inputSchema);
    while (pending.length) {
      const name = pending.pop();
      if (reachable.has(name)) continue;
      if (!Object.hasOwn(defs, name)) throw new Error(`missing schema definition: ${name}`);
      reachable.add(name); collect(defs[name]);
    }
    for (const name of Object.keys(defs)) if (!reachable.has(name)) delete defs[name];
    if (!Object.keys(defs).length) delete output.inputSchema.$defs;
  }
  return output;
}

export function forwardArguments(name, args) {
  if (![SEARCH, EXPLORE].includes(name)) throw new Error("tool not exposed");
  if (!args || typeof args !== "object" || Array.isArray(args)) throw new Error("arguments must be an object");
  if (args.root !== "/app") throw new Error("root must be /app");
  if (controlled.some(k => Object.hasOwn(args, k))) throw new Error("experiment-controlled parameter");
  return name === SEARCH ? { ...args, autoUpdate: false, freshness: "eventual" } : structuredClone(args);
}

export function observation(name, response, operation) {
  if (response.error || response.result?.isError) return { status: "error", evidence: 0, error_code: response.error?.code ?? null };
  if (name === SEARCH) {
    const result = graphObservation(response);
    if (["absent", "invalid"].includes(result.status)) return result;
    const text = response.result.content.filter(p => p.type === "text").map(p => p.text).join("\n");
    const marker = "\nGraph\n```json\n";
    const value = JSON.parse(text.slice(text.lastIndexOf(marker) + marker.length, -4));
    if (!Array.isArray(value.evidence) || (value.status !== "degraded" && !value.view)
      || (value.execution === null && value.status !== "degraded")
      || (value.execution != null && (typeof value.execution !== "object" || Array.isArray(value.execution))))
      return { status: "invalid", evidence: 0 };
    return { ...result, stop_reason: value.execution?.stop_reason ?? value.reason ?? null,
      usage: value.execution?.usage ?? null, diagnostics: value.diagnostics ?? [], empty: value.evidence.length === 0 };
  }
  try {
    const value = response.result.structuredContent ?? JSON.parse(response.result.content.filter(p => p.type === "text").map(p => p.text).join("\n"));
    if (!["complete", "partial"].includes(value.execution?.status) || !value.view || !Array.isArray(value.evidence))
      return { status: "invalid", evidence: 0 };
    return { status: value.execution.status, evidence: value.evidence.length, view: value.view,
      stop_reason: value.execution.stop_reason ?? null, usage: value.execution.usage ?? null,
      next_cursor: value.next_cursor ?? null, locations: value.locations ?? [],
      empty: !["nodes", "relations", "paths", "references", "relation_types"].some(k => value[k]?.length) && !value.semantic
        && !(operation === "locate" && value.locations?.some(location => location.candidates?.length)) };
  } catch { return { status: "invalid", evidence: 0 }; }
}

export function bridge(config, probe = false) {
  if (config.profile !== "zg-agent-choice") throw new Error("invalid autonomous profile");
  const backend = spawn("zg", ["--server", "--stdio", "--mcp-toolset", "agent"], { stdio: ["pipe", "pipe", "inherit"] });
  const pending = new Map();
  let publishedInstructions = "";
  const secrets = ["DASHSCOPE_API_KEY", "ZVEC_GREP_API_KEY", "GLM_API_KEY", "OPENAI_API_KEY"].map(k => process.env[k]).filter(Boolean);
  const log = event => {
    const copy = structuredClone(event);
    for (const key of ["request", "original", "forwarded"])
      if (copy[key]?.params?.arguments) copy[key].params.arguments = redact(copy[key].params.arguments);
    let line = JSON.stringify(copy);
    for (const secret of secrets) line = line.replaceAll(JSON.stringify(secret).slice(1, -1), "[REDACTED]");
    appendFileSync(config.log, line + "\n");
  };
  const send = value => process.stdout.write(JSON.stringify(value) + "\n");
  log({ event: "start", config });
  const input = createInterface({ input: process.stdin });
  const receive = line => {
    const original = JSON.parse(line), message = structuredClone(original);
    if (message.method === "notifications/cancelled") log({ event: "cancel", request_id: message.params?.requestId });
    if (message.method === "tools/call") {
      try { message.params.arguments = forwardArguments(message.params.name, message.params.arguments); }
      catch (error) {
        const response = { jsonrpc: "2.0", id: message.id, error: { code: -32602, message: error.message } };
        log({ event: "rejected", original, response }); send(response); return;
      }
      log({ event: "request", original, forwarded: message });
    }
    if (message.id !== undefined) pending.set(message.id, { request: message, start: performance.now() });
    backend.stdin.write(JSON.stringify(message) + "\n");
  };
  if (!probe) input.on("line", receive).on("close", () => backend.stdin.end());
  createInterface({ input: backend.stdout }).on("line", line => {
    const response = JSON.parse(line), call = pending.get(response.id);
    if (call?.request.method === "initialize" && response.result) {
      const source = response.result.instructions ?? "";
      response.result.instructions = [source, config.instructions].filter(Boolean).join("\n");
      publishedInstructions = response.result.instructions;
      log({ event: "instructions", source, instructions: response.result.instructions });
    }
    if (call?.request.method === "tools/list" && response.result) {
      const source = response.result.tools;
      const tools = source.filter(t => [SEARCH, EXPLORE].includes(t.name)).map(publicTool);
      if (tools.length !== 2) throw new Error("candidate must expose search and explore");
      response.result = { ...response.result, tools };
      log({ event: "tools", source_tools: source, tools, instructions: publishedInstructions });
      if (probe) {
        send({ ready: true, tool_names: tools.map(t => t.name), contract_hash: contractHash({ tools, instructions: publishedInstructions }) });
        backend.stdin.end(); input.close();
      }
    }
    if (call?.request.method === "tools/call") log({ event: "response", request: call.request, response,
      elapsed_ms: performance.now() - call.start, graph: observation(call.request.params.name, response,
        call.request.params.arguments.request?.operation?.operation) });
    pending.delete(response.id);
    if (!probe) send(response);
    else if (response.id === 1) {
      if (response.error) throw new Error("MCP initialization failed");
      backend.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");
      receive(JSON.stringify({ jsonrpc: "2.0", id: 2, method: "tools/list", params: {} }));
    }
  });
  backend.on("error", error => { log({ event: "backend_error", message: error.message }); process.exitCode = 1; process.stdin.destroy(); });
  backend.on("exit", (code, signal) => {
    log({ event: "exit", code, signal, pending_ids: [...pending.keys()] });
    process.exitCode = code ?? 1; process.stdin.destroy();
  });
  process.on("SIGTERM", () => backend.kill("SIGTERM"));
  process.on("SIGINT", () => backend.kill("SIGINT"));
  if (probe) {
    const timer = setTimeout(() => backend.kill("SIGTERM"), 40000);
    backend.once("exit", () => clearTimeout(timer));
    receive(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: {
      protocolVersion: "2024-11-05", capabilities: {}, clientInfo: { name: "zg-choice-catalog", version: "1" },
    } }));
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href)
  bridge(JSON.parse(readFileSync(process.argv[2], "utf8")), process.argv[3] === "--probe");
