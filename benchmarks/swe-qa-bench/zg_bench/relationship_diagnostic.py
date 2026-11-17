"""Oracle capability probes and a frozen, one-decision request replay (no judge)."""
from __future__ import annotations

import argparse
import ast
import copy
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import time
import urllib.request
import urllib.error

CANDIDATE = "85cb58345dac16ac62d85ae99365869cebcc8304"
SOURCE_RUN = "43824775591"
CAPTURE_SHA = "2b151a1e8c17c2f988335619f31a6c8359c8c7591f39888c0577d5ea80247033"
BODY_SHA = "a497d31eae098b2c2dd854cd5ca7748a9fced2ecc20d1630c93012b11cac8c75"
DATA = Path(__file__).parent / "swe_qa/data"
# Frozen before querying. Both directions, limit=20, definition-file restriction.
ANCHORS = {
    "matplotlib-37": [
        ("lib/matplotlib/_mathtext.py", "TruetypeFonts::_get_info"),
        ("lib/matplotlib/_mathtext.py", "Fonts::render_glyph"),
        ("lib/matplotlib/_mathtext.py", "Output::to_vector"),
        ("lib/matplotlib/_mathtext.py", "Output::to_raster"),
        ("lib/matplotlib/textpath.py", "TextToPath::get_glyphs_mathtext"),
    ],
    "sympy-38": [
        ("sympy/matrices/matrices.py", "MatrixDeprecated::minorEntry"),
        ("sympy/matrices/matrices.py", "MatrixDeterminant::minor"),
        ("sympy/matrices/determinant.py", "::_minor"),
        ("sympy/matrices/determinant.py", "::_minor_submatrix"),
        ("sympy/matrices/matrixbase.py", "MatrixBase::extract"),
    ],
    "xarray-32": [
        ("xarray/core/indexes.py", "Index::from_variables"),
        ("xarray/core/indexes.py", "PandasIndex::from_variables"),
        ("xarray/core/coordinates.py", "Coordinates::from_xindex"),
        ("xarray/core/common.py", "DataWithCoords::assign_coords"),
        ("xarray/core/coordinates.py", "Coordinates::update"),
        ("xarray/core/indexes.py", "Index::create_variables"),
        ("xarray/core/indexes.py", "PandasIndex::create_variables"),
    ],
}
READY = ("Diagnostic environment fact: the /app repository graph index has already "
         "been built with zg --index --graph and verified ready. The connected "
         "relationship tools can query it now without rebuilding or authorization.")
DIRECTIVE = ("For the next action, call zvec_grep_zvec_grep_callees with root=/app, "
             "symbol=Coordinates.update, globs=xarray/core/coordinates.py and limit=20 "
             "to locate its downstream calls before doing more manual reference searches.")


def encoded(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def replay_body(original, additions):
    body = copy.deepcopy(original)
    if additions:
        body["messages"][0]["content"] += "\n\n" + "\n".join(additions)
    return body


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def run(command, *, cwd, output, env=None):
    result = subprocess.run(command, cwd=cwd, env=env, text=True, capture_output=True)
    save(output, {"command": command, "cwd": str(cwd), "returncode": result.returncode,
                  "stdout": result.stdout, "stderr": result.stderr})
    result.check_returncode()
    return result.stdout


def definitions(root, anchors):
    """Independently retain pinned source, exact definitions and AST call expressions."""
    found = []
    for file, symbol in anchors:
        text = (root / file).read_text()
        tree = ast.parse(text)
        scope, name = symbol.rsplit("::", 1)
        matches = []
        def visit(node, parents):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                if node.name == name and "::".join(parents) == scope:
                    matches.append(node)
            for child in ast.iter_child_nodes(node):
                visit(child, parents + [node.name] if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)) else parents)
        visit(tree, [])
        if len(matches) != 1:
            raise ValueError(f"Oracle definition not unique: {file} {symbol}: {len(matches)}")
        node = matches[0]
        found.append({"file": file, "symbol": symbol, "line": node.lineno,
                      "end_line": node.end_lineno, "source_sha256": digest(text.encode()),
                      "source": "\n".join(text.splitlines()[node.lineno-1:node.end_lineno]),
                      "calls": [{"line": c.lineno, "byte_column": c.col_offset + 1,
                                 "expression": ast.get_source_segment(text, c)}
                                for c in ast.walk(node) if isinstance(c, ast.Call)]})
    return found


