"""Evidence collection and offline task-paired reporting for Augment v1."""

from __future__ import annotations

import hashlib
import json
import math
import os
import random
import re
import statistics
from datetime import datetime
from pathlib import Path

from . import SweQaError
from .augment_protocol import (
    DATA, EXECUTION, GRAPH, INSTRUCTIONS, PROFILES, PROTOCOL, SEED,
    digest, matrix, read_json, tasks, write_json,
)
from .collect import _message_text
from .judge import SCORE_KEYS, _default_completion, _judge_candidate

METRICS = ("input_tokens", "output_tokens", "total_tokens", "tool_calls", "zg_calls", "native_calls", "wall_seconds")


def number(value, *, integer=False):
    return (not isinstance(value, bool) and isinstance(value, int if integer else (int, float))
            and math.isfinite(value) and value >= 0)


def json_lines(path: Path) -> list[dict]:
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip():
            value = json.loads(line)
            if not isinstance(value, dict):
                raise ValueError(f"invalid JSONL record in {path}")
            rows.append(value)
    return rows


def opencode_usage(path: Path) -> dict:
    """OpenCode input excludes cache; Harbor 0.18 adds cache.read, not write."""
    events = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("{"):
            event = json.loads(line)
            if not isinstance(event, dict):
                raise ValueError("OpenCode event must be an object")
            events.append(event)
    finishes = [event["part"] for event in events if event.get("type") == "step_finish"]
    if not finishes:
        return {"complete": False, "input_tokens": None, "output_tokens": None,
                "total_tokens": None, "cache_read": None, "cache_write": None,
                "harbor_input_tokens": None, "terminal": False}
    input_total = output_total = cache_read = cache_write = 0
    cache_available = True
    for finish in finishes:
        usage = finish.get("tokens", {})
        if not number(usage.get("input"), integer=True) or not number(usage.get("output"), integer=True):
            raise ValueError("OpenCode step_finish missing input/output usage")
        input_total += usage["input"]
        output_total += usage["output"]
        cache = usage.get("cache")
        if cache is None:
            cache_available = False
        elif not number(cache.get("read"), integer=True) or not number(cache.get("write"), integer=True):
            raise ValueError("invalid cache usage")
        else:
            cache_read += cache["read"]
            cache_write += cache["write"]
    # Incomplete streams retain observed usage but are not declared complete accounting.
    terminal = finishes[-1].get("reason") == "stop" and not any(event.get("type") == "error" for event in events)
    observed = input_total + cache_read + cache_write if cache_available else None
    return {
        "complete": terminal and cache_available, "input_tokens": observed,
        "output_tokens": output_total, "total_tokens": None if observed is None else observed + output_total,
        "cache_read": cache_read if cache_available else None,
        "cache_write": cache_write if cache_available else None,
        "harbor_input_tokens": input_total + cache_read,
        "terminal": terminal, "observed_noncache_input": input_total,
        "accounting": "OpenCode input + cache.read + cache.write; cache is not added again to Harbor totals",
    }


def provider_usage(path: Path) -> dict:
    events = json_lines(path)
    requests = {event["id"] for event in events if event["event"] == "request"}
    if any(event.get("model") != "qwen3.7-max" for event in events if event["event"] == "request"):
        raise ValueError("provider model differs from frozen protocol")
    responses = [event for event in events if event["event"] == "response"]
    if len({event["id"] for event in responses}) != len(responses):
        raise ValueError("duplicate provider usage response")
    if requests != {event["id"] for event in responses} or not requests:
        raise ValueError("missing provider usage response")
    input_tokens = output_tokens = 0
    cached = []
    for event in responses:
        usage = event.get("usage")
        if event["status"] != 200 or not isinstance(usage, dict):
            raise ValueError("provider call failed or omitted usage")
        if not number(usage.get("prompt_tokens"), integer=True) or not number(usage.get("completion_tokens"), integer=True):
            raise ValueError("provider omitted token totals")
        input_tokens += usage["prompt_tokens"]
        output_tokens += usage["completion_tokens"]
        detail = usage.get("prompt_tokens_details") or {}
        cached.append(detail.get("cached_tokens"))
    return {"input_tokens": input_tokens, "output_tokens": output_tokens,
            "total_tokens": input_tokens + output_tokens,
            "cache_read": sum(cached) if all(number(value, integer=True) for value in cached) else None,
            "calls": len(responses)}


