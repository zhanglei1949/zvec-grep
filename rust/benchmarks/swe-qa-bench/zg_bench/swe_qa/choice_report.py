"""Evidence collection and task-paired reporting for the independent three-arm protocol."""
from __future__ import annotations

import hashlib
import json
import os
import random
import re
import statistics
from copy import deepcopy
from datetime import datetime
from pathlib import Path

from . import SweQaError
from . import augment_report as legacy
from .augment_protocol import INSTRUCTIONS as FIXED_INSTRUCTIONS
from .choice_protocol import (AUTO, COMPARISONS, DATA, EXECUTION, GRAPH, PROFILES, PROTOCOL,
    READONLY_INSTRUCTIONS, ROOT, SEED, configuration, digest, matrix, read_json, tasks, write_json)
from .collect import _message_text
from .judge import SCORE_KEYS, _default_completion, _judge_candidate

METRICS = legacy.METRICS
number, json_lines = legacy.number, legacy.json_lines
opencode_usage, provider_usage = legacy.opencode_usage, legacy.provider_usage
bootstrap, percentile = legacy.bootstrap, legacy.percentile
SEARCH, EXPLORE = "zvec_grep_search", "zvec_grep_explore"


def contract_hash(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()


def _safe_file(root, name):
    path = (root / name).resolve()
    if not path.is_relative_to(root.resolve()):
        raise ValueError("artifact path escapes experiment")
    return path


def checked_manifest(experiment):
    manifest = read_json(experiment / "manifest.json")
    if manifest.get("protocol") != PROTOCOL or manifest.get("matrix") != matrix(manifest.get("tier")):
        raise ValueError("unknown protocol or changed call matrix")
    if manifest.get("profiles") != list(PROFILES):
        raise ValueError("changed experiment profiles")
    if digest(experiment / "candidate.tgz") != manifest["candidate"]["sha256"]:
        raise ValueError("candidate package changed")
    if set(manifest["assets"]) != {"selection.json", "references.json"}:
        raise ValueError("missing frozen assets")
    for name, expected in manifest["assets"].items():
        if digest(DATA / name) != expected:
            raise ValueError("benchmark assets changed")
    if not manifest.get("harness"):
        raise ValueError("missing frozen harness inventory")
    for name, expected in manifest["harness"].items():
        if digest(_safe_file(ROOT, name)) != expected:
            raise ValueError(f"benchmark harness changed: {name}")
    if manifest.get("lock_sha256") != digest(ROOT / "uv.lock"):
        raise ValueError("benchmark dependency lock changed")
    return manifest


def _score_evidence(experiment):
    return {path.relative_to(experiment).as_posix(): digest(_safe_file(experiment, path.relative_to(experiment)))
            for path in sorted((experiment / "scores").glob("*.json"))}


def _verify_trial_evidence(experiment, records):
    for row in records:
        for path, expected_hash in row.get("evidence", {}).items():
            if digest(_safe_file(experiment, path)) != expected_hash:
                raise ValueError(f"trial evidence changed: {path}")


def verify_report_evidence(experiment, accepted):
    """Recheck an accepted report's inputs without running trials or judging again."""
    collection_path = experiment / "collection.json"
    if accepted.get("collection_sha256") != digest(collection_path):
        raise ValueError("collection changed since accepted report")
    collection = read_json(collection_path)
    if collection.get("protocol") != PROTOCOL or collection.get("manifest_sha256") != accepted["manifest_sha256"]:
        raise ValueError("collection identity mismatch")
    _verify_trial_evidence(experiment, collection["trials"])
    if accepted.get("score_evidence") != _score_evidence(experiment):
        raise ValueError("scores changed since accepted report")


def _observe(name, response, operation=None):
    if response.get("error") or response.get("result", {}).get("isError"):
        return {"status": "error", "evidence": 0}
    result = response.get("result", {})
    text = "\n".join(p["text"] for p in result.get("content", []) if p.get("type") == "text")
    try:
        if name == SEARCH:
            marker = "\nGraph\n```json\n"
            if marker not in text:
                return {"status": "absent", "evidence": 0}
            if not text.endswith("\n```"):
                return {"status": "invalid", "evidence": 0}
            value = json.loads(text.rsplit(marker, 1)[1][:-4])
            status = value.get("status")
        else:
            value = result.get("structuredContent")
            if value is None:
                value = json.loads(text)
            status = value.get("execution", {}).get("status")
            if status not in ("complete", "partial"):
                return {"status": "invalid", "evidence": 0}
        if (status not in ("complete", "partial", "degraded") or not isinstance(value.get("evidence"), list)
            or (status != "degraded" and not value.get("view"))):
            return {"status": "invalid", "evidence": 0}
        execution = value.get("execution", {})
        if execution is None and status == "degraded":
            execution = {}
        if not isinstance(execution, dict):
            return {"status": "invalid", "evidence": 0}
        has_results = any(value.get(k) for k in ("nodes", "relations", "paths", "references", "relation_types", "semantic"))
        if operation == "locate":
            has_results = has_results or any(location.get("candidates") for location in value.get("locations", []))
        return {"status": status, "evidence": len(value["evidence"]), "view": value.get("view"),
                "stop_reason": execution.get("stop_reason", value.get("reason")),
                "next_cursor": value.get("next_cursor"), "usage": execution.get("usage"),
                "diagnostics": value.get("diagnostics", []),
                "locations": value.get("locations", []),
                "empty": len(value["evidence"]) == 0 if name == SEARCH else not has_results}
    except (ValueError, TypeError, KeyError, AttributeError):
        return {"status": "invalid", "evidence": 0}


def _arguments(call):
    args = call.get("arguments", {})
    return json.loads(args) if isinstance(args, str) else args


def _selection(request):
    params = request.get("params", {})
    args = params.get("arguments", {})
    name = params.get("name")
    args = args if isinstance(args, dict) else {}
    traversal = {}
    if name == EXPLORE:
        req = args.get("request") if isinstance(args.get("request"), dict) else {}
        operation = req.get("operation") if isinstance(req.get("operation"), dict) else {}
        mode = "explore"
        traversal = operation.get("traversal", operation)
    else:
        operation = {}
        graph = args.get("graph", {"mode": "off"})
        graph = graph if isinstance(graph, dict) else {"mode": "invalid"}
        mode = graph.get("mode", "invalid")
        scope = graph.get("scope") if isinstance(graph.get("scope"), dict) else {}
        traversal = graph.get("expansion", scope.get("traversal", {}))
    traversal = traversal if isinstance(traversal, dict) else {}
    req = args.get("request") if isinstance(args.get("request"), dict) else {}
    return {"tool": name, "mode": mode, "operation": operation.get("operation"),
            "relations": traversal.get("relations"), "direction": traversal.get("direction"),
            "min_hops": traversal.get("min_hops"), "max_hops": traversal.get("max_hops"),
            "scope": args.get("graph", {}).get("scope") if isinstance(args.get("graph"), dict) else None,
            "boundary": req.get("boundary", traversal.get("boundary")),
            "result_filter": req.get("result_filter", traversal.get("result_filter")),
            "budget": args.get("execution", args.get("graphExecution")),
            "cursor": req.get("cursor"), "page_size": req.get("page_size"),
            "relation_filters": req.get("relation_filters")}


def audit_mcp(events, profile):
    issues, tools, calls, pending, seen, cancelled = [], [], [], {}, set(), set()
    starts, instruction_event = 0, None
    if profile != AUTO:
        # Preserve legacy treatment checks; parameter rejections remain observed Agent errors.
        fixed = legacy.audit_mcp([e for e in events if e["event"] != "rejected"], profile)
        issues.extend(fixed["issues"])
    for event in events:
        kind = event["event"]
        if kind == "start":
            starts += 1
            cfg = event["config"]
            if cfg.get("profile") != profile:
                issues.append("proxy profile drift")
            if profile == AUTO and cfg.get("instructions") != READONLY_INSTRUCTIONS:
                issues.append("readonly guidance drift")
        elif kind == "instructions" and profile == AUTO:
            instruction_event = event
            expected = "\n".join(s for s in (event.get("source", ""), READONLY_INSTRUCTIONS) if s)
            if event.get("instructions") != expected:
                issues.append("product guidance overridden")
        elif kind == "tools":
            tools.append({"tools": event["tools"], "instructions": event["instructions"]})
            if profile == AUTO:
                names = {t["name"] for t in event["tools"]}
                if names != {SEARCH, EXPLORE} or instruction_event is None or event["instructions"] != instruction_event["instructions"]:
                    issues.append("incomplete autonomous tool discovery")
                source = {t["name"]: t for t in event.get("source_tools", [])}
                for tool in event["tools"]:
                    original = source.get(tool["name"], {})
                    if tool.get("description") != original.get("description"):
                        issues.append("product tool description overridden")
                    for field, schema in original.get("inputSchema", {}).get("properties", {}).items():
                        if field not in ("autoUpdate", "freshness", "apiKey") and tool["inputSchema"]["properties"].get(field) != schema:
                            issues.append("product input schema overridden")
                    for name, definition in tool.get("inputSchema", {}).get("$defs", {}).items():
                        if original.get("inputSchema", {}).get("$defs", {}).get(name) != definition:
                            issues.append("product input definition overridden")
        elif kind in ("request", "rejected"):
            original = event.get("original", event.get("request"))
            forwarded = event.get("forwarded", event.get("request")) if kind == "request" else None
            request = forwarded or original
            key = request["id"]
            if key in seen:
                issues.append("duplicate MCP request ID")
            seen.add(key)
            call = {"request_id": key, **_selection(request), "original": original if profile == AUTO or kind == "rejected" else None,
                    "forwarded": forwarded, "response": event.get("response"), "elapsed_ms": None,
                    "graph": {"status": "error" if kind == "rejected" else "pending", "evidence": 0}}
            calls.append(call)
            if profile == AUTO and forwarded:
                expected = deepcopy(original)
                if request["params"]["name"] == SEARCH:
                    expected["params"]["arguments"].update(autoUpdate=False, freshness="eventual")
                if expected != forwarded:
                    issues.append("Agent parameters changed by proxy")
            if kind == "request":
                pending[key] = call
            else:
                if event["response"].get("id") != key:
                    issues.append("unpaired MCP response")
                if original["params"].get("name") not in ({SEARCH, EXPLORE} if profile == AUTO else {SEARCH}):
                    issues.append("unexposed tool invoked")
        elif kind == "response":
            request = event["request"]
            call = pending.pop(request["id"], None)
            if call is None or call["forwarded"] != request or event["response"].get("id") != request["id"]:
                issues.append("unpaired MCP response")
                continue
            call.update(response=event["response"], elapsed_ms=event["elapsed_ms"], graph=_observe(call["tool"], event["response"], call["operation"]))
            if call["graph"]["status"] == "invalid":
                issues.append("invalid graph response")
            if call["mode"] in ("augment", "constrain") and call["graph"]["status"] == "absent":
                issues.append("requested graph result absent")
            if call["mode"] == "off" and call["graph"]["status"] not in ("absent", "error"):
                issues.append("graph result in ordinary retrieval")
        elif kind == "cancel":
            cancelled.add(event["request_id"])
        elif kind == "backend_error":
            issues.append("MCP backend failure")
    for key, call in pending.items():
        if key in cancelled:
            call["graph"]["status"] = "cancelled"
        else:
            issues.append("missing MCP response")
    for call in calls:
        call["cancelled"] = call["request_id"] in cancelled
    if starts != 1:
        issues.append("missing or duplicate proxy start")
    if not tools:
        issues.append("missing Agent tool discovery")
    counts = {mode: sum(c["mode"] == mode for c in calls) for mode in ("off", "augment", "constrain", "explore")}
    return {"issues": issues, "tools": tools, "calls": calls, "requests": len(calls), "modes": counts,
            "mode_switches": sum(a["mode"] != b["mode"] for a, b in zip(calls, calls[1:])),
            "errors": sum(c["graph"]["status"] == "error" for c in calls),
            "partial": sum(c["graph"]["status"] == "partial" for c in calls),
            "degraded": sum(c["graph"]["status"] == "degraded" for c in calls),
            "with_evidence": sum(c["graph"]["evidence"] > 0 for c in calls)}


def bind_trajectory(record, trajectory_calls):
    remaining = [c for c in trajectory_calls if any(c["function_name"].endswith(t) for t in (SEARCH, EXPLORE))]
    for wire in record["mcp"]["calls"]:
        for index, call in enumerate(remaining):
            if not call["function_name"].endswith(wire["tool"]):
                continue
            raw = _arguments(call)
            expected = deepcopy(raw)
            if wire["forwarded"] and isinstance(expected, dict):
                if record["profile"] != AUTO:
                    expected.update(autoUpdate=False, freshness="eventual", graph={"mode": "off"} if record["profile"] == "zg-off" else GRAPH)
                    if record["profile"] == "zg-augment":
                        expected["graphExecution"] = EXECUTION
                elif wire["tool"] == SEARCH:
                    expected.update(autoUpdate=False, freshness="eventual")
            actual = (wire["forwarded"] or wire["original"])["params"]["arguments"]
            if expected == actual:
                wire["original"] = {"tool_call_id": call.get("tool_call_id"), "params": {"name": wire["tool"], "arguments": raw}}
                remaining.pop(index)
                break
        else:
            record["issues"].append("trajectory / MCP arguments mismatch")
        view = wire["graph"].get("view")
        if view is not None and view != record["expected_view"]:
            record["issues"].append("graph/text view drift")
    if remaining:
        record["issues"].append("unmatched trajectory calls")

def empty_trial(row: dict) -> dict:
    return {**row, "id": row["job"], "status": "missing", "answer": "", "issues": [],
            "metrics": dict.fromkeys(METRICS), "usage": None, "mcp": None, "evidence": {}}


def _collect_usage(trial, record):
    """Keep observed costs even when another trial artifact cannot be read."""
    record["usage_complete"] = False
    for key, filename, parse in (("usage", "opencode.txt", opencode_usage),
                                 ("provider_usage", "choice-provider-usage.jsonl", provider_usage)):
        path = trial / "agent" / filename
        if key == "usage" and not path.is_file():
            continue
        try:
            value = parse(path)
            record[key] = value
            # Provider totals are authoritative; OpenCode is the observed fallback.
            record["metrics"].update({name: value[name] for name in ("input_tokens", "output_tokens", "total_tokens")})
        except (ValueError, OSError, KeyError, TypeError, AttributeError) as error:
            record["status"] = "invalid"
            record["issues"].append(str(error))


def _reconcile_usage(result, record):
    usage, provider = record.get("usage"), record.get("provider_usage")
    context = result.get("agent_result") or {}
    harbor_matches = usage is not None and context.get("n_input_tokens") == usage["harbor_input_tokens"] and context.get("n_output_tokens") == usage["output_tokens"]
    if usage is not None and not harbor_matches:
        record["issues"].append("OpenCode / Harbor token reconciliation failed")
    normalized = usage["input_tokens"] if usage else None
    if normalized is None:
        normalized = context.get("n_input_tokens")
    provider_matches = usage is not None and provider is not None and normalized == provider["input_tokens"] and usage["output_tokens"] == provider["output_tokens"]
    if provider is not None and not provider_matches:
        record["issues"].append("provider / OpenCode token reconciliation failed")
    record["usage_complete"] = bool(usage and usage["complete"] and harbor_matches and provider_matches
                                    and record["exception"] is None and record["status"] != "invalid")


def collect_trial(experiment: Path, row: dict, record: dict, candidate_hash: str) -> dict:
    job = experiment / "jobs" / row["job"]
    results = [path for path in job.glob("*/result.json") if (path.parent / "agent").is_dir()]
    if len(results) != 1:
        record["issues"].append(f"expected one trial, found {len(results)}")
        return record
    trial = results[0].parent
    _collect_usage(trial, record)
    record["evidence"] = {path.relative_to(experiment).as_posix(): digest(path)
                          for path in trial.rglob("*") if path.is_file()}
    result = read_json(results[0])
    info = result.get("agent_info") or {}
    if (info.get("name") != "opencode" or info.get("version") != "1.18.4"
        or info.get("model_info") != {"name": "qwen3.7-max", "provider": "dashscope"}):
        record["issues"].append("Agent/model identity differs from frozen protocol")
    if result.get("task_name") != row["task_slug"] or not result.get("finished_at"):
        record["issues"].append("wrong task or unfinished trial")
    record["exception"] = result.get("exception_info")
    if record["status"] != "invalid":
        record["status"] = "failed" if record["exception"] else "completed"
    _reconcile_usage(result, record)
    timing_path = trial / "agent" / "choice-timing.json"
    if timing_path.is_file():
        timing = read_json(timing_path)
    else:
        timing = result.get("agent_execution")
    if timing and timing.get("finished_at"):
        elapsed = (datetime.fromisoformat(timing["finished_at"]) - datetime.fromisoformat(timing["started_at"])).total_seconds()
        if not number(elapsed) or elapsed <= 0:
            raise ValueError("invalid Agent execution timing")
        record["metrics"]["wall_seconds"] = elapsed
    setup_path = trial / "agent" / "zvec-grep-setup.json"
    if not setup_path.is_file():
        record["issues"].append("missing graph setup evidence")
        record["usage_complete"] = False
        return record
    setup = read_json(setup_path)
    if setup.get("status") != "ready" or setup.get("choice_profile") != row["profile"] or not setup.get("graph_preflight", {}).get("ready") or not setup.get("catalog_preflight", {}).get("ready"):
        record["issues"].append("graph setup not ready or profile mismatch")
    identity = setup.get("index_seed_identity") or {}
    if (setup.get("protocol") != PROTOCOL or setup.get("package_sha256") != candidate_hash
        or setup.get("embedding_model") != "qwen/qwen3.7-text-embedding"
        or identity.get("commit") != row["commit"] or identity.get("package") != candidate_hash):
        record["issues"].append("package, embedding or repository identity drift")
    record["seed_key"] = setup.get("graph_seed_key")
    record["index_inventory"] = setup.get("index_inventory")
    if not record["index_inventory"] or identity.get("protocol") != PROTOCOL or identity.get("graph_options") != {"scope": "code"}:
        record["issues"].append("missing or changed canonical index identity")
    if record["seed_key"] != hashlib.sha256(json.dumps(identity, separators=(",", ":"), sort_keys=True).encode()).hexdigest():
        record["issues"].append("canonical index key mismatch")
    record["setup_seconds"] = setup.get("total_duration_seconds")
    after = trial / "agent" / "choice-index-after.json"
    if not after.is_file() or read_json(after) != setup.get("index_inventory"):
        record["issues"].append("missing or changed post-query index inventory")
    preflight = audit_mcp(json_lines(trial / "agent" / "choice-preflight.jsonl"), "zg-augment")
    record["issues"].extend(preflight["issues"])
    catalog = audit_mcp(json_lines(trial / "agent" / "choice-catalog.jsonl"), AUTO)
    record["issues"].extend(catalog["issues"])
    record["catalog_contracts"] = catalog["tools"]
    if not catalog["tools"]:
        record["issues"].append("missing product catalog preflight")
    elif setup.get("catalog_preflight", {}).get("contract_hash") != contract_hash(catalog["tools"][0]):
        record["issues"].append("product catalog hash mismatch")
    record["expected_view"] = next((call["graph"].get("view") for call in preflight["calls"] if call["graph"].get("view")), None)
    if record["expected_view"] is None:
        record["issues"].append("missing graph preflight version")
    proxy = trial / "agent" / "choice-mcp.jsonl"
    record["mcp"] = audit_mcp(json_lines(proxy), row["profile"]) if proxy.is_file() else audit_mcp([], row["profile"])
    record["issues"].extend(record["mcp"]["issues"])
    record["tool_contracts"] = record["mcp"]["tools"]
    expected_contracts = catalog["tools"] if row["profile"] == AUTO else preflight["tools"]
    if record["mcp"]["tools"] and record["mcp"]["tools"] != expected_contracts:
        record["issues"].append("Agent tool contract differs from preflight")
    trajectory_path = trial / "agent" / "trajectory.json"
    if not trajectory_path.is_file():
        record["issues"].append("missing trajectory; observed costs retained")
        record["usage_complete"] = False
        return record
    trajectory = read_json(trajectory_path)
    steps = [step for step in trajectory["steps"] if step.get("source") == "agent"]
    calls = [call for step in steps for call in step.get("tool_calls", [])]
    zg_calls = sum(any(call["function_name"].endswith(name) for name in (SEARCH, EXPLORE)) for call in calls)
    for call in calls:
        name = call["function_name"]
        if "zvec_grep" in name and not any(name.endswith(tool) for tool in (SEARCH, EXPLORE)):
            record["issues"].append("unexposed zg tool invoked")
        if row["profile"] != AUTO and name.endswith(EXPLORE):
            record["issues"].append("non-search zg tool invoked")
        if name in ("bash", "shell") and re.search(r"\bzg\b|\.zvec-grep", json.dumps(call["arguments"])):
            record["issues"].append("direct zg shell/storage access")
    if zg_calls != record["mcp"]["requests"]:
        record["issues"].append("trajectory / MCP call count mismatch")
    bind_trajectory(record, calls)
    if record["usage"] and record["usage"]["terminal"] and steps and not steps[-1].get("tool_calls"):
        record["answer"] = _message_text(steps[-1].get("message"))
    if not record["answer"] and record["status"] != "invalid":
        record["status"] = "no_answer"
    record["metrics"].update({"tool_calls": len(calls), "zg_calls": zg_calls, "native_calls": len(calls) - zg_calls})
    return record



def collect(experiment: Path) -> dict:
    manifest = checked_manifest(experiment)
    records = []
    for row in manifest["matrix"]:
        record = empty_trial(row)
        try:
            collect_trial(experiment, row, record, manifest["candidate"]["sha256"])
        except (ValueError, OSError, KeyError, TypeError, AttributeError) as error:
            record["status"], record["usage_complete"] = "invalid", False
            record["issues"].append(str(error))
        records.append(record)
    value = {"protocol": PROTOCOL, "manifest_sha256": digest(experiment / "manifest.json"), "trials": records}
    write_json(experiment / "collection.json", value)
    return value


def judge(experiment: Path) -> None:
    collection = read_json(experiment / "collection.json")
    manifest = checked_manifest(experiment)
    if collection["protocol"] != PROTOCOL or collection["manifest_sha256"] != digest(experiment / "manifest.json"):
        raise ValueError("collection identity mismatch")
    if digest(DATA / "references.json") != manifest["assets"]["references.json"]:
        raise ValueError("judge references changed after input freeze")
    references = {row["task_id"]: row for row in read_json(DATA / "references.json")["references"]}
    rows = [row for row in collection["trials"] if row["answer"]]
    random.Random(SEED).shuffle(rows)
    api_key = os.environ.get("GLM_API_KEY", os.environ.get("OPENAI_API_KEY", "")).strip()
    if rows and not api_key:
        raise ValueError("set GLM_API_KEY for the independent judge")
    completion = _default_completion()
    scores = experiment / "scores"
    scores.mkdir(exist_ok=False)
    write_json(scores / "order.json", [row["id"] for row in rows])
    for row in rows:
        reference = references[row["task_id"]]
        result = {"id": row["id"], "answer_sha256": hashlib.sha256(row["answer"].encode()).hexdigest()}
        try:
            score = _judge_candidate(completion_fn=completion, api_key=api_key,
                api_base=manifest["judge_endpoint"],
                question=reference["question"], reference=reference["reference_answer"],
                candidate=row["answer"], attempts=1)
            score.update({"label": "glm-5.2-independent-judge-v1", "self_judge": False})
            result.update({"status": "scored", "judge": score})
        except SweQaError as error:
            result.update({"status": "pending", "error": str(error)})
        write_json(scores / (row["id"] + ".json"), result)


def _quality(experiment, row, judge_usage):
    if not row["answer"]:
        row["quality"], row["rubric"] = 0, dict.fromkeys(SCORE_KEYS, 0)
        return True
    path = experiment / "scores" / (row["id"] + ".json")
    saved = read_json(path) if path.is_file() else None
    if saved is None or saved["status"] != "scored":
        row["quality"], row["rubric"] = None, dict.fromkeys(SCORE_KEYS)
        return False
    if saved["id"] != row["id"] or saved["answer_sha256"] != hashlib.sha256(row["answer"].encode()).hexdigest():
        raise ValueError("score belongs to a different answer")
    judged = saved["judge"]
    if (judged["model"] != "glm-5.2" or judged["self_judge"] is not False
        or set(judged["scores"]) != set(SCORE_KEYS)
        or any(not number(v, integer=True) or not 1 <= v <= 20 for v in judged["scores"].values())
        or judged["total"] != sum(judged["scores"].values())):
        raise ValueError("invalid independent judge scores")
    row["quality"], row["rubric"] = judged["total"], judged["scores"]
    judge_usage["calls"] += 1
    judge_usage["latency_seconds"] += judged["latency_seconds"]
    for key in ("input_tokens", "output_tokens"):
        value = judged["usage"][key]
        judge_usage[key] = None if value is None or judge_usage[key] is None else judge_usage[key] + value
    return True


def report(experiment: Path) -> dict:
    manifest = checked_manifest(experiment)
    collection = read_json(experiment / "collection.json")
    if collection["protocol"] != PROTOCOL or collection["manifest_sha256"] != digest(experiment / "manifest.json"):
        raise ValueError("collection identity mismatch")
    expected = {row["job"]: row for row in manifest["matrix"]}
    records = deepcopy(collection["trials"])
    if len(records) != len(expected) or {row["id"] for row in records} != set(expected):
        raise ValueError("missing or duplicate trial records")
    _verify_trial_evidence(experiment, records)
    issues, seeds, inventories, contracts, catalogs, views = [], {}, {}, {}, set(), {}
    scoring_complete = True
    judge_usage = {"calls": 0, "input_tokens": 0, "output_tokens": 0, "latency_seconds": 0.0}
    for row in records:
        if any(row.get(k) != v for k, v in expected[row["id"]].items()):
            raise ValueError("trial differs from frozen matrix")
        issues += [f"{row['id']}: {issue}" for issue in row["issues"]]
        if row.get("seed_key"):
            seeds.setdefault(row["repository"], set()).add(row["seed_key"])
        if row.get("index_inventory"):
            inventories.setdefault(row["repository"], set()).add(contract_hash(row["index_inventory"]))
        if row.get("expected_view"):
            views.setdefault(row["repository"], set()).add(contract_hash(row["expected_view"]))
        for contract in row.get("catalog_contracts", []):
            catalogs.add(contract_hash(contract))
        for contract in row.get("tool_contracts", []):
            contracts.setdefault(row["profile"], set()).add(contract_hash(contract))
        scoring_complete = _quality(experiment, row, judge_usage) and scoring_complete
    if any(len(group) != 1 for values in (seeds, inventories, views) for group in values.values()):
        issues.append("canonical index or view differs across arms")
    if len(catalogs) != 1 or any(len(group) != 1 for group in contracts.values()):
        issues.append("product tool contract drift")
    if contracts.get("zg-off", set()) != contracts.get("zg-augment", set()):
        issues.append("fixed-arm tool contracts differ")

    metrics = ("quality", *SCORE_KEYS, *METRICS)
    cases = []
    for task in tasks(manifest["tier"]):
        means = {}
        for profile in PROFILES:
            trials = [r for r in records if r["task_id"] == task["task_id"] and r["profile"] == profile]
            values = {}
            for key in metrics:
                samples = [r["quality"] if key == "quality" else r["rubric"][key] if key in SCORE_KEYS else r["metrics"][key] for r in trials]
                values[key] = statistics.mean(samples) if samples and all(v is not None for v in samples) else None
            means[profile] = values
        cases.append({"task_id": task["task_id"], "profiles": means})
    comparisons = {}
    for name, treatment, baseline in COMPARISONS:
        pair_cases = [{"metrics": {key: {"off": c["profiles"][baseline][key], "augment": c["profiles"][treatment][key]} for key in metrics}} for c in cases]
        aggregate = {key: legacy.summarize(pair_cases, key) for key in metrics}
        for value in aggregate.values():
            value["baseline_mean"], value["treatment_mean"] = value.pop("off"), value.pop("augment")
        state = "incomplete" if issues or not scoring_complete else "uncertain"
        interval = aggregate["quality"]["ci95"]
        if state != "incomplete" and interval:
            if interval[0] > 0:
                usage_complete = all(r.get("usage_complete", False) for r in records if r["profile"] in (treatment, baseline))
                resources = [aggregate[k] for k in ("total_tokens", "tool_calls", "wall_seconds")]
                if not usage_complete:
                    state = "quality_improved_resources_incomplete"
                elif all(r["complete"] and r["change_pct_ci95"] and r["change_pct_ci95"][1] < 0 for r in resources):
                    state = "quality_and_efficiency_improved"
                else:
                    state = "quality_improved_cost_mixed"
            elif interval[1] < 0:
                state = "quality_regressed"
        comparisons[name] = {"treatment": treatment, "baseline": baseline, "aggregate": aggregate, "interpretation": state}
    diagnostics = {}
    for profile in PROFILES:
        trials = [r for r in records if r["profile"] == profile]
        calls = [c for r in trials for c in (r.get("mcp") or {}).get("calls", [])]
        elapsed = [c["profiles"][profile]["wall_seconds"] for c in cases]
        operations = sorted({c["operation"] for c in calls if isinstance(c.get("operation"), str) and c["operation"]})
        diagnostics[profile] = {
            "trials": len(trials), "failed_trials": sum(r["status"] in ("failed", "no_answer", "missing", "invalid") for r in trials),
            "integrity_failed_trials": sum(bool(r["issues"]) for r in trials),
            "modes": {mode: sum(c["mode"] == mode for c in calls) for mode in ("off", "augment", "constrain", "explore")},
            "mode_switches": sum((r.get("mcp") or {}).get("mode_switches", 0) for r in trials),
            "operations": {op: sum(c["operation"] == op for c in calls) for op in operations},
            "errors": sum(c["graph"]["status"] == "error" for c in calls),
            "partial": sum(c["graph"]["status"] == "partial" for c in calls),
            "degraded": sum(c["graph"]["status"] == "degraded" for c in calls),
            "cancelled": sum(c["cancelled"] for c in calls),
            "responses_with_evidence": sum(c["graph"]["evidence"] > 0 for c in calls),
            "empty_graph_responses": sum(c["mode"] != "off" and c["graph"].get("empty", False) for c in calls),
            "ambiguous_locations": sum(loc.get("status") == "ambiguous" for c in calls for loc in c["graph"].get("locations", [])),
            "continuation_calls": sum(c.get("cursor") is not None for c in calls),
            "median_seconds": statistics.median(elapsed) if all(v is not None for v in elapsed) else None,
            "p95_seconds": percentile(elapsed, .95) if all(v is not None for v in elapsed) else None,
        }
    result = {"protocol": PROTOCOL, "tier": manifest["tier"], "manifest_sha256": collection["manifest_sha256"],
              "collection_sha256": digest(experiment / "collection.json"), "score_evidence": _score_evidence(experiment),
              "integrity": {"passed": not issues, "issues": issues}, "scoring_complete": scoring_complete,
              "usage_complete": all(r.get("usage_complete", False) for r in records),
              "primary_comparison": COMPARISONS[0][0], "comparisons": comparisons, "cases": cases,
              "diagnostics": diagnostics, "trials": records, "judge_usage": judge_usage,
              "limitations": manifest["limitations"]}
    destination = experiment / "report"
    destination.mkdir(exist_ok=False)
    write_json(destination / "report.json", result)
    _render(destination, result)
    return result


def _render(destination, result):
    lines = [f"# {PROTOCOL}", "", f"Integrity: {result['integrity']['passed']}; scoring complete: {result['scoring_complete']}; usage complete: {result['usage_complete']}.",
             "", "Primary comparison: choice_vs_off. Tool distributions are not selection accuracy.",
             "", "| Comparison | Metric | Baseline | Treatment | Difference | 95% CI | Change % |", "|---|---|---:|---:|---:|---|---:|"]
    for name, comparison in result["comparisons"].items():
        for metric, value in comparison["aggregate"].items():
            lines.append(f"| {name} ({comparison['interpretation']}) | {metric} | {value['baseline_mean']} | {value['treatment_mean']} | {value['delta']} | {value['ci95']} | {value.get('change_pct')} |")
    lines += ["", "## Per-task quality", "", "| Task | zg-off | zg-augment | zg-agent-choice |", "|---|---:|---:|---:|"]
    for case in result["cases"]:
        lines.append("| " + case["task_id"] + " | " + " | ".join(str(case["profiles"][p]["quality"]) for p in PROFILES) + " |")
    lines += ["", "## Tool use", "", "```json", json.dumps(result["diagnostics"], indent=2), "```", "", "## Limits", *["- " + s for s in result["limitations"]]]
    (destination / "report.md").write_text("\n".join(lines) + "\n")
    # Use the primary comparison to choose blind source-review cases, retaining all three answers.
    candidates = [c for c in result["cases"] if all(c["profiles"][p]["quality"] is not None for p in PROFILES)]
    candidates.sort(key=lambda c: c["profiles"][AUTO]["quality"] - c["profiles"]["zg-off"]["quality"])
    selected = ([c for c in candidates if c["profiles"][AUTO]["quality"] < c["profiles"]["zg-off"]["quality"]][:3]
                + [c for c in candidates[::-1] if c["profiles"][AUTO]["quality"] > c["profiles"]["zg-off"]["quality"]][:3])
    review, key = ["# Blind source review", "Record supported / contradicted / insufficient evidence and source paths/lines. Do not change scores."], {}
    rng = random.Random(SEED)
    for case in selected:
        profiles = list(PROFILES); rng.shuffle(profiles)
        task = next(t for t in tasks(result["tier"]) if t["task_id"] == case["task_id"])
        review += ["", "## " + case["task_id"], task["question"],
                   "Repository: " + task["repository"], "Commit: " + task["repository_commit"]]
        key[case["task_id"]] = {}
        for index, profile in enumerate(profiles):
            label = chr(65 + index)
            key[case["task_id"]][label] = profile
            answers = [r for r in result["trials"] if r["task_id"] == case["task_id"] and r["profile"] == profile]
            review += ["", "### Answer " + label, *[r["answer"] for r in answers]]
    (destination / "review.md").write_text("\n".join(review) + "\n")
    write_json(destination / "review-key.json", key)
