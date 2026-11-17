"""At most five fresh Xarray sessions, gated on the first canonical MCP result."""
from __future__ import annotations

import argparse
import ast
import copy
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time

from .fresh_guidance_e2e import matching_guidance, mock_model, trial
from .relationship_diagnostic import MCP, digest, prepare, run, save
from .engines.registry import resolve_opencode_model

CANONICAL_URL = "http://127.0.0.1:18991/mcp/canonical"
BASELINE = Path(__file__).parents[1] / "fixtures/caller-callee-canonical-baseline-20261117.json"
INVALID_LIMIT = 3


def read_events(path):
    if not path.exists():
        return []
    result = []
    for line in path.read_text(errors="replace").splitlines():
        try:
            result.append(json.loads(line))
        except ValueError:
            pass  # Preserve the file; a trailing event may still be streaming.
    return result


def invalid_streak(events):
    """Count distinct, consecutive completed invalid actions with the same error."""
    seen = set()
    last = None
    streak = 0
    for event in events:
        part = event.get("part", {})
        state = part.get("state", {})
        if event.get("type") != "tool_use" or state.get("status") not in ("completed", "error"):
            continue
        identity = part.get("callID") or part.get("id")
        if not identity or identity in seen:
            continue
        seen.add(identity)
        if part.get("tool") != "invalid":
            last, streak = None, 0
            continue
        signature = json.dumps(state.get("input", {}), sort_keys=True)
        streak = streak + 1 if signature == last else 1
        last = signature
    return streak


def bounded_answer(command, *, cwd, env, stdout, stderr, timeout):
    """Keep the old wall limit and stop a repeated-invalid loop without retries."""
    started = time.monotonic()
    reason = None
    with subprocess.Popen(command, cwd=cwd, env=env, stdout=stdout, stderr=stderr,
                          start_new_session=True) as process:
        while process.poll() is None:
            if time.monotonic() - started >= timeout:
                reason = "wall_timeout"
            elif invalid_streak(read_events(Path(stdout.name))) >= INVALID_LIMIT:
                reason = "three_consecutive_identical_invalid_actions"
            if reason:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                break
            time.sleep(0.25)
        result = subprocess.CompletedProcess(command, process.wait())
    result.stop_reason = reason
    return result


def check_surfaces(legacy, canonical, frozen):
    for key in ("guidance", "instructions", "tools"):
        if legacy[key] != frozen[key]:
            raise ValueError(f"Legacy {key} differs from the preceding cohort")
    for key in ("guidance", "instructions"):
        if canonical[key] != frozen[key]:
            raise ValueError(f"Canonical {key} changed the guidance intervention")
    old_tools = {t["name"]: t for t in legacy["tools"]}
    new_tools = {t["name"]: t for t in canonical["tools"]}
    if old_tools.keys() != new_tools.keys() or len(old_tools) != 2:
        raise ValueError("Expected the same two relationship tools")
    for name, new in new_tools.items():
        expected = copy.deepcopy(old_tools[name])
        old_schema, new_schema = expected["inputSchema"], new["inputSchema"]
        for field in ("root", "symbol", "limit"):
            if old_schema["properties"][field] != new_schema["properties"][field]:
                raise ValueError(f"Unexpected schema change: {name}/{field}")
        if any(key in json.dumps(new_schema) for key in ('"$defs"', '"$ref"', '"anyOf"', '"oneOf"')):
            raise ValueError("Canonical schema still contains references or unions")
        properties = new_schema["properties"]
        if properties["globs"]["type"] != "array" or properties["globs"]["items"]["type"] != "object":
            raise ValueError("Canonical globs must be rule arrays")
        if properties["insensitiveGlobs"]["type"] != "array" or properties["insensitiveGlobs"]["items"]["type"] != "string":
            raise ValueError("Canonical insensitiveGlobs must be string arrays")
        # Only the two filters and their now-unreferenced definitions may differ.
        comparison = copy.deepcopy(new_schema)
        comparison["properties"]["globs"] = old_schema["properties"]["globs"]
        comparison["properties"]["insensitiveGlobs"] = old_schema["properties"]["insensitiveGlobs"]
        if "$defs" in old_schema:
            comparison["$defs"] = old_schema["$defs"]
        if comparison != old_schema:
            raise ValueError(f"Other schema fields changed: {name}")
        expected["inputSchema"] = new_schema
        if expected != new:
            raise ValueError(f"Tool metadata changed: {name}")
    if matching_guidance(legacy) != matching_guidance(canonical):
        raise ValueError("Effective AGENTS.md or MCP instructions changed")