def audit_mcp(events: list[dict], profile: str) -> dict:
    requests = {}
    responses = []
    tools = []
    issues = []
    for event in events:
        if event["event"] == "start":
            config = event["config"]
            if config["profile"] != profile or config["graph"] != GRAPH or config["graph_execution"] != EXECUTION or config["instructions"] != INSTRUCTIONS:
                issues.append("proxy configuration drift")
        elif event["event"] == "tools":
            tools.append({"tools": event["tools"], "instructions": event["instructions"]})
        elif event["event"] == "request":
            request = event["request"]
            if request["id"] in requests:
                issues.append("duplicate MCP request ID")
            requests[request["id"]] = request
            args = request["params"]["arguments"]
            expected = {"mode": "off"} if profile == "zg-off" else GRAPH
            if (request["params"]["name"] != "zvec_grep_search" or args.get("graph") != expected
                or args.get("autoUpdate") is not False or args.get("freshness") != "eventual"
                or args.get("root") != "/app"):
                issues.append("incorrect experimental treatment")
            if (profile == "zg-off" and "graphExecution" in args) or (profile == "zg-augment" and args.get("graphExecution") != EXECUTION):
                issues.append("incorrect execution budget")
        elif event["event"] == "response":
            if requests.pop(event["request"]["id"], None) != event["request"]:
                issues.append("unpaired MCP response")
            responses.append(event)
            status = event["graph"]["status"]
            if status == "invalid" or (profile == "zg-augment" and status == "absent"):
                issues.append("missing or invalid graph result")
            if profile == "zg-off" and status not in ("absent", "error"):
                issues.append("graph result in Off arm")
        elif event["event"] in ("rejected", "backend_error"):
            issues.append(event["event"])
    return {
        "issues": issues, "pending_calls": len(requests), "tools": tools,
        "requests": len(responses) + len(requests),
        "graph_requests": len(responses) + len(requests) if profile == "zg-augment" else 0,
        "with_evidence": sum(event["graph"]["evidence"] > 0 for event in responses),
        "errors": sum(event["graph"]["status"] == "error" for event in responses),
        "partial": sum(event["graph"]["status"] == "partial" for event in responses),
        "degraded": sum(event["graph"]["status"] == "degraded" for event in responses),
    }


def empty_trial(row: dict) -> dict:
    return {**row, "id": row["job"], "status": "missing", "answer": "", "issues": [],
            "metrics": dict.fromkeys(METRICS), "usage": None, "mcp": None, "evidence": {}}


