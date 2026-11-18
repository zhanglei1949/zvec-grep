"""Three-arm dispatch, the exact known defect, and a blind Qwen judge probe."""
import copy
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from zg_bench.release_e2e import check_smoke_judges, make_smoke_task, run
from zg_bench.release_smoke import KNOWN_CALLER_ISSUE, smoke_arguments, smoke_review


class ThreeArmSmokeTests(unittest.TestCase):
    def row(self, root, group="agent"):
        agent = root / "agent"
        agent.mkdir()
        calls, rpc, wire, attempts = [], [], [], []
        outputs = {
            "search": "#1 lib.rs:2-2\nsymbol: normalize\nvalue.max(0)\n",
            "callers": "save lib.rs:4\nimport_record lib.rs:6\ncall at lib.rs:6:43\nexport_record lib.rs:8\ncall at lib.rs:8:43\nstorage::flush lib.rs:16\ncall at lib.rs:16:39\n",
            "callees": "save lib.rs:4\nnormalize lib.rs:2\ncall at lib.rs:4:34\n",
        }
        for tool, arguments in smoke_arguments(group).items():
            for actor in ("root", "child"):
                sequence = len(calls) + 1
                stamp = 200 + 50 * sequence
                name = "zvec_grep_" + tool
                calls.append({"name": "zvec_grep_" + name, "input": arguments, "session_id": actor,
                              "call_id": str(sequence), "status": "completed", "output": outputs[tool],
                              "started_at": stamp + 1, "finished_at": stamp + 30})
                rpc.append({"sequence": sequence, "name": name, "arguments": arguments,
                            "status": "succeeded", "text": outputs[tool]})
                wire.append({"type": "request", "sequence": sequence, "time": stamp})
                attempts.append({"name": "zvec_grep_" + name, "valid_json": True, "schema_valid": True})
        (agent / "model-requests.jsonl").write_text(json.dumps({"type": "request", "time": 100, "body": {"tools": [{}]}}) + "\n")
        (agent / "mcp-execution.jsonl").write_text("\n".join(map(json.dumps, wire)))
        return {"group": group, "status": "completed", "evidence": str(root), "model_attempts": attempts,
                "session_usage": {"root_session_id": "root"}, "calls": calls, "mcp": {"calls": rpc},
                "surface": {"preflight_calls": [1]}, "native_surface": {"preflight_calls": [{"name": "zvec_grep_" + t} for t in outputs]}}

    def test_three_prompts_include_exact_combined_fts_globs_and_only_available_tools(self):
        for group, count in (("search", 1), ("agent", 3), ("relationships", 2)):
            with self.subTest(group=group), tempfile.TemporaryDirectory() as temporary:
                suite = make_smoke_task(Path(temporary), group)
                prompt = (suite.path / "release-dev/instruction.md").read_text()
                self.assertEqual(len(smoke_arguments(group)), count)
                self.assertIn("subagent_type explore", prompt)
                if group != "relationships":
                    self.assertIn('{"root":"/app","fts":"normalize","globs":"lib.rs"}', prompt)
                else:
                    self.assertNotIn("zvec_grep_zvec_grep_search", prompt)

    def test_exact_known_false_edge_is_retained_as_quality_defect(self):
        with tempfile.TemporaryDirectory() as temporary:
            row = self.row(Path(temporary))
            review = smoke_review(row)
            self.assertTrue(review["technical_passed"])
            self.assertFalse(review["all_returned_locations_correct"])
            self.assertEqual(len(review["calls"]), 6)
            self.assertEqual(len(review["known_quality_defects"]), 2)
            self.assertEqual({r["issue"] for r in review["known_quality_defects"]}, {KNOWN_CALLER_ISSUE})
            self.assertIn("storage::flush", row["calls"][2]["output"])

    def test_unknown_false_edge_and_missing_correct_edge_still_block(self):
        for old, new in (("16:39", "16:40"), ("call at lib.rs:8:43\n", "")):
            with self.subTest(change=new), tempfile.TemporaryDirectory() as temporary:
                row = self.row(Path(temporary))
                for call in row["calls"]:
                    call["output"] = call["output"].replace(old, new)
                for call in row["mcp"]["calls"]:
                    call["text"] = call["text"].replace(old, new)
                self.assertIn("callers_fixture_locations_differ_from_source", smoke_review(row)["errors"])

    def test_sdk_completion_without_unique_matching_wire_call_is_not_enough(self):
        with tempfile.TemporaryDirectory() as temporary:
            row = self.row(Path(temporary), "search")
            row["mcp"]["calls"] = []
            result = smoke_review(row)
            self.assertFalse(result["technical_passed"])
            self.assertIn("search_not_executed_by_main", result["errors"])
            self.assertIn("search_not_executed_by_child", result["errors"])

    def test_qwen_judge_model_and_blind_payload_are_saved(self):
        runtime = SimpleNamespace(api_key="fake-test-secret", configuration=SimpleNamespace(base_url="http://127.0.0.1"))
        for response_model, expected in (("qwen3.8-max", True), ("glm-5.2", False)):
            with self.subTest(response_model=response_model), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                requests = []
                def completion(**kwargs):
                    requests.append(kwargs)
                    return {"model": response_model, "choices": [{"message": {"content": json.dumps({k: 17 for k in ("correctness", "completeness", "relevance", "clarity", "coherence")})}}],
                            "usage": {"prompt_tokens": 50, "completion_tokens": 20}}
                rows = [{"task": "release-dev", "group": group, "round": 0, "repeat": 1, "status": "completed",
                         "answer": "save delegates to normalize; only import_record and export_record call the root save."}
                        for group in ("search", "agent", "relationships")]
                with patch("zg_bench.release_e2e.resolve_opencode_model", return_value=runtime), \
                     patch("zg_bench.release_e2e.default_completion", return_value=completion), \
                     patch("zg_bench.engines.judge.time.sleep"):
                    result = check_smoke_judges(rows, root, "qwen3.8-max")
                self.assertEqual(result["passed"], expected)
                self.assertEqual(len(requests), 3 if expected else 9)
                for file in root.rglob("request-1.json"):
                    body = json.loads(file.read_text())
                    self.assertEqual(body["model"], "openai/qwen3.8-max")
                    self.assertNotIn("fake-test-secret", file.read_text())
                    self.assertNotIn("tools", body)
                    self.assertNotIn("group", body)
                self.assertFalse((root / "trials.json").exists())
                self.assertTrue((root / "technical-trials.json").exists())

    def test_passing_technical_checks_and_judge_do_not_start_formal_without_flag(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "result"
            def job(*args, **kwargs):
                return [{"group": kwargs["group"], "status": "completed"}]
            with patch("zg_bench.release_e2e.verify_release", return_value={"platform": "linux-x64-gnu"}), \
                 patch("zg_bench.release_e2e.shutil.which", return_value="docker"), \
                 patch("zg_bench.release_e2e.resolve_opencode_model"), \
                 patch("zg_bench.release_e2e.subprocess.check_output", return_value="f" * 40), \
                 patch("zg_bench.release_e2e.prepare_overlay", return_value=Path(temporary) / "compose.json"), \
                 patch("zg_bench.release_e2e.run_job", side_effect=job) as jobs, \
                 patch("zg_bench.release_e2e.smoke_review", return_value={"errors": [], "technical_passed": True}), \
                 patch("zg_bench.release_e2e.check_smoke_judges", return_value={"passed": True}) as judges, \
                 patch("zg_bench.release_e2e.load_suite") as formal:
                code = run(SimpleNamespace(release=Path("release.json"), output=output, execute_full=False, model="qwen3.8-max"))
            self.assertEqual(code, 0)
            self.assertEqual(jobs.call_count, 3)
            self.assertTrue(all(c.kwargs["model"] == "custom-openai/qwen3.8-max" and c.kwargs["number"] == 0 for c in jobs.call_args_list))
            judges.assert_called_once()
            formal.assert_not_called()
            self.assertEqual(json.loads((output / "execution-status.json").read_text())["formal_trial_records"], 0)


if __name__ == "__main__":
    unittest.main()
