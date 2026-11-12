"""Single-arm execution evidence and cross-run caller/callee comparisons.

Uses the existing Harbor runner, collector and judge without manufacturing a
second profile or applying the legacy score-difference exclusion filter.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path
from statistics import mean

from zg_bench.core.io import load_object
from zg_bench.engines.judge import default_completion, judge_candidate, judge_generation_metadata
from zg_bench.engines.registry import resolve_opencode_model
from zg_bench.swe_qa.collect import _completed_job, _job_dirs, _profile_result, _select_trials
from zg_bench.swe_qa.pairs import load_references

GROUPS = ("relationships", "search", "agent")
TOOLS = ("search", "callers", "callees")
ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "zg_bench/swe_qa/data"


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def audit_calls(calls: list[dict]) -> dict:
    counts = {tool: {"attempted": 0, "succeeded": 0, "failed": 0, "unknown": 0, "empty": 0, "empty_unknown": 0} for tool in TOOLS}
    shell = []
    for call in calls:
        name = call["name"]
        tool = next((tool for tool in TOOLS if name.endswith("zvec_grep_" + tool)), None)
        if tool is not None:
            count = counts[tool]
            count["attempted"] += 1
            status = call["status"]
            count[{"completed": "succeeded", "error": "failed"}.get(status, "unknown")] += 1
            if status == "completed":
                output = call.get("output")
                if isinstance(output, str) and tool in ("callers", "callees"):
                    if output.lstrip().startswith("No matches.") or ("No resolved call sites." in output and "call at " not in output):
                        count["empty"] += 1
                    elif "call at " not in output:
                        count["empty_unknown"] += 1
                else:
                    count["empty_unknown"] += 1
        if name in ("bash", "shell"):
            inputs = call.get("input")
            if isinstance(inputs, str):
                inputs = json.loads(inputs)
            command = inputs.get("command", "") if isinstance(inputs, dict) else ""
            if re.search(r"(?:^|[\s;&|/])zg\s", command):
                shell.append(call["id"])
    return {"tools": counts, "shell_zg_call_ids": shell}


def collect(args: argparse.Namespace) -> None:
    jobs = _job_dirs(args.runs_dir, "zvec-grep")
    if len(jobs) != 1:
        raise ValueError(f"expected one Harbor job, found {len(jobs)}")
    job = jobs[0]
    _completed_job(job, expected_trials=args.trials)
    reference = load_references(DATA / "references.json")[args.task]
    runtime = resolve_opencode_model("custom-openai/glm-5.2", require_credentials=True)
    completion = default_completion()
    trials = []
    for index, (directory, result) in enumerate(_select_trials(job, args.task, expected_trials=args.trials), 1):
        trial = _profile_result(profile="zvec-grep", job_dir=job, trial_dir=directory, result=result, trial_index=index)
        audit = load_object(directory / "agent/tool-calls.json", label="tool call audit")
        if not audit["complete"]:
            raise ValueError(f"incomplete tool audit: {directory.name}")
        trial["audit"] = audit_calls(audit["calls"])
        trial["calls"] = audit["calls"]
        trial["surface"] = load_object(directory / "agent/tool-surface.json", label="live tool surface")
        trial["evidence"] = str(directory.relative_to(args.runs_dir))
        trial["judge"] = judge_candidate(
            completion_fn=completion, api_key=runtime.api_key,
            api_base=runtime.configuration.base_url, question=reference["question"],
            reference=reference["reference_answer"], candidate=trial["answer"], attempts=3,
        )
        trials.append(trial)
        # Retain earlier evidence if a later judgement fails.
        write_json(args.output / "partial-trials.json", trials)
    setup_paths = sorted(job.glob("*/agent/zvec-grep-setup.json"))
    setup = [load_object(path, label="setup evidence") for path in setup_paths]
    if len(setup) != args.trials or any(s["mcp_toolset"] != args.group or not s["graph_index"] for s in setup):
        raise ValueError("setup evidence does not match experiment")
    surfaces = {t["surface"]["sha256"] for t in trials}
    if len(surfaces) != 1:
        raise ValueError("tool instructions changed between trials")
    report = {
        "protocol": "caller-callee-e2e-v1", "group": args.group, "phase": args.phase,
        "candidate": args.candidate, "harness": args.harness,
        "model": "custom-openai/glm-5.2", "endpoint_sha256": hashlib.sha256(runtime.configuration.base_url.encode()).hexdigest(),
        "judge_config": judge_generation_metadata(), "graph_index": True,
        "task": args.task, "expected_trials": args.trials, "surface_sha256": next(iter(surfaces)),
        "run_url": args.run_url, "trials": trials, "setup": setup,
    }
    write_json(args.output / "task-report.json", report)
    (args.output / "task-report.sha256").write_text(hashlib.sha256((args.output / "task-report.json").read_bytes()).hexdigest() + "\n")
    (args.output / "report.md").write_text(render([report]))


def render(reports: list[dict]) -> str:
    lines = ["# Caller/Callee E2E", "", "All completed tasks; no score/token outlier exclusions. Unused tools remain in the denominator.", "", "| Group | Tasks | Trials | Judge | Input tokens | Output tokens | Tools | Agent seconds |", "|---|---:|---:|---:|---:|---:|---:|---:|"]
    for group in GROUPS:
        selected = [r for r in reports if r["group"] == group]
        if not selected:
            continue
        trials = [trial for r in selected for trial in r["trials"]]
        scores = [mean(t["judge"]["total"] for t in r["trials"]) for r in selected]
        values = [sum(mean(t[key] for t in r["trials"]) for r in selected) for key in ("input_tokens", "output_tokens", "tool_calls", "agent_wall_seconds")]
        lines.append(f"| {group} | {len(selected)} | {len(trials)} | {mean(scores):.2f} | " + " | ".join(f"{value:.2f}" for value in values) + " |")
    lines.extend(["", "## Paired comparisons", "", "Only tasks completed in both arms enter each comparison. Full per-arm results above remain unfiltered.", "", "```json", json.dumps(comparisons(reports), indent=2), "```", "", "## Tool usage rates", "", "```json", json.dumps(usage_rates(reports), indent=2), "```", "", "## Trial tool usage", "", "| Group | Task | Trial | Judge | Search | Callers | Callees | Shell zg |", "|---|---|---|---:|---:|---:|---:|---:|"])
    for report in reports:
        for trial in report["trials"]:
            counts = trial["audit"]["tools"]
            lines.append(f"| {report['group']} | {report['task']} | {trial['trial_name']} | {trial['judge']['total']} | " + " | ".join(str(counts[t]["attempted"]) for t in TOOLS) + f" | {len(trial['audit']['shell_zg_call_ids'])} |")
    lines.extend(["", "Costs missing from the custom provider are unknown. Empty-response classification is unknown unless supported by explicit response evidence. Raw calls retain parameters, responses, status and session IDs.", "", "Usage-stratified results are descriptive, not causal. GLM self-judging and the fixed 20-task selection limit generalization."])
    return "\n".join(lines) + "\n"


def usage_rates(reports: list[dict]) -> dict:
    result = {}
    for group in GROUPS:
        trials = [t for r in reports if r["group"] == group for t in r["trials"]]
        result[group] = {"trials": len(trials), "tools": {
            tool: {"used_trials": sum(t["audit"]["tools"][tool]["attempted"] > 0 for t in trials),
                   "use_rate": mean(t["audit"]["tools"][tool]["attempted"] > 0 for t in trials) if trials else None,
                   **{key: sum(t["audit"]["tools"][tool][key] for t in trials) for key in ("attempted", "succeeded", "failed", "unknown", "empty", "empty_unknown")}}
            for tool in TOOLS}}
    return result


def comparisons(reports: list[dict]) -> list[dict]:
    by_group = {g: {r["task"]: r for r in reports if r["group"] == g} for g in GROUPS}
    result = []
    for treatment, control in (("agent", "search"), ("relationships", "search"), ("agent", "relationships")):
        tasks = sorted(by_group[treatment].keys() & by_group[control].keys())
        comparison = {"treatment": treatment, "control": control, "paired_tasks": tasks, "judge_delta": None, "metrics": {}}
        if tasks:
            comparison["judge_delta"] = mean(
                mean(t["judge"]["total"] for t in by_group[treatment][task]["trials"]) - mean(t["judge"]["total"] for t in by_group[control][task]["trials"])
                for task in tasks)
            for key in ("input_tokens", "output_tokens", "tool_calls", "agent_wall_seconds", "cost_usd"):
                totals = []
                for group in (treatment, control):
                    values = [[t[key] for t in by_group[group][task]["trials"]] for task in tasks]
                    totals.append(None if any(v is None for row in values for v in row) else sum(mean(row) for row in values))
                after, before = totals
                comparison["metrics"][key] = {"treatment": after, "control": before, "change_pct": (after - before) / before * 100 if after is not None and before not in (None, 0) else None}
        result.append(comparison)
    return result


def aggregate(args: argparse.Namespace) -> None:
    reports = []
    for path in sorted(args.reports.rglob("task-report.json")):
        if hashlib.sha256(path.read_bytes()).hexdigest() != path.with_suffix(".sha256").read_text().strip():
            raise ValueError(f"report checksum mismatch: {path}")
        reports.append(load_object(path, label="experiment report"))
    identities = {(r["protocol"], r["candidate"], r["harness"], r["phase"], r["expected_trials"], r["model"], r["endpoint_sha256"]) for r in reports}
    if len(identities) > 1:
        raise ValueError("incompatible experiment reports")
    seen = set()
    surfaces = {}
    for report in reports:
        if report["phase"] != args.phase or report["group"] not in GROUPS:
            raise ValueError("unexpected phase or group")
        if len(report["trials"]) != report["expected_trials"] or not report["graph_index"]:
            raise ValueError("incomplete trials or graph configuration")
        previous = surfaces.setdefault(report["group"], report["surface_sha256"])
        if previous != report["surface_sha256"]:
            raise ValueError("tool instructions differ across tasks in one arm")
        key = (report["group"], report["task"])
        if key in seen:
            raise ValueError(f"duplicate task report: {key}; select one complete attempt explicitly")
        seen.add(key)
    selection = load_object(DATA / "selection.json", label="selection")
    expected_tasks = ["requests-16"] if args.phase == "smoke" else [t["task_slug"] for t in selection["tasks"]]
    groups = GROUPS if args.group == "all" else (args.group,)
    missing = [(g, t) for g in groups for t in expected_tasks if (g, t) not in seen]
    write_json(args.output / "report.json", {"reports": reports, "missing": missing, "complete": not missing, "comparisons": comparisons(reports), "usage_rates": usage_rates(reports)})
    text = render(reports) + "\nMissing tasks: " + json.dumps(missing) + "\n"
    (args.output / "report.md").write_text(text)
    if missing:
        raise ValueError(f"incomplete experiment: {len(missing)} missing group/task reports")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    task = sub.add_parser("collect")
    task.add_argument("--runs-dir", type=Path, required=True)
    task.add_argument("--task", required=True)
    task.add_argument("--trials", type=int, required=True)
    task.add_argument("--candidate", required=True)
    task.add_argument("--harness", required=True)
    task.add_argument("--run-url", required=True)
    task.add_argument("--group", choices=GROUPS, required=True)
    summary = sub.add_parser("aggregate")
    summary.add_argument("--reports", type=Path, required=True)
    summary.add_argument("--group", choices=(*GROUPS, "all"), required=True)
    for command in (task, summary):
        command.add_argument("--phase", choices=("smoke", "full"), required=True)
        command.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    (collect if args.command == "collect" else aggregate)(args)


if __name__ == "__main__":
    main()