def collect_trial(experiment: Path, row: dict, record: dict, candidate_hash: str) -> dict:
    job = experiment / "jobs" / row["job"]
    results = [path for path in job.glob("*/result.json") if (path.parent / "agent").is_dir()]
    if len(results) != 1:
        record["issues"].append(f"expected one trial, found {len(results)}")
        return record
    trial = results[0].parent
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
    record["status"] = "failed" if record["exception"] else "completed"
    raw_path = trial / "agent" / "opencode.txt"
    if raw_path.is_file():
        usage = opencode_usage(raw_path)
        record["usage"] = usage
        record["metrics"].update({key: usage[key] for key in ("input_tokens", "output_tokens", "total_tokens")})
        record["usage_complete"] = usage["complete"] and record["exception"] is None
        context = result.get("agent_result") or {}
        if context.get("n_input_tokens") != usage["harbor_input_tokens"] or context.get("n_output_tokens") != usage["output_tokens"]:
            record["issues"].append("OpenCode / Harbor token reconciliation failed")
    timing_path = trial / "agent" / "augment-timing.json"
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
        return record
    setup = read_json(setup_path)
    if setup.get("status") != "ready" or setup.get("graph_profile") != row["profile"] or not setup.get("graph_preflight", {}).get("ready"):
        record["issues"].append("graph setup not ready or profile mismatch")
    identity = setup.get("index_seed_identity") or {}
    if (setup.get("protocol") != PROTOCOL or setup.get("package_sha256") != candidate_hash
        or setup.get("embedding_model") != "qwen/qwen3.7-text-embedding"
        or identity.get("commit") != row["commit"] or identity.get("package") != candidate_hash):
        record["issues"].append("package, embedding or repository identity drift")
    record["seed_key"] = setup.get("graph_seed_key")
    record["setup_seconds"] = setup.get("total_duration_seconds")
    after = trial / "agent" / "augment-index-after.json"
    if not after.is_file() or read_json(after) != setup.get("index_inventory"):
        record["issues"].append("missing or changed post-query index inventory")
    preflight = audit_mcp(json_lines(trial / "agent" / "augment-preflight.jsonl"), "zg-augment")
    record["issues"].extend(preflight["issues"])
    record["tool_contracts"] = preflight["tools"]
    proxy = trial / "agent" / "augment-mcp.jsonl"
    record["mcp"] = audit_mcp(json_lines(proxy), row["profile"]) if proxy.is_file() else {
        "issues": [], "pending_calls": 0, "tools": [], "requests": 0,
        "graph_requests": 0, "with_evidence": 0, "errors": 0, "partial": 0, "degraded": 0,
    }
    if not record["mcp"]["tools"]:
        record["issues"].append("missing Agent MCP tool discovery evidence")
    record["issues"].extend(record["mcp"]["issues"])
    trajectory_path = trial / "agent" / "trajectory.json"
    if not trajectory_path.is_file():
        record["issues"].append("missing trajectory; observed costs retained")
        return record
    trajectory = read_json(trajectory_path)
    steps = [step for step in trajectory["steps"] if step.get("source") == "agent"]
    calls = [call for step in steps for call in step.get("tool_calls", [])]
    zg_calls = sum(call["function_name"].endswith("zvec_grep_search") for call in calls)
    for call in calls:
        name = call["function_name"]
        if "zvec_grep" in name and not name.endswith("zvec_grep_search"):
            record["issues"].append("non-search zg tool invoked")
        if name in ("bash", "shell") and re.search(r"\bzg\b|\.zvec-grep", json.dumps(call["arguments"])):
            record["issues"].append("direct zg shell/storage access")
    if zg_calls != record["mcp"]["requests"]:
        record["issues"].append("trajectory / MCP call count mismatch")
    record["tool_contracts"].extend(record["mcp"]["tools"])
    if record["usage"] and record["usage"]["terminal"] and steps and not steps[-1].get("tool_calls"):
        record["answer"] = _message_text(steps[-1].get("message"))
    if not record["answer"]:
        record["status"] = "no_answer"
    record["metrics"].update({"tool_calls": len(calls), "zg_calls": zg_calls, "native_calls": len(calls) - zg_calls})
    provider = provider_usage(trial / "agent" / "augment-provider-usage.jsonl")
    record["provider_usage"] = provider
    normalized = record["usage"]["input_tokens"] if record["usage"] else None
    if normalized is None:
        normalized = (result.get("agent_result") or {}).get("n_input_tokens")
    if normalized != provider["input_tokens"] or record["usage"]["output_tokens"] != provider["output_tokens"]:
        record["issues"].append("provider / OpenCode token reconciliation failed")
        record["usage_complete"] = False
    record["metrics"].update({key: provider[key] for key in ("input_tokens", "output_tokens", "total_tokens")})
    return record


def collect(experiment: Path) -> dict:
    manifest = read_json(experiment / "manifest.json")
    if manifest["protocol"] != PROTOCOL or manifest["matrix"] != matrix(manifest["tier"]):
        raise ValueError("unknown protocol or changed call matrix")
    if digest(experiment / "candidate.tgz") != manifest["candidate"]["sha256"]:
        raise ValueError("candidate package changed")
    for name, expected in manifest["assets"].items():
        if digest(DATA / name) != expected:
            raise ValueError(f"frozen benchmark asset changed: {name}")
    records = []
    for row in manifest["matrix"]:
        record = empty_trial(row)
        try:
            collect_trial(experiment, row, record, manifest["candidate"]["sha256"])
        except (ValueError, OSError, KeyError, TypeError) as error:
            record["status"] = "invalid"
            record["usage_complete"] = False
            record["issues"].append(str(error))
        records.append(record)
    value = {"protocol": PROTOCOL, "manifest_sha256": digest(experiment / "manifest.json"), "trials": records}
    write_json(experiment / "collection.json", value)
    return value


