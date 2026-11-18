"""Bounded CI shards for the unchanged 900-slot official-release protocol."""
from __future__ import annotations

import argparse
import concurrent.futures
import dataclasses
import hashlib
import json
import os
import shutil
import subprocess
from pathlib import Path

from .engines.registry import execution_environment
from .release_analysis import GROUPS, summarize, render_report, evidence_manifest
from .release_e2e import DATA, prepare_overlay, release_command, run_job, score_rows
from .release_packages import sha256, verify_release, write_json
from .runner import load_suite

MODEL = "custom-openai/qwen3.8-max"


def plan() -> dict:
    selected = json.loads((DATA / "selection.json").read_text())["tasks"]
    sources, tasks = {}, []
    for task in selected:
        key = (task["repository"], task["repository_commit"])
        index_id = sources.setdefault(key, task["task_slug"])
        tasks.append({"task": task["task_slug"], "index": index_id})
    if len(tasks) != 20 or len({t["task"] for t in tasks}) != 20:
        raise ValueError("Full release run requires exactly the locked twenty tasks")
    return {"tasks": tasks, "indexes": [{"task": task} for task in sources.values()],
            "source": {t["task_slug"]: t for t in selected}, "planned_trials": 900,
            "input_sha256": {name: sha256(DATA / name) for name in ("selection.json", "references.json", "index.ignore")}}


def validate_index(directory: Path, task: str, manifest: dict) -> Path:
    expected = plan()["source"][task]
    checks = list(directory.rglob("payload-sha256.json"))
    if len(checks) != 1:
        raise ValueError("Expected exactly one frozen repository index")
    seed = checks[0].parent
    identity = json.loads((seed / "identity.json").read_text())
    digest = hashlib.sha256(json.dumps(identity, separators=(",", ":"), sort_keys=True).encode()).hexdigest()
    if seed.name != digest or (seed / "complete").read_text().strip() != digest:
        raise ValueError("Frozen index identity or completion marker differs")
    if (identity["repo_commit"] != expected["repository_commit"] or identity["zvec_grep_version"] != "0.3.1"
            or identity["zvec_grep_package"] != manifest["packages"][0]["sha256"]
            or identity["release_platform_sha256"] != manifest["packages"][1]["sha256"]
            or identity["embedding_model"] != "local/potion-code-16m-v2" or identity["device"] != "cpu"
            or identity["index_ignore_sha256"] != plan()["input_sha256"]["index.ignore"]
            or not identity["graph_index"]):
        raise ValueError("Frozen index does not match the selected source/release/model")
    workspace = seed / "workspace"
    hashes = {str(p.relative_to(workspace)): sha256(p) for p in workspace.rglob("*") if p.is_file()}
    if not hashes or hashes != json.loads(checks[0].read_text()):
        raise ValueError("Frozen index payload hash mismatch")
    return seed


def identity(task: str, number: int, manifest: dict) -> dict:
    p = plan()
    if task not in p["source"] or number not in (1, 2, 3):
        raise ValueError("Unexpected task or round")
    return {"protocol": "qa20-release-full-shards-v1", "task": task, "round": number,
            "harness": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
            "release": manifest, "model": MODEL, "judge_model": "qwen3.8-max",
            "input_sha256": p["input_sha256"], "source_commit": p["source"][task]["repository_commit"]}


def index_command(suite, release: Path, jobs: Path, overlay: Path) -> list[str]:
    command = release_command(suite, release, jobs, "agent", 1, overlay=overlay, model=MODEL)
    command[command.index("--agent") + 1] = "zg_bench.agents.release_index:ReleaseIndexOnly"
    command[command.index("--n-attempts") + 1] = "1"
    # Harbor resolves --agent-env before constructing even an index-only agent.
    # This stage never runs OpenCode, so it must not require model credentials.
    for position in range(len(command) - 2, -1, -1):
        if command[position] == "--agent-env" and command[position + 1].startswith("OPENAI_API_KEY="):
            del command[position:position + 2]
    return command


