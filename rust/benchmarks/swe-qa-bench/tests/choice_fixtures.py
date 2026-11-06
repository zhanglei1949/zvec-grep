"""Synthetic trial artifacts for offline plumbing tests, not benchmark questions."""
from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
from pathlib import Path

from zg_bench.swe_qa import augment_protocol as old
from zg_bench.swe_qa.augment import package_identity
from zg_bench.swe_qa.choice_protocol import *
from zg_bench.swe_qa.choice_report import contract_hash


def jsonl(path, rows):
    path.write_text("".join(json.dumps(row) + "\n" for row in rows))


def catalogs():
    raw = read_json(ROOT / "tests/fixtures/mcp-agent-tools-graphdb.json")
    script = """
import { publicTool as automatic } from './zg_bench/swe_qa/choice_proxy.mjs';
import { publicTool as fixed } from './zg_bench/swe_qa/augment_proxy.mjs';
let s=''; for await(const c of process.stdin) s+=c;
const v=JSON.parse(s); process.stdout.write(JSON.stringify({automatic:v.tools.map(automatic), fixed:[fixed(v.tools.find(t=>t.name==='zvec_grep_search'))]}));
"""
    projected = json.loads(subprocess.run(["node", "--input-type=module", "-e", script], cwd=ROOT,
        input=json.dumps(raw), text=True, capture_output=True, check=True).stdout)
    return raw, projected


def discovery(profile, raw, projected):
    if profile == AUTO:
        instructions = raw["instructions"] + "\n" + READONLY_INSTRUCTIONS
        return [
            {"event": "start", "config": {"profile": AUTO, "instructions": READONLY_INSTRUCTIONS}},
            {"event": "instructions", "source": raw["instructions"], "instructions": instructions},
            {"event": "tools", "source_tools": raw["tools"], "tools": projected["automatic"], "instructions": instructions},
        ]
    return [
        {"event": "start", "config": {"profile": profile, "graph": GRAPH, "graph_execution": EXECUTION, "instructions": old.INSTRUCTIONS}},
        {"event": "tools", "tools": projected["fixed"], "instructions": old.INSTRUCTIONS},
    ]


def response(name, view, *, status="complete", ambiguous=False, cursor=None):
    if name == "zvec_grep_explore":
        value = {"view": view, "nodes": [], "relations": [], "paths": [], "references": [], "evidence": [],
                 "locations": [{"status": "ambiguous", "candidates": []}] if ambiguous else [],
                 "coverage": {"domain": "code", "ready": True, "note": "fixture"},
                 "execution": {"status": status, "stop_reason": "graph_budget" if status == "partial" else None, "usage": {"nodes": 1}}}
        if cursor:
            value["next_cursor"] = cursor
        return {"id": 3, "result": {"structuredContent": value, "content": [{"type": "text", "text": json.dumps(value)}]}}
    value = {"status": status, "view": view, "evidence": [], "nodes": [], "relations": []}
    return {"id": 3, "result": {"content": [{"type": "text", "text": "ordinary\nGraph\n```json\n" + json.dumps(value) + "\n```"}]}}


def event_call(profile, args, view, *, name="zvec_grep_search", status="complete", ambiguous=False, cursor=None):
    original = {"id": 3, "method": "tools/call", "params": {"name": name, "arguments": args}}
    forwarded = json.loads(json.dumps(original))
    if name == "zvec_grep_search":
        forwarded["params"]["arguments"].update(autoUpdate=False, freshness="eventual")
        if profile != AUTO:
            forwarded["params"]["arguments"]["graph"] = {"mode": "off"} if profile == "zg-off" else GRAPH
            if profile == "zg-augment":
                forwarded["params"]["arguments"]["graphExecution"] = EXECUTION
    reply = response(name, view, status=status, ambiguous=ambiguous, cursor=cursor)
    is_off = name == "zvec_grep_search" and forwarded["params"]["arguments"].get("graph", {"mode": "off"}) == {"mode": "off"}
    if is_off:
        reply = {"id": 3, "result": {"content": [{"type": "text", "text": "ordinary"}]}}
    request = {"event": "request", "original": original, "forwarded": forwarded} if profile == AUTO else {"event": "request", "request": forwarded}
    observed = {"status": "absent" if is_off else status, "evidence": 0}
    if not is_off:
        observed["view"] = view
    return [request, {"event": "response", "request": forwarded, "response": reply, "elapsed_ms": 5, "graph": observed}]


