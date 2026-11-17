"""Release cohort accounting, including failures and task-level uncertainty."""
from __future__ import annotations

import base64
import hashlib
import json
import gzip
import random
import re
from pathlib import Path
from statistics import mean, stdev

GROUPS = ("search", "agent", "relationships")
RELATED = ("requests-16", "astropy-38", "conan-19", "django-21", "sympy-38", "xarray-32", "matplotlib-37")
METRICS = ("input_tokens", "uncached_input_tokens", "cache_read_tokens", "cache_write_tokens", "output_tokens", "tool_calls", "agent_wall_seconds")


def events(path: Path) -> list[dict]:
    if not path.exists():
        return []
    result = []
    for line in path.read_text().splitlines():
        try:
            result.append(json.loads(line))
        except json.JSONDecodeError:
            # Preserve usable records after a hard kill; completeness is false.
            break
    return result


def rpc_body(value: str) -> list[dict]:
    body = base64.b64decode(value).decode()
    if body.lstrip().startswith("{"):
        return [json.loads(body)]
    return [json.loads(line[5:].strip()) for line in body.splitlines() if line.startswith("data:") and line[5:].strip()]


def tool_attempts(agent: Path) -> list[dict]:
    """Read provider-emitted arguments, including calls OpenCode cannot parse."""
    attempts = []
    for path in sorted((agent / "model-responses").glob("*.raw")):
        calls = {}
        payload = path.read_bytes()
        if payload.startswith(b"\x1f\x8b"):
            payload = gzip.decompress(payload)
        raw = payload.decode(errors="replace")
        for line in raw.splitlines():
            if not line.startswith("data:") or line[5:].strip() == "[DONE]":
                continue
            try:
                message = json.loads(line[5:].strip())
            except json.JSONDecodeError:
                continue
            for choice in message.get("choices", []):
                for delta in choice.get("delta", {}).get("tool_calls", []):
                    call = calls.setdefault((choice.get("index", 0), delta.get("index", 0)), {"name": "", "arguments_raw": ""})
                    function = delta.get("function", {})
                    call["name"] += function.get("name") or ""
                    call["arguments_raw"] += function.get("arguments") or ""
        for call in calls.values():
            if not call["name"].startswith("zvec_grep_") and call["name"] != "invalid":
                continue
            try:
                call["arguments"] = json.loads(call["arguments_raw"])
                call["valid_json"] = isinstance(call["arguments"], dict)
            except json.JSONDecodeError:
                call["valid_json"] = False
            attempts.append({"response": str(path.relative_to(agent)), **call})
    return attempts


def mcp_calls(agent: Path) -> dict:
    log = events(agent / "mcp-execution.jsonl")
    replies = {event["sequence"]: event for event in log if event["type"] == "response"}
    records = []
    for event in log:
        if event["type"] != "request" or event["method"] != "POST":
            continue
        try:
            rpc = json.loads(event["body"])
        except json.JSONDecodeError:
            continue
        if rpc.get("method") != "tools/call":
            continue
        row = {"sequence": event["sequence"], "name": rpc["params"]["name"], "arguments": rpc["params"].get("arguments", {}), "status": "unknown"}
        reply = replies.get(event["sequence"])
        if reply:
            values = rpc_body(reply["body_base64"])
            value = next((value for value in values if value.get("id") == rpc.get("id")), {})
            result = value.get("result", {})
            row.update(result=result, rpc_error=value.get("error"))
            row["status"] = "failed" if value.get("error") or result.get("isError") or reply["status"] != 200 else "succeeded"
            text = "\n".join(part["text"] for part in result.get("content", []) if part.get("type") == "text")
            row["text"] = text
            if row["name"].endswith(("callers", "callees")) and row["status"] == "succeeded":
                row["locations"] = [{"path": match[0], "line": int(match[1]), "byte_column": int(match[2])}
                                    for match in re.findall(r"call at (.+?):(\d+):(\d+)", text)]
                row["return_kind"] = ("locations" if row["locations"] else "unmatched_definition" if text.lstrip().startswith("No matches.")
                                      else "no_resolved_calls" if "No resolved call sites." in text else "unknown")
                row["source_review"] = "pending" if row["locations"] else "not_applicable"
        records.append(row)
    return {"complete": bool(log) and log[-1]["type"] == "closed", "calls": records,
            "rejected_calls": [event for event in log if event["type"] == "rejected_call"],
            "transport_errors": [event for event in log if event["type"] in ("transport_error", "protocol_error")]}


