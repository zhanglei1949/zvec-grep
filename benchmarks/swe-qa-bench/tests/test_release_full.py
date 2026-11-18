"""CI partitioning must preserve the accepted protocol and all 900 trial slots."""
import asyncio
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import yaml

from zg_bench import release_full as full
from zg_bench.agents.release_index import ReleaseIndexOnly
from zg_bench.agents.release_opencode import ReleaseOpenCode
from zg_bench.release_analysis import METRICS
from zg_bench.release_packages import write_json

ROOT = Path(__file__).resolve().parents[3]
MANIFEST = {"packages": [{"sha256": "main"}, {"sha256": "platform"}]}


def snapshot(root, task="conan-1"):
    identity = {"repo_commit": full.plan()["source"][task]["repository_commit"],
                "zvec_grep_version": "0.3.1", "zvec_grep_package": "main",
                "release_platform_sha256": "platform", "embedding_model": "local/potion-code-16m-v2",
                "device": "cpu", "graph_index": True,
                "index_ignore_sha256": full.plan()["input_sha256"]["index.ignore"]}
    key = hashlib.sha256(json.dumps(identity, separators=(",", ":"), sort_keys=True).encode()).hexdigest()
    seed = root / key
    write_json(seed / "identity.json", identity)
    (seed / "complete").write_text(key)
    write_json(seed / "workspace/graph.json", {"example": True})
    write_json(seed / "payload-sha256.json", ReleaseOpenCode._snapshot_hashes(seed / "workspace"))
    return seed


class FullReleaseTests(unittest.TestCase):
    def test_twenty_tasks_share_eleven_commit_bound_indexes(self):
        p = full.plan()
        self.assertEqual(len(p["tasks"]), 20)
        self.assertEqual(len(p["indexes"]), 11)
        self.assertEqual(p["planned_trials"], 900)
        indexes = {r["task"]: r["index"] for r in p["tasks"]}
        self.assertEqual(indexes["conan-19"], indexes["conan-1"])
        self.assertEqual(indexes["xarray-32"], indexes["xarray-46"])

    def test_snapshot_hashes_source_and_package_identity_are_required(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); seed = snapshot(root)
            self.assertEqual(full.validate_index(root, "conan-19", MANIFEST), seed)
            with self.assertRaisesRegex(ValueError, "source/release/model"):
                full.validate_index(root, "requests-16", MANIFEST)
            (seed / "workspace/graph.json").write_text("changed")
            with self.assertRaisesRegex(ValueError, "payload hash"):
                full.validate_index(root, "conan-1", MANIFEST)

    def test_index_only_run_has_no_model_or_environment_access(self):
        with tempfile.TemporaryDirectory() as temp:
            fake = SimpleNamespace(logs_dir=Path(temp))
            asyncio.run(ReleaseIndexOnly.run(fake, "question is ignored", None, None))
            self.assertEqual(json.loads((Path(temp) / "index-only.json").read_text()),
                             {"model_requests": 0, "answering_started": False})

    def test_formal_missing_snapshot_cannot_silently_rebuild(self):
        with tempfile.TemporaryDirectory() as temp:
            fake = SimpleNamespace(_require_index_seed=True, _index_seed_dir=lambda key: Path(temp) / key)
            with self.assertRaisesRegex(RuntimeError, "refusing to rebuild"):
                asyncio.run(ReleaseOpenCode._restore_index_seed(fake, None, "/app", seed_key="missing"))

    def test_task_runs_three_arms_and_five_slots_without_changing_model(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); seed = snapshot(root / "input")
            args = SimpleNamespace(task="conan-19", round=2, release=root / "release.json", index=root / "input", output=root / "case")
            def job(*unused, **kw):
                self.assertEqual(kw["model"], full.MODEL)
                self.assertTrue(kw["require_index_seed"])
                self.assertEqual(kw["number"], 2)
                return [{"task": args.task, "group": kw["group"], "round": 2, "repeat": n,
                         "status": "completed", "first_request_sent": True} for n in range(1, 6)]
            def judge(rows, output, **kw):
                self.assertEqual(kw["model"], "qwen3.8-max")
                for row in rows:
                    row["judge"] = {"total": 80}
                    kw["on_progress"]()
            with patch.object(full, "verify_release", return_value=MANIFEST), \
                 patch.object(full, "prepare_overlay", return_value=root / "overlay"), \
                 patch.object(full, "run_job", side_effect=job) as jobs, \
                 patch.object(full, "score_rows", side_effect=judge), patch.dict(full.os.environ):
                full.run_task(args)
            self.assertEqual(jobs.call_count, 3)
            records = json.loads((args.output / "records/task-records.json").read_text())
            self.assertEqual(len(records["rows"]), 15)
            self.assertTrue(all(row["judge"]["total"] == 80 for row in records["rows"]))
            self.assertTrue((args.output / "index-snapshots" / seed.name).exists())
            self.assertTrue((args.output / "evidence-sha256.json").exists())

    def test_all_nine_hundred_records_aggregate_once_and_missing_is_not_zero(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); records = root / "records"; target = None
            for number in (1, 2, 3):
                for task in full.plan()["source"]:
                    rows = [{"task": task, "round": number, "group": group, "repeat": repeat,
                             "status": "completed", "first_request_sent": True, "usage_complete": True,
                             "judge": {"total": 81 if group == "agent" else 80},
                             "model_attempts": [{"name": "zvec_grep_zvec_grep_callers"}] if group == "agent" else [],
                             **{metric: 10 for metric in METRICS}}
                            for group in full.GROUPS for repeat in range(1, 6)]
                    target = records / f"{number}-{task}/task-records.json"
                    write_json(target, {"identity": full.identity(task, number, MANIFEST), "rows": rows, "protocol_errors": False})
            args = SimpleNamespace(records=records, release=root / "release.json", output=root / "summary", through_round=3)
            with patch.object(full, "verify_release", return_value=MANIFEST):
                self.assertEqual(full.aggregate(args), 0)
                result = json.loads((args.output / "report.json").read_text())
                self.assertTrue(result["complete"])
                self.assertEqual(result["planned"], 900)
                self.assertEqual(result["comparisons"][0]["metrics"]["score"]["delta_ci95"], [1, 1])
                self.assertEqual(result["relationship_adoption"]["agent"]["selected_trials"], 300)
                target.unlink()
                args.output = root / "incomplete"
                # Missing artifact represents 15 unknown trials, never 15 zeros.
                with patch.object(full, "summarize", wraps=full.summarize) as summary:
                    self.assertEqual(full.aggregate(args), 0)
                missing = [r for r in summary.call_args.args[0] if r["status"] == "missing"]
                self.assertEqual(len(missing), 15)
                self.assertTrue(all(r["judge"] is None and "input_tokens" not in r for r in missing))

    def test_duplicate_artifact_and_wrong_harness_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            meta = full.identity("requests-16", 1, MANIFEST)
            row = {"group": "search", "task": "requests-16", "round": 1, "repeat": 1}
            value = {"identity": meta, "rows": [row, row], "protocol_errors": False}
            write_json(root / "records/task-records.json", value)
            args = SimpleNamespace(records=root / "records", release=root / "release.json", output=root / "summary", through_round=1)
            with patch.object(full, "verify_release", return_value=MANIFEST):
                with self.assertRaisesRegex(ValueError, "Duplicate"):
                    full.aggregate(args)
                value["identity"]["harness"] = "different"
                write_json(root / "records/task-records.json", value)
                with self.assertRaisesRegex(ValueError, "identity"):
                    full.aggregate(args)


