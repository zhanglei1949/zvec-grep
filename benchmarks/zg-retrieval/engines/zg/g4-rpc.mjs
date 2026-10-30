import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

/** Sequential public JSON-RPC over the CLI-generated stdio command; no SDK or hidden API. */
export class PublicMcp {
  constructor(command, options) {
    this.child = spawn(command[0], command.slice(1), {
      ...options,
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.pending = new Map();
    this.sequence = 0;
    this.stderr = "";
    this.child.stderr.on("data", (b) => {
      this.stderr += b;
    });
    this.closed = new Promise((resolve) => {
      const fail = (e) => {
        for (const p of this.pending.values()) p.reject(e);
        this.pending.clear();
        resolve();
      };
      this.child.once("error", fail);
      this.child.once("close", (code, signal) =>
        fail(new Error(`MCP closed (${code}/${signal})`)),
      );
    });
    createInterface({ input: this.child.stdout }).on("line", (line) => {
      let message;
      try {
        message = JSON.parse(line);
      } catch {
        for (const p of this.pending.values())
          p.reject(new Error("non-JSON MCP stdout"));
        return;
      }
      if (Object.hasOwn(message, "method") && Object.hasOwn(message, "id")) {
        // Never grant unrequested remote access. Local code runs need no elicitation.
        this.send({
          jsonrpc: "2.0",
          id: message.id,
          ...(message.method === "elicitation/create"
            ? { result: { action: "cancel" } }
            : {
                error: { code: -32601, message: "Unsupported client method" },
              }),
        });
        return;
      }
      const pending = this.pending.get(message.id);
      if (pending) {
        this.pending.delete(message.id);
        message.error
          ? pending.reject(new Error(JSON.stringify(message.error)))
          : pending.resolve(message.result);
      }
    });
  }
  send(message) {
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }
  async request(method, params, timeout = 40_000) {
    const id = ++this.sequence;
    let timer;
    try {
      return await new Promise((resolve, reject) => {
        this.pending.set(id, { resolve, reject });
        timer = setTimeout(() => {
          this.pending.delete(id);
          this.send({
            jsonrpc: "2.0",
            method: "notifications/cancelled",
            params: { requestId: id, reason: "benchmark transport deadline" },
          });
          reject(new Error("MCP transport deadline"));
        }, timeout);
        this.send({ jsonrpc: "2.0", id, method, params });
      });
    } finally {
      clearTimeout(timer);
    }
  }
  async initialize() {
    const result = await this.request("initialize", {
      protocolVersion: "2025-11-25",
      capabilities: {},
      clientInfo: { name: "zg-retrieval-g4", version: "7" },
    });
    this.send({ jsonrpc: "2.0", method: "notifications/initialized" });
    return result;
  }
  async close() {
    this.child.stdin.end();
    const timer = setTimeout(() => this.child.kill("SIGKILL"), 10_000);
    try {
      await this.closed;
    } finally {
      clearTimeout(timer);
    }
  }
}