def judge(experiment: Path) -> None:
    collection = read_json(experiment / "collection.json")
    manifest = read_json(experiment / "manifest.json")
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


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    index = (len(ordered) - 1) * fraction
    low = math.floor(index)
    high = math.ceil(index)
    return ordered[low] + (ordered[high] - ordered[low]) * (index - low)


def bootstrap(values: list[float]) -> list[float] | None:
    if not values:
        return None
    rng = random.Random(SEED)
    means = [statistics.mean(rng.choices(values, k=len(values))) for _ in range(10000)]
    return [percentile(means, 0.025), percentile(means, 0.975)]


def summarize(cases: list[dict], metric: str) -> dict:
    paired = [case["metrics"][metric] for case in cases]
    complete = all(row["off"] is not None and row["augment"] is not None for row in paired)
    if not complete:
        return {"complete": False, "off": None, "augment": None, "delta": None, "ci95": None}
    changes = [row["augment"] - row["off"] for row in paired]
    quality_metric = metric in ("quality", *SCORE_KEYS)
    relative = [] if quality_metric else [(row["augment"] - row["off"]) / row["off"] * 100 for row in paired if row["off"] != 0]
    return {"complete": True,
            "off": statistics.mean(row["off"] for row in paired),
            "augment": statistics.mean(row["augment"] for row in paired),
            "delta": statistics.mean(changes), "ci95": bootstrap(changes),
            "change_pct": statistics.mean(relative) if relative else None,
            "change_pct_ci95": bootstrap(relative), "relative_sample_count": len(relative),
            "improved": sum(change > 0 if metric in ("quality", *SCORE_KEYS) else change < 0 for change in changes),
            "unchanged": changes.count(0),
            "regressed": sum(change < 0 if metric in ("quality", *SCORE_KEYS) else change > 0 for change in changes)}