class FullWorkflowTests(unittest.TestCase):
    def load(self, name):
        return yaml.load((ROOT / f".github/workflows/{name}.yml").read_text(), Loader=yaml.BaseLoader)

    def test_full_and_smoke_are_exclusive_and_rounds_are_sequential(self):
        entry = self.load("swe-qa-bench")["jobs"]
        self.assertIn("!inputs.release_execute_full", entry["release-e2e"]["if"])
        self.assertIn("inputs.release_execute_full", entry["release-full"]["if"])
        jobs = self.load("swe-qa-release-full")["jobs"]
        self.assertEqual(jobs["round-1"]["needs"], ["prepare", "indexes"])
        for number in (2, 3):
            self.assertEqual(jobs[f"round-{number}"]["needs"], ["prepare", f"summary-{number-1}"])
            self.assertEqual(jobs[f"round-{number}"]["with"]["round_number"], str(number))
        for job in jobs.values():
            if "steps" in job:
                guard = job["steps"][0]
                self.assertEqual(guard["id"], "authorize")
                self.assertIn("admin|maintain", guard["run"])
                self.assertIn("BENCH_TRIGGERING_ACTOR", guard["run"])
                self.assertEqual(job["steps"][1]["uses"], "actions/checkout@v6")

    def test_two_task_jobs_each_run_three_arms_and_index_has_no_model_key(self):
        job = self.load("swe-qa-release-shards")["jobs"]["shard"]
        self.assertEqual(job["strategy"]["max-parallel"], "2")
        self.assertEqual(job["strategy"]["fail-fast"], "false")
        self.assertEqual(job["steps"][0]["id"], "authorize")
        index = next(s for s in job["steps"] if "release_full index" in s.get("run", ""))
        self.assertNotIn("GLM_API_KEY", index.get("env", {}))
        task = next(s for s in job["steps"] if "release_full task" in s.get("run", ""))
        self.assertIn("GLM_API_KEY", task["env"])
        self.assertIn('"$TASK_ID"', task["run"])
        uploads = [s["with"]["name"] for s in job["steps"] if s.get("uses", "").startswith("actions/upload-artifact")]
        self.assertEqual(len(uploads), 3)


if __name__ == "__main__":
    unittest.main()
