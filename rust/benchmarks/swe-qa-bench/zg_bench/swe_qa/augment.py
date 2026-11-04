"""Local entry point: python -m zg_bench.swe_qa.augment --help."""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import shutil
import subprocess
import tarfile
from dataclasses import replace
from datetime import UTC, datetime
from pathlib import Path

from .augment_protocol import (
    DATA, PROTOCOL, ROOT, configuration, digest, matrix, read_json, write_json,
)
from .validation import validate_assets


def package_identity(path: Path) -> dict:
    """Check the packaged payload, rather than trusting a candidate ref label."""
    with tarfile.open(path, "r:gz") as archive:
        def contents(name):
            stream = archive.extractfile("package/" + name)
            if stream is None:
                raise ValueError(f"missing package member: {name}")
            return stream.read()
        package = json.loads(contents("package.json"))
        if package.get("os") != ["linux"] or package.get("cpu") != ["x64"]:
            raise ValueError("Docker tasks require the Linux x64 Rust local package")
        checksums = json.loads(contents("checksums.json"))
        files = checksums["files"]
        binary = contents("bin/zg")
        if binary[:4] != b"\x7fELF" or binary[18:20] != b"\x3e\x00":
            raise ValueError("candidate is not an ELF x86-64 Rust executable")
        for name, expected in files.items():
            payload = contents(name)
            if len(payload) != expected["bytes"] or hashlib.sha256(payload).hexdigest() != expected["sha256"]:
                raise ValueError(f"native payload checksum mismatch: {name}")
        if "bin/zg" not in files:
            raise ValueError("binary missing from checksums")
    return {"sha256": digest(path), "metadata": package, "native_payload": checksums}


def commands(output: Path, tier: str, package: str, package_hash: str) -> list[list[str]]:
    from ..runner import build_harbor_command, load_suite
    suite = load_suite("swe-qa-bench", tier=tier)
    result = []
    for row in matrix(tier):
        command = build_harbor_command(
            replace(suite, tasks=(row["task_slug"],)), profile="zvec-grep",
            agent="opencode", model="qwen3.7-max", jobs_dir=output / "jobs",
            job_name=row["job"], n_attempts=1, zvec_grep_package=package,
            zvec_grep_package_sha256=package_hash,
        )
        command[command.index("--agent") + 1] = "zg_bench.agents.augment_opencode:AugmentOpenCode"
        for index, value in enumerate(command):
            if value.startswith("opencode_config="):
                config = json.loads(value.split("=", 1)[1])
                config["mcp"] = {"zvec_grep": {
                    "type": "local", "command": ["node", "/tmp/zg-augment-proxy.mjs", "/tmp/zg-augment-proxy.json"],
                    "enabled": True, "timeout": 40000,
                }}
                config["instructions"] = ["/tmp/zg-augment-instructions.md"]
                config["tools"] = {"task": False}
                config["provider"]["dashscope"]["options"]["baseURL"] = "http://127.0.0.1:7998"
                command[index] = "opencode_config=" + json.dumps(config, separators=(",", ":"))
        for key, value in {
            "graph_profile": row["profile"], "graph_seed_dir": str(output / "seeds"),
            "repository_commit": row["commit"],
        }.items():
            command.extend(["--agent-kwarg", f"{key}={value}"])
        command.extend(["--max-retries", "0"])
        result.append(command)
    return result


def freeze(output: Path, tier: str, package: Path) -> dict:
    validate_assets(selection_path=DATA / "selection.json", references_path=DATA / "references.json", dataset_path=ROOT / "datasets")
    identity = package_identity(package)
    output.mkdir(parents=True, exist_ok=False)
    candidate = output / "candidate.tgz"
    shutil.copyfile(package, candidate)
    repository = ROOT.parents[2]
    def git(*args):
        return subprocess.check_output(["git", "-C", str(repository), *args])
    diff = git("diff", "HEAD", "--binary")
    (output / "source.patch").write_bytes(diff)
    untracked = git("ls-files", "--others", "--exclude-standard", "-z").split(b"\0")
    sources = {
        name.decode(): digest(repository / name.decode())
        for name in untracked if name and (repository / name.decode()).is_file()
    }
    harness = {path.relative_to(ROOT).as_posix(): digest(path)
               for folder in (ROOT / "zg_bench", ROOT / "datasets", ROOT / "suites")
               for path in sorted(folder.rglob("*"))
               if path.is_file() and "__pycache__" not in path.parts}
    manifest = {
        **configuration(), "tier": tier, "created_at": datetime.now(UTC).isoformat(),
        "candidate": identity, "commit": git("rev-parse", "HEAD").decode().strip(),
        "source_status": git("status", "--porcelain").decode(),
        "source_patch_sha256": digest(output / "source.patch"),
        "untracked_sources": sources, "harness": harness,
        "assets": {name: digest(DATA / name) for name in ("selection.json", "references.json")},
        "lock_sha256": digest(ROOT / "uv.lock"), "python": platform.python_version(),
        "host": {"system": platform.system(), "machine": platform.machine(), "cpu": platform.processor()},
        "planned_runs": len(matrix(tier)), "matrix": matrix(tier),
    }
    write_json(output / "manifest.json", manifest)
    write_json(output / "matrix.json", matrix(tier))
    return manifest


