#!/usr/bin/env python3
"""Reproducible local L1 Release measurements for G2.4.

Every measured operation is a new process. Model warm-up and text-only setup are
outside measured intervals. Only temporary workspaces created by this runner are
removed; source checkouts, model files and user caches are never cleaned.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from collections import Counter
from pathlib import Path
from typing import Any

RSS_INTERVAL = 0.010
DISK_INTERVAL = 0.050
EXTENSIONS = {
    ".rs", ".py", ".pyi", ".pyw", ".md", ".markdown", ".ts", ".tsx",
    ".mjs", ".js", ".jsx", ".c", ".h", ".cc", ".cpp", ".hpp", ".go",
    ".java", ".json", ".toml", ".yaml", ".yml", ".txt",
}
EXCLUDED_PARTS = {
    ".git", ".zvec-grep", "target", "thirdparty", "node_modules", "dist",
    ".archify", ".kg-staging", "g2.4-resources", "g2-g3-handoff",
    "graph-build-optimization",
}
SCENARIOS = (
    "text_full", "text_and_graph", "graph_only", "unchanged", "update_rust",
    "update_markdown", "add_target", "delete_target",
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def run(command: list[str], *, cwd: Path, env: dict[str, str], log: Path) -> dict[str, Any]:
    started = time.monotonic()
    with log.open("w") as output:
        completed = subprocess.run(command, cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT)
    record = {"command": command, "cwd": str(cwd), "exit_code": completed.returncode,
              "wall_time_ms": (time.monotonic() - started) * 1000, "log": str(log)}
    if completed.returncode:
        raise RuntimeError(f"command failed; see {log}: {' '.join(command)}")
    return record


def logical_bytes(path: Path) -> int:
    total = 0
    if not path.exists():
        return total
    for root, directories, files in os.walk(path):
        directories[:] = [d for d in directories if not (Path(root) / d).is_symlink()]
        for name in files:
            item = Path(root) / name
            try:
                if not item.is_symlink():
                    total += item.stat().st_size
            except FileNotFoundError:
                pass
    return total


def graph_builds_bytes(workspace: Path) -> int:
    home = workspace / ".zvec-grep"
    roots = [home / "graph/builds"]
    roots.extend((home / "generations").glob("*/graph/builds"))
    return sum(logical_bytes(root) for root in roots)


def rss_bytes(pid: int) -> int | None:
    completed = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True)
    value = completed.stdout.strip()
    return int(value.splitlines()[0]) * 1024 if value else None


def measured(command: list[str], *, env: dict[str, str], workspace: Path | None,
             stdout_path: Path, stderr_path: Path) -> dict[str, Any]:
    before = logical_bytes(workspace) if workspace else 0
    graph_before = graph_builds_bytes(workspace) if workspace else 0
    started = time.monotonic()
    with stdout_path.open("w") as stdout, stderr_path.open("w") as stderr:
        process = subprocess.Popen(command, env=env, stdout=stdout, stderr=stderr)
        peak_rss = 0
        peak_disk = before
        peak_graph = graph_before
        rss_samples = disk_samples = 0
        next_disk = 0.0
        while process.poll() is None:
            current = rss_bytes(process.pid)
            if current is not None:
                peak_rss = max(peak_rss, current)
                rss_samples += 1
            elapsed = time.monotonic() - started
            if workspace and elapsed >= next_disk:
                peak_disk = max(peak_disk, logical_bytes(workspace))
                if workspace:
                    peak_graph = max(peak_graph, graph_builds_bytes(workspace))
                disk_samples += 1
                next_disk = elapsed + DISK_INTERVAL
            time.sleep(RSS_INTERVAL)
        returncode = process.wait()
    if workspace:
        peak_disk = max(peak_disk, logical_bytes(workspace))
    if workspace:
        peak_graph = max(peak_graph, graph_builds_bytes(workspace))
    wall = time.monotonic() - started
    after = logical_bytes(workspace) if workspace else 0
    return {
        "command": command, "exit_code": returncode,
        "external_wall_time_micros": int(wall * 1_000_000),
        "observed_peak_rss_bytes": peak_rss,
        "workspace_bytes_before": before, "workspace_bytes_after": after,
        "workspace_bytes_delta": after - before,
        "observed_peak_workspace_bytes": peak_disk,
        "observed_peak_workspace_delta_bytes": peak_disk - before,
        "graph_builds_bytes_before": graph_before,
        "observed_peak_graph_builds_bytes": peak_graph,
        "observed_peak_graph_builds_delta_bytes": peak_graph - graph_before,
        "rss_samples": rss_samples, "disk_samples": disk_samples,
        "rss_interval_ms": RSS_INTERVAL * 1000, "disk_interval_ms": DISK_INTERVAL * 1000,
        "stdout": str(stdout_path), "stderr": str(stderr_path),
    }


def inventory(source: Path) -> list[dict[str, Any]]:
    listed = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        cwd=source, check=True, capture_output=True,
    ).stdout.split(b"\0")
    records = []
    for raw in sorted(filter(None, listed)):
        relative = Path(os.fsdecode(raw))
        if any(part in EXCLUDED_PARTS for part in relative.parts) or relative.suffix not in EXTENSIONS:
            continue
        path = source / relative
        if not path.is_file() or path.is_symlink():
            continue
        records.append({"path": relative.as_posix(), "bytes": path.stat().st_size, "sha256": sha256(path)})
    return records


def graph_options(source: Path, records: list[dict[str, Any]]) -> dict[str, Any]:
    paths = {record["path"] for record in records}
    crates = sorted({str(Path(path).parent) for path in paths
                     if path.startswith("rust/crates/") and path.endswith("/Cargo.toml")})
    roots = [{"root": root, "language": "rust", "namespace": root.replace("/", "-"),
              "module_prefix": [], "layout": "flat"} for root in crates]
    # This checked-in Rust corpus intentionally has no Cargo.toml. Its source
    # root is explicit; the broader benchmarks Python root must not claim it.
    fixture_root = "benchmarks/zg-graph/corpus/rust/src"
    if any(Path(path).is_relative_to(fixture_root) for path in paths):
        roots.append({"root": fixture_root, "language": "rust", "namespace": "fixture-rust",
                      "module_prefix": [], "layout": "flat"})
    python_groups = sorted({Path(record["path"]).parts[0] for record in records
                            if Path(record["path"]).suffix in {".py", ".pyi", ".pyw"}})
    roots.extend({"root": group, "language": "python", "namespace": f"python-{group}",
                  "module_prefix": [], "layout": "flat"} for group in python_groups)
    return {"scope": "both", "module_roots": roots, "rules": {"version": "zg-relations-v1"}}


def disk_breakdown(workspace: Path) -> dict[str, Any]:
    home = workspace / ".zvec-grep"
    manifest_path = home / "manifest.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {}
    generation = manifest.get("storageGeneration")
    storage_home = home / "generations" / generation if generation else home
    graph = storage_home / "graph"
    builds = graph / "builds"
    head_path = graph / "HEAD.json"
    head = json.loads(head_path.read_text()) if head_path.exists() else None
    selected = head.get("graphBuildId") if head else None
    candidates = []
    current = 0
    if builds.exists():
        for candidate in builds.iterdir():
            if candidate.is_dir():
                size = logical_bytes(candidate)
                if candidate.name == selected:
                    current = size
                else:
                    candidates.append({"graph_build_id": candidate.name, "bytes": size})
    zvec = logical_bytes(storage_home / "storage")
    graph_total = logical_bytes(graph)
    metadata = graph_total - logical_bytes(builds)
    attempt_metadata = sum(path.stat().st_size for path in builds.glob("*/attempt.json")) if builds.exists() else 0
    return {
        "home_bytes": logical_bytes(home), "active_generation": generation,
        "active_generation_bytes": logical_bytes(storage_home), "zvec_bytes": zvec,
        "graph_current_bytes": current, "graph_candidates": candidates,
        "graph_candidates_bytes": sum(c["bytes"] for c in candidates),
        "graph_metadata_bytes": metadata,
        "attempt_metadata_bytes": attempt_metadata,
        "discard_metadata_bytes": logical_bytes(graph / "discards"),
        "text_and_other_bytes": logical_bytes(storage_home) - zvec - graph_total,
        "head": head,
    }


def graph_result(payload: dict[str, Any]) -> dict[str, Any]:
    graph = payload["result"].get("graph")
    if not graph:
        raise AssertionError("graph result missing")
    assert graph["graph_candidate_committed"] and graph["graph_selected"]
    head = graph.get("graph_build_id")
    assert head
    return graph


def assert_indexed(payload: dict[str, Any]) -> None:
    assert payload["result"]["files_failed"] == 0, payload["result"]["failed_files"]


def summarize(runs: list[dict[str, Any]], corpus_files: int) -> dict[str, Any]:
    walls = [run["payload"]["wall_time_micros"] for run in runs]
    rss = [run["measurement"]["observed_peak_rss_bytes"] for run in runs]
    disk = [run["measurement"]["observed_peak_workspace_delta_bytes"] for run in runs]
    result = {
        "wall_time_micros_median": statistics.median(walls), "wall_time_micros_max": max(walls),
        "wall_time_micros_per_1000_all_files": statistics.median(walls) * 1000 / corpus_files,
        "observed_peak_rss_bytes_median": statistics.median(rss), "observed_peak_rss_bytes_max": max(rss),
        "observed_peak_workspace_delta_bytes_median": statistics.median(disk),
        "observed_peak_workspace_delta_bytes_max": max(disk),
    }
    graphs = [run["payload"]["result"].get("graph") for run in runs]
    graphs = [graph for graph in graphs if graph]
    if graphs:
        coverage = [sum(domain["coverage"] for domain in graph["domains"].values()) for graph in graphs]
        result["graph_fact_files_median"] = statistics.median(coverage)
        result["wall_time_micros_per_1000_graph_fact_files"] = (
            statistics.median(walls) * 1000 / statistics.median(coverage) if statistics.median(coverage) else None
        )
        result["diagnostics_median"] = statistics.median(len(graph["diagnostics"]) for graph in graphs)
        result["domains"] = {domain: {
            "ready": [graph["domains"][domain]["ready"] for graph in graphs],
            "coverage_median": statistics.median(graph["domains"][domain]["coverage"] for graph in graphs),
            "node_count_median": statistics.median(graph["domains"][domain]["nodeCount"] for graph in graphs),
            "reference_count_median": statistics.median(graph["domains"][domain]["referenceCount"] for graph in graphs),
        } for domain in graphs[0]["domains"]}
    return result


def environment(repo: Path, rust: Path, zvec: Path, model_cache: Path) -> dict[str, Any]:
    cpu = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True)
    disk = subprocess.run(["diskutil", "info", "/"], capture_output=True, text=True)
    graphdb = subprocess.run(["git", "-C", str(rust / "thirdparty/graphdb"), "rev-parse", "HEAD"],
                             check=True, capture_output=True, text=True).stdout.strip()
    head = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"],
                          check=True, capture_output=True, text=True).stdout.strip()
    diff = subprocess.run(["git", "-C", str(repo), "diff", "--binary"], capture_output=True).stdout
    model_files = [{"path": str(path.relative_to(model_cache)), "bytes": path.stat().st_size,
                    "sha256": sha256(path)} for path in sorted(model_cache.rglob("*"))
                   if path.is_file() and not path.name.endswith(".lock")
                   and "potion-code-16m-v2" in str(path).lower()]
    return {
        "platform": platform.platform(), "machine": platform.machine(), "python": sys.version,
        "cpu": cpu.stdout.strip() or "unavailable", "memory_bytes": os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES"),
        "disk": disk.stdout, "rust": subprocess.run(["rustc", "--version"], capture_output=True, text=True).stdout.strip(),
        "build_profile": "release", "cache_scope": "cold process; warm model file cache; cold-download time excluded",
        "repo_head": head, "unstaged_diff_sha256": hashlib.sha256(diff).hexdigest(),
        "graphdb_commit": graphdb, "zvec_library": str(zvec), "zvec_sha256": sha256(zvec),
        "model": "local/potion-code-16m-v2", "device": "cpu", "remote_embedding_authorized": False,
        "model_cache": str(model_cache), "model_files": model_files,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    default_repo = Path(__file__).resolve().parents[3]
    parser.add_argument("--repo", type=Path, default=default_repo)
    parser.add_argument("--output", type=Path, default=default_repo / "knowledge/.artifacts/graph-build/resources")
    parser.add_argument("--model-cache", type=Path, default=Path.home() / ".zvec-grep/models")
    parser.add_argument("--zvec-lib-dir", type=Path, default=Path("/tmp/zg-ann-fix/native-baseline/lib"))
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--scenarios", help="comma-separated scenario names; default: all")
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--keep-workspaces", action="store_true")
    args = parser.parse_args()
    if args.repeats < 3:
        parser.error("G2.4 requires at least three measured repeats")
    scenarios = tuple(args.scenarios.split(",")) if args.scenarios else SCENARIOS
    if len(set(scenarios)) != len(scenarios) or any(name not in SCENARIOS for name in scenarios):
        parser.error(f"--scenarios must contain unique names from {','.join(SCENARIOS)}")
    repo, output = args.repo.resolve(), args.output.resolve()
    rust = repo / "rust"
    args.model_cache = args.model_cache.resolve()
    args.zvec_lib_dir = args.zvec_lib_dir.resolve()
    zvec = args.zvec_lib_dir / ("libzvec_c_api.dylib" if sys.platform == "darwin" else "libzvec_c_api.so")
    if not zvec.is_file():
        parser.error(f"missing fixed zvec library: {zvec}")
    output.mkdir(parents=True, exist_ok=True)
    (output / "raw").mkdir(exist_ok=True)
    env = dict(os.environ, ZVEC_LIB_DIR=str(args.zvec_lib_dir), DYLD_LIBRARY_PATH=str(args.zvec_lib_dir),
               CARGO_BUILD_JOBS="14")
    env.pop("DYLD_PRINT_LIBRARIES", None)
    commands: list[dict[str, Any]] = []
    engine = rust / "target/release/examples/graph_build"
    digest_test = None
    if not args.skip_build:
        knowledge_build = ["cargo", "build", "--release", "-j", "14", "-p", "zg-knowledge",
                           "--features", "graphdb", "--example", "graph_build"]
        commands.append(run(knowledge_build, cwd=rust, env=env, log=output / "release-knowledge-build.log"))
        engine_build = ["cargo", "build", "--release", "-j", "14", "-p", "zg-engine",
                        "--features", "knowledge-graph-graphdb", "--example", "graph_build",
                        "--message-format=json"]
        commands.append(run(engine_build, cwd=rust, env=env, log=output / "release-engine-build.log"))
        for line in (output / "release-engine-build.log").read_text().splitlines():
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            target = message.get("target", {})
            if (message.get("reason") == "compiler-artifact" and message.get("executable")
                    and target.get("name") == "graph_build" and "example" in target.get("kind", [])):
                engine = Path(message["executable"])
        test_build = ["cargo", "test", "--release", "-j", "14", "-p", "zg-engine",
                      "--features", "knowledge-graph-graphdb", "--lib", "--no-run", "--message-format=json"]
        commands.append(run(test_build, cwd=rust, env=env, log=output / "release-test-build.log"))
        for line in (output / "release-test-build.log").read_text().splitlines():
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if message.get("reason") == "compiler-artifact" and message.get("executable"):
                digest_test = Path(message["executable"])
    if not engine.is_file():
        parser.error(f"missing Release host example: {engine}")
    if digest_test is None:
        candidates = sorted((rust / "target/release/deps").glob("zg_engine-*"), key=lambda p: p.stat().st_mtime)
        digest_test = next((p for p in reversed(candidates) if p.is_file() and os.access(p, os.X_OK)), None)
    if digest_test is None:
        parser.error("missing Release engine test binary")

    records = inventory(repo)
    if not records:
        raise RuntimeError("empty L1 inventory")
    options = graph_options(repo, records)
    if not options["module_roots"]:
        raise RuntimeError("no graph module roots")
    write_json(output / "graph-options.json", options)
    temporary = Path(tempfile.mkdtemp(prefix="zg-g2.4-l1-"))
    env.update(ZVEC_GREP_WORKSPACE_REGISTRY=str(temporary / "workspaces.json"),
               ZVEC_GREP_HOME=str(temporary / "application-home"))
    frozen = temporary / "frozen"
    try:
        for record in records:
            destination = frozen / record["path"]
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(repo / record["path"], destination)
        for record in records:
            assert sha256(frozen / record["path"]) == record["sha256"]
        corpus = {
            "level": "L1", "selection": "git cached and nonignored untracked source files; fixed extension and exclusion policy",
            "files": len(records), "bytes": sum(r["bytes"] for r in records),
            "manifest_sha256": hashlib.sha256(json.dumps(records, sort_keys=True).encode()).hexdigest(),
            "extensions": dict(Counter(Path(r["path"]).suffix for r in records)),
            "code_files": sum(Path(r["path"]).suffix in {".rs", ".py", ".pyi", ".pyw"} for r in records),
            "code_bytes": sum(r["bytes"] for r in records if Path(r["path"]).suffix in {".rs", ".py", ".pyi", ".pyw"}),
            "document_files": sum(Path(r["path"]).suffix in {".md", ".markdown"} for r in records),
            "document_bytes": sum(r["bytes"] for r in records if Path(r["path"]).suffix in {".md", ".markdown"}),
            "text_only_bytes": sum(r["bytes"] for r in records if Path(r["path"]).suffix not in {".rs", ".py", ".pyi", ".pyw", ".md", ".markdown"}),
            "records": records,
        }
        write_json(output / "corpus-manifest.json", corpus)
        digest_records = [{"id": index, "path": r["path"], "size_bytes": r["bytes"], "content_hash": r["sha256"]}
                          for index, r in enumerate(records, 1)]
        write_json(output / "file-records.json", digest_records)

        warm = temporary / "model-warmup"
        warm.mkdir()
        (warm / "README.md").write_text("# G2.4 model cache warm-up\n")
        warm_stdout, warm_stderr = output / "raw/model-warmup.stdout.json", output / "raw/model-warmup.stderr.log"
        warm_measure = measured([str(engine), str(warm), "-", str(args.model_cache)], env=env,
                                workspace=warm, stdout_path=warm_stdout, stderr_path=warm_stderr)
        if warm_measure["exit_code"]:
            raise RuntimeError(f"model warm-up failed; see {warm_stderr}")
        write_json(output / "raw/model-warmup.json", warm_measure)
        env_report = environment(repo, rust, zvec, args.model_cache)

        def execute(scenario: str, repeat: int) -> dict[str, Any]:
            workspace = temporary / f"{scenario}-{repeat}"
            shutil.copytree(frozen, workspace)
            graph_json = json.dumps(options, separators=(",", ":"))
            setup_command = None
            if scenario == "graph_only":
                setup_command = [str(engine), str(workspace), "-", str(args.model_cache)]
            elif scenario != "text_full" and scenario != "text_and_graph":
                setup_command = [str(engine), str(workspace), graph_json, str(args.model_cache)]
            setup_payload = None
            if setup_command:
                setup_out = output / f"raw/{scenario}-{repeat}-setup.stdout.json"
                setup_err = output / f"raw/{scenario}-{repeat}-setup.stderr.log"
                setup = measured(setup_command, env=env, workspace=workspace, stdout_path=setup_out, stderr_path=setup_err)
                if setup["exit_code"]:
                    raise RuntimeError(f"setup failed for {scenario}/{repeat}; see {setup_err}")
                setup_payload = json.loads(setup_out.read_text())
                assert_indexed(setup_payload)
                if scenario == "graph_only":
                    assert setup_payload["result"].get("graph") is None
                else:
                    setup_graph = graph_result(setup_payload)
                    setup_head = disk_breakdown(workspace)["head"]
                    assert setup_head and setup_head["graphBuildId"] == setup_graph["graph_build_id"]
            changes: list[dict[str, str]] = []
            previous_build = setup_payload["result"]["graph"]["graph_build_id"] if setup_payload and scenario != "graph_only" else None
            previous_disk = disk_breakdown(workspace)
            if scenario == "update_rust":
                target = "rust/crates/zg-engine/src/api/index/graph.rs"
                path = workspace / target
                path.write_text(path.read_text() + "\n// G2.4 L1 measured single-file update.\n")
                changes = [{"kind": "upsert", "path": target}]
            elif scenario == "update_markdown":
                target = next(r["path"] for r in records if Path(r["path"]).suffix in {".md", ".markdown"})
                path = workspace / target
                path.write_text(path.read_text() + "\nG2.4 L1 measured documentation update.\n")
                changes = [{"kind": "upsert", "path": target}]
            elif scenario == "add_target":
                target = "rust/crates/zg-engine/src/g2_4_l1_added.rs"
                (workspace / target).write_text("pub fn g2_4_l1_added() -> u8 { 4 }\n")
                changes = [{"kind": "upsert", "path": target}]
            elif scenario == "delete_target":
                target = "rust/crates/zg-engine/src/api/index/graph.rs"
                (workspace / target).unlink()
                changes = [{"kind": "delete", "path": target}]
            graph_argument = "-" if scenario == "text_full" else graph_json
            command = [str(engine), str(workspace), graph_argument, str(args.model_cache)]
            if changes:
                command.append(json.dumps(changes, separators=(",", ":")))
            if scenario == "graph_only":
                command.extend(["[]", "--graph-only"])
            stdout = output / f"raw/{scenario}-{repeat}.stdout.json"
            stderr = output / f"raw/{scenario}-{repeat}.stderr.log"
            measurement = measured(command, env=env, workspace=workspace, stdout_path=stdout, stderr_path=stderr)
            if measurement["exit_code"]:
                raise RuntimeError(f"measurement failed for {scenario}/{repeat}; see {stderr}")
            payload = json.loads(stdout.read_text())
            assert_indexed(payload)
            checks: dict[str, Any] = {"files_failed_zero": True}
            if scenario != "text_full":
                graph = graph_result(payload)
                checks["selected"] = True
                after_head = disk_breakdown(workspace)["head"]
                assert after_head and after_head["graphBuildId"] == graph["graph_build_id"]
                checks["head_matches_selection"] = True
                if scenario == "graph_only":
                    assert graph["mode"] == "graph_only" and not graph["text_published"]
                    assert payload["model_events"] == 0 and payload["model_downloaded_bytes"] == 0
                    checks.update(graph_only=True, zero_model_events=True, zero_downloads=True)
                elif scenario == "unchanged":
                    assert graph["mode"] == "unchanged" and graph["graph_build_id"] == previous_build
                    checks.update(unchanged=True, no_new_candidate=True)
                elif previous_build:
                    assert graph["graph_build_id"] != previous_build
                    assert after_head["textRevision"] > previous_disk["head"]["textRevision"]
                    assert after_head["catalogDigest"] != previous_disk["head"]["catalogDigest"]
                    checks.update(new_build_selected=True, text_revision_advanced=True, catalog_digest_changed=True)
                    if scenario in {"add_target", "delete_target"}:
                        expected = 1 if scenario == "add_target" else 0
                        assert payload["result"]["files_added"] == expected
                        assert payload["result"]["files_deleted"] == (1 if scenario == "delete_target" else 0)
                        checks["target_change_applied"] = True
                        checks["negative_resolution_or_directory_change"] = "deleted target catalog/HEAD changed; unresolved references remain graph diagnostics/facts, not host readiness" if scenario == "delete_target" else "new target included in changed catalog and selected HEAD"
            else:
                assert payload["result"].get("graph") is None
                checks["text_baseline_without_graph"] = True
            return {"scenario": scenario, "repeat": repeat, "measurement": measurement, "payload": payload,
                    "checks": checks, "disk": disk_breakdown(workspace)}

        scenario_results: dict[str, Any] = {}
        for scenario in scenarios:
            runs = []
            for repeat in range(1, args.repeats + 1):
                print(f"G2.4 {scenario} repeat {repeat}/{args.repeats}", flush=True)
                record = execute(scenario, repeat)
                write_json(output / f"raw/{scenario}-{repeat}.json", record)
                runs.append(record)
            scenario_results[scenario] = {"runs": runs, "summary": summarize(runs, len(records))}
            write_json(output / "resources.partial.json", {"scenarios": scenario_results})

        digest_runs = []
        for repeat in range(1, args.repeats + 1):
            digest_output = output / f"raw/catalog-digest-{repeat}.json"
            digest_env = dict(env, ZG_G2_CATALOG_RECORDS=str(output / "file-records.json"),
                              ZG_G2_CATALOG_DIGEST_OUTPUT=str(digest_output))
            stdout = output / f"raw/catalog-digest-{repeat}.stdout.log"
            stderr = output / f"raw/catalog-digest-{repeat}.stderr.log"
            measurement = measured([str(digest_test), "--exact", "storage::graph_state::tests::catalog_digest_cost_probe",
                                    "--nocapture", "--test-threads=1"], env=digest_env, workspace=None,
                                   stdout_path=stdout, stderr_path=stderr)
            if measurement["exit_code"]:
                raise RuntimeError(f"catalog digest probe failed; see {stderr}")
            payload = json.loads(digest_output.read_text())
            assert payload["repeat_equal"] and payload["records"] == len(records)
            digest_runs.append({"repeat": repeat, "measurement": measurement, "payload": payload})
        assert len({r["payload"]["digest"] for r in digest_runs}) == 1
        digest_summary = {
            "duration_micros_median": statistics.median(r["payload"]["duration_micros"] for r in digest_runs),
            "duration_micros_max": max(r["payload"]["duration_micros"] for r in digest_runs),
            "observed_peak_rss_bytes_median": statistics.median(r["measurement"]["observed_peak_rss_bytes"] for r in digest_runs),
            "observed_peak_rss_bytes_max": max(r["measurement"]["observed_peak_rss_bytes"] for r in digest_runs),
            "records": len(records), "digest": digest_runs[0]["payload"]["digest"],
        }

        text = scenario_results.get("text_full", {}).get("summary")
        joint = scenario_results.get("text_and_graph", {}).get("summary")
        graph_only = scenario_results.get("graph_only", {}).get("summary")
        graph_disk = zvec_disk = observed_coexist = None
        if joint:
            graph_disk = statistics.median(r["disk"]["graph_current_bytes"] for r in scenario_results["text_and_graph"]["runs"])
            zvec_disk = statistics.median(r["disk"]["zvec_bytes"] for r in scenario_results["text_and_graph"]["runs"])
            maintenance = [name for name in ("update_rust", "update_markdown", "add_target", "delete_target") if name in scenario_results]
            if maintenance:
                observed_coexist = max(r["measurement"]["observed_peak_graph_builds_bytes"]
                                       for name in maintenance for r in scenario_results[name]["runs"])
        update_medians = {name: scenario_results[name]["summary"]["wall_time_micros_median"]
                          for name in ("update_rust", "update_markdown") if name in scenario_results}
        thresholds = []
        if text and graph_only:
            thresholds.append({"item": "graph_only_vs_text_time", "target": "<=20%", "actual": graph_only["wall_time_micros_median"] / text["wall_time_micros_median"], "unit": "ratio", "pass": graph_only["wall_time_micros_median"] <= text["wall_time_micros_median"] * .20})
        if text and joint:
            thresholds.extend([
                {"item": "joint_minus_text_vs_text_time", "target": "<=30% (diagnostic difference)", "actual": (joint["wall_time_micros_median"] - text["wall_time_micros_median"]) / text["wall_time_micros_median"], "unit": "ratio", "pass": joint["wall_time_micros_median"] - text["wall_time_micros_median"] <= text["wall_time_micros_median"] * .30},
                {"item": "peak_rss_joint_vs_text", "target": "<=1.5x", "actual": joint["observed_peak_rss_bytes_median"] / text["observed_peak_rss_bytes_median"], "unit": "ratio", "pass": joint["observed_peak_rss_bytes_median"] <= text["observed_peak_rss_bytes_median"] * 1.5},
            ])
        if graph_only:
            zero_model = all(r["payload"]["model_events"] == 0 and r["payload"]["model_downloaded_bytes"] == 0
                             for r in scenario_results["graph_only"]["runs"])
            thresholds.append({"item": "graph_only_zero_model", "target": "zero model events and downloads", "actual": zero_model, "pass": zero_model})
        if len(update_medians) == 2:
            thresholds.append({"item": "single_file_graph_maintenance", "target": "<=500ms each @L1 (median)", "actual": update_medians, "unit": "microseconds", "pass": all(v <= 500_000 for v in update_medians.values())})
        if graph_disk is not None:
            thresholds.append({"item": "graph_vs_zvec_disk", "target": "<=50%", "actual": graph_disk / zvec_disk if zvec_disk else None, "unit": "ratio", "pass": bool(zvec_disk) and graph_disk <= zvec_disk * .5})
        if observed_coexist is not None:
            thresholds.append({"item": "coexisting_graph_disk", "target": "<= current graph * 2.1; >3x triggers candidate-cost risk", "actual": observed_coexist / graph_disk if graph_disk else None, "unit": "ratio", "pass": bool(graph_disk) and observed_coexist <= graph_disk * 2.1})
        thresholds.append({"item": "catalog_digest_l1", "target": "<=100ms @about1k (median)", "actual": digest_summary["duration_micros_median"], "unit": "microseconds", "pass": digest_summary["duration_micros_median"] <= 100_000})
        candidate_cost_risk = bool((graph_disk and observed_coexist and observed_coexist > graph_disk * 3) or
                                   (text and graph_only and graph_only["wall_time_micros_median"] > text["wall_time_micros_median"]))
        unrun = [name for name in SCENARIOS if name not in scenarios] + ["L2 frozen external repositories", "L3 synthetic corpus", "catalog digest @10k", "catalog digest @100k",
                 "cold model download", "non-macOS platforms", "native mid-transaction SIGKILL", "HEAD fsync crash window",
                 "native commit cancellation/timeout window"]
        resources = {
            "schema": "zg-g2.4-resources-v1", "generated_epoch_ms": int(time.time() * 1000),
            "environment": env_report, "corpus": {k: v for k, v in corpus.items() if k != "records"},
            "corpus_manifest": "corpus-manifest.json", "graph_options": "graph-options.json",
            "sampling": {"rss_interval_ms": 10, "disk_interval_ms": 50, "peak_semantics": "observed sampled peak, not a guaranteed hard peak", "disk_bytes": "logical file lengths, not allocated blocks"},
            "commands": commands, "scenarios": scenario_results, "catalog_digest": {"runs": digest_runs, "summary": digest_summary},
            "thresholds": thresholds, "candidate_rebuild_cost_too_high": bool(candidate_cost_risk), "unrun": unrun,
        }
        verification = {
            "schema": "zg-g2.4-verification-v1", "runner_exit_code": 0, "repeats": args.repeats,
            "release_build_exit_codes": [c["exit_code"] for c in commands],
            "scenario_exit_codes": {name: [r["measurement"]["exit_code"] for r in value["runs"]] for name, value in scenario_results.items()},
            "correctness_checks": {name: [r["checks"] for r in value["runs"]] for name, value in scenario_results.items()},
            "catalog_digest_exit_codes": [r["measurement"]["exit_code"] for r in digest_runs],
            "catalog_digest_repeat_equal": True, "thresholds": thresholds,
            "candidate_rebuild_cost_too_high": bool(candidate_cost_risk), "unrun": unrun,
            "reproduce": "python3 rust/benchmarks/graph-build/runner.py --repeats 3" + (f" --scenarios {args.scenarios}" if args.scenarios else ""),
        }
        write_json(output / "resources.json", resources)
        write_json(output / "verification.json", verification)
        (output / "resources.partial.json").unlink(missing_ok=True)
        print(json.dumps({"output": str(output), "thresholds": thresholds,
                          "candidate_rebuild_cost_too_high": candidate_cost_risk}, indent=2), flush=True)
        return 0
    finally:
        if not args.keep_workspaces:
            shutil.rmtree(temporary)
        else:
            print(f"kept runner-owned workspaces: {temporary}", flush=True)


if __name__ == "__main__":
    raise SystemExit(main())
