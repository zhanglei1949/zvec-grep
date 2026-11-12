"""Capture the live MCP surface using only the Python standard library."""
import hashlib
import json
import os
from pathlib import Path
from urllib.request import Request, urlopen


def probe(url: str, group: str, fixture: str | None = None) -> dict:
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}

    def request(method: str, params: dict, identifier: int | None) -> dict:
        payload = {"jsonrpc": "2.0", "method": method, "params": params}
        if identifier is not None:
            payload["id"] = identifier
        with urlopen(Request(url, data=json.dumps(payload).encode(), headers=headers), timeout=60) as response:
            session = response.headers.get("Mcp-Session-Id")
            if session:
                headers["Mcp-Session-Id"] = session
            text = response.read().decode()
        if not text:
            return {}
        if text.startswith("event:") or text.startswith("data:"):
            messages = [json.loads(line[5:].strip()) for line in text.splitlines() if line.startswith("data:") and line[5:].strip()]
            value = next(message for message in messages if message.get("id") == identifier)
        else:
            value = json.loads(text)
        if "error" in value:
            raise ValueError(value["error"])
        return value["result"]

    initialized = request("initialize", {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "zg-bench-probe", "version": "1"}}, 1)
    headers["MCP-Protocol-Version"] = initialized["protocolVersion"]
    request("notifications/initialized", {}, None)
    tools = request("tools/list", {}, 2)["tools"]
    expected = {
        "search": {"zvec_grep_search"},
        "relationships": {"zvec_grep_callers", "zvec_grep_callees"},
        "agent": {"zvec_grep_search", "zvec_grep_callers", "zvec_grep_callees"},
    }[group]
    if {tool["name"] for tool in tools} != expected:
        raise ValueError("live MCP tool list differs from experiment arm")
    config_home = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
    guidance = (config_home / "opencode/AGENTS.md").read_text()
    surface = {"instructions": initialized["instructions"], "tools": tools, "guidance": guidance}
    surface["sha256"] = hashlib.sha256(json.dumps(surface, sort_keys=True).encode()).hexdigest()
    if fixture is not None:
        evidence = []
        for number, name in enumerate(sorted(expected), 3):
            arguments = {"root": fixture}
            if name == "zvec_grep_search":
                arguments["fts"] = "normalize"
            else:
                arguments["symbol"] = "normalize" if name.endswith("callers") else "import_record"
            result = request("tools/call", {"name": name, "arguments": arguments}, number)
            if result.get("isError"):
                raise ValueError(result)
            content = json.dumps(result)
            if "normalize" not in content and name.endswith("search"):
                raise ValueError("search missed fixture symbol")
            if not name.endswith("search") and ("save" not in content or "call at" not in content):
                raise ValueError("relationship query missed known call site")
            evidence.append({"name": name, "arguments": arguments, "result": result})
        surface["preflight_calls"] = evidence
    return surface


if __name__ == "__main__":
    import sys
    print(json.dumps(probe(sys.argv[1], sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else None), indent=2))