def report(experiment: Path) -> dict:
    manifest = read_json(experiment / "manifest.json")
    collection = read_json(experiment / "collection.json")
    if collection["protocol"] != PROTOCOL or collection["manifest_sha256"] != digest(experiment / "manifest.json"):
        raise ValueError("collection identity mismatch")
    expected = {row["job"] for row in manifest["matrix"]}
    expected_rows = {row["job"]: row for row in manifest["matrix"]}
    records = collection["trials"]
    if len(records) != len(expected) or {row["id"] for row in records} != expected:
        raise ValueError("missing or duplicate trial records")
    issues = [f"{row['id']}: {issue}" for row in records for issue in row["issues"]]
    schemas = set()
    seeds = {}
    scoring_complete = True
    judge_usage = {"calls": 0, "input_tokens": 0, "output_tokens": 0, "latency_seconds": 0.0}
    for row in records:
        if any(row.get(key) != value for key, value in expected_rows[row["id"]].items()):
            raise ValueError("trial record differs from frozen matrix")
        for path, expected_hash in row.get("evidence", {}).items():
            if digest(experiment / path) != expected_hash:
                raise ValueError(f"trial evidence changed: {path}")
        for tool in row.get("tool_contracts", []):
            schemas.add(json.dumps(tool, sort_keys=True))
        key = row.get("seed_key")
        if key:
            seeds.setdefault(row["repository"], set()).add(key)
        if row["answer"]:
            score_path = experiment / "scores" / (row["id"] + ".json")
            score = read_json(score_path) if score_path.is_file() else None
            if score is None or score["status"] != "scored":
                scoring_complete = False
                row["quality"] = None
                row["rubric"] = dict.fromkeys(SCORE_KEYS)
            else:
                if score["id"] != row["id"] or score["answer_sha256"] != hashlib.sha256(row["answer"].encode()).hexdigest():
                    raise ValueError("score belongs to a different answer")
                judged = score["judge"]
                if (judged["model"] != "glm-5.2" or judged["self_judge"] is not False
                    or set(judged["scores"]) != set(SCORE_KEYS)
                    or any(not number(v, integer=True) or not 1 <= v <= 20 for v in judged["scores"].values())
                    or judged["total"] != sum(judged["scores"].values())):
                    raise ValueError("invalid independent judge scores")
                row["quality"] = judged["total"]
                row["rubric"] = judged["scores"]
                judge_usage["calls"] += 1
                judge_usage["latency_seconds"] += judged["latency_seconds"]
                for metric in ("input_tokens", "output_tokens"):
                    usage = judged["usage"][metric]
                    if usage is None or judge_usage[metric] is None:
                        judge_usage[metric] = None
                    else:
                        judge_usage[metric] += usage
        else:
            row["quality"] = 0
            row["rubric"] = dict.fromkeys(SCORE_KEYS, 0)
    if len(schemas) != 1 or any(len(keys) != 1 for keys in seeds.values()):
        issues.append("tool contract or canonical seed differs across arms")
    all_metrics = ("quality", *SCORE_KEYS, *METRICS)
    cases = []
    for task in tasks(manifest["tier"]):
        case = {"task_id": task["task_id"], "metrics": {}, "profiles": {}}
        for profile in PROFILES:
            trials = [row for row in records if row["task_id"] == task["task_id"] and row["profile"] == profile]
            case["profiles"][profile] = trials
            values = {}
            for metric in all_metrics:
                samples = [row["quality"] if metric == "quality" else row["rubric"][metric] if metric in SCORE_KEYS else row["metrics"][metric] for row in trials]
                values[metric] = statistics.mean(samples) if samples and all(value is not None for value in samples) else None
            case["profiles"][profile + "_means"] = values
        for metric in all_metrics:
            case["metrics"][metric] = {"off": case["profiles"]["zg-off_means"][metric], "augment": case["profiles"]["zg-augment_means"][metric]}
        cases.append(case)
    aggregate = {metric: summarize(cases, metric) for metric in all_metrics}
    passed = not issues
    all_usage = all(row.get("usage_complete", False) for row in records)
    quality = aggregate["quality"]
    interpretation = "incomplete" if not passed or not scoring_complete else "uncertain"
    if interpretation != "incomplete" and quality["ci95"][0] > 0:
        resource = [aggregate[key] for key in ("total_tokens", "tool_calls", "wall_seconds")]
        if not all_usage:
            interpretation = "quality_improved_resources_incomplete"
        elif all(row["complete"] and row["change_pct_ci95"] and row["change_pct_ci95"][1] < 0 for row in resource):
            interpretation = "quality_and_efficiency_improved"
        else:
            interpretation = "quality_improved_cost_mixed"
    elif interpretation != "incomplete" and quality["ci95"][1] < 0:
        interpretation = "quality_regressed"
    distribution = {}
    answered_quality = {}
    profile_diagnostics = {}
    for profile in PROFILES:
        values = [case["profiles"][profile + "_means"]["wall_seconds"] for case in cases]
        distribution[profile] = {"n_tasks": len(values), "median": statistics.median(values), "p95": percentile(values, .95)} if all(v is not None for v in values) else None
        trials = [row for row in records if row["profile"] == profile]
        answered = [row["quality"] for row in trials if row["answer"] and row["quality"] is not None]
        answered_quality[profile] = {"scored_answers": len(answered),
            "mean_score_conditional_on_answer": statistics.mean(answered) if answered else None}
        observations = [row["mcp"] for row in trials if row.get("mcp")]
        graph_requests = sum(row["graph_requests"] for row in observations)
        profile_diagnostics[profile] = {
            "trials": len(trials), "zg_used_rate": sum((row["metrics"]["zg_calls"] or 0) > 0 for row in trials) / len(trials),
            "graph_requests": graph_requests,
            "partial_rate": sum(row["partial"] for row in observations) / graph_requests if graph_requests else None,
            "degraded_rate": sum(row["degraded"] for row in observations) / graph_requests if graph_requests else None,
            "search_errors": sum(row["errors"] for row in observations),
            "responses_with_evidence": sum(row["with_evidence"] for row in observations),
        }
    result = {
        "protocol": PROTOCOL, "tier": manifest["tier"], "manifest_sha256": collection["manifest_sha256"],
        "integrity": {"passed": passed, "issues": issues}, "scoring_complete": scoring_complete,
        "usage_complete": all_usage,
        "interpretation": interpretation, "cases": cases, "aggregate": aggregate,
        "judge": {"model": "glm-5.2", "self_judge": False, "usage": judge_usage},
        "latency_task_means": distribution,
        "answered_quality": answered_quality,
        "diagnostics": {
            "planned_trials": len(expected), "no_answer": sum(not row["answer"] for row in records),
            "zg_used_trials": sum((row["metrics"].get("zg_calls") or 0) > 0 for row in records),
            "profiles": profile_diagnostics,
            **{key: sum(row.get("mcp", {}).get(key, 0) for row in records if row.get("mcp"))
               for key in ("requests", "graph_requests", "with_evidence", "errors", "partial", "degraded")},
        },
        "manual_review": "pending; inspect review.md and retain source-backed review notes separately",
        "limitations": manifest["limitations"],
    }
    destination = experiment / "report"
    destination.mkdir(exist_ok=False)
    write_json(destination / "report.json", result)
    lines = [f"# {PROTOCOL}", "", f"Tier: {manifest['tier']}; interpretation: {interpretation}.",
             f"Integrity: {passed}; scoring complete: {scoring_complete}; usage complete: {result['usage_complete']}.", "",
             "Input tokens include cache read/write exactly once. Judge usage is separate.",
             "Resource changes are equal-weight means of task percentage changes; CI resamples tasks, not trials.", "",
             "| Metric | zg-off | zg-augment | Absolute change | 95% CI | Change % | 95% CI (%) |", "|---|---:|---:|---:|---|---:|---|"]
    def fmt(value):
        return "N/A" if value is None else f"{value:.2f}"
    for metric, values in aggregate.items():
        lines.append(f"| {metric} | {fmt(values['off'])} | {fmt(values['augment'])} | {fmt(values['delta'])} | {values['ci95']} | {fmt(values.get('change_pct'))} | {values.get('change_pct_ci95')} |")
    lines += ["", "## Per-task quality", "", "| Task | zg-off | zg-augment | Delta |", "|---|---:|---:|---:|"]
    for case in cases:
        values = case["metrics"]["quality"]
        delta = None if None in values.values() else values["augment"] - values["off"]
        lines.append(f"| {case['task_id']} | {fmt(values['off'])} | {fmt(values['augment'])} | {fmt(delta)} |")
    lines += ["", "## Valid-answer Judge means (conditional on answering)", "",
              "| Profile | Scored answers | Mean |", "|---|---:|---:|"]
    for profile, values in answered_quality.items():
        lines.append(f"| {profile} | {values['scored_answers']} | {fmt(values['mean_score_conditional_on_answer'])} |")
    lines += ["", "## Per-task resource means", "",
              "| Task | Off total tokens | Augment total tokens | Off tools | Augment tools | Off seconds | Augment seconds |",
              "|---|---:|---:|---:|---:|---:|---:|"]
    for case in cases:
        cells = [fmt(case["metrics"][metric][arm]) for metric in ("total_tokens", "tool_calls", "wall_seconds") for arm in ("off", "augment")]
        lines.append("| " + " | ".join([case["task_id"], *cells]) + " |")
    lines += ["", "## Integrity issues", "", *issues, "", "Manual source review: pending (review.md).", "", *manifest["limitations"]]
    (destination / "report.md").write_text("\n".join(lines) + "\n")
    review_packet(destination, cases)
    return result


