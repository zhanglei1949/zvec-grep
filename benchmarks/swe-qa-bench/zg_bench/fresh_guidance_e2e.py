"""Fresh OpenCode sessions with real AGENTS.md loading and matching MCP guidance."""
from __future__ import annotations

import contextlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import threading
import time

from .relationship_diagnostic import CANDIDATE, DIRECTIVE as LEGACY_DIRECTIVE, READY, digest, prepare, run, save
from .engines.registry import resolve_opencode_model
from .engines.opencode.config import build_opencode_config
from .agents.opencode_usage import CALL_AUDIT_SQL, session_tool_calls, summarize_session_usage

ROOT = Path(__file__).parent
END = "<!-- ZVEC_GREP_END -->"
PARAMETER_CLAUSE = "root=/app, symbol=Coordinates.update, globs=xarray/core/coordinates.py and limit=20"
PARAMETERS = {"root": "/app", "symbol": "Coordinates.update", "globs": "xarray/core/coordinates.py", "limit": 20}
JSON_EXAMPLE = "\n```json\n" + json.dumps(PARAMETERS, indent=2) + "\n```\n"
DIRECTIVE = LEGACY_DIRECTIVE.replace(PARAMETER_CLAUSE, JSON_EXAMPLE)


def guidance_body(text):
    return "\n".join(line for line in text.splitlines() if line not in ("<!-- ZVEC_GREP_START -->", END, "## zvec-grep")).strip()


def matching_guidance(surface):
    additions = READY + "\n" + DIRECTIVE
    agents = surface["guidance"].replace(END, additions + "\n" + END)
    instructions = surface["instructions"].rstrip() + "\n" + additions.replace("zvec_grep_zvec_grep_", "zvec_grep_")
    if guidance_body(agents).replace("zvec_grep_zvec_grep_", "zvec_grep_") != instructions:
        raise ValueError("AGENTS.md and MCP instructions differ after host name normalization")
    return agents, instructions