def bootstrap(pairs: list[tuple[float, float]], *, draws: int = 10_000, seed: int = 42) -> dict:
    if not pairs:
        return {"tasks": 0, "delta": None, "delta_ci95": None, "change_pct": None, "change_pct_ci95": None}
    randomizer = random.Random(seed)
    deltas, changes = [], []
    for _ in range(draws):
        sample = randomizer.choices(pairs, k=len(pairs))
        control, treatment = mean(p[0] for p in sample), mean(p[1] for p in sample)
        deltas.append(treatment - control)
        if control:
            changes.append((treatment / control - 1) * 100)
    def interval(values):
        if not values:
            return None
        values = sorted(values)
        def percentile(fraction):
            index = (len(values) - 1) * fraction
            lower = int(index)
            return values[lower] + (values[min(lower + 1, len(values) - 1)] - values[lower]) * (index - lower)
        return [percentile(.025), percentile(.975)]
    control, treatment = mean(p[0] for p in pairs), mean(p[1] for p in pairs)
    return {"tasks": len(pairs), "delta": treatment - control, "delta_ci95": interval(deltas),
            "change_pct": (treatment / control - 1) * 100 if control else None,
            "change_pct_ci95": interval(changes) if len(changes) == draws else None}


def summarize(rows: list[dict], tasks: list[str], *, rounds: int = 3) -> dict:
    expected = {(group, task, number, repeat) for group in GROUPS for task in tasks for number in range(1, rounds + 1) for repeat in range(1, 6)}
    seen = set()
    for row in rows:
        key = tuple(row[k] for k in ("group", "task", "round", "repeat"))
        if key not in expected or key in seen:
            raise ValueError("Unexpected or duplicate release trial slot")
        seen.add(key)
    successful = [row for row in rows if row["status"] == "completed" and row.get("judge") is not None]
    by_task = {}
    for group in GROUPS:
        by_task[group] = {}
        for task in tasks:
            selected = [row for row in successful if row["group"] == group and row["task"] == task]
            complete = len(selected) == rounds * 5
            metrics = {metric: mean(row[metric] for row in selected) if selected and all(row.get(metric) is not None for row in selected) else None for metric in METRICS}
            scores = [row["judge"]["total"] for row in selected]
            by_task[group][task] = {"complete": complete, "scored": len(selected), "planned": rounds * 5,
                                    "score": mean(scores) if scores else None, "score_stddev": stdev(scores) if len(scores) > 1 else None, **metrics}
    comparisons = []
    for treatment, control in (("agent", "search"), ("relationships", "search"), ("agent", "relationships")):
        for label, subset in (("qa20", tasks), ("related-seven", [task for task in tasks if task in RELATED])):
            paired = [task for task in subset if by_task[treatment][task]["complete"] and by_task[control][task]["complete"]]
            metrics = {}
            for metric in ("score", *METRICS):
                values = [(by_task[control][task][metric], by_task[treatment][task][metric]) for task in paired]
                metrics[metric] = bootstrap([pair for pair in values if None not in pair])
            comparisons.append({"treatment": treatment, "control": control, "subset": label, "paired_tasks": paired,
                                "missing_tasks": sorted(set(subset) - set(paired)), "metrics": metrics})
    groups = {}
    for group in GROUPS:
        selected = [row for row in rows if row["group"] == group]
        group_tasks = list(by_task[group].values())
        groups[group] = {"planned": len(tasks) * rounds * 5, "observed": len(selected),
                         "completed": sum(row["status"] == "completed" for row in selected),
                         "scored": sum(row["status"] == "completed" and row.get("judge") is not None for row in selected),
                         "task_equal_score": mean(task["score"] for task in group_tasks) if group_tasks and all(task["complete"] for task in group_tasks) else None,
                         "task_equal_metrics": {metric: mean(task[metric] for task in group_tasks) if group_tasks and all(task["complete"] and task[metric] is not None for task in group_tasks) else None for metric in METRICS},
                         "observed_usage_lower_bound": {metric: sum(row.get(metric) or 0 for row in selected) for metric in METRICS if "seconds" not in metric},
                         "usage_incomplete_trials": sum(not row.get("usage_complete", False) for row in selected),
                         "setup_seconds": {key: sum(row.get("setup", {}).get(key, 0) for row in selected) for key in ("install_duration_seconds", "index_duration_seconds", "index_seed_restore_duration_seconds", "mcp_setup_duration_seconds")},
                         "tool_funnel": tool_funnel(selected)}
    return {"complete": seen == expected and len(successful) == len(expected), "planned": len(expected), "observed": len(rows),
            "missing_slots": [dict(zip(("group", "task", "round", "repeat"), key)) for key in sorted(expected - seen)],
            "groups": groups, "by_task": by_task, "comparisons": comparisons,
            "by_round": {str(number): {group: {"completed": sum(row["status"] == "completed" for row in rows if row["round"] == number and row["group"] == group),
                "scores": [row["judge"]["total"] for row in successful if row["round"] == number and row["group"] == group]} for group in GROUPS} for number in range(1, rounds + 1)},
            "failures": [row for row in rows if row["status"] != "completed" or row.get("judge") is None]}