def source_valid_results(directory, root):
    """Validate returned target, endpoint definitions and call positions against AST."""
    definitions = json.loads((directory.parent / "source-definitions.json").read_text())
    target = next(x for x in definitions if x["symbol"] == "Coordinates::update")
    events = read_events(directory / "mcp-execution.jsonl")
    calls = {e["sequence"]: e for e in events if e.get("type") == "call"}
    checks = []
    for event in events:
        call = calls.get(event.get("sequence"))
        if event.get("type") != "result" or not call or call["name"] != "zvec_grep_callees":
            continue
        result = event.get("result") or {}
        text = "\n".join(x.get("text", "") for x in result.get("content", []))
        check = {"sequence": event["sequence"], "arguments": call["effective_arguments"],
                 "success": event.get("http_status") == 200 and not event.get("error") and not result.get("isError"),
                 "text": text, "valid_positions": [], "invalid_positions": []}
        expected_header = f'{target["symbol"]}  {target["file"]}:{target["line"]}'
        check["target_matches"] = text.splitlines()[:1] == [expected_header]
        check["arguments_unchanged"] = call["requested_arguments"] == call["effective_arguments"]
        sites = re.findall(r"^      call at (.+):(\d+):(\d+)$", text, re.M)
        endpoints = re.findall(r"^    (\S+)  (.+):(\d+)$", text, re.M)
        endpoint_validity = []
        for name, file, line in endpoints:
            path = (root / file).resolve()
            valid = root.resolve() in path.parents and path.is_file()
            if valid:
                tree = ast.parse(path.read_text())
                def qualified_matches(node, scope=()):
                    if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.lineno == int(line):
                        return "::".join((*scope, node.name)) == name
                    nested = (*scope, node.name) if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)) else scope
                    return any(qualified_matches(child, nested) for child in ast.iter_child_nodes(node))
                valid = qualified_matches(tree)
            endpoint_validity.append(valid)
            (check["valid_positions"] if valid else check["invalid_positions"]).append({"kind": "definition", "name": name, "file": file, "line": int(line)})
        check["verified_relationships"] = []
        check["unverified_relationships"] = []
        for index, (file, line, column) in enumerate(sites):
            valid = file == target["file"] and target["line"] <= int(line) <= target["end_line"]
            expression = None
            if valid:
                tree = ast.parse((root / file).read_text())
                node = next((n for n in ast.walk(tree) if isinstance(n, ast.Call) and n.lineno == int(line) and n.col_offset + 1 == int(column)), None)
                valid = node is not None
                if node:
                    expression = ast.get_source_segment((root / file).read_text(), node)
            (check["valid_positions"] if valid else check["invalid_positions"]).append({"kind": "call", "file": file, "line": int(line), "column": int(column), "expression": expression})
            binding = False
            if valid and index < len(endpoints) and endpoint_validity[index]:
                name, endpoint_file, _ = endpoints[index]
                if isinstance(node.func, ast.Name) and name == node.func.id:
                    binding = endpoint_file == target["file"]
                    for imported in ast.walk(tree):
                        if isinstance(imported, ast.ImportFrom) and imported.module and imported.level == 0:
                            binding |= any((alias.asname or alias.name) == node.func.id and alias.name == name
                                           and endpoint_file == imported.module.replace(".", "/") + ".py" for alias in imported.names)
                elif isinstance(node.func, ast.Attribute) and isinstance(node.func.value, ast.Name) and node.func.value.id in ("self", "cls"):
                    binding = endpoint_file == target["file"] and name == target["symbol"].rsplit("::", 1)[0] + "::" + node.func.attr
            edge = {"endpoint": endpoints[index] if index < len(endpoints) else None,
                    "call": {"file": file, "line": int(line), "column": int(column), "expression": expression}}
            # Position existence alone cannot validate a resolved edge (e.g. builtin
            # len incorrectly bound to StringAccessor::len in the historical graph).
            (check["verified_relationships"] if binding else check["unverified_relationships"]).append(edge)
        args = call["effective_arguments"]
        check["source_valid_nonempty"] = bool(check["success"] and check["target_matches"] and check["arguments_unchanged"]
            and args.get("root") == str(root) and args.get("symbol") in ("Coordinates.update", "Coordinates::update")
            and len(endpoints) == len(sites) and check["verified_relationships"])
        checks.append(check)
    return checks


