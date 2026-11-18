"""Seven independent real-OpenCode checks; never starts QA20 or a judge."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
from pathlib import Path

from .agents.opencode_usage import session_tool_calls, summarize_session_usage
from .engines.registry import resolve_opencode_model
from .release_analysis import events, evidence_manifest
from .release_e2e import ROOT, make_smoke_task, prepare_overlay, run_job
from .release_packages import sha256, verify_release, write_json

QUERY = "normalize negative values before saving"
CASES = [
    {"id": "minimal-search", "phase": "minimal", "tool": "search", "arguments": {"root": "/app", "query": QUERY}},
    {"id": "minimal-callers", "phase": "minimal", "tool": "callers", "arguments": {"root": "/app", "symbol": "::save"}},
    {"id": "minimal-callees", "phase": "minimal", "tool": "callees", "arguments": {"root": "/app", "symbol": "::save"}},
    {"id": "optional-search-fts", "phase": "optional", "tool": "search", "arguments": {"root": "/app", "query": QUERY, "fts": "normalize"}},
    {"id": "optional-search-globs", "phase": "optional", "tool": "search", "arguments": {"root": "/app", "query": QUERY, "globs": "lib.rs"}},
    {"id": "optional-callers-globs", "phase": "optional", "tool": "callers", "arguments": {"root": "/app", "symbol": "::save", "globs": "lib.rs"}},
    {"id": "optional-callees-globs", "phase": "optional", "tool": "callees", "arguments": {"root": "/app", "symbol": "::save", "globs": "lib.rs"}},
]


def baseline_protocol(directory: Path, manifest: dict) -> tuple[Path, dict]:
    paths = [path for path in directory.rglob("protocol.json")
             if json.loads(path.read_text()).get("protocol", "").startswith("release-0.3.1-chain-check-")]
    if len(paths) != 1:
        raise ValueError("Expected exactly one baseline chain protocol")
    root, protocol = paths[0].parent, json.loads(paths[0].read_text())
    if (protocol["cases"] != CASES or protocol["fixture_sha256"] != sha256(ROOT / "fixtures/caller-callee-dev/lib.rs")
            or protocol["release"] != manifest):
        raise ValueError("Baseline cases, fixture or official release differ")
    snapshots = root / "index-snapshots"
    checks = list(snapshots.glob("*/payload-sha256.json"))
    if len(checks) != 1:
        raise ValueError("Expected one baseline fixture index snapshot")
    workspace = checks[0].parent / "workspace"
    actual = {str(path.relative_to(workspace)): sha256(path) for path in sorted(workspace.rglob("*")) if path.is_file()}
    if not actual or actual != json.loads(checks[0].read_text()):
        raise ValueError("Baseline index snapshot differs from its recorded hashes")
    return root, protocol


def resumable_cases(root: Path, protocol: dict, model: str) -> list[str]:
    if protocol.get("model", "custom-openai/glm-5.2") != model:
        raise ValueError("Completed sessions can only be resumed for the identical model")
    result = []
    for path in sorted((root / "cases").glob("*/case-report.json")):
        row = json.loads(path.read_text())
        if (row["session_status"] == "completed" and row["published_guidance_schema_unchanged"] and row["actual_context_verified"]
                and not row["invalid_schema_attempts"] and all(actor["executed_successfully"] for actor in row["actors"].values())):
            result.append(row["case"]["id"])
        elif any(event["type"] == "request" for log in path.parent.rglob("model-requests.jsonl") for event in events(log)):
            raise ValueError("Refusing to rerun a failed case that already emitted a model request")
    return result


def make_case(output: Path, case: dict):
    suite = make_smoke_task(output, "agent")
    task = suite.path / "release-dev"
    arguments = json.dumps(case["arguments"], separators=(",", ":"))
    name = "zvec_grep_zvec_grep_" + case["tool"]
    instruction = (
        "This is a tool integration check on the current development workspace. "
        f"First, you (the main agent) must actually call {name} with exactly {arguments}. "
        "Use exactly these argument keys and values; do not add optional arguments. "
        "After your own call returns, use the task tool once with subagent_type explore. "
        f"Tell that child to actually call {name} with exactly {arguments}, then read the relevant source. "
        "Both you and the child should report the returned source locations and what they demonstrate. "
        "Do not substitute native search or a different MCP tool for the requested call. "
        "Read source only after the requested MCP call. Do not retry identical failing arguments. "
        "If the tool fails, report the failure. Do not modify files, configuration or guidance."
    )
    (task / "instruction.md").write_text(instruction + "\n")
    metadata = task / "task.toml"
    metadata.write_text(metadata.read_text().replace("timeout_sec = 1800.0", "timeout_sec = 300.0"))
    return suite


def useful_output(tool: str, output: str) -> dict:
    source = (ROOT / "fixtures/caller-callee-dev/lib.rs").read_text().splitlines()
    if tool == "search":
        correct = "lib.rs:2" in output and "normalize" in output and "value.max(0)" in output
        return {"correct_and_useful": correct, "contains_useful_information": correct, "expected": "normalization definition at lib.rs:2, including max(0)"}
    actual = {(int(line), int(column)) for line, column in re.findall(r"call at lib\.rs:(\d+):(\d+)", output)}
    if tool == "callers":
        expected = {(index, line.index("save(") + 1) for index, line in enumerate(source, 1) if line.startswith(("pub fn import_record", "pub fn export_record"))}
        endpoints = all(re.search(rf"\b{name}\s+lib\.rs:{line}\b", output) for name, line in (("import_record", 6), ("export_record", 8)))
    else:
        expected = {(index, line.index("normalize(") + 1) for index, line in enumerate(source, 1) if line.startswith("pub fn save")}
        endpoints = bool(re.search(r"\bnormalize\s+lib\.rs:2\b", output))
    matched_definition = bool(re.search(r"^(?:::)?save\s+lib\.rs:4\b", output))
    return {"correct_and_useful": matched_definition and endpoints and actual == expected,
            "contains_useful_information": bool(matched_definition and endpoints and actual & expected),
            "matched_definition_correct": matched_definition,
            "expected_call_sites": sorted(expected), "actual_call_sites": sorted(actual),
            "correct_call_sites": sorted(actual & expected), "unexpected_call_sites": sorted(actual - expected),
            "missing_call_sites": sorted(expected - actual)}


def session_evidence(row: dict) -> tuple[str | None, list[dict], dict | None]:
    if row.get("session_usage", {}).get("root_session_id"):
        return row["session_usage"]["root_session_id"], row.get("calls", []), row.get("session_usage")
    path = Path(row["evidence"]) / "agent/opencode-session-snapshot.json"
    if not path.exists():
        return None, row.get("calls", []), None
    snapshot = json.loads(path.read_text())
    sessions = [json.loads(record["data"]) if isinstance(record["data"], str) else record["data"] for record in snapshot if record["kind"] == "session"]
    roots = [session["id"] for session in sessions if not session.get("parent_id")]
    if len(roots) != 1:
        return None, [], None
    usage = summarize_session_usage(snapshot, roots[0], capture_complete=False)
    calls = session_tool_calls(snapshot, {session["session_id"] for session in usage["sessions"]})
    return roots[0], calls, usage


def evaluate(case: dict, row: dict) -> dict:
    root_id, calls, usage = session_evidence(row)
    surface, native = row.get("surface", {}), row.get("native_surface", {})
    unchanged = bool(surface) and all(surface.get(key) == native.get(key) for key in ("guidance", "instructions", "tools"))
    log = events(Path(row["evidence"]) / "agent/model-requests.jsonl")
    first = next((event for event in log if event["type"] == "request" and event["body"].get("tools")), None)
    verified = [event for event in log if event["type"] == "guidance_verified"]
    context_correct = bool(first and verified and verified[0].get("first_task_request")) and not row.get("context_errors")
    rpc_times = {event["sequence"]: event["time"] for event in events(Path(row["evidence"]) / "agent/mcp-execution.jsonl") if event["type"] == "request"}
    agent_rpc = [call for call in row.get("mcp", {}).get("calls", []) if first and rpc_times.get(call["sequence"], 0) >= first["time"]]
    matching = [call for call in calls if call["name"].endswith("zvec_grep_" + case["tool"]) and call.get("input") == case["arguments"]]
    actor_rows = {}
    for actor in ("main", "child"):
        selected = [call for call in matching if (call["session_id"] == root_id) == (actor == "main")]
        checks = []
        for call in selected:
            source = useful_output(case["tool"], call.get("output") or "")
            # OpenCode's state timestamp can lag the proxy's send timestamp by
            # one millisecond. Also require exact parameters, matching text and
            # a unique RPC candidate; timestamps alone cannot assign an actor.
            wire = [rpc["sequence"] for rpc in agent_rpc if rpc["name"].endswith("zvec_grep_" + case["tool"]) and rpc["arguments"] == case["arguments"] and rpc["status"] == "succeeded"
                    and rpc.get("text", "").strip() == (call.get("output") or "").strip()
                    and (call.get("started_at") or 0) - 5 <= rpc_times[rpc["sequence"]] <= (call.get("finished_at") or 0)]
            if len(wire) != 1:
                wire = []
            checks.append({"call_id": call["call_id"], "session_id": call["session_id"], "status": call["status"], "source": source, "mcp_sequences": wire})
        actor_rows[actor] = {
            "executed_successfully": bool(root_id and any(check["status"] == "completed" and check["mcp_sequences"] for check in checks)),
            "contains_useful_information": bool(root_id and any(check["status"] == "completed" and check["source"]["contains_useful_information"] and check["mcp_sequences"] for check in checks)),
            "passed": bool(root_id and any(check["status"] == "completed" and check["source"]["correct_and_useful"] and check["mcp_sequences"] for check in checks)),
            "calls": checks}
    target_attempts = [attempt for attempt in row.get("model_attempts", []) if attempt["name"].endswith("zvec_grep_" + case["tool"])]
    result = {"case": case, "session_status": row["status"], "root_session_id": root_id,
              "published_guidance_schema_unchanged": unchanged, "actual_context_verified": context_correct,
              "actors": actor_rows, "model_attempts": target_attempts,
              "invalid_json_attempts": sum(not attempt["valid_json"] for attempt in target_attempts),
              "invalid_schema_attempts": sum(not attempt.get("schema_valid", False) for attempt in target_attempts),
              "agent_mcp_calls": agent_rpc, "guard_stops": row.get("guard_stops", []), "usage": usage,
              "evidence": row["evidence"]}
    result["passed"] = unchanged and context_correct and row["status"] == "completed" and all(actor["passed"] for actor in actor_rows.values()) and not result["invalid_schema_attempts"]
    return result


def render(results: list[dict]) -> str:
    lines = ["# 0.3.1 最小参数与可选参数验收", "", "每项1个独立 OpenCode 会话；全部保留发布版三个工具、schema及指导。未运行QA20和评分。", "",
             "| 项目 | 主Agent执行成功 | 子代理执行成功 | 返回全部正确 | 会话完成 | 指导/schema原样保留 | 参数错误尝试 | 验收 |", "|---|---|---|---|---|---|---:|---|"]
    for result in results:
        row = [result["case"]["id"], *[str(result["actors"][actor]["executed_successfully"]) for actor in ("main", "child")], str(all(result["actors"][actor]["passed"] for actor in ("main", "child"))), result["session_status"],
               str(result["published_guidance_schema_unchanged"]), str(result["invalid_schema_attempts"]), str(result["passed"])]
        lines.append("| " + " | ".join(row) + " |")
    lines += ["", "有效调用要求：参数与该项设定完全一致，实际到达MCP并成功，返回结果与独立fixture源码相符。", "",
              "主会话和子代理分别按真实session ID识别。成功的直接探针不计入Agent调用；失败和不完整会话保留。", "",
              "原始请求、SSE、MCP请求/响应、会话树、参数和源码核验见 cases/ 与 report.json。"]
    return "\n".join(lines) + "\n"


def run(args) -> None:
    manifest_path, output = args.release.resolve(), args.output.resolve()
    manifest = verify_release(manifest_path)
    model = "custom-openai/" + args.model
    resolved = resolve_opencode_model(model, require_credentials=True).configuration
    output.mkdir(parents=True, exist_ok=True)
    baseline = baseline_protocol(args.baseline.resolve(), manifest) if args.baseline else None
    if args.resume_completed and not baseline:
        raise ValueError("Resuming requires a verified baseline archive")
    resumed = resumable_cases(*baseline, model) if args.resume_completed else []
    protocol = {"protocol": "release-0.3.1-chain-check-v2", "release": manifest,
                "harness": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
                "model": model, "endpoint_sha256": hashlib.sha256(resolved.base_url.encode()).hexdigest(),
                "error_guard_version": 1,
                "generation": {"temperature": 0, "seed": 42, "enable_thinking": True, "reasoning_effort": "high", "max_tokens": 32000},
                "baseline_harness": baseline[1]["harness"] if baseline else None,
                "resumed_cases": resumed,
                "cases": CASES, "formal_trials": 0, "sessions_per_case": 1, "request_limit": 12, "agent_timeout_seconds": 300,
                "fixture_sha256": sha256(ROOT / "fixtures/caller-callee-dev/lib.rs")}
    path = output / "protocol.json"
    if path.exists() and json.loads(path.read_text()) != protocol:
        raise ValueError("Chain check protocol changed between phases")
    write_json(path, protocol)
    seeds = output / "index-snapshots"
    if baseline and args.phase == "minimal":
        if seeds.exists():
            raise ValueError("Refusing to overwrite existing index snapshots")
        shutil.copytree(baseline[0] / "index-snapshots", seeds)
        write_json(output / "baseline-protocol.json", baseline[1])
        for case_id in resumed:
            shutil.copytree(baseline[0] / "cases" / case_id, output / "cases" / case_id)
    seeds.mkdir(exist_ok=True)
    os.environ["ZG_BENCH_INDEX_SEED_DIR"] = str(seeds)
    overlay = prepare_overlay(output, "agent", 0, str(output))
    try:
        for case in CASES:
            if case["phase"] != args.phase:
                continue
            root = output / "cases" / case["id"]
            if case["id"] in resumed:
                continue
            if root.exists():
                raise ValueError("Refusing to rerun or overwrite a diagnostic case")
            root.mkdir(parents=True)
            suite = make_case(root, case)
            rows = run_job(suite, manifest=manifest_path, output=root, group="agent", number=0,
                           task="release-dev", overlay=overlay, smoke=True, request_limit=12, model=model)
            write_json(root / "case-report.json", evaluate(case, rows[0]))
        results = [json.loads(path.read_text()) for path in sorted((output / "cases").glob("*/case-report.json"))]
        write_json(output / "report.json", {"protocol": protocol, "results": results, "formal_trials": 0})
        (output / "RESULT.md").write_text(render(results))
    finally:
        write_json(output / "evidence-sha256.json", evidence_manifest(output))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--phase", choices=("minimal", "optional"), required=True)
    parser.add_argument("--model", choices=("glm-5.2", "qwen3.8-max"), default="glm-5.2")
    parser.add_argument("--baseline", type=Path, help="Downloaded baseline artifact; restore its verified fixture index")
    parser.add_argument("--resume-completed", action="store_true", help="Preserve completed same-model sessions; retry preparation failures only")
    run(parser.parse_args())


if __name__ == "__main__":
    main()
