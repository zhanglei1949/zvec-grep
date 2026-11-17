"""Gated official 0.3.1 evaluation: three technical smokes, then 900 trials.

No product code or historical report is changed. All new outputs live in one
dedicated run directory; a failed smoke cannot fall through to the formal set.
"""
from __future__ import annotations

import argparse
import concurrent.futures
import dataclasses
import hashlib
import json
import os
import random
import re
import shutil
import subprocess
import time
from pathlib import Path
from datetime import datetime
from jsonschema import Draft202012Validator

from .engines.judge import default_completion, judge_candidate, _response_mapping
from .engines.registry import execution_environment, resolve_opencode_model
from .model_requests import audit_model_requests
from .release_analysis import GROUPS, METRICS, events, evidence_manifest, mcp_calls, summarize, tool_attempts, render_report
from .release_packages import VERSION, prepare_release, sha256, verify_release, verify_install, write_json
from .runner import BenchmarkSuite, build_harbor_command, load_suite
from .swe_qa.collect import _profile_result
from .swe_qa.pairs import load_references
from .tool_experiment import audit_calls

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "zg_bench/swe_qa/data"
MODEL_REVISION = "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b"


def release_command(suite: BenchmarkSuite, manifest: Path, jobs: Path, group: str, number: int, *, overlay: Path, smoke: bool = False) -> list[str]:
    if group not in GROUPS or number not in (0, 1, 2, 3) or smoke != (number == 0):
        raise ValueError("Invalid release stage or arm")
    command = build_harbor_command(suite, profile="zvec-grep", agent="opencode", model="custom-openai/glm-5.2",
        jobs_dir=jobs, job_name=f"r{number}-{group}-zvec-grep", n_attempts=1 if smoke else 5, max_retries=0,
        mcp_toolset=group, record_model_requests=True, embedding_model="local/potion-code-16m-v2",
        zvec_grep_package="@zvec/zvec-grep@0.3.1")
    command[command.index("--agent") + 1] = "zg_bench.agents.release_opencode:ReleaseOpenCode"
    command[command.index("--extra-docker-compose") + 1] = str(overlay)
    command += ["--agent-kwarg", f"release_bundle={manifest.resolve()}", "--agent-kwarg", f"round_number={number}"]
    # No old native binding package is installed by the release adapter.
    for index in range(len(command) - 1, 0, -1):
        if command[index].startswith("zvec_binding_package="):
            command[index] = "zvec_binding_package=@zvec/zvec-grep-linux-x64-gnu@0.3.1"
    if smoke:
        command += ["--agent-kwarg", "technical_smoke=true"]
    return command


def prepare_overlay(output: Path, group: str, slot: int, run_id: str) -> Path:
    prefix = "zg-release-" + hashlib.sha256(run_id.encode()).hexdigest()[:12]
    names = {"release-node": f"{prefix}-{group}-{slot}", "release-models": f"{prefix}-models"}
    for name in names.values():
        subprocess.run(["docker", "volume", "create", name], stdout=subprocess.DEVNULL, check=True)
    path = output / "overlays" / f"{group}-{slot}.json"
    write_json(path, {"services": {"main": {"volumes": [
        {"type": "volume", "source": "release-node", "target": "/root/.nvm"},
        {"type": "volume", "source": "release-models", "target": "/root/.zvec-grep/models"},
    ]}}, "volumes": {key: {"external": True, "name": value} for key, value in names.items()}})
    return path


def make_smoke_task(output: Path, group: str) -> BenchmarkSuite:
    task = output / "development-tasks" / group / "release-dev"
    (task / "environment").mkdir(parents=True)
    shutil.copyfile(ROOT / "fixtures/caller-callee-dev/lib.rs", task / "environment/lib.rs")
    (task / "environment/Dockerfile").write_text('''FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git && rm -rf /var/lib/apt/lists/*
COPY lib.rs /app/lib.rs
WORKDIR /app
RUN git init && git -c user.name=fixture -c user.email=fixture@example.invalid add lib.rs && GIT_AUTHOR_DATE=2026-01-01T00:00:00Z GIT_COMMITTER_DATE=2026-01-01T00:00:00Z git -c user.name=fixture -c user.email=fixture@example.invalid commit -m fixture
''')
    (task / "task.toml").write_text('''version = "1.0"
[agent]
timeout_sec = 1800.0
[verifier]
timeout_sec = 60.0
[environment]
build_timeout_sec = 1200.0
''')
    (task / "tests").mkdir()
    shutil.copyfile(ROOT / "datasets/xarray-32/tests/test.sh", task / "tests/test.sh")
    if group == "search":
        action = 'call zvec_grep_zvec_grep_search with {"root":"/app","fts":"normalize","globs":"lib.rs"}'
    else:
        action = ('call zvec_grep_zvec_grep_callers with {"root":"/app","symbol":"::save","globs":"lib.rs","limit":20}, '
                  'then call zvec_grep_zvec_grep_callees with the same arguments')
    prompt = ("This is an integration test in the current workspace. First " + action + ". "
              "Then use the task tool with subagent_type explore and ask that subagent to repeat those same MCP calls. "
              "Both you and the subagent must actually execute the advertised tools and inspect their returned source locations. "
              "Read the relevant source and report which functions directly call save and what save calls. Do not use web tools.")
    (task / "instruction.md").write_text(prompt + "\n")
    return BenchmarkSuite(name="release-dev", dataset=None, tier="full", tasks=("release-dev",), path=task.parent)