def build_index(args) -> None:
    manifest = verify_release(args.release.resolve())
    if args.task not in {r["task"] for r in plan()["indexes"]}:
        raise ValueError("Index builders use the unique source representatives")
    output = args.output.resolve(); output.mkdir(parents=True, exist_ok=False)
    seeds = output / "index-snapshots"; seeds.mkdir()
    os.environ["ZG_BENCH_INDEX_SEED_DIR"] = str(seeds)
    suite = dataclasses.replace(load_suite("swe-qa-bench", tier="full"), tasks=(args.task,))
    overlay = prepare_overlay(output, "agent", 0, str(output))
    command = index_command(suite, args.release.resolve(), output / "jobs", overlay)
    write_json(output / "command.json", command)
    with (output / "stdout.log").open("w") as stdout, (output / "stderr.log").open("w") as stderr:
        result = subprocess.run(command, env=execution_environment(agent="opencode", model=MODEL), stdout=stdout, stderr=stderr)
    if list(output.rglob("model-requests.jsonl")) or len(list(output.rglob("index-only.json"))) != 1:
        raise RuntimeError("Index preparation did not finish without an answering session")
    seed = validate_index(seeds, args.task, manifest)
    write_json(output / "index-manifest.json", {**identity(args.task, 1, manifest), "seed_key": seed.name,
               "model_requests": 0, "harbor_returncode": result.returncode})


def task_records(output: Path, meta: dict, rows: list[dict]) -> None:
    write_json(output / "records/task-records.json", {"identity": meta, "rows": rows,
               "protocol_errors": any(row.get("context_errors") for row in rows)})


def run_task(args) -> None:
    manifest = verify_release(args.release.resolve())
    meta = identity(args.task, args.round, manifest)
    seed = validate_index(args.index.resolve(), args.task, manifest)
    output = args.output.resolve(); output.mkdir(parents=True, exist_ok=False)
    seeds = output / "index-snapshots"; seeds.mkdir()
    shutil.copytree(seed, seeds / seed.name)
    os.environ["ZG_BENCH_INDEX_SEED_DIR"] = str(seeds)
    suite = dataclasses.replace(load_suite("swe-qa-bench", tier="full"), tasks=(args.task,))
    overlays = {group: prepare_overlay(output, group, 0, str(output)) for group in GROUPS}
    rows = []
    def worker(group):
        return run_job(suite, manifest=args.release.resolve(), output=output, group=group, number=args.round,
                       task=args.task, overlay=overlays[group], model=MODEL, require_index_seed=True)
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as executor:
        futures = {executor.submit(worker, group): group for group in GROUPS}
        for future in concurrent.futures.as_completed(futures):
            group = futures[future]
            try:
                rows.extend(future.result())
            except Exception as error:
                rows.extend({"task": args.task, "group": group, "round": args.round, "repeat": repeat,
                             "status": "missing", "failure_stage": "infrastructure", "exception_type": type(error).__name__,
                             "judge": None, "usage_complete": False, "evidence": str(output)} for repeat in range(1, 6))
            task_records(output, meta, rows)
    # A judge failure only retries the saved answer; Agent attempts stay fixed.
    score_rows(rows, output, model="qwen3.8-max", on_progress=lambda: task_records(output, meta, rows))
    task_records(output, meta, rows)
    write_json(output / "execution-status.json", {"formal_trial_records": len(rows),
               "trials_with_model_requests": sum(bool(row.get("first_request_sent")) for row in rows),
               "answering_retries": 0, "round": args.round, "task": args.task})
    write_json(output / "evidence-sha256.json", evidence_manifest(output))