def run(output: Path, tier: str, package: Path, smoke: Path | None, dry_run: bool) -> None:
    from ..runner import execution_environment, prepare_setup_cache, validate_profile_credentials
    if not dry_run:
        if shutil.which("docker") is None:
            raise ValueError("Docker is required; no Agent trial was started")
        subprocess.run(["docker", "info"], check=True, capture_output=True)
        validate_profile_credentials(("zvec-grep",), agent="opencode", model="qwen3.7-max")
        if tier == "full":
            if smoke is None:
                raise ValueError("full run requires --smoke pointing to an accepted smoke experiment")
            accepted = read_json(smoke / "report" / "report.json")
            prior = read_json(smoke / "manifest.json")
            if (not accepted["integrity"]["passed"] or not accepted["scoring_complete"]
                or not accepted["usage_complete"] or prior["tier"] != "smoke"):
                raise ValueError("smoke experiment has not passed integrity and scoring")
            if prior["candidate"] != package_identity(package) or any(prior[key] != configuration()[key] for key in configuration()):
                raise ValueError("candidate or protocol differs from accepted smoke")
    manifest = freeze(output, tier, package)
    if tier == "full" and not dry_run and manifest["harness"] != prior["harness"]:
        raise ValueError("benchmark harness changed since accepted smoke; run a new smoke experiment")
    if dry_run:
        planned = commands(output, tier, str(output / "candidate.tgz"), manifest["candidate"]["sha256"])
        write_json(output / "commands.json", planned)
        print(f"{PROTOCOL}: {len(planned)} planned runs; no Docker or model calls")
        return
    prepared = prepare_setup_cache("opencode", "zvec-grep", zvec_grep_package=str(output / "candidate.tgz"))
    planned = commands(output, tier, prepared.zvec_grep_package, manifest["candidate"]["sha256"])
    write_json(output / "commands.json", planned)
    outcomes = []
    try:
        for row, command in zip(matrix(tier), planned, strict=True):
            started = datetime.now(UTC).isoformat()
            print(f"{row['job']}: starting", flush=True)
            with (output / f"{row['job']}.log").open("x") as log:
                result = subprocess.run(command, cwd=ROOT,
                    env=execution_environment(agent="opencode", model="qwen3.7-max"),
                    stdout=log, stderr=subprocess.STDOUT, check=False)
            outcomes.append({**row, "started_at": started,
                             "finished_at": datetime.now(UTC).isoformat(), "returncode": result.returncode})
            print(f"{row['job']}: exit {result.returncode}", flush=True)
            if result.returncode:
                print("Failure retained; continuing remaining planned trials without retry", flush=True)
    finally:
        write_json(output / "outcomes.json", outcomes)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    run_parser = sub.add_parser("run", help="freeze and execute paired trials (or --dry-run)")
    run_parser.add_argument("--output", type=Path, required=True)
    run_parser.add_argument("--package", type=Path, required=True, help="Linux x64 Rust pack:local .tgz")
    run_parser.add_argument("--tier", choices=("smoke", "full"), default="smoke")
    run_parser.add_argument("--smoke", type=Path)
    run_parser.add_argument("--dry-run", action="store_true")
    for name in ("collect", "judge", "report"):
        child = sub.add_parser(name)
        child.add_argument("experiment", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "run":
            run(args.output.resolve(), args.tier, args.package.resolve(), args.smoke, args.dry_run)
        else:
            from .augment_report import collect, judge, report
            {"collect": collect, "judge": judge, "report": report}[args.command](args.experiment.resolve())
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"error: {error}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
