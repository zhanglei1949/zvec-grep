// Capture provider usage before OpenCode normalizes cache counters. Never log headers or content.
import http from "node:http";
import https from "node:https";
import { appendFileSync, openSync } from "node:fs";
import { fork } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";

export class UsageReader {
  constructor() { this.buffer = ""; this.usage = null; }
  line(line) {
    const value = line.startsWith("data:") ? line.slice(5).trim() : line.trim();
    if (!value || value === "[DONE]") return;
    // Only parse JSON events; SSE comments and keepalives are not usage records.
    if (!value.startsWith("{")) return;
    const event = JSON.parse(value);
    if (event.usage) this.usage = event.usage;
  }
  feed(text) {
    this.buffer += text;
    let newline;
    while ((newline = this.buffer.indexOf("\n")) >= 0) {
      this.line(this.buffer.slice(0, newline));
      this.buffer = this.buffer.slice(newline + 1);
    }
  }
  end() { if (this.buffer.trim()) this.line(this.buffer); return this.usage; }
}

export function relay({ upstream, log, port = 7998 }) {
  let sequence = 0;
  const record = (value) => appendFileSync(log, `${JSON.stringify(value)}\n`);
  const server = http.createServer((request, response) => {
    if (request.url === "/health" && request.method === "GET") {
      response.end("ready");
      return;
    }
    if (request.method !== "POST" || request.url !== "/chat/completions") {
      response.writeHead(404).end();
      return;
    }
    const id = ++sequence;
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => {
      const body = Buffer.concat(chunks);
      const parameters = JSON.parse(body);
      const model = parameters.model;
      if (parameters.stream === true)
        parameters.stream_options = { ...parameters.stream_options, include_usage: true };
      const forwarded = parameters.stream === true ? Buffer.from(JSON.stringify(parameters)) : body;
      record({ event: "request", id, model });
      const url = new URL(`${upstream.replace(/\/$/, "")}/chat/completions`);
      const transport = url.protocol === "https:" ? https : http;
      const reader = new UsageReader();
      const decoder = new TextDecoder();
      let parseError = false;
      const headers = { ...request.headers, host: url.host, "accept-encoding": "identity" };
      delete headers["transfer-encoding"];
      headers["content-length"] = String(forwarded.length);
      const upstreamRequest = transport.request(url, {
        method: "POST", headers,
      }, (source) => {
        response.writeHead(source.statusCode, source.headers);
        source.on("data", (chunk) => {
          response.write(chunk);
          try { reader.feed(decoder.decode(chunk, { stream: true })); }
          catch { parseError = true; }
        });
        source.on("end", () => {
          let usage = null;
          try { reader.feed(decoder.decode()); usage = reader.end(); }
          catch { parseError = true; }
          record({ event: "response", id, status: source.statusCode,
            usage: parseError ? null : usage, parse_error: parseError });
          response.end();
        });
        source.on("error", () => {
          record({ event: "error", id, reason: "upstream response interrupted" });
          response.destroy();
        });
      });
      upstreamRequest.on("error", () => {
        record({ event: "error", id, reason: "upstream request failed" });
        if (!response.headersSent) response.writeHead(502);
        response.end();
      });
      response.on("close", () => { if (!response.writableEnded) upstreamRequest.destroy(); });
      upstreamRequest.end(forwarded);
    });
  });
  server.listen(port, "127.0.0.1");
  return server;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (process.argv[2] === "--launch") {
    // Acknowledge listening before starting the Agent; do not race daemon startup.
    const child = fork(fileURLToPath(import.meta.url), process.argv.slice(3), {
      detached: true, execArgv: [],
      stdio: ["ignore", "ignore", openSync(`${process.argv[4]}.stderr`, "a"), "ipc"],
    });
    const timer = setTimeout(() => { child.kill(); process.exitCode = 1; }, 10000);
    child.once("error", () => { clearTimeout(timer); process.exitCode = 1; });
    child.once("exit", (code) => { clearTimeout(timer); if (code) process.exitCode = code; });
    child.once("message", (message) => {
      if (message !== "ready") throw new Error("invalid relay readiness message");
      clearTimeout(timer);
      child.disconnect();
      child.unref();
    });
  } else {
    const server = relay({ upstream: process.argv[2], log: process.argv[3] });
    server.once("listening", () => { if (process.send) process.send("ready"); });
  }
}