def aggregate(args) -> int:
    p = plan(); manifest = verify_release(args.release.resolve())
    rows, seen, protocols = [], set(), []
    for file in sorted(args.records.rglob("task-records.json")):
        record = json.loads(file.read_text()); meta = record["identity"]
        expected = identity(meta["task"], meta["round"], manifest)
        if meta != expected or meta["round"] > args.through_round:
            raise ValueError("Mixed full-run task/release/model/harness identity")
        protocols.append(bool(record["protocol_errors"]))
        for row in record["rows"]:
            key = tuple(row[k] for k in ("group", "task", "round", "repeat"))
            if key in seen or row["task"] != meta["task"] or row["round"] != meta["round"]:
                raise ValueError("Duplicate or misplaced full-run trial slot")
            seen.add(key); rows.append(row)
    for number in range(1, args.through_round + 1):
        for task in p["source"]:
            for group in GROUPS:
                for repeat in range(1, 6):
                    key = (group, task, number, repeat)
                    if key not in seen:
                        rows.append(dict(zip(("group", "task", "round", "repeat"), key), status="missing",
                                         failure_stage="infrastructure", judge=None, usage_complete=False))
    report = summarize(rows, list(p["source"]), rounds=args.through_round)
    report["through_round"] = args.through_round
    report["full_cohort_planned"] = 900
    report["model"] = MODEL; report["judge_model"] = "qwen3.8-max"
    report["relationship_adoption"] = {}
    for group in GROUPS:
        selected = [r for r in rows if r["group"] == group]
        adopted = sum(any(c["name"].endswith(("zvec_grep_callers", "zvec_grep_callees")) for c in r.get("model_attempts", [])) for r in selected)
        report["relationship_adoption"][group] = {"planned_slots_so_far": len(selected), "selected_trials": adopted,
             "selection_rate": adopted / len(selected) if selected else None,
             "trials_with_model_requests": sum(bool(r.get("first_request_sent")) for r in selected)}
    args.output.mkdir(parents=True, exist_ok=False)
    write_json(args.output / "trials.json", rows)
    write_json(args.output / "report.json", report)
    adoption_lines = ["", "## 关系工具采用率", "", "| 组别 | 选择关系工具的会话 / 本轮累计计划 | 采用率 |", "|---|---:|---:|"]
    for group, value in report["relationship_adoption"].items():
        adoption_lines.append(f"| {group} | {value['selected_trials']} / {value['planned_slots_so_far']} | {value['selection_rate']:.2%} |")
    (args.output / "report.md").write_text(render_report(report) + "\n".join(adoption_lines) + "\n")
    write_json(args.output / "progress.json", {"round": args.through_round, "planned": 900,
               "records": len(rows), "model_started": sum(bool(r.get("first_request_sent")) for r in rows),
               "scored": sum(r.get("judge") is not None for r in rows), "protocol_errors": any(protocols)})
    return 2 if any(protocols) else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("plan")
    for command in ("index", "task"):
        sub = commands.add_parser(command)
        sub.add_argument("--task", required=True)
        sub.add_argument("--release", type=Path, required=True)
        sub.add_argument("--output", type=Path, required=True)
        if command == "task":
            sub.add_argument("--index", type=Path, required=True)
            sub.add_argument("--round", type=int, choices=(1, 2, 3), required=True)
    sub = commands.add_parser("aggregate")
    sub.add_argument("--records", type=Path, required=True)
    sub.add_argument("--release", type=Path, required=True)
    sub.add_argument("--output", type=Path, required=True)
    sub.add_argument("--through-round", type=int, choices=(1, 2, 3), required=True)
    args = parser.parse_args()
    if args.command == "plan":
        p = plan()
        print("tasks=" + json.dumps(p["tasks"], separators=(",", ":")))
        print("indexes=" + json.dumps(p["indexes"], separators=(",", ":")))
    elif args.command == "index": build_index(args)
    elif args.command == "task": run_task(args)
    else: raise SystemExit(aggregate(args))


if __name__ == "__main__":
    main()
