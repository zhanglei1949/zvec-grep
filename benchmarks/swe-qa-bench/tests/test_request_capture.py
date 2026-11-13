import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

from zg_bench.model_requests import audit_model_requests
from zg_bench.tool_experiment import DATA, aggregate, audit_calls, write_json
from zg_bench.swe_qa.selection import select_ci_tasks


class ModelRequestTests(unittest.TestCase):
    def test_missing_context_is_a_diagnostic_result_not_an_omitted_trial(self):
        surface = {"guidance": "relation guidance", "tools": [{"name": "zvec_grep_callers"}]}
        events = [
            {"type": "started", "upstream": "http://upstream/v1"},
            {"type": "request", "id": 1, "time": 1, "body_sha256": "body", "body": {
                "model": "glm-5.2", "tools": [{"function": {"name": "grep"}}],
                "messages": [{"role": "system", "content": "native only"}]}},
            {"type": "closed"},
        ]
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "requests.jsonl"
            path.write_text("\n".join(json.dumps(event) for event in events))
            audit = audit_model_requests(path, surface, [])
            self.assertEqual(audit["context_issue_request_ids"], [1])
            self.assertEqual(audit["requests"][0]["missing_zg_tools"], ["zvec_grep_zvec_grep_callers"])
            self.assertFalse(audit["requests"][0]["guidance_present"])
            path.write_text("\n".join(json.dumps(event) for event in events[:-1]))
            with self.assertRaisesRegex(ValueError, "did not complete"):
                audit_model_requests(path, surface, [])

    def test_relationship_policy_is_identical_after_discovery_in_both_arms(self):
        fixture = json.loads((Path(__file__).resolve().parents[1] / "fixtures/caller-callee-guidance-v2.json").read_text())["groups"]
        self.assertEqual(fixture["relationships"]["instructions"].split("For a known symbol", 1)[1], fixture["agent"]["instructions"].split("For a known symbol", 1)[1])

    def test_diagnostic_requires_only_two_arms_three_tasks_two_trials(self):
        selection = json.loads((DATA / "selection.json").read_text())
        _, tasks = select_ci_tasks(selection, "diagnostic-3")
        self.assertEqual(tasks, ["matplotlib-37", "sympy-38", "xarray-32"])
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for group in ("relationships", "agent"):
                for task in tasks:
                    path = root / "input" / group / task / "task-report.json"
                    trial = {"trial_name": "fixture", "judge": {"total": 50}, "input_tokens": 100,
                        "output_tokens": 20, "tool_calls": 0, "agent_wall_seconds": 1, "cost_usd": None,
                        "audit": audit_calls([]), "model_requests": {"tool_bearing_requests": 1,
                        "context_issue_request_ids": [], "invalid_zg_attempts": []}}
                    write_json(path, {"protocol": "caller-callee-context-diagnostic-v1", "candidate": "candidate",
                        "harness": "harness", "phase": "diagnostic", "expected_trials": 2,
                        "model": "custom-openai/glm-5.2", "endpoint_sha256": "endpoint", "group": group,
                        "task": task, "graph_index": True, "surface_sha256": group, "trials": [trial] * 2})
                    path.with_suffix(".sha256").write_text(hashlib.sha256(path.read_bytes()).hexdigest())
            aggregate(SimpleNamespace(reports=root / "input", output=root / "output", phase="diagnostic", group="all"))
            result = json.loads((root / "output/report.json").read_text())
            self.assertTrue(result["complete"])
            self.assertEqual(sum(len(r["trials"]) for r in result["reports"]), 12)
