"""Caller/callee diagnostic task set: frozen assets, scopes, and experiment wiring."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

import yaml

from zg_bench.core.errors import SweQaError
from zg_bench.runner import load_suite
from zg_bench.swe_qa.selection import CI_SCOPES, select_ci_tasks
from zg_bench.swe_qa.validation_cc import validate_caller_callee_assets
from zg_bench.tool_experiment import DATA, TASK_SETS, aggregate, audit_calls, experiment_tasks, write_json

ROOT = Path(__file__).resolve().parents[1]
SELECTION = DATA / "selection-caller-callee.json"
REFERENCES = DATA / "references-caller-callee.json"
DATASET = ROOT / "datasets-caller-callee"


def _selection() -> dict:
    return json.loads(SELECTION.read_text())


class CallerCalleeAssetTests(unittest.TestCase):
    def test_reference_answers_exclude_candidate_tool_observations(self) -> None:
        references = json.loads(REFERENCES.read_text())
        self.assertEqual(references["reference_revision"], 2)
        records = {r["task_id"]: r for r in references["references"]}
        observations = {
            "requests:cc1": "a callers query returns Session::send plus the test callers",
            "matplotlib:cc1": "callers of _make_line returns empty edges in the preflight build",
            "sympy:cc1": "callers results for ode_order are truncated at the default 20-edge limit",
        }
        for task_id, observation in observations.items():
            with self.subTest(task=task_id):
                self.assertNotIn(observation, records[task_id]["reference_answer"])
                self.assertIn(observation, records[task_id]["verification_notes"])
        for task_id, record in records.items():
            with self.subTest(task=task_id):
                for phrase in ("Tool-behavior note:", "static callers query", "preflight build", "older candidate builds"):
                    self.assertNotIn(phrase, record["reference_answer"])

    def test_frozen_assets_validate(self) -> None:
        result = validate_caller_callee_assets(
            selection_path=SELECTION, references_path=REFERENCES, dataset_path=DATASET
        )
        self.assertTrue(result["valid"])
        self.assertEqual(result["suite"], "caller-callee")
        self.assertEqual(result["task_count"], 11)
        self.assertTrue(result["references_are_judge_only"])

    def test_question_hash_mismatch_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            selection = _selection()
            selection["tasks"][0]["question_hash"] = "0" * 64
            path = Path(temp) / "selection.json"
            path.write_text(json.dumps(selection))
            with self.assertRaisesRegex(SweQaError, "SHA256"):
                validate_caller_callee_assets(
                    selection_path=path, references_path=REFERENCES, dataset_path=DATASET
                )

    def test_anchor_tasks_must_omit_source_index(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            selection = _selection()
            anchor = next(t for t in selection["tasks"] if t["task_id"] == "requests:cc1")
            anchor["source_index"] = 0
            path = Path(temp) / "selection.json"
            path.write_text(json.dumps(selection))
            with self.assertRaisesRegex(SweQaError, "omit source_index"):
                validate_caller_callee_assets(
                    selection_path=path, references_path=REFERENCES, dataset_path=DATASET
                )

    def test_scenario_composition_is_enforced(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            selection = _selection()
            selection["tasks"][0]["scenario"] = "control"
            path = Path(temp) / "selection.json"
            path.write_text(json.dumps(selection))
            with self.assertRaisesRegex(SweQaError, "6 main, 1 control, 1 stress, and 3 anchor"):
                validate_caller_callee_assets(
                    selection_path=path, references_path=REFERENCES, dataset_path=DATASET
                )

    def test_reference_leak_into_dataset_is_rejected(self) -> None:
        references = json.loads(REFERENCES.read_text())["references"]
        with tempfile.TemporaryDirectory() as temp:
            dataset = Path(temp) / "datasets"
            shutil.copytree(DATASET, dataset)
            leaked = dataset / "requests-cc1" / "tests" / "test.sh"
            leaked.write_text(leaked.read_text() + "\n# " + references[0]["reference_answer"])
            with self.assertRaisesRegex(SweQaError, "leaked"):
                validate_caller_callee_assets(
                    selection_path=SELECTION, references_path=REFERENCES, dataset_path=dataset
                )


class CallerCalleeScopeTests(unittest.TestCase):
    def test_shared_scopes_select_frozen_tasks(self) -> None:
        selection = _selection()
        task_ids, slugs = select_ci_tasks(selection, "smoke")
        self.assertEqual(task_ids, ["requests:cc1", "requests:45", "sympy:cc1"])
        self.assertEqual(slugs, ["requests-cc1", "requests-45", "sympy-cc1"])
        self.assertEqual(list(load_suite("caller-callee", tier="smoke").tasks), slugs)
        task_ids, slugs = select_ci_tasks(selection, "all-full")
        self.assertEqual(len(task_ids), 11)
        self.assertEqual(task_ids, selection["gate"]["full_tasks"])
        suite = load_suite("caller-callee", tier="full")
        self.assertEqual(list(suite.tasks), slugs)
        self.assertEqual(suite.path, DATASET.resolve())

    def test_removed_scopes_cannot_silently_select_either_task_set(self) -> None:
        qa20 = json.loads((DATA / "selection.json").read_text())
        for scope in ("cc-smoke", "cc-full"):
            self.assertNotIn(scope, CI_SCOPES)
            for selection in (qa20, _selection()):
                with self.subTest(scope=scope, tasks=len(selection["tasks"])):
                    with self.assertRaisesRegex(SweQaError, "unsupported SWE-QA scope"):
                        select_ci_tasks(selection, scope)

    def test_task_set_registry_points_at_frozen_files(self) -> None:
        for name, entry in TASK_SETS.items():
            with self.subTest(task_set=name):
                self.assertTrue((DATA / entry["selection"]).is_file())
                self.assertTrue((DATA / entry["references"]).is_file())
        self.assertNotIn("diagnostic", TASK_SETS["caller-callee"]["scopes"])


class CallerCalleeExperimentTests(unittest.TestCase):
    def _report(self, task: str, group: str = "agent", task_set: str = "caller-callee") -> dict:
        trial = {
            "trial_name": "fixture", "judge": {"total": 50}, "input_tokens": 100,
            "output_tokens": 20, "tool_calls": 0, "agent_wall_seconds": 1,
            "cost_usd": None, "audit": audit_calls([]),
        }
        return {
            "protocol": "caller-callee-e2e-v1", "candidate": "candidate",
            "harness": "harness", "phase": "full", "expected_trials": 5,
            "model": "custom-openai/glm-5.2", "endpoint_sha256": "endpoint",
            "group": group, "task": task, "graph_index": True,
            "surface_sha256": "surface", "task_set": task_set, "trials": [trial] * 5,
        }

    def _write_report(self, root: Path, report: dict) -> None:
        path = root / "artifacts" / report["task"] / "task-report.json"
        write_json(path, report)
        path.with_suffix(".sha256").write_text(
            hashlib.sha256(path.read_bytes()).hexdigest()
        )

    def test_aggregate_completes_for_all_eleven_tasks(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for task in _selection()["tasks"]:
                self._write_report(root, self._report(task["task_slug"]))
            args = SimpleNamespace(
                reports=root / "artifacts", group="agent", phase="full",
                task_set="caller-callee", output=root / "out",
            )
            aggregate(args)
            report = json.loads((args.output / "report.json").read_text())
            self.assertTrue(report["complete"])
            self.assertEqual(len(report["reports"]), 11)

    def test_diagnostic_phase_is_rejected_for_caller_callee(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "artifacts").mkdir()
            args = SimpleNamespace(
                reports=root / "artifacts", group="agent", phase="diagnostic",
                task_set="caller-callee", output=root / "out",
            )
            with self.assertRaisesRegex(ValueError, "not defined for task set"):
                aggregate(args)

    def test_mixed_task_sets_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self._write_report(root, self._report("requests-cc1"))
            self._write_report(root, self._report("requests-16", task_set="qa20"))
            args = SimpleNamespace(
                reports=root / "artifacts", group="agent", phase="full",
                task_set="caller-callee", output=root / "out",
            )
            with self.assertRaisesRegex(ValueError, "incompatible experiment reports"):
                aggregate(args)


    def test_reports_outside_selected_tasks_or_arm_are_rejected(self) -> None:
        for task, group, phase, error in (
            ("requests-16", "agent", "full", "unexpected task"),
            ("matplotlib-cc1", "agent", "smoke", "unexpected task"),
            ("requests-cc1", "search", "full", "unexpected phase or group"),
        ):
            with self.subTest(task=task, group=group, phase=phase), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                report = self._report(task, group=group)
                report["phase"] = phase
                self._write_report(root, report)
                args = SimpleNamespace(
                    reports=root / "artifacts", group="agent", phase=phase,
                    task_set="caller-callee", output=root / "out",
                )
                with self.assertRaisesRegex(ValueError, error):
                    aggregate(args)

    def test_mixed_guidance_or_judge_settings_are_rejected(self) -> None:
        for field, before, after in (
            ("guidance_variant", "direct", "flow"),
            ("judge_config", {"temperature": 0}, {"temperature": 1}),
            ("judge_config", None, {"temperature": 0}),
        ):
            with self.subTest(field=field, before=before), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                for task, value in (("requests-cc1", before), ("sympy-cc1", after)):
                    report = self._report(task)
                    if value is not None:
                        report[field] = value
                    self._write_report(root, report)
                args = SimpleNamespace(
                    reports=root / "artifacts", group="agent", phase="full",
                    task_set="caller-callee", output=root / "out",
                )
                with self.assertRaisesRegex(ValueError, "incompatible experiment reports"):
                    aggregate(args)


class CallerCalleeWorkflowTests(unittest.TestCase):
    def test_invalid_flow_combinations_fail_before_task_execution(self) -> None:
        workflow = yaml.load((ROOT.parents[1] / ".github/workflows/swe-qa-bench.yml").read_text(), Loader=yaml.BaseLoader)
        matrix = next(step for step in workflow["jobs"]["validate"]["steps"] if step.get("id") == "task-matrix")
        for task_set, group, phase, variant, valid in (
            ("qa20", "agent", "diagnostic", "flow", False),
            ("qa20", "legacy", "smoke", "flow", False),
            ("qa20", "agent", "diagnostic", "direct", True),
            ("qa20", "agent", "full", "flow", True),
            ("caller-callee", "agent", "smoke", "flow", True),
            ("caller-callee", "agent", "full", "direct", True),
            ("caller-callee", "agent", "diagnostic", "direct", False),
            ("caller-callee", "legacy", "smoke", "direct", False),
        ):
            with self.subTest(task_set=task_set, group=group, phase=phase, variant=variant), tempfile.TemporaryDirectory() as temp:
                env = {**os.environ, "TASK_SET": task_set, "EXPERIMENT_GROUP": group,
                       "EXPERIMENT_PHASE": phase, "GUIDANCE_VARIANT": variant,
                       "SCOPE": "all-full", "GITHUB_OUTPUT": str(Path(temp) / "output"),
                       "BENCH_TEST_PYTHON": sys.executable}
                # Reuse this interpreter without uv setup; run the real matrix CLI.
                script = 'uv() { shift 4; "$BENCH_TEST_PYTHON" "$@"; }\n' + matrix["run"]
                result = subprocess.run(["bash", "-c", script], cwd=ROOT.parents[1], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, valid, result.stdout + result.stderr)
                output = Path(env["GITHUB_OUTPUT"])
                if valid:
                    values = dict(line.split("=", 1) for line in output.read_text().splitlines())
                    self.assertEqual(json.loads(values["tasks"]), experiment_tasks(task_set, phase))
                else:
                    self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
