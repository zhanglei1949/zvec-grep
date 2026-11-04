// Benchmark-only JSON-RPC bridge. Both arms expose exactly the same search tool.
import { spawn } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";
import { performance } from "node:perf_hooks";

export const SEARCH = "zvec_grep_search";
const managed = ["graph", "graphExecution", "autoUpdate", "freshness", "apiKey"];

export function publicTool(tool) {
  const output = structuredClone(tool);
  for (const key of managed) delete output.inputSchema.properties[key];
  output.inputSchema.required = (output.inputSchema.required ?? []).filter(
    (key) => !managed.includes(key),
  );
  output.inputSchema.additionalProperties = false;
  return output;
}

export function forwardArguments(args, config) {
  if (!args || typeof args !== "object" || Array.isArray(args))
    throw new Error("search arguments must be an object");
  if (managed.some((key) => Object.hasOwn(args, key)))
    throw new Error("experiment-controlled search parameter");
  if (args.root !== "/app") throw new Error("search root must be /app");
  return {
    ...args, autoUpdate: false, freshness: "eventual",
    graph: config.profile === "zg-off" ? { mode: "off" } : config.graph,
    ...(config.profile === "zg-augment" ? { graphExecution: config.graph_execution } : {}),
  };
}

export function graphObservation(response) {
  if (response.error || response.result?.isError) return { status: "error", evidence: 0 };
  const text = (response.result?.content ?? [])
    .filter((part) => part.type === "text").map((part) => part.text).join("\n");
  const marker = "\nGraph\n```json\n";
  const start = text.lastIndexOf(marker);
  if (start < 0) return { status: "absent", evidence: 0 };
  if (!text.endsWith("\n```")) return { status: "invalid", evidence: 0 };
  try {
    const graph = JSON.parse(text.slice(start + marker.length, -4));
    if (!["complete", "partial", "degraded"].includes(graph.status))
      return { status: "invalid", evidence: 0 };
    return { status: graph.status, evidence: graph.evidence.length, view: graph.view };
  } catch {
    return { status: "invalid", evidence: 0 };
  }
}

export function bridge(config, probe = false) {
  if (!["zg-off", "zg-augment"].includes(config.profile)) throw new Error("invalid profile");
  const backend = spawn("zg", ["--server", "--stdio", "--mcp-toolset", "agent"], {
    stdio: ["pipe", "pipe", "inherit"],
  });
  const pending = new Map();
  const log = (event) => appendFileSync(config.log, `${JSON.stringify(event)}\n`);
  const send = (value) => process.stdout.write(`${JSON.stringify(value)}\n`);
  log({ event: "start", profile: config.profile, config });
  const input = createInterface({ input: process.stdin });
  const receive = (line) => {
    const message = JSON.parse(line);
    if (message.method === "tools/call") {
      try {
        if (message.params?.name !== SEARCH) throw new Error("tool not exposed");
        message.params.arguments = forwardArguments(message.params.arguments, config);
      } catch (error) {
        const response = { jsonrpc: "2.0", id: message.id,
          error: { code: -32602, message: error.message } };
        log({ event: "rejected", request: message, response });
        send(response);
        return;
      }
    }
    if (message.id !== undefined) pending.set(message.id, { request: message, start: performance.now() });
    if (message.method === "tools/call") log({ event: "request", request: message });
    backend.stdin.write(`${JSON.stringify(message)}\n`);
  };
  if (!probe) input.on("line", receive).on("close", () => backend.stdin.end());
  createInterface({ input: backend.stdout }).on("line", (line) => {
    const response = JSON.parse(line);
    const call = pending.get(response.id);
    if (call?.request.method === "initialize" && response.result)
      response.result.instructions = config.instructions;
    if (call?.request.method === "tools/list" && response.result) {
      const tool = response.result.tools.find((item) => item.name === SEARCH);
      if (!tool) throw new Error("backend has no search tool");
      response.result = { tools: [publicTool(tool)] };
      log({ event: "tools", tools: response.result.tools, instructions: config.instructions });
    }
    if (call?.request.method === "tools/call") log({
      event: "response", request: call.request, response,
      elapsed_ms: performance.now() - call.start, graph: graphObservation(response),
    });
    pending.delete(response.id);
    if (!probe) send(response);
    else if (response.id === 1) {
      if (response.error) throw new Error("MCP initialization failed");
      backend.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`);
      receive(JSON.stringify({ jsonrpc: "2.0", id: 2, method: "tools/list", params: {} }));
    } else if (response.id === 2) {
      receive(JSON.stringify({ jsonrpc: "2.0", id: 3, method: "tools/call",
        params: { name: SEARCH, arguments: { root: "/app", fts: ["def"], limit: 1 } } }));
    } else if (response.id === 3) {
      const observed = graphObservation(response);
      if (!["complete", "partial"].includes(observed.status) || !observed.view)
        throw new Error(`graph preflight failed: ${observed.status}`);
      send({ ready: true, graph: observed });
      backend.stdin.end();
      input.close();
    }
  });
  backend.on("error", (error) => {
    log({ event: "backend_error", message: error.message });
    process.exitCode = 1;
    process.stdin.destroy();
  });
  backend.on("exit", (code, signal) => {
    log({ event: "exit", code, signal, pending: [...pending.values()].map((x) => x.request) });
    process.exitCode = code ?? 1;
    process.stdin.destroy();
  });
  process.on("SIGTERM", () => backend.kill("SIGTERM"));
  process.on("SIGINT", () => backend.kill("SIGINT"));
  if (probe) {
    const watchdog = setTimeout(() => { backend.kill("SIGTERM"); process.exitCode = 1; }, 40000);
    backend.once("exit", () => clearTimeout(watchdog));
    receive(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: {
      protocolVersion: "2024-11-05", capabilities: {},
      clientInfo: { name: "zg-augment-preflight", version: "1" },
    } }));
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href)
  bridge(JSON.parse(readFileSync(process.argv[2], "utf8")), process.argv[3] === "--probe");