def fixture(experiment: Path, *, candidate: Path | None = None):
    raw, projected = catalogs()
    if candidate is None:
        (experiment / "candidate.tgz").write_bytes(b"offline-package-fixture")
        identity = {"sha256": digest(experiment / "candidate.tgz")}
    else:
        shutil.copyfile(candidate, experiment / "candidate.tgz")
        identity = package_identity(experiment / "candidate.tgz")
    manifest = {**configuration(), "tier": "smoke", "matrix": matrix("smoke"),
                "candidate": identity,
                "assets": {p: digest(DATA / p) for p in ("selection.json", "references.json")},
                "harness": {"zg_bench/swe_qa/choice_protocol.py": digest(ROOT / "zg_bench/swe_qa/choice_protocol.py")},
                "lock_sha256": digest(ROOT / "uv.lock")}
    write_json(experiment / "manifest.json", manifest)
    for index, row in enumerate(manifest["matrix"]):
        agent = experiment / "jobs" / row["job"] / "trial" / "agent"
        agent.mkdir(parents=True)
        view = {"generation": row["repository"], "text_revision": 1, "graph": {"build_id": "build"}}
        identity = {"protocol": PROTOCOL, "commit": row["commit"], "package": manifest["candidate"]["sha256"],
                    "embedding": manifest["embedding"], "endpoint": manifest["embedding_endpoint"],
                    "platform": "linux/amd64", "workdir": "/app", "graph_options": {"scope": "code"}}
        key = hashlib.sha256(json.dumps(identity, separators=(",", ":"), sort_keys=True).encode()).hexdigest()
        catalog = discovery(AUTO, raw, projected)
        contract = {k: catalog[-1][k] for k in ("tools", "instructions")}
        write_json(agent / "zvec-grep-setup.json", {
            "status": "ready", "choice_profile": row["profile"], "protocol": PROTOCOL,
            "package_sha256": manifest["candidate"]["sha256"], "embedding_model": manifest["embedding"],
            "graph_seed_key": key, "index_seed_identity": identity, "index_inventory": {"text": "hash", "graph": "hash"},
            "graph_preflight": {"ready": True}, "catalog_preflight": {"ready": True, "contract_hash": contract_hash(contract)},
        })
        write_json(agent / "choice-index-after.json", {"text": "hash", "graph": "hash"})
        timing = {"started_at": "2026-11-03T00:00:00+00:00", "finished_at": "2026-11-03T00:00:04+00:00"}
        write_json(agent / "choice-timing.json", timing)
        preflight = discovery("zg-augment", raw, projected) + event_call("zg-augment", {"root": "/app", "fts": ["def"]}, view)
        jsonl(agent / "choice-preflight.jsonl", preflight)
        jsonl(agent / "choice-catalog.jsonl", catalog)
        args, name = {"root": "/app", "query": "implementation"}, "zvec_grep_search"
        if row["profile"] == AUTO:
            task_index = next(i for i, t in enumerate(tasks("smoke")) if t["task_id"] == row["task_id"])
            if task_index == 1:
                args["graph"] = {"mode": "augment", "expansion": {"direction": "incoming", "max_hops": 1}}
            elif task_index == 2:
                args["graph"] = {"mode": "constrain", "scope": {"anchor": {"by": "name", "name": "f"}}}
            elif task_index >= 3:
                name = "zvec_grep_explore"
                args = {"root": "/app", "request": {"domain": "code", "operation": {"operation": "neighbors", "anchor": {"by": "name", "name": "f"}, "relations": ["calls"], "direction": "incoming"}}}
                if task_index == 4:
                    args["request"]["cursor"] = "page-1"
        events = discovery(row["profile"], raw, projected) + event_call(row["profile"], args, view, name=name)
        jsonl(agent / "choice-mcp.jsonl", events)
        write_json(agent / "trajectory.json", {"steps": [
            {"source": "agent", "tool_calls": [{"function_name": name, "arguments": args, "tool_call_id": "one"}, {"function_name": "read", "arguments": {}, "tool_call_id": "two"}]},
            {"source": "agent", "message": "Answer", "tool_calls": []},
        ]})
        write_json(agent.parent / "result.json", {"task_name": row["task_slug"], "finished_at": timing["finished_at"],
            "exception_info": None, "agent_execution": timing, "agent_result": {"n_input_tokens": 30, "n_output_tokens": 10},
            "agent_info": {"name": "opencode", "version": "1.18.4", "model_info": {"name": "qwen3.7-max", "provider": "dashscope"}}})
        jsonl(agent / "opencode.txt", [{"type": "step_finish", "part": {"reason": "stop", "tokens": {"input": 25, "output": 10, "cache": {"read": 5, "write": 2}}}}])
        jsonl(agent / "choice-provider-usage.jsonl", [{"event": "request", "id": 1, "model": "qwen3.7-max"},
            {"event": "response", "id": 1, "status": 200, "usage": {"prompt_tokens": 32, "completion_tokens": 10}}])
    return manifest


def scores(experiment, collection):
    (experiment / "scores").mkdir()
    for row in collection["trials"]:
        points = {"zg-off": 10, "zg-augment": 12, AUTO: 14}[row["profile"]]
        write_json(experiment / "scores" / (row["id"] + ".json"), {
            "id": row["id"], "answer_sha256": hashlib.sha256(row["answer"].encode()).hexdigest(), "status": "scored",
            "judge": {"model": "glm-5.2", "self_judge": False, "total": points * 5,
                      "scores": {k: points for k in ("correctness", "completeness", "relevance", "clarity", "coherence")},
                      "latency_seconds": 1, "usage": {"input_tokens": 10, "output_tokens": 5}},
        })