class MCP:
    def __init__(self, output, url="http://127.0.0.1:18991/mcp"):
        self.output = output
        self.url = url
        self.headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
        self.sequence = 0
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def call(self, method, params, *, notification=False):
        self.sequence += 1
        payload = {"jsonrpc": "2.0", "method": method, "params": params}
        if not notification:
            payload["id"] = self.sequence
        entry = {"time": time.time(), "request": payload}
        try:
            req = urllib.request.Request(self.url, encoded(payload), self.headers)
            with self.opener.open(req, timeout=90) as response:
                raw = response.read().decode()
                entry.update(status=response.status, raw_response=raw)
                if response.headers.get("Mcp-Session-Id"):
                    self.headers["Mcp-Session-Id"] = response.headers["Mcp-Session-Id"]
                if not raw:
                    return {}
                if "text/event-stream" in response.headers.get("Content-Type", ""):
                    messages = [json.loads(line[5:]) for line in raw.splitlines() if line.startswith("data:") and line[5:].strip()]
                    value = next(x for x in messages if x.get("id") == payload.get("id"))
                else:
                    value = json.loads(raw)
                entry["response"] = value
                if value.get("error"):
                    raise ValueError(str(value["error"]))
                return value.get("result", {})
        except Exception as error:
            entry["error"] = str(error)
            raise
        finally:
            with self.output.open("a") as out:
                out.write(json.dumps(entry, ensure_ascii=False) + "\n")


def prepare(task, root, output, *, candidate=CANDIDATE, mcp_url="http://127.0.0.1:18991/mcp"):
    selection = json.loads((DATA / "selection-caller-callee.json").read_text())
    item = next(t for t in selection["tasks"] if t["task_slug"] == task)
    run(["git", "init", str(root)], cwd=output, output=output / "clone-init.json")
    run(["git", "fetch", "--depth=1", item["repository_url"], item["repository_commit"]], cwd=root, output=output / "clone-fetch.json")
    run(["git", "checkout", "--detach", "FETCH_HEAD"], cwd=root, output=output / "clone-checkout.json")
    source = definitions(root, ANCHORS[task])
    save(output / "source-definitions.json", source)
    for file, _ in ANCHORS[task]:
        target = output / "pinned-source" / file
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes((root / file).read_bytes())
    ignore = DATA / "index.ignore"
    (root / ".zg-bench-index.ignore").write_bytes(ignore.read_bytes())
    with (root / ".git/info/exclude").open("a") as out:
        out.write("\n.zg-bench-index.ignore\n")
    manifest = {"candidate": candidate, "task": task, "source": item,
                "embedding": "local/potion-code-16m-v2", "binding": "@zvec/bindings-linux-x64@0.5.0",
                "ignore_sha256": digest(ignore.read_bytes()), "anchors": ANCHORS[task],
                "harness": os.environ.get("GITHUB_SHA"), "run_id": os.environ.get("GITHUB_RUN_ID")}
    save(output / "manifest.json", manifest)
    run(["zg", "--index", "--graph", "--embedding", manifest["embedding"], "--ignore-file", ".zg-bench-index.ignore"], cwd=root, output=output / "index.json")
    run(["zg", "--status", "--check-ready"], cwd=root, output=output / "status.json")
    env = {**os.environ, "ZVEC_GREP_GUIDANCE_VARIANT": "direct", "ZVEC_GREP_INSTALL_SKIP_SERVER": "1",
           "ZVEC_GREP_SERVER_URL": mcp_url, "XDG_CONFIG_HOME": str(output / "config")}
    run(["zg", "--install", "--target", "opencode", "--mcp-transport", "http", "--mcp-toolset", "relationships", "--yes"], cwd=root, output=output / "install.json", env=env)
    run(["zg", "--server", "on", "--home", str(output / "daemon"), "--listen", "127.0.0.1:18991", "--mcp-toolset", "relationships"], cwd=root, output=output / "server.json", env=env)
    mcp = MCP(output / "mcp-rpc.jsonl", mcp_url)
    init = mcp.call("initialize", {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "zg-relationship-diagnostic", "version": "1"}})
    mcp.headers["MCP-Protocol-Version"] = init["protocolVersion"]
    mcp.call("notifications/initialized", {}, notification=True)
    surface = {"instructions": init["instructions"], **mcp.call("tools/list", {})}
    surface["guidance"] = (output / "config/opencode/AGENTS.md").read_text()
    save(output / "surface.json", surface)
    return mcp