def cohort_gate(result, checks):
    return bool(result.get("complete") and any(x["source_valid_nonempty"] for x in checks))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True)
    candidate = os.environ.get("DIAGNOSTIC_CANDIDATE", "")
    if not re.fullmatch("[0-9a-f]{40}", candidate):
        raise ValueError("An immutable candidate SHA is required")
    root = Path("/app")
    results = []
    try:
        prepare("xarray-32", root, output, candidate=candidate, mcp_url=CANONICAL_URL)
        base = json.loads((output / "surface.json").read_text())
        probe = MCP(output / "legacy-mcp-rpc.jsonl")
        init = probe.call("initialize", {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "canonical-control-probe", "version": "1"}})
        probe.headers["MCP-Protocol-Version"] = init["protocolVersion"]
        probe.call("notifications/initialized", {}, notification=True)
        legacy = {"guidance": base["guidance"], "instructions": init["instructions"], **probe.call("tools/list", {})}
        save(output / "legacy-surface.json", legacy)
        check_surfaces(legacy, base, json.loads(BASELINE.read_text()))
        question = json.loads((output / "manifest.json").read_text())["source"]["question"]
        save(output / "protocol.json", {"kind": "canonical relationship schema real-chain acceptance", "comparison_run": "43846728650",
             "max_real_sessions": 5, "pilot_sessions": 1, "pilot_gate": "complete answer and source-valid nonempty Coordinates.update callees result",
             "early_stop": {"consecutive_identical_invalid_actions": INVALID_LIMIT, "wall_seconds": 900},
             "only_model_visible_change": "relationship parameter schemas from /mcp/canonical",
             "guidance_changed": False, "legacy_surface_matches_previous": True, "retry_failed_or_zero_call_sessions": False,
             "base_fixture_sha256": digest(BASELINE.read_bytes()), "judge_calls": 0})
        with mock_model() as upstream:
            proof = trial(output / "contract-preflight", question, base, upstream, "contract-only", contract_only=True,
                          mcp_upstream=CANONICAL_URL, answer_runner=bounded_answer)
        if not proof["complete"]:
            raise RuntimeError("No-model OpenCode contract preflight failed: " + proof.get("error", "unknown"))
        runtime = resolve_opencode_model("custom-openai/glm-5.2", require_credentials=True)
        for i in range(1, 6):
            directory = output / f"trial-{i}"
            result = trial(directory, question, base, runtime.configuration.base_url, runtime.api_key,
                           mcp_upstream=CANONICAL_URL, answer_runner=bounded_answer)
            checks = source_valid_results(directory, root)
            save(directory / "source-validation.json", checks)
            result["source_valid_nonempty"] = any(x["source_valid_nonempty"] for x in checks)
            results.append(result)
            save(output / "results.json", results)
            print(json.dumps({k: v for k, v in result.items() if k != "usage"}), flush=True)
            if not result["complete"] or (i == 1 and not cohort_gate(result, checks)):
                save(output / "stopped.json", {"after_trial": i, "reason": "pilot_gate_failed" if i == 1 else "session_incomplete", "remaining_sessions_not_started": 5-i})
                raise RuntimeError("Stop at the first failed gate; no sessions retried")
    finally:
        run(["zg", "--server", "off", "--home", str(output / "daemon")], cwd=root, output=output / "server-off.json")


if __name__ == "__main__":
    main()
