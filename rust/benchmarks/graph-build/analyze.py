#!/usr/bin/env python3
"""Stage-cost and source-coverage diagnostics; not the G2.4 threshold benchmark.

Runs the unchanged build strategy against one frozen local inventory. It uses
existing local model files and the fixed native library. No remote embedding.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

import runner


def profiles(path: Path) -> list[dict]:
    return [json.loads(line.removeprefix("ZG_GRAPH_PROFILE "))
            for line in path.read_text().splitlines() if line.startswith("ZG_GRAPH_PROFILE ")]


def selected_sources(payload: dict) -> set[str]:
    return {source["source"] for domain in payload["result"]["graph"]["domains"].values()
            for source in domain.get("sources", []) if source["source"] is not None}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    repo_default = Path(__file__).resolve().parents[3]
    parser.add_argument("--repo", type=Path, default=repo_default)
    parser.add_argument("--output", type=Path, default=repo_default / "knowledge/.artifacts/graph-build/handoff")
    parser.add_argument("--model-cache", type=Path, default=Path.home() / ".zvec-grep/models")
    parser.add_argument("--zvec-lib-dir", type=Path, default=Path("/private/tmp/zg-ann-fix/native-baseline/lib"))
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--keep-workspace", action="store_true")
    args = parser.parse_args()
    repo, output = args.repo.resolve(), args.output.resolve()
    binary = (args.binary or repo / "rust/target/release/examples/graph_build").resolve()
    output.mkdir(parents=True, exist_ok=True)
    (output / "raw").mkdir(exist_ok=True)
    records = runner.inventory(repo)
    options = runner.graph_options(repo, records)
    runner.write_json(output / "corpus-manifest.json", {
        "records": records, "files": len(records), "bytes": sum(r["bytes"] for r in records),
        "manifest_sha256": hashlib.sha256(json.dumps(records, sort_keys=True).encode()).hexdigest(),
    })
    runner.write_json(output / "graph-options.json", options)
    temporary = Path(tempfile.mkdtemp(prefix="zg-g2-g3-handoff-"))
    workspace = temporary / "workspace"
    env = dict(os.environ, ZVEC_LIB_DIR=str(args.zvec_lib_dir.resolve()),
               DYLD_LIBRARY_PATH=str(args.zvec_lib_dir.resolve()),
               ZVEC_GREP_WORKSPACE_REGISTRY=str(temporary / "workspaces.json"),
               ZVEC_GREP_HOME=str(temporary / "application-home"))
    env.pop("DYLD_PRINT_LIBRARIES", None)
    env.pop("ZG_GRAPH_PROFILE", None)
    library = args.zvec_lib_dir / ("libzvec_c_api.dylib" if sys.platform == "darwin" else "libzvec_c_api.so")
    summary = {"purpose": "one-run diagnostic, not threshold acceptance", "workspace": str(workspace),
               "binary": str(binary), "binary_sha256": runner.sha256(binary),
               "environment": runner.environment(repo, repo / "rust", library, args.model_cache),
               "corpus_manifest": str(output / "corpus-manifest.json"), "runs": {}}
    try:
        for record in records:
            destination = workspace / record["path"]
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(repo / record["path"], destination)
            assert runner.sha256(destination) == record["sha256"]

        def execute(name: str, graph: str, changes: list | None = None, graph_only: bool = False) -> dict:
            command = [str(binary), str(workspace), graph, str(args.model_cache.resolve())]
            if changes is not None or graph_only:
                command.append(json.dumps(changes or []))
            if graph_only:
                command.append("--graph-only")
            stdout, stderr = output / f"raw/{name}.json", output / f"raw/{name}.log"
            process_env = dict(env)
            if graph != "-":
                process_env["ZG_GRAPH_PROFILE"] = "1"
            started = time.monotonic()
            with stdout.open("w") as out, stderr.open("w") as err:
                completed = subprocess.run(command, env=process_env, stdout=out, stderr=err)
            result = {"command": command, "exit_code": completed.returncode,
                      "external_seconds": time.monotonic() - started,
                      "stdout": str(stdout), "stderr": str(stderr), "profiles": profiles(stderr)}
            summary["runs"][name] = result
            runner.write_json(output / "analysis.partial.json", summary)
            if completed.returncode:
                raise RuntimeError(f"{name} failed; see {stderr}")
            payload = json.loads(stdout.read_text())
            runner.assert_indexed(payload)
            result["payload"] = payload
            print(f"{name}: {result['external_seconds']:.3f}s", flush=True)
            if graph != "-":
                runner.graph_result(payload)
            return payload

        execute("text_setup", "-")
        graph_json = json.dumps(options)
        initial = execute("graph_only", graph_json, graph_only=True)
        assert initial["model_events"] == 0 and initial["model_downloaded_bytes"] == 0
        selected = selected_sources(initial)
        graph_extensions = {".rs", ".py", ".pyi", ".pyw", ".md", ".markdown"}
        frozen_sources = {r["path"] for r in records if Path(r["path"]).suffix in graph_extensions}
        historical = json.loads((Path(__file__).resolve().parent / "fixtures/g2.4-corpus-manifest.json").read_text())
        historical_sources = {r["path"] for r in historical["records"]
                              if Path(r["path"]).suffix in graph_extensions}
        runner.write_json(output / "coverage.json", {
            "domains": initial["result"]["graph"]["domains"],
            "frozen_graph_sources_not_selected": sorted(frozen_sources - selected),
            "historical_graph_sources_not_selected": sorted(historical_sources - selected),
            "selection_note": "domain denominator is actual host selection, not the outer frozen inventory",
        })
        target = "rust/crates/zg-engine/src/api/index/graph.rs"
        path = workspace / target
        path.write_text(path.read_text() + "\n// G2 to G3 stage-cost diagnostic.\n")
        execute("update_rust", graph_json, [{"kind": "upsert", "path": target}])
        unchanged = execute("unchanged", graph_json)
        assert unchanged["result"]["graph"]["mode"] == "unchanged"
        runner.write_json(output / "analysis.json", summary)
    finally:
        if not args.keep_workspace:
            shutil.rmtree(temporary)


if __name__ == "__main__":
    main()