def review_packet(destination: Path, cases: list[dict]) -> None:
    differences = [(case["metrics"]["quality"]["augment"] - case["metrics"]["quality"]["off"], case)
                   for case in cases if None not in case["metrics"]["quality"].values()]
    selected = sorted((item for item in differences if item[0] > 0), key=lambda item: -item[0])[:3]
    selected += sorted((item for item in differences if item[0] < 0), key=lambda item: item[0])[:3]
    rng = random.Random(SEED)
    rng.shuffle(selected)
    packet = ["# Blind source review", "", "Check each key claim and source citation against the pinned source.",
              "Record verified / contradicted / insufficient evidence and file/line support; do not edit scores.", ""]
    key = []
    questions = {task["task_id"]: task for task in tasks("full")}
    for index, (_, case) in enumerate(selected, 1):
        task = questions[case["task_id"]]
        packet += [f"## Case {index}", "", f"{task['repository']} @ {task['repository_commit']}", "", task["question"], ""]
        answers = [row for profile in PROFILES for row in case["profiles"][profile] if row["answer"]]
        rng.shuffle(answers)
        for number_, row in enumerate(answers, 1):
            alias = f"case-{index}-answer-{number_}"
            packet += [f"### {alias}", "", row["answer"], ""]
            key.append({"alias": alias, "trial_id": row["id"]})
    (destination / "review.md").write_text("\n".join(packet) + "\n")
    write_json(destination / "review-key.json", key)
