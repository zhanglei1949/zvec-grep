#!/usr/bin/env python3
"""Compare fixed-budget Release binaries through the real TP graph publication path."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import tempfile

import runner


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--binary", action="append", required=True, help="LABEL=EXECUTABLE")
    parser.add_argument("--corpus-root", type=Path, required=True)
    parser.add_argument("--audit-binary", type=Path, required=True)
    parser.add_argument("--reference-audit-binary", type=Path,
                        help="matching auditor for the first binary when its fact contract differs")
    parser.add_argument("--text-seed", type=Path, required=True,
                        help="shared text-only index snapshot; created on the first run")
    parser.add_argument("--model-cache", type=Path, required=True)
    parser.add_argument("--zvec-lib-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--profile", action="store_true")
    parser.add_argument("--scenario", choices=("fresh", "rebuild", "semantic"), default="fresh")
    parser.add_argument("--semantic-seed-binary", type=Path)
    args = parser.parse_args()
    # Keep one experiment active per shared seed, including fixture preparation.
    benchmark_lock = args.text_seed.with_name(args.text_seed.name + ".benchmark.lock").open("a")
    fcntl.flock(benchmark_lock, fcntl.LOCK_EX)
    if args.scenario == "semantic" and args.semantic_seed_binary is None:
        parser.error("semantic scenario requires --semantic-seed-binary")
    if args.repeats < 1 or (not args.profile and args.repeats < 3):
        parser.error("at least three unprofiled repeats or one diagnostic profile required")
    binaries = {}
    for item in args.binary:
        label, path = item.split("=", 1)
        if label in binaries or not label.replace("-", "").isalnum():
            parser.error("binary labels must be unique alphanumeric names with optional hyphens")
        binaries[label] = Path(path).resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    raw = output / "raw"
    raw.mkdir()
    records = runner.inventory(args.corpus_root)
    options = runner.graph_options(args.corpus_root, records)
    manifest = {"records": records, "files": len(records),
                "sha256": hashlib.sha256(json.dumps(records, sort_keys=True).encode()).hexdigest()}
    runner.write_json(output / "corpus-manifest.json", manifest)
    runner.write_json(output / "graph-options.json", options)
    env = dict(os.environ, ZVEC_LIB_DIR=str(args.zvec_lib_dir),
               DYLD_LIBRARY_PATH=str(args.zvec_lib_dir))
    env.pop("ZG_GRAPH_PROFILE", None)
    runs = []
    with tempfile.TemporaryDirectory(prefix="zg-tp-batch-") as temporary:
        base = Path(temporary)
        env.update(ZVEC_GREP_WORKSPACE_REGISTRY=str(base / "registry.json"),
                   ZVEC_GREP_HOME=str(base / "home"))
        frozen = base / "frozen"
        for record in records:
            destination = frozen / record["path"]
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(args.corpus_root / record["path"], destination)
            assert runner.sha256(destination) == record["sha256"]
        labels = list(binaries)
        seed = args.text_seed.resolve()
        if not seed.exists():
            shutil.copytree(frozen, seed)
            setup_out, setup_err = raw / "text-setup.json", raw / "text-setup.log"
            setup = runner.measured([str(binaries[labels[0]]), str(seed), "-", str(args.model_cache)],
                                    env=env, workspace=seed,
                                    stdout_path=setup_out, stderr_path=setup_err)
            runner.write_json(raw / "text-setup-measurement.json", setup)
            if setup["exit_code"]:
                raise RuntimeError(f"text setup failed: {setup_err}")
            runner.assert_indexed(json.loads(setup_out.read_text()))
        for record in records:
            assert runner.sha256(seed / record["path"]) == record["sha256"]
        assert runner.disk_breakdown(seed)["head"] is None
        if args.scenario != "fresh":
            scenario_seed = seed.with_name(seed.name + "-" + args.scenario)
            if not scenario_seed.exists():
                shutil.copytree(seed, scenario_seed)
                seed_env = dict(env, ZVEC_GREP_WORKSPACE_REGISTRY=str(base / "seed-registry.json"))
                # A policy change forces a real rebuild while keeping text and
                # existing code identities unchanged for semantic inheritance.
                seed_options = dict(options, scope="code")
                with (raw / "graph-seed.log").open("w") as log:
                    subprocess.run([str(binaries[labels[0]]), str(scenario_seed), json.dumps(seed_options),
                                    str(args.model_cache), "[]", "--graph-only"], env=seed_env,
                                   stdout=log, stderr=subprocess.STDOUT, check=True)
                if args.scenario == "semantic":
                    with (raw / "semantic-seed.log").open("w") as log:
                        subprocess.run([str(args.semantic_seed_binary), str(scenario_seed)],
                                       env=seed_env, stdout=log, stderr=subprocess.STDOUT, check=True)
                    fixture = json.loads((raw / "semantic-seed.log").read_text())
                    runner.write_json(scenario_seed.with_name(scenario_seed.name + "-fixture.json"), fixture)
            seed = scenario_seed
        semantic_fixture = None
        if args.scenario == "semantic":
            semantic_fixture = json.loads(seed.with_name(seed.name + "-fixture.json").read_text())
        previous_head = runner.disk_breakdown(seed)["head"]
        for repeat in range(args.repeats):
            # Rotate the order so every variant occupies each position once.
            for label in labels[repeat % len(labels):] + labels[:repeat % len(labels)]:
                name = f"{label}-{repeat + 1}"
                print(name, flush=True)
                workspace = base / name
                # Identical text generations/file IDs make fact digests comparable.
                shutil.copytree(seed, workspace)
                binary = binaries[label]
                binary_sha256 = runner.sha256(binary)
                run_env = dict(env, ZVEC_GREP_WORKSPACE_REGISTRY=str(base / f"{name}-registry.json"))
                measured_env = dict(run_env)
                if args.profile:
                    measured_env["ZG_GRAPH_PROFILE"] = "1"
                stdout, stderr = raw / f"{name}.stdout.json", raw / f"{name}.stderr.log"
                command = [str(binary), str(workspace), json.dumps(options),
                           str(args.model_cache), "[]", "--graph-only"]
                measurement = runner.measured(command, env=measured_env, workspace=workspace,
                                              stdout_path=stdout, stderr_path=stderr)
                if measurement["exit_code"]:
                    raise RuntimeError(f"graph build failed: {stderr}")
                assert runner.sha256(binary) == binary_sha256, "benchmark binary changed during measurement"
                payload = json.loads(stdout.read_text())
                runner.assert_indexed(payload)
                graph = runner.graph_result(payload)
                assert graph["mode"] == "graph_only" and not graph["text_published"]
                assert payload["model_events"] == payload["model_downloaded_bytes"] == 0
                disk = runner.disk_breakdown(workspace)
                assert disk["head"]["graphBuildId"] == graph["graph_build_id"]
                if previous_head:
                    assert graph["graph_build_id"] != previous_head["graphBuildId"]
                    assert disk["head"]["textRevision"] == previous_head["textRevision"]
                storage = workspace / ".zvec-grep"
                if disk["active_generation"]:
                    storage = storage / "generations" / disk["active_generation"]
                build = storage / "graph/builds" / graph["graph_build_id"]
                build_record = json.loads((build / "build.json").read_text())
                spec_fields = {
                    "graph_build_id": "graphBuildId", "operation_id": "operationId",
                    "attempt": "attempt", "base_text_revision": "baseTextRevision",
                    "base_catalog_digest": "baseCatalogDigest", "base_head": "baseHead",
                    "scope": "scope", "configuration": "configuration",
                    "policy_fingerprint": "policyFingerprint", "rules_fingerprint": "rulesFingerprint",
                    "module_roots_fingerprint": "moduleRootsFingerprint",
                }
                spec = raw / f"{name}-spec.json"
                runner.write_json(spec, {key: build_record[value] for key, value in spec_fields.items()})
                audit_path = raw / f"{name}-audit.json"
                with (raw / f"{name}-audit.log").open("w") as log:
                    auditor = args.reference_audit_binary if label == labels[0] and args.reference_audit_binary else args.audit_binary
                    subprocess.run([str(auditor), str(workspace), str(build / "db"),
                                    str(spec), str(audit_path)], env=env, stdout=log,
                                   stderr=subprocess.STDOUT, check=True)
                audit = json.loads(audit_path.read_text())
                assert audit["proof_verified"] and audit["facts_digest"] == audit["catalog_digest"]
                if args.scenario == "semantic":
                    assert audit["semantic"]["digest"] == semantic_fixture["semantic_digest"]
                    assert audit["semantic"]["entities"] == 1000
                    assert audit["semantic"]["episodes"] == 1000
                    assert audit["semantic"]["facts"] == 2000
                    assert audit["semantic"]["associations"] == 3000
                    assert audit["semantic"]["mentions"] == 1000
                profiles = [json.loads(line.removeprefix("ZG_GRAPH_PROFILE "))
                            for line in stderr.read_text().splitlines()
                            if line.startswith("ZG_GRAPH_PROFILE ")]
                # Reopen in another process and require the selected graph to be reused.
                reopen_out, reopen_err = raw / f"{name}-reopen.json", raw / f"{name}-reopen.log"
                reopen = runner.measured([str(binary), str(workspace), json.dumps(options),
                                          str(args.model_cache)], env=run_env, workspace=workspace,
                                         stdout_path=reopen_out, stderr_path=reopen_err)
                if reopen["exit_code"]:
                    raise RuntimeError(f"TP reopen failed: {reopen_err}")
                reopened = runner.graph_result(json.loads(reopen_out.read_text()))
                assert reopened["mode"] == "unchanged"
                assert reopened["graph_build_id"] == graph["graph_build_id"]
                record = {"label": label, "repeat": repeat + 1, "binary_sha256": binary_sha256,
                          "measurement": measurement,
                          "wall_time_micros": payload["wall_time_micros"], "graph": graph,
                          "disk": disk, "build_record": build_record, "profiles": profiles,
                          "audit": {key: audit[key] for key in ("proof_verified", "facts_digest", "source_projection_digest", "counts", "semantic")},
                          "tp_reopen_unchanged": True}
                runner.write_json(raw / f"{name}.json", record)
                runs.append(record)
                runner.write_json(output / "partial.json", runs)
                shutil.rmtree(workspace)
    summaries = {}
    assert len({r["audit"]["source_projection_digest"] for r in runs}) == 1
    for label in binaries:
        selected = [r for r in runs if r["label"] == label]
        assert all(r["build_record"]["importBudget"] == selected[0]["build_record"]["importBudget"]
                   for r in selected)
        summary = {"budget": selected[0]["build_record"]["importBudget"], "runs": len(selected)}
        for field in ("wall_time_micros",):
            summary[field] = [r[field] for r in selected]
            summary[field + "_median"] = statistics.median(summary[field])
        for field in ("observed_peak_rss_bytes", "observed_peak_workspace_delta_bytes",
                      "observed_peak_graph_builds_delta_bytes"):
            values = [r["measurement"][field] for r in selected]
            summary[field] = values
            summary[field + "_median"] = statistics.median(values)
        if args.profile:
            for parent, stage in (("import.write", "import.batch_write"),
                                  ("import.write", "import.batch_confirm"),
                                  ("import.write", "import.admit")):
                measurements = [next(m for m in r["profiles"][0]["measurements"]
                                     if m["parent"] == parent and m["stage"] == stage)
                                for r in selected]
                summary[stage] = {"calls": [m["calls"] for m in measurements],
                                  "inclusive_micros_median": statistics.median(
                                      m["inclusive_micros"] for m in measurements)}
            summary["commits"] = [r["profiles"][0]["counters"] for r in selected]
        summaries[label] = summary
    runner.write_json(output / "results.json", {
        "profile": args.profile, "scenario": args.scenario, "summaries": summaries, "runs": runs,
        "binaries": {label: {"path": str(path), "sha256": runner.sha256(path)}
                     for label, path in binaries.items()},
        "corpus_sha256": manifest["sha256"],
        "measurement_scope": "fresh process; graph-only through checkpoint and HEAD publication; text setup excluded",
        "peak_semantics": "sampled RSS and logical file bytes; nominal 10ms RSS/50ms disk, not hard peaks",
    })
    (output / "partial.json").unlink()
    print(json.dumps(summaries, indent=2), flush=True)


if __name__ == "__main__":
    main()