@contextlib.contextmanager
def mock_model():
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            value = {"id": "contract-only", "object": "chat.completion.chunk", "created": int(time.time()), "model": body["model"],
                     "choices": [{"index": 0, "delta": {"role": "assistant", "content": "CONTRACT_ONLY"}, "finish_reason": "stop"}],
                     "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}
            self.wfile.write(("data: " + json.dumps(value) + "\n\ndata: [DONE]\n\n").encode())

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/v1"
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def start_node(script, arguments, output, processes):
    stderr = (output / (script.stem + ".stderr")).open("w")
    process = subprocess.Popen(["node", str(script), *map(str, arguments)], stdout=subprocess.PIPE, stderr=stderr, text=True)
    processes.append((process, stderr))
    control = process.stdout.readline()
    if not control:
        raise RuntimeError(f"{script.name} did not start")
    return json.loads(control)


def trial(directory, question, base, upstream, api_key, *, contract_only=False):
    directory.mkdir(parents=True)
    processes = []
    metadata = {"contract_only": contract_only, "started": time.time(), "complete": False}
    try:
        guidance, instructions = matching_guidance(base)
        (directory / "instructions.txt").write_text(instructions)
        proxy = start_node(ROOT / "fixed_search_proxy.mjs", ["http://127.0.0.1:18991/mcp", directory / "mcp-execution.jsonl", directory / "mcp.closed", directory / "instructions.txt"], directory, processes)
        env = {**os.environ, "XDG_CONFIG_HOME": str(directory / "config"), "XDG_DATA_HOME": str(directory / "data"),
               "XDG_STATE_HOME": str(directory / "state"), "XDG_CACHE_HOME": str(directory / "cache"),
               "ZVEC_GREP_GUIDANCE_VARIANT": "direct", "ZVEC_GREP_INSTALL_SKIP_SERVER": "1", "ZVEC_GREP_SERVER_URL": proxy["url"],
               "OPENCODE_DISABLE_MODELS_FETCH": "true", "OPENCODE_DISABLE_EXTERNAL_SKILLS": "true", "OPENAI_API_KEY": api_key}
        config_directory = directory / "config/opencode"
        env["OPENCODE_CONFIG"] = str(config_directory / "opencode.json")
        run(["zg", "--install", "--target", "opencode", "--mcp-transport", "http", "--mcp-toolset", "relationships", "--yes"], cwd="/app", output=directory / "install.json", env=env)
        agents = config_directory / "AGENTS.md"
        if agents.read_text() != base["guidance"]:
            raise ValueError("Fresh installed AGENTS.md differs from frozen base")
        (directory / "AGENTS.before.md").write_text(agents.read_text())
        agents.write_text(guidance)
        if agents.read_text() != guidance:
            raise ValueError("AGENTS.md write/readback differs")
        surface_raw = run(["node", str(ROOT / "mcp_probe.mjs"), proxy["url"], "relationships"], cwd="/app", output=directory / "surface-probe.json", env=env)
        surface = json.loads(surface_raw)
        if surface["guidance"] != guidance or surface["instructions"] != instructions or surface["tools"] != base["tools"]:
            raise ValueError("Live guidance or relationship tool definitions differ")
        save(directory / "surface.json", surface)
        contract = {"guidance": guidance, "instructions": instructions, "tool_names": ["zvec_grep_" + t["name"] for t in surface["tools"]]}
        save(directory / "request-contract.json", contract)
        recorder = start_node(ROOT / "model_request_recorder.mjs", [upstream, directory / "model-requests.jsonl", directory / "recorder.closed", directory / "request-contract.json"], directory, processes)
        config = build_opencode_config(resolve_opencode_model("custom-openai/glm-5.2").configuration, profile="zvec-grep", tool_experiment=True)
        config["provider"]["custom-openai"]["options"]["baseURL"] = recorder["baseURL"]
        config["mcp"]["zvec_grep"]["url"] = proxy["url"]
        config.update(autoupdate=False, share="disabled", lsp=False)
        save(config_directory / "opencode.json", config)
        before = json.loads(run(["opencode", "db", "SELECT count(*) AS n FROM session", "--format", "json"], cwd="/app", output=directory / "sessions-before.json", env=env))
        if before != [{"n": 0}]:
            raise ValueError("OpenCode data directory is not a fresh session store")
        with (directory / "opencode.jsonl").open("w") as stdout, (directory / "opencode.stderr").open("w") as stderr:
            command = ["opencode", "run", "--dir", "/app", "--format", "json", "--model", "custom-openai/glm-5.2", "--dangerously-skip-permissions", "--", question]
            save(directory / "command.json", command)
            result = subprocess.run(command, cwd="/app", env=env, stdout=stdout, stderr=stderr, timeout=900)
        metadata["returncode"] = result.returncode
        snapshot = json.loads(run(["opencode", "db", CALL_AUDIT_SQL, "--format", "json"], cwd="/app", output=directory / "snapshot-command.json", env=env))
        save(directory / "session-snapshot.json", snapshot)
        events = [json.loads(line) for line in (directory / "opencode.jsonl").read_text().splitlines() if line.strip().startswith("{")]
        root_ids = {e["sessionID"] for e in events if e.get("sessionID")}
        if len(root_ids) != 1:
            raise ValueError("Expected one fresh root session")
        usage = summarize_session_usage(snapshot, next(iter(root_ids)), capture_complete=result.returncode == 0)
        save(directory / "session-usage.json", usage)
        calls = session_tool_calls(snapshot, {s["session_id"] for s in usage["sessions"]})
        save(directory / "tool-calls.json", calls)
        answers = [e["part"].get("text", "") for e in events if e.get("type") == "text" and e.get("part", {}).get("type") == "text"]
        (directory / "answer.txt").write_text("\n".join(answers))
        requests = [json.loads(line) for line in (directory / "model-requests.jsonl").read_text().splitlines()]
        main = [e for e in requests if e["type"] == "request" and e["body"].get("tools")]
        verified = [e for e in requests if e["type"] == "guidance_verified"]
        first = [e for e in verified if e.get("first_task_request")]
        if not main or len(first) != 1 or first[0]["id"] != main[0]["id"] or len(main) != len(verified):
            raise ValueError("Actual first task request or later guidance verification incomplete")
        if any(e["type"] == "guidance_rejected" for e in requests):
            raise ValueError("Actual guidance gate rejected a request")
        if contract_only and ((directory / "answer.txt").read_text().strip() != "CONTRACT_ONLY" or calls):
            raise ValueError("Contract-only OpenCode preflight produced unexpected actions")
        if result.returncode != 0 or not usage["complete"] or not answers:
            raise ValueError("OpenCode answering session incomplete")
        dirty = run(["git", "status", "--porcelain", "--untracked-files=no"], cwd="/app", output=directory / "source-after.json")
        if dirty.strip():
            raise ValueError("Answering agent changed pinned source")
        metadata.update(complete=True, root_session_id=next(iter(root_ids)), first_request_id=main[0]["id"],
                        first_request_sha256=main[0]["body_sha256"], task_requests=len(main), guidance_verified=len(verified),
                        first_tool=calls[0]["name"] if calls else None, relationship_calls=sum(c["name"].startswith("zvec_grep_") for c in calls),
                        endpoint_sha256=digest(upstream.encode()), usage=usage["total"])
    except Exception as error:
        metadata["error"] = str(error)
    finally:
        for process, stderr in reversed(processes):
            process.terminate()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            stderr.close()
        metadata["ended"] = time.time()
        save(directory / "result.json", metadata)
    return metadata


def main():
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True)
    if os.environ.get("DIAGNOSTIC_CANDIDATE") != CANDIDATE:
        raise ValueError("Frozen candidate required")
    try:
        prepare("xarray-32", Path("/app"), output)
        base = json.loads((output / "surface.json").read_text())
        question = json.loads((output / "manifest.json").read_text())["source"]["question"]
        save(output / "protocol.json", {"kind": "fresh OpenCode instruction compliance E2E", "real_trials": 5, "contract_only_sessions": 1,
             "directive_format": "complete JSON example, globs retained", "comparison_run": "43843188341",
             "guidance_file": "per-trial config/opencode/AGENTS.md", "mcp_override": "initialize instructions only; tool metadata and relationship arguments unchanged",
             "api_tool_choice": "auto", "question": question, "provided_target": "Coordinates.update", "judge_calls": 0, "retry_on_choice": False})
        with mock_model() as upstream:
            proof = trial(output / "contract-preflight", question, base, upstream, "contract-only", contract_only=True)
        if not proof["complete"]:
            raise RuntimeError("No-model real OpenCode contract preflight failed: " + proof.get("error", "unknown"))
        runtime = resolve_opencode_model("custom-openai/glm-5.2", require_credentials=True)
        results = []
        for i in range(1, 6):
            result = trial(output / f"trial-{i}", question, base, runtime.configuration.base_url, runtime.api_key)
            results.append(result)
            save(output / "results.json", results)
            print(json.dumps({k: v for k, v in result.items() if k != "usage"}), flush=True)
        if not all(r["complete"] for r in results):
            raise RuntimeError("At least one prespecified trial incomplete; all records retained")
    finally:
        run(["zg", "--server", "off", "--home", str(output / "daemon")], cwd="/app", output=output / "server-off.json")


if __name__ == "__main__":
    main()
