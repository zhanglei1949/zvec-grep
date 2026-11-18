"""Pinned fixture evidence; known product defects never rewrite tool output."""
from __future__ import annotations

import json
import re
from pathlib import Path

from .release_analysis import events
from .release_packages import sha256

ROOT = Path(__file__).resolve().parents[1]
KNOWN_CALLER_ISSUE = "https://github.com/zvec-ai/zvec-grep/issues/434"
KNOWN_FIXTURE_SHA256 = "66bb356637fe71154d1e39ce1b7012428c5658ea9ccb0d2e629df75b1efa3d03"


def smoke_arguments(group: str) -> dict[str, dict]:
    tools = {"search": {"root": "/app", "fts": "normalize", "globs": "lib.rs"},
             "callers": {"root": "/app", "symbol": "::save", "globs": "lib.rs", "limit": 20},
             "callees": {"root": "/app", "symbol": "::save", "globs": "lib.rs", "limit": 20}}
    selected = {"search": ("search",), "agent": ("search", "callers", "callees"),
                "relationships": ("callers", "callees")}[group]
    return {name: tools[name] for name in selected}


def useful_output(tool: str, output: str) -> dict:
    source = (ROOT / "fixtures/caller-callee-dev/lib.rs").read_text().splitlines()
    if tool == "search":
        correct = "lib.rs:2" in output and "normalize" in output and "value.max(0)" in output
        return {"correct_and_useful": correct, "contains_useful_information": correct,
                "expected": "normalization definition at lib.rs:2, including max(0)"}
    actual = {(int(line), int(column)) for line, column in re.findall(r"call at lib\.rs:(\d+):(\d+)", output)}
    if tool == "callers":
        expected = {(index, line.index("save(") + 1) for index, line in enumerate(source, 1)
                    if line.startswith(("pub fn import_record", "pub fn export_record"))}
        endpoints = all(re.search(rf"\b{name}\s+lib\.rs:{line}\b", output)
                        for name, line in (("import_record", 6), ("export_record", 8)))
    else:
        expected = {(index, line.index("normalize(") + 1) for index, line in enumerate(source, 1)
                    if line.startswith("pub fn save")}
        endpoints = bool(re.search(r"\bnormalize\s+lib\.rs:2\b", output))
    matched = bool(re.search(r"^(?:::)?save\s+lib\.rs:4\b", output))
    return {"correct_and_useful": bool(matched and endpoints and actual == expected),
            "contains_useful_information": bool(matched and endpoints and actual & expected),
            "matched_definition_correct": matched, "expected_call_sites": sorted(expected),
            "actual_call_sites": sorted(actual), "correct_call_sites": sorted(actual & expected),
            "unexpected_call_sites": sorted(actual - expected), "missing_call_sites": sorted(expected - actual)}


def verified_fixture_baseline(directory: Path, manifest: dict) -> tuple[Path, dict]:
    paths = [path for path in directory.rglob("protocol.json")
             if json.loads(path.read_text()).get("protocol", "").startswith("release-0.3.1-chain-check-")]
    if len(paths) != 1:
        raise ValueError("Expected exactly one baseline chain protocol")
    root, protocol = paths[0].parent, json.loads(paths[0].read_text())
    if protocol["fixture_sha256"] != sha256(ROOT / "fixtures/caller-callee-dev/lib.rs") or protocol["release"] != manifest:
        raise ValueError("Baseline fixture or official release differ")
    checks = list((root / "index-snapshots").glob("*/payload-sha256.json"))
    if len(checks) != 1:
        raise ValueError("Expected one baseline fixture index snapshot")
    workspace = checks[0].parent / "workspace"
    actual = {str(path.relative_to(workspace)): sha256(path) for path in sorted(workspace.rglob("*")) if path.is_file()}
    if not actual or actual != json.loads(checks[0].read_text()):
        raise ValueError("Baseline index snapshot differs from its recorded hashes")
    return root, protocol


def smoke_review(row: dict) -> dict:
    errors, defects, checks = [], [], []
    if row["status"] != "completed":
        errors.append("technical_session_incomplete")
    attempts = row.get("model_attempts", [])
    if any(not call["valid_json"] for call in attempts):
        errors.append("model_emitted_invalid_arguments")
    if any(call["name"] == "invalid" or not call.get("schema_valid") for call in attempts):
        errors.append("model_arguments_failed_published_schema")
    if row.get("context_errors"):
        errors.append("guidance_or_schema_rejected")
    agent = Path(row.get("evidence", "/missing")) / "agent"
    first = next((event for event in events(agent / "model-requests.jsonl")
                  if event["type"] == "request" and event["body"].get("tools")), None)
    times = {event["sequence"]: event["time"] for event in events(agent / "mcp-execution.jsonl") if event["type"] == "request"}
    root = row.get("session_usage", {}).get("root_session_id")
    for tool, arguments in smoke_arguments(row["group"]).items():
        if not any(call["name"].endswith("zvec_grep_" + tool) for call in attempts):
            errors.append(f"{tool}_raw_model_attempt_missing")
        for actor in ("main", "child"):
            selected = [call for call in row.get("calls", [])
                        if call["name"].endswith("zvec_grep_" + tool) and call["status"] == "completed"
                        and call.get("input") == arguments and (call["session_id"] == root) == (actor == "main")]
            linked = False
            for call in selected:
                text = call.get("output") or ""
                wire = [rpc for rpc in row.get("mcp", {}).get("calls", [])
                        if first and times.get(rpc["sequence"], 0) >= first["time"]
                        and rpc["name"].endswith("zvec_grep_" + tool) and rpc["status"] == "succeeded"
                        and rpc["arguments"] == arguments and rpc.get("text", "").strip() == text.strip()
                        and (call.get("started_at") or 0) - 5 <= times[rpc["sequence"]] <= (call.get("finished_at") or 0)]
                if not root or len(wire) != 1:
                    continue
                linked = True
                source = useful_output(tool, text)
                record = {"tool": tool, "actor": actor, "session_id": call["session_id"], "call_id": call["call_id"],
                          "mcp_sequence": wire[0]["sequence"], "arguments": arguments, "source": source}
                checks.append(record)
                if not source["correct_and_useful"]:
                    known = (tool == "callers" and source["contains_useful_information"] and not source["missing_call_sites"]
                             and source["unexpected_call_sites"] == [(16, 39)]
                             and sha256(ROOT / "fixtures/caller-callee-dev/lib.rs") == KNOWN_FIXTURE_SHA256
                             and bool(re.search(r"storage::flush\s+lib\.rs:16\b", text)))
                    if known:
                        defects.append({**record, "issue": KNOWN_CALLER_ISSUE, "accepted_as_known_product_defect": True})
                    else:
                        errors.append(f"{tool}_fixture_locations_differ_from_source")
            if not linked:
                errors.append(f"{tool}_not_executed_by_{actor}")
    if not row.get("surface", {}).get("preflight_calls"):
        errors.append("native_mcp_preflight_missing")
    if {call["name"] for call in row.get("native_surface", {}).get("preflight_calls", [])} != {"zvec_grep_search", "zvec_grep_callers", "zvec_grep_callees"}:
        errors.append("three_tool_native_preflight_missing")
    return {"technical_passed": not errors, "errors": sorted(set(errors)), "known_quality_defects": defects,
            "all_returned_locations_correct": bool(checks) and not defects and all(c["source"]["correct_and_useful"] for c in checks),
            "calls": checks}