def execute_readonly(name, arguments, root, mcp):
    if name in ("zvec_grep_zvec_grep_callers", "zvec_grep_zvec_grep_callees"):
        if arguments.get("root") != str(root):
            raise ValueError("MCP query root differs from /app")
        return mcp.call("tools/call", {"name": name.removeprefix("zvec_grep_"), "arguments": arguments})
    def checked_path(value):
        path = Path(value)
        if not path.is_absolute():
            path = root / path
        path = path.resolve()
        if path != root and root not in path.parents:
            raise ValueError("Read-only executor path escapes /app")
        return path
    head_limit = None
    if name == "read":
        path = checked_path(arguments["filePath"])
        start = arguments.get("offset", 1)
        limit = arguments.get("limit", 2000)
        return {"file": str(path), "offset": start, "limit": limit,
                "text": "\n".join(path.read_text().splitlines()[start-1:start-1+limit])}
    if name in ("grep", "glob"):
        path = checked_path(arguments.get("path", str(root)))
        command = ["rg", "--files", str(path)] if name == "glob" else ["rg", "-n", "--", arguments["pattern"], str(path)]
        include = arguments.get("include") if name == "grep" else arguments.get("pattern")
        if include:
            command[1:1] = ["-g", include]
    elif name == "bash":
        # Parse the small read-only subset; never send model text to a shell.
        parts = shlex.split(arguments["command"])
        if "|" in parts:
            pos = parts.index("|")
            tail = parts[pos+1:]
            if tail == ["head"]:
                head_limit = 10
            elif len(tail) == 3 and tail[:2] == ["head", "-n"] and tail[2].isdigit():
                head_limit = int(tail[2])
            elif len(tail) == 2 and tail[0] == "head" and tail[1].startswith("-") and tail[1][1:].isdigit():
                head_limit = int(tail[1][1:])
            else:
                return {"not_executed": "Unsupported pipeline", "arguments": arguments}
            parts = parts[:pos]
        if not parts or parts[0] != "rg" or any(x in parts for x in [";", "&&", "||", ">", ">>", "&"]) or any(x.startswith(("--pre", "--hostname-bin")) for x in parts):
            return {"not_executed": "Unsupported read-only shell command", "arguments": arguments}
        for arg in parts[1:]:
            if arg.startswith("/"):
                checked_path(arg)
        command = parts
    else:
        return {"not_executed": "Outside read-only one-step diagnostic", "arguments": arguments}
    result = subprocess.run(command, cwd=root, capture_output=True, text=True, timeout=30)
    stdout = result.stdout if head_limit is None else "".join(result.stdout.splitlines(keepends=True)[:head_limit])
    return {"command": command, "head_limit": head_limit, "returncode": result.returncode, "stdout": stdout, "stderr": result.stderr}


def execute_saved(source, root, output, mcp):
    """Retry only saved read-only actions, preserving original failures and model responses."""
    frozen = json.loads(next(source.glob("**/frozen-branch.json")).read_text())
    if frozen["body_sha256"] != BODY_SHA:
        raise ValueError("Saved actions are not from the frozen branch")
    paths = sorted(source.glob("**/replay/*/completion.json"))
    if len(paths) != 20:
        raise ValueError("Expected all twenty original model decisions")
    for path in paths:
        request = path.with_name("request.json").read_bytes()
        metadata = json.loads(path.with_name("metadata.json").read_text())
        if digest(request) != metadata["request_sha256"]:
            raise ValueError("Saved request checksum differs")
        execution = []
        for call in json.loads(path.read_text())["tool_calls"]:
            execution.append({"call": call, "result": execute_readonly(call["function"]["name"], json.loads(call["function"]["arguments"]), root, mcp)})
        save(output / "re-executed" / path.parent.name / "tool-execution.json", {"model_requests": 0, "source_run": os.environ.get("DIAGNOSTIC_REPLAY_RUN"), "request_sha256": digest(request), "completion_sha256": digest(path.read_bytes()), "execution": execution})


def parse_completion(raw):
    calls = {}
    content, reasoning, usage, finishes = [], [], None, []
    for line in raw.splitlines():
        if not line.startswith("data:") or not line[5:].strip() or line[5:].strip() == "[DONE]":
            continue
        value = json.loads(line[5:])
        if value.get("usage"):
            usage = value["usage"]
        for choice in value.get("choices", []):
            delta = choice.get("delta", {})
            content.append(delta.get("content") or "")
            reasoning.append(delta.get("reasoning_content") or "")
            if choice.get("finish_reason"):
                finishes.append(choice["finish_reason"])
            for call in delta.get("tool_calls", []):
                item = calls.setdefault(call["index"], {"id": "", "type": "function", "function": {"name": "", "arguments": ""}})
                item["id"] += call.get("id", "")
                for field in ("name", "arguments"):
                    item["function"][field] += call.get("function", {}).get(field, "")
    if not finishes:
        raise ValueError("Incomplete model SSE response (no finish_reason)")
    return {"content": "".join(content), "reasoning_content": "".join(reasoning),
            "tool_calls": [calls[i] for i in sorted(calls)], "usage": usage, "finish_reasons": finishes}