def collect_trial(directory: Path, *, task: str, group: str, number: int, repeat: int) -> dict:
    row = {"task": task, "group": group, "round": number, "repeat": repeat, "status": "failed",
           "failure_stage": "setup", "evidence": str(directory), "judge": None, "usage_complete": False}
    agent = directory / "agent"
    usage_path = agent / "session-usage.json"
    if usage_path.exists():
        usage = json.loads(usage_path.read_text())
        row.update({key: usage.get("total", {}).get(key) for key in METRICS if key != "agent_wall_seconds"})
        row["usage_complete"] = usage.get("complete", False)
    row["model_attempts"] = tool_attempts(agent)
    native_surface = agent / "tool-surface.native.json"
    if native_surface.exists():
        row["native_surface"] = json.loads(native_surface.read_text())
    surface_path = agent / "tool-surface.json"
    if surface_path.exists():
        row["surface"] = json.loads(surface_path.read_text())
        schemas = {"zvec_grep_" + tool["name"]: tool["inputSchema"] for tool in row["surface"]["tools"]}
        for call in row["model_attempts"]:
            schema = schemas.get(call["name"])
            call["schema_valid"] = bool(call["valid_json"] and schema is not None and Draft202012Validator(schema).is_valid(call["arguments"]))
    row["mcp"] = mcp_calls(agent)
    captured = events(agent / "model-requests.jsonl")
    row["first_request_sent"] = any(event["type"] == "request" for event in captured)
    row["context_errors"] = [event for event in captured if event["type"] == "guidance_rejected"]
    row["guard_stops"] = [event for event in captured if event["type"] == "guard_stop"]
    row["failure_stage"] = "agent" if row["first_request_sent"] else "setup"
    row["calls"] = []
    calls_path = agent / "tool-calls.json"
    if calls_path.exists():
        row["calls"] = json.loads(calls_path.read_text()).get("calls", [])
    row["tool_audit"] = audit_calls(row["calls"])
    setup = agent / "zvec-grep-setup.json"
    if setup.exists():
        row["setup"] = json.loads(setup.read_text())
    try:
        result = json.loads((directory / "result.json").read_text())
        row["exception"] = result.get("exception_info")
        timing = result.get("agent_execution") or {}
        if timing.get("started_at") and timing.get("finished_at"):
            row["agent_wall_seconds"] = (datetime.fromisoformat(timing["finished_at"]) - datetime.fromisoformat(timing["started_at"])).total_seconds()
        values = _profile_result(profile="zvec-grep", job_dir=directory.parent, trial_dir=directory, result=result, trial_index=repeat)
        surface = row["surface"]
        request_audit = audit_model_requests(agent / "model-requests.jsonl", surface, row["calls"])
        row["surface"] = surface
        row["request_audit"] = request_audit
        verified = [event for event in captured if event["type"] == "guidance_verified"]
        responses = [event for event in captured if event["type"] == "response"]
        raw_complete = all((agent / "model-responses" / f"{event['id']}.raw").exists() for event in responses)
        if row["context_errors"] or not verified or not verified[0].get("first_task_request") or request_audit["context_issue_request_ids"] or not raw_complete or not row["mcp"]["complete"]:
            raise ValueError("Incomplete release context or transport evidence")
        row.update(values, status="completed", failure_stage=None)
    except Exception as error:
        row["collection_error"] = f"{type(error).__name__}: {error}"
        if row["context_errors"]:
            row["failure_stage"] = "context"
    return row


def collect_job(jobs: Path, *, task: str, group: str, number: int, expected: int) -> list[dict]:
    directories = sorted(jobs.glob("*/*/result.json"), key=lambda path: json.loads(path.read_text()).get("started_at") or "")
    if len(directories) > expected:
        raise ValueError("Unexpected extra trials; release trials must never be retried")
    rows = [collect_trial(path.parent, task=task, group=group, number=number, repeat=index)
            for index, path in enumerate(directories, 1)]
    for repeat in range(len(rows) + 1, expected + 1):
        rows.append({"task": task, "group": group, "round": number, "repeat": repeat, "status": "missing",
                     "failure_stage": "setup", "usage_complete": False, "judge": None, "evidence": str(jobs)})
    return rows


