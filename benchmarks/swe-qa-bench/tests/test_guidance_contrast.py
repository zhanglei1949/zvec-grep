"""Boundaries of the six-task, voluntary-use guidance contrast."""
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

import yaml

from zg_bench.tool_experiment import ROOT, aggregate, audit_calls, experiment_tasks, write_json


class GuidanceContrastTests(unittest.TestCase):
    def test_every_experiment_phase_records_actual_requests(self):
        workflow = yaml.load((ROOT.parents[1] / ".github/workflows/swe-qa-bench.yml").read_text(), Loader=yaml.BaseLoader)
        step = next(s for s in workflow["jobs"]["experiment-task"]["steps"] if s.get("name") == "Run one experiment arm and judge its trials")
        for phase, trials in (("smoke", 1), ("full", 5), ("diagnostic", 2), ("guidance-contrast", 2)):
            with self.subTest(phase=phase), tempfile.TemporaryDirectory() as temp:
                capture = Path(temp) / "commands.jsonl"
                env = {**os.environ, "GLM_API_KEY": "fake-key", "SWE_QA_MODEL": "glm-5.2",
                       "EXPERIMENT_PHASE": phase, "EXPERIMENT_GROUP": "relationships", "TASK_NAME": "fixture",
                       "TASK_SET": "caller-callee", "GUIDANCE_VARIANT": "flow", "CANDIDATE_COMMIT": "candidate",
                       "GITHUB_SHA": "harness", "GITHUB_REPOSITORY": "owner/repo", "GITHUB_RUN_ID": "1",
                       "GITHUB_RUN_ATTEMPT": "1", "RUNNER_TEMP": temp, "CAPTURE": str(capture)}
                stub = 'uv() { "' + sys.executable + '" -c \'import json, os, sys; open(os.environ["CAPTURE"], "a").write(json.dumps(sys.argv[1:])+"\\n")\' "$@"; }\n'
                script = step["run"].replace("${{ inputs.embedding }}", "local")
                subprocess.run(["bash", "-c", stub + script], env=env, check=True, capture_output=True)
                command = json.loads(capture.read_text().splitlines()[0])
                self.assertIn("--record-model-requests", command)
                self.assertEqual(command[command.index("--n-attempts") + 1], str(trials))

    def test_frozen_variants_only_change_the_positioning_sentence(self):
        frozen = json.loads((ROOT / "fixtures/caller-callee-guidance-contrast-v1.json").read_text())["variants"]
        direct, flow = frozen["direct"], frozen["flow"]
        self.assertEqual(direct["tools"], flow["tools"])
        for field in ("instructions", "guidance"):
            before, after = (surface[field].split("For a known symbol", 1) for surface in (direct, flow))
            self.assertEqual(before[0], after[0])
            self.assertEqual(before[1].split(". A known symbol", 1)[1], after[1].split(". A known symbol", 1)[1])
            self.assertIn("no tool call is mandatory", flow[field])

    def test_contrast_keeps_zero_call_trials_and_requires_all_six_tasks(self):
        tasks = experiment_tasks("caller-callee", "guidance-contrast")
        self.assertEqual(tasks, ["matplotlib-37", "sympy-38", "xarray-32", "requests-cc1", "matplotlib-cc2", "sympy-cc2"])
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for task in tasks:
                trial = {"trial_name": "fixture", "judge": {"total": 50}, "input_tokens": 100,
                         "output_tokens": 20, "tool_calls": 0, "agent_wall_seconds": 1,
                         "cost_usd": None, "audit": audit_calls([]), "model_requests": {
                             "tool_bearing_requests": 1, "context_issue_request_ids": [], "invalid_zg_attempts": []}}
                report = {"protocol": "caller-callee-guidance-contrast-v1", "candidate": "candidate",
                          "harness": "harness", "phase": "guidance-contrast", "expected_trials": 2,
                          "model": "custom-openai/glm-5.2", "endpoint_sha256": "endpoint",
                          "group": "relationships", "task": task, "task_set": "caller-callee",
                          "guidance_variant": "flow", "graph_index": True, "surface_sha256": "surface",
                          "trials": [trial] * 2}
                path = root / "input" / task / "task-report.json"
                write_json(path, report)
                path.with_suffix(".sha256").write_text(hashlib.sha256(path.read_bytes()).hexdigest())
            args = SimpleNamespace(reports=root / "input", output=root / "out", phase="guidance-contrast", group="relationships", task_set="caller-callee")
            aggregate(args)
            result = json.loads((args.output / "report.json").read_text())
            self.assertTrue(result["complete"])
            self.assertEqual(result["usage_rates"]["relationships"]["trials"], 12)
            self.assertEqual(result["usage_rates"]["relationships"]["tools"]["callees"]["used_trials"], 0)
            path.unlink()
            with self.assertRaisesRegex(ValueError, "1 missing"):
                aggregate(args)


if __name__ == "__main__":
    unittest.main()