def replay(args, root, output, mcp):
    from .engines.registry import resolve_opencode_model
    runtime = resolve_opencode_model("custom-openai/glm-5.2", require_credentials=True)
    capture = next(args.source.glob("**/xarray-32__wp7vyRX/agent/model-requests.jsonl"))
    if digest(capture.read_bytes()) != CAPTURE_SHA:
        raise ValueError("Source request capture differs from frozen trial")
    events = [json.loads(x) for x in capture.read_text().splitlines()]
    event = next(x for x in events if x["type"] == "request" and x["id"] == 16)
    original = event["body"]
    if digest(encoded(original)) != BODY_SHA or event["body_sha256"] != BODY_SHA:
        raise ValueError("Frozen branch body differs")
    manifests = [json.loads(p.read_text()) for p in args.capability.glob("**/manifest.json")]
    capability = next(m for m in manifests if m.get("task") == "xarray-32")
    current = json.loads((output / "manifest.json").read_text())
    for field in ("candidate", "embedding", "binding", "ignore_sha256", "source"):
        if current[field] != capability[field]:
            raise ValueError(f"Capability/replay identity differs: {field}")
    surface = json.loads((output / "surface.json").read_text())
    if not all(t["function"]["description"] == next(s["description"] for s in surface["tools"] if t["function"]["name"] == "zvec_grep_" + s["name"]) for t in original["tools"] if t["function"]["name"].startswith("zvec_")):
        raise ValueError("Live relationship descriptions differ from captured request")
    save(output / "frozen-branch.json", {"source_run": SOURCE_RUN, "source_capture_sha256": CAPTURE_SHA, "event_id": 16, "body_sha256": BODY_SHA,
                                        "capability_run": capability["run_id"], "ready": READY, "directive": DIRECTIVE,
                                        "order": "round-robin baseline, ready, directive, both; repeats=5", "retries": 0})
    for repeat in range(1, 6):
        for arm, additions in [("baseline", []), ("ready", [READY]), ("directive", [DIRECTIVE]), ("both", [READY, DIRECTIVE])]:
            dest = output / "replay" / f"{repeat}-{arm}"
            dest.mkdir(parents=True)
            body = replay_body(original, additions)
            data = encoded(body)
            (dest / "request.json").write_bytes(data)
            metadata = {"arm": arm, "repeat": repeat, "request_sha256": digest(data), "started": time.time(), "attempt": 1}
            try:
                request = urllib.request.Request(runtime.configuration.base_url.rstrip("/") + "/chat/completions", data,
                                                 {"Content-Type": "application/json", "Authorization": "Bearer " + runtime.api_key})
                with urllib.request.urlopen(request, timeout=600) as response:
                    metadata["http_status"] = response.status
                    with (dest / "response.sse").open("wb") as stream:
                        while chunk := response.read(8192):
                            stream.write(chunk)
                result = parse_completion((dest / "response.sse").read_text())
                save(dest / "completion.json", result)
                execution = []
                for call in result["tool_calls"]:
                    try:
                        value = execute_readonly(call["function"]["name"], json.loads(call["function"]["arguments"]), root, mcp)
                        execution.append({"call": call, "result": value})
                    except Exception as error:
                        execution.append({"call": call, "error": str(error)})
                save(dest / "tool-execution.json", execution)
                metadata["complete"] = True
            except urllib.error.HTTPError as error:
                (dest / "response-error.txt").write_bytes(error.read())
                metadata.update(complete=False, http_status=error.code, error=str(error))
            except Exception as error:
                metadata.update(complete=False, error=str(error))
            metadata["ended"] = time.time()
            save(dest / "metadata.json", metadata)
            print(json.dumps(metadata), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage", choices=["capability", "replay", "execute"], required=True)
    parser.add_argument("--task", choices=list(ANCHORS), required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--capability", type=Path)
    args = parser.parse_args()
    if os.environ.get("DIAGNOSTIC_CANDIDATE") != CANDIDATE:
        raise ValueError("Diagnostic requires frozen Rust candidate")
    if args.stage == "replay" and (args.task != "xarray-32" or args.source is None or args.capability is None):
        raise ValueError("Replay requires frozen Xarray source and preceding capability artifacts")
    if args.stage == "execute" and (args.task != "xarray-32" or args.source is None):
        raise ValueError("Execution requires saved Xarray decisions")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    root = Path("/app")
    try:
        mcp = prepare(args.task, root, output)
        if args.stage == "capability":
            results = []
            for file, symbol in ANCHORS[args.task]:
                for direction in ("callers", "callees"):
                    parameters = {"root": "/app", "symbol": symbol, "globs": file, "limit": 20}
                    result = mcp.call("tools/call", {"name": "zvec_grep_" + direction, "arguments": parameters})
                    results.append({"direction": direction, "arguments": parameters, "result": result})
                    save(output / "capability.json", results)
        elif args.stage == "replay":
            replay(args, root, output, mcp)
        else:
            execute_saved(args.source, root, output, mcp)
    finally:
        run(["zg", "--server", "off", "--home", str(output / "daemon")], cwd=root, output=output / "server-off.json")


if __name__ == "__main__":
    main()