def check_smoke(row: dict) -> list[str]:
    errors = []
    if row["status"] != "completed":
        errors.append("technical_session_incomplete")
    attempts = row.get("model_attempts", [])
    if any(not call["valid_json"] for call in attempts):
        errors.append("model_emitted_invalid_arguments")
    if any(call["name"].startswith("zvec_grep_") and not call.get("schema_valid") for call in attempts):
        errors.append("model_arguments_failed_published_schema")
    if row.get("context_errors"):
        errors.append("guidance_or_schema_rejected")
    required = ["search"] if row["group"] == "search" else ["callers", "callees"]
    for tool in required:
        if not any(call["name"].endswith("zvec_grep_" + tool) for call in attempts):
            errors.append(f"{tool}_raw_model_attempt_missing")
        calls = [call for call in row.get("calls", []) if call["name"].endswith("zvec_grep_" + tool) and call["status"] == "completed"
                 and isinstance(call.get("input"), dict) and call["input"].get("globs") == "lib.rs"]
        sessions = {call["session_id"] for call in calls}
        root_session = row.get("session_usage", {}).get("root_session_id")
        if len(sessions) < 2 or root_session not in sessions:
            errors.append(f"{tool}_not_executed_by_both_main_and_child")
        if tool != "search" and not all("call at" in (call.get("output") or "") for call in calls):
            errors.append(f"{tool}_no_locations")
        if tool != "search":
            source = (ROOT / "fixtures/caller-callee-dev/lib.rs").read_text().splitlines()
            expected = {(index, line.index("save(") + 1) for index, line in enumerate(source, 1) if line.startswith(("pub fn import_record", "pub fn export_record"))} if tool == "callers" else {(index, line.index("normalize(") + 1) for index, line in enumerate(source, 1) if line.startswith("pub fn save")}
            for call in calls:
                locations = {(int(line), int(column)) for line, column in re.findall(r"call at lib\.rs:(\d+):(\d+)", call.get("output") or "")}
                if locations != expected:
                    errors.append(f"{tool}_fixture_locations_differ_from_source")
    if not row.get("surface", {}).get("preflight_calls"):
        errors.append("native_mcp_preflight_missing")
    if {call["name"] for call in row.get("native_surface", {}).get("preflight_calls", [])} != {"zvec_grep_search", "zvec_grep_callers", "zvec_grep_callees"}:
        errors.append("three_tool_native_preflight_missing")
    return errors


def run_job(suite: BenchmarkSuite, *, manifest: Path, output: Path, group: str, number: int, task: str, overlay: Path, smoke: bool = False) -> list[dict]:
    destination = output / ("smoke" if smoke else f"round-{number}") / group / task
    destination.mkdir(parents=True, exist_ok=False)
    jobs = destination / "jobs"
    command = release_command(suite, manifest, jobs, group, number, overlay=overlay, smoke=smoke)
    write_json(destination / "command.json", command)
    started = time.monotonic()
    with (destination / "harbor.stdout").open("w") as stdout, (destination / "harbor.stderr").open("w") as stderr:
        result = subprocess.run(command, env=execution_environment(agent="opencode", model="custom-openai/glm-5.2"), stdout=stdout, stderr=stderr)
    rows = collect_job(jobs, task=task, group=group, number=number, expected=1 if smoke else 5)
    write_json(destination / "task-report.json", {"returncode": result.returncode, "wall_seconds": time.monotonic() - started, "trials": rows})
    print(json.dumps({"round": number, "group": group, "task": task, "completed": sum(row["status"] == "completed" for row in rows), "planned": len(rows)}), flush=True)
    return rows


def score_rows(rows: list[dict], output: Path) -> None:
    references = load_references(DATA / "references.json")
    runtime = resolve_opencode_model("custom-openai/glm-5.2", require_credentials=True)
    complete = default_completion()
    selected = [row for row in rows if row["status"] == "completed" and row.get("judge") is None and "judge_error" not in row]
    random.Random(42).shuffle(selected)
    for row in selected:
        directory = output / f"round-{row['round']}" / row["group"] / row["task"] / f"judge-{row['repeat']}"
        directory.mkdir(exist_ok=False)
        responses = []
        def recording_completion(**kwargs):
            write_json(directory / f"request-{len(responses)+1}.json", {key: value for key, value in kwargs.items() if key not in ("api_key", "api_base")})
            try:
                response = complete(**kwargs)
            except Exception as error:
                # Provider exception text can echo headers; persist only its type.
                responses.append({"error_type": type(error).__name__})
                write_json(directory / f"response-{len(responses)}.json", responses[-1])
                raise
            responses.append(_response_mapping(response))
            write_json(directory / f"response-{len(responses)}.json", responses[-1])
            return response
        reference = references[row["task"].replace("-", ":")]
        try:
            row["judge"] = judge_candidate(completion_fn=recording_completion, api_key=runtime.api_key,
                api_base=runtime.configuration.base_url, question=reference["question"], reference=reference["reference_answer"],
                candidate=row["answer"], attempts=3)
        except Exception as error:
            row["judge_error"] = type(error).__name__
            row["failure_stage"] = "judge"
        write_json(directory / "judgement.json", row.get("judge"))
        write_json(output / "trials.json", rows)