def tool_funnel(rows: list[dict]) -> dict:
    result = {}
    for tool in ("search", "callers", "callees"):
        matches = lambda name: name.endswith("zvec_grep_" + tool)
        attempts = [call for row in rows for call in row.get("model_attempts", []) if matches(call["name"])]
        calls = [call for row in rows for call in row.get("mcp", {}).get("calls", []) if matches(call["name"])]
        records = [call for call in calls if call.get("return_kind") == "locations"]
        result[tool] = {"selected_trials": sum(any(matches(call["name"]) for call in row.get("model_attempts", [])) for row in rows),
                        "model_attempts": len(attempts), "valid_json": sum(call.get("valid_json", False) for call in attempts),
                        "schema_valid": sum(call.get("schema_valid", False) for call in attempts),
                        "reached_mcp": len(calls), "succeeded": sum(call["status"] == "succeeded" for call in calls),
                        "returned_location_queries": len(records), "location_records": sum(len(call["locations"]) for call in records),
                        "source_review_pending": sum(call.get("source_review") == "pending" for call in records),
                        "return_kinds": {kind: sum(call.get("return_kind") == kind for call in calls) for kind in ("locations", "unmatched_definition", "no_resolved_calls", "unknown")}}
    return result


def render_report(report: dict) -> str:
    lines = ["# 官方0.3.1 QA20 E2E", "", f"计划 {report['planned']} 次，已有 {report['observed']} 次记录；完整：{report['complete']}。", "",
             "| 组别 | 完成 / 计划 | 已评分 | 等权均分 | 输入token / 次 | 未缓存输入 / 次 | 输出token / 次 | Agent秒 / 次 |",
             "|---|---:|---:|---:|---:|---:|---:|---:|"]
    def show(value):
        return "未知或未完整" if value is None else f"{value:.2f}"
    for group, row in report["groups"].items():
        values = [row["task_equal_score"], *[row["task_equal_metrics"][key] for key in ("input_tokens", "uncached_input_tokens", "output_tokens", "agent_wall_seconds")]]
        lines.append(f"| {group} | {row['completed']} / {row['planned']} | {row['scored']} | " + " | ".join(map(show, values)) + " |")
    lines += ["", "评分与效率比较仅使用完整配对题；失败、缺失及已知用量均保留在JSON中。", "",
              "位置记录的正确性、相关性和实际使用须逐条核对源码；pending不计作有效调用。", "",
              "## 主要比较：组合组 − Search组", ""]
    comparison = report["comparisons"][0]
    lines.append(f"完整配对题数：{len(comparison['paired_tasks'])}。")
    for key in ("score", "input_tokens", "uncached_input_tokens", "output_tokens", "agent_wall_seconds"):
        value = comparison["metrics"][key]
        lines.append(f"- {key}：差值 {show(value['delta'])}；95%区间 {value['delta_ci95']}；变化率 {show(value['change_pct'])}%。")
    lines += ["", "逐题、分轮、7题补充分析、完整调用轨迹和失败清单见 report.json 与原始证据。", "",
              "结论描述工具集合及对应指导的整体效果；调用分层不用于推断 caller/callee 的因果收益。"]
    return "\n".join(lines) + "\n"


def evidence_manifest(root: Path) -> dict:
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(root.rglob("*")) if path.is_file() and path.name != "evidence-sha256.json"}
