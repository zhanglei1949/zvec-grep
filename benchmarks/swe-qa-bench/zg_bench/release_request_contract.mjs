// Assertions observe the OpenCode wire request; they never repair it.
export function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") return Object.fromEntries(Object.keys(value).sort().filter((key) => key !== "$schema").map((key) => [key, canonical(value[key])]));
  return value;
}
const stable = (value) => JSON.stringify(canonical(value));
const text = (value) => typeof value === "string" ? value : Array.isArray(value) ? value.filter((part) => part.type === "text").map((part) => part.text).join("\n") : "";
const normalized = (value) => value.split(/\r?\n/).map((line) => line.trim()).join("\n").trim();

export function verifyReleaseRequest(body, contract, first) {
  const names = body.tools.map((tool) => tool.function.name).filter((name) => name.startsWith("zvec_grep_")).sort();
  if (stable(names) !== stable([...contract.tool_names].sort())) throw new Error("Actual tool set differs from release arm");
  const system = body.messages.filter((message) => message.role === "system").map((message) => text(message.content)).join("\n");
  const messages = body.messages.map((message) => text(message.content)).join("\n");
  const block = /<server name="zvec_grep">([\s\S]*?)<\/server>/.exec(system)?.[1];
  if (!messages.includes(contract.guidance.trim()) || !block || normalized(block) !== normalized(contract.instructions))
    throw new Error("Installed release guidance or MCP instructions absent from actual request");
  for (const tool of body.tools.filter((tool) => names.includes(tool.function.name))) {
    if (stable(tool.function.parameters) !== stable(contract.schemas[tool.function.name]))
      throw new Error("Actual tool schema differs from published schema");
  }
  if (first && (body.messages.length !== 2 || body.messages[0].role !== "system" || body.messages[1].role !== "user"))
    throw new Error("First task request contains previous conversation history");
  if (body.tool_choice !== "auto" || body.model !== "glm-5.2" || body.temperature !== 0 || body.seed !== 42 ||
      body.max_tokens !== 32000 || body.enable_thinking !== true || body.reasoning_effort !== "high")
    throw new Error("Actual generation settings differ from frozen protocol");
}

export function repeatedToolError(messages, threshold = 3) {
  const calls = new Map();
  let previous = null, count = 0;
  for (const message of messages) {
    for (const call of message.tool_calls || []) calls.set(call.id, call.function);
    if (message.role !== "tool") continue;
    const call = calls.get(message.tool_call_id);
    const content = text(message.content);
    if (!call || !(call.name === "invalid" || /^\s*(error\b|invalid\b|json parse error|failed to (deserialize parameters|validate|execute)|validation error\b|the arguments provided to the tool are invalid|model tried to call unavailable tool)/i.test(content))) { previous = null; count = 0; continue; }
    let args = call.arguments;
    try { args = JSON.parse(args); } catch { /* Invalid arguments are evidence. */ }
    const fingerprint = stable({ tool: call.name, arguments: args, error: content });
    count = fingerprint === previous ? count + 1 : 1;
    previous = fingerprint;
    if (count >= threshold) return { count, fingerprint, tool: call.name, error: content };
  }
  return null;
}