def run(args) -> int:
    manifest_path = args.release.resolve()
    manifest = verify_release(manifest_path)
    if manifest["platform"] != "linux-x64-gnu":
        raise ValueError("Use the Linux release package on the Linux/Docker runner")
    if shutil.which("docker") is None:
        raise RuntimeError("Release E2E requires Docker; use the Linux Actions entry")
    resolve_opencode_model("custom-openai/glm-5.2", require_credentials=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    selection = json.loads((DATA / "selection.json").read_text())
    tasks = [task["task_slug"] for task in selection["tasks"]]
    identity = {"protocol": "qa20-release-e2e-0.3.1-v1", "release": manifest, "harness": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
                "rounds": 3, "trials_per_task_group_round": 5, "formal_trials": 900, "model_revision": MODEL_REVISION,
                "input_sha256": {name: sha256(DATA / name) for name in ("selection.json", "references.json", "index.ignore")}}
    write_json(output / "protocol.json", identity)
    seeds = output / "index-snapshots"
    seeds.mkdir()
    os.environ["ZG_BENCH_INDEX_SEED_DIR"] = str(seeds)
    overlays = {(group, slot): prepare_overlay(output, group, slot, str(output)) for group in GROUPS for slot in (0, 1)}
    smoke_rows = []
    rows = []
    try:
        for group in GROUPS:
            smoke_rows.extend(run_job(make_smoke_task(output, group), manifest=manifest_path, output=output,
                group=group, number=0, task="release-dev", overlay=overlays[group, 0], smoke=True))
            if check_smoke(smoke_rows[-1]):
                break
        smoke = {"passed": len(smoke_rows) == len(GROUPS) and all(not check_smoke(row) for row in smoke_rows),
                 "groups": [{"group": row["group"], "errors": check_smoke(row)} for row in smoke_rows],
                 "not_started_groups": [group for group in GROUPS if group not in {row["group"] for row in smoke_rows}], "trials": smoke_rows}
        write_json(output / "smoke-gate.json", smoke)
        if not smoke["passed"]:
            print("Technical smoke blocked formal evaluation; no QA20 model trial was started.", flush=True)
            return 2
        if not args.execute_full:
            print("Technical smoke passed; formal evaluation was not requested.", flush=True)
            return 0
        base = load_suite("swe-qa-bench", tier="full")
        for number in range(1, 4):
            def worker(group, slot):
                result = []
                for task in tasks[slot::2]:
                    result.extend(run_job(dataclasses.replace(base, tasks=(task,)), manifest=manifest_path, output=output,
                        group=group, number=number, task=task, overlay=overlays[group, slot]))
                return result
            with concurrent.futures.ThreadPoolExecutor(max_workers=6) as executor:
                futures = [executor.submit(worker, group, slot) for group in GROUPS for slot in (0, 1)]
                for future in concurrent.futures.as_completed(futures):
                    rows.extend(future.result())
                    write_json(output / "trials.json", rows)
            score_rows(rows, output)
            report = summarize(rows, tasks)
            write_json(output / "report.json", report)
            (output / "report.md").write_text(render_report(report))
            if any(row.get("context_errors") for row in rows):
                print("Protocol context error; later rounds stopped.", flush=True)
                return 2
        return 0 if summarize(rows, tasks)["complete"] else 2
    finally:
        write_json(output / "evidence-sha256.json", evidence_manifest(output))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    fetch = commands.add_parser("fetch-release")
    fetch.add_argument("--output", type=Path, required=True)
    fetch.add_argument("--platform", default="linux-x64-gnu", choices=("linux-x64-gnu", "darwin-arm64"))
    install = commands.add_parser("verify-install", help="official postinstall/runtime check; no Agent or model")
    install.add_argument("--release", type=Path, required=True)
    install.add_argument("--prefix", type=Path, required=True)
    execute = commands.add_parser("run")
    execute.add_argument("--release", type=Path, required=True)
    execute.add_argument("--output", type=Path, required=True)
    execute.add_argument("--execute-full", action="store_true", help="only after all three technical smokes pass")
    args = parser.parse_args()
    if args.command == "fetch-release":
        prepare_release(args.output, platform=args.platform)
    elif args.command == "verify-install":
        print(json.dumps(verify_install(args.release.resolve(), args.prefix)))
    else:
        raise SystemExit(run(args))


if __name__ == "__main__":
    main()
