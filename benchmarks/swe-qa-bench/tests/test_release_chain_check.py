"""The chain check preserves product surfaces and separates actor evidence."""
import copy
import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

import yaml

from zg_bench.release_chain_check import CASES, QUERY, baseline_protocol, evaluate, make_case, resumable_cases, useful_output
from zg_bench.release_e2e import release_command
from zg_bench.release_packages import sha256
from zg_bench.agents.release_opencode import MODEL_FILES, MODEL_PREFIX, embedding_cache_warmup_command

ROOT = Path(__file__).resolve().parents[1]


class ChainCheckTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which("node"), "requires Node for cache preflight")
    def test_cold_model_cache_warms_outside_frozen_workspace_and_warm_cache_is_reused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture = root / "fixture"
            fixture.mkdir()
            index = fixture / "frozen-index"
            index.write_bytes(b"unchanged")
            binary = root / "bin"
            binary.mkdir()
            fake = binary / "zg"
            fake.write_text("#!/bin/sh\nprintf '%s\\n' \"$PWD\" >> \"$WARMUP_TRACE\"\n")
            fake.chmod(0o755)
            env = {**os.environ, "PATH": str(binary) + os.pathsep + os.environ["PATH"],
                   "ZVEC_GREP_MODEL_CACHE": str(root / "cache"), "WARMUP_TRACE": str(root / "trace")}
            subprocess.run(["bash", "-ec", embedding_cache_warmup_command()], cwd=fixture, env=env, check=True, capture_output=True)
            scratch = Path((root / "trace").read_text().strip())
            self.assertNotEqual(scratch, fixture)
            self.assertFalse(scratch.exists())
            self.assertEqual(index.read_bytes(), b"unchanged")
            cache = root / "cache" / MODEL_PREFIX
            for file in MODEL_FILES:
                target = cache / file
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(b"present")
            subprocess.run(["bash", "-ec", embedding_cache_warmup_command()], cwd=fixture, env=env, check=True, capture_output=True)
            self.assertEqual(len((root / "trace").read_text().splitlines()), 1)

    def test_minimal_and_optional_cases_are_distinct(self):
        self.assertEqual(len(CASES), 7)
        for case in CASES:
            keys = set(case["arguments"])
            base = {"root", "query" if case["tool"] == "search" else "symbol"}
            if case["phase"] == "minimal":
                self.assertEqual(keys, base)
            else:
                self.assertEqual(len(keys - base), 1)
                self.assertTrue(keys - base <= {"fts", "globs"})

    def test_development_prompts_do_not_change_guidance_or_run_qa20(self):
        with tempfile.TemporaryDirectory() as temporary:
            suite = make_case(Path(temporary), CASES[0])
            prompt = (suite.path / "release-dev/instruction.md").read_text()
            self.assertIn(json.dumps(CASES[0]["arguments"], separators=(",", ":")), prompt)
            self.assertIn("main agent", prompt)
            self.assertIn("subagent_type explore", prompt)
            self.assertNotIn('"fts"', prompt)
            self.assertNotIn('"globs"', prompt)
            self.assertIn("timeout_sec = 300.0", (suite.path / "release-dev/task.toml").read_text())
            self.assertEqual(list(Path(temporary).rglob("AGENTS.md")), [])

    def test_source_validation_rejects_empty_and_wrong_targets(self):
        self.assertTrue(useful_output("search", "#1 lib.rs:2-2\nsymbol: normalize\nvalue.max(0)")["correct_and_useful"])
        self.assertFalse(useful_output("search", "save lib.rs:4")["correct_and_useful"])
        text = "save  lib.rs:4\n Callees:\n normalize  lib.rs:2\n call at lib.rs:4:34\n"
        self.assertTrue(useful_output("callees", text)["correct_and_useful"])
        self.assertFalse(useful_output("callees", text.replace("save  lib.rs:4", "storage::save  lib.rs:14"))["correct_and_useful"])
        callers = "save  lib.rs:4\n import_record  lib.rs:6\n call at lib.rs:6:43\n export_record  lib.rs:8\n call at lib.rs:8:43\n"
        self.assertTrue(useful_output("callers", callers)["correct_and_useful"])
        self.assertFalse(useful_output("callers", "No resolved call sites.")["correct_and_useful"])

    def row(self, directory):
        surface = {"guidance": "published", "instructions": "published MCP", "tools": [{"name": "zvec_grep_search"}]}
        agent = directory / "agent"
        agent.mkdir()
        log = [{"type": "request", "id": 1, "time": 100, "body": {"tools": [{}]}}, {"type": "guidance_verified", "first_task_request": True}]
        (agent / "model-requests.jsonl").write_text("\n".join(map(json.dumps, log)))
        (agent / "mcp-execution.jsonl").write_text("\n".join(json.dumps({"type": "request", "sequence": sequence, "time": stamp}) for sequence, stamp in [(1, 10), (2, 210), (3, 310)]))
        output = "#1 lib.rs:2-2\nsymbol: normalize\nvalue.max(0)"
        calls = [{"name": "zvec_grep_zvec_grep_search", "input": CASES[0]["arguments"], "session_id": actor,
                  "call_id": actor, "status": "completed", "output": output, "started_at": stamp, "finished_at": stamp + 50}
                 for actor, stamp in [("root", 200), ("child", 300)]]
        mcp = [{"sequence": sequence, "name": "zvec_grep_search", "arguments": CASES[0]["arguments"], "status": "succeeded", "text": output} for sequence in (1, 2, 3)]
        return {"status": "completed", "surface": surface, "native_surface": copy.deepcopy(surface), "session_usage": {"root_session_id": "root"},
                "calls": calls, "model_attempts": [{"name": "zvec_grep_zvec_grep_search", "valid_json": True, "schema_valid": True}],
                "context_errors": [], "mcp": {"calls": mcp}, "evidence": str(directory)}

    def test_both_actors_need_successful_wire_calls_and_exact_arguments(self):
        with tempfile.TemporaryDirectory() as temporary:
            row = self.row(Path(temporary))
            self.assertTrue(evaluate(CASES[0], row)["passed"])
            row["calls"][1]["input"] = {**row["calls"][1]["input"], "globs": "lib.rs"}
            result = evaluate(CASES[0], row)
            self.assertFalse(result["actors"]["child"]["passed"])
            self.assertFalse(result["passed"])

    def test_direct_native_probe_never_counts_as_an_agent_call(self):
        with tempfile.TemporaryDirectory() as temporary:
            row = self.row(Path(temporary))
            row["mcp"]["calls"] = row["mcp"]["calls"][:1]
            self.assertFalse(evaluate(CASES[0], row)["passed"])
            row["mcp"]["calls"] += [{"sequence": 2, "name": "zvec_grep_search", "arguments": CASES[0]["arguments"], "status": "succeeded"}]
            row["surface"]["instructions"] = "altered"
            self.assertFalse(evaluate(CASES[0], row)["published_guidance_schema_unchanged"])

    def test_one_millisecond_state_lag_needs_identical_payload_and_unique_rpc(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            row = self.row(root)
            row["calls"][0]["started_at"] = 211
            self.assertTrue(evaluate(CASES[0], row)["actors"]["main"]["passed"])
            row["mcp"]["calls"][1]["text"] = "different source"
            self.assertFalse(evaluate(CASES[0], row)["actors"]["main"]["passed"])

    def test_workflow_chain_mode_cannot_fall_through_to_formal(self):
        workflow = yaml.load((ROOT.parents[1] / ".github/workflows/swe-qa-bench.yml").read_text(), Loader=yaml.BaseLoader)
        self.assertIn("!inputs.release_chain_check", workflow["jobs"]["package-candidate"]["if"])
        steps = workflow["jobs"]["release-e2e"]["steps"]
        formal = next(step for step in steps if step.get("name") == "Technical smokes then three formal rounds")
        self.assertIn("!inputs.release_chain_check", formal["if"])
        diagnostic = [step for step in steps if "zg_bench.release_chain_check" in step.get("run", "")]
        self.assertEqual(len(diagnostic), 2)
        self.assertTrue(all("--execute-full" not in step["run"] for step in diagnostic))
        self.assertTrue(all('--model "$SWE_QA_MODEL"' in step["run"] for step in diagnostic))

    def test_model_switch_preserves_all_other_client_and_generation_settings(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            suite = make_case(directory, CASES[0])
            configs = []
            for model in ("glm-5.2", "qwen3.8-max"):
                command = release_command(suite, Path("release.json"), Path("jobs"), "agent", 0,
                                          overlay=Path("compose.json"), smoke=True, model="custom-openai/" + model)
                self.assertEqual(command[command.index("--model") + 1], "custom-openai/" + model)
                config = json.loads(next(value.split("=", 1)[1] for value in command if value.startswith("opencode_config=")))
                selected = config["provider"]["custom-openai"]["models"].pop(model)
                selected.pop("name")
                config["provider"]["custom-openai"]["models"]["selected"] = selected
                config["model"] = "selected"
                configs.append(config)
            self.assertEqual(configs[0], configs[1])
            with self.assertRaisesRegex(ValueError, "limited to technical"):
                release_command(suite, Path("release.json"), Path("jobs"), "agent", 1,
                                overlay=Path("compose.json"), model="custom-openai/qwen3.8-max")

    def test_baseline_index_and_cases_must_match_before_reusing(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            manifest = {"version": "0.3.1"}
            protocol = {"protocol": "release-0.3.1-chain-check-v1", "release": manifest, "cases": CASES,
                        "fixture_sha256": sha256(ROOT / "fixtures/caller-callee-dev/lib.rs")}
            (directory / "protocol.json").write_text(json.dumps(protocol))
            seed = directory / "index-snapshots/key"
            workspace = seed / "workspace"
            workspace.mkdir(parents=True)
            source = workspace / "index.bin"
            source.write_bytes(b"frozen index")
            (seed / "payload-sha256.json").write_text(json.dumps({"index.bin": sha256(source)}))
            self.assertEqual(baseline_protocol(directory, manifest)[0], directory)
            source.write_bytes(b"different index")
            with self.assertRaisesRegex(ValueError, "recorded hashes"):
                baseline_protocol(directory, manifest)

    def test_recovery_preserves_completed_sessions_only_for_same_model(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for case, status in ((CASES[0], "completed"), (CASES[1], "failed")):
                directory = root / "cases" / case["id"]
                directory.mkdir(parents=True)
                row = {"case": case, "session_status": status, "published_guidance_schema_unchanged": True,
                       "actual_context_verified": True, "invalid_schema_attempts": 0,
                       "actors": {actor: {"executed_successfully": True} for actor in ("main", "child")}}
                (directory / "case-report.json").write_text(json.dumps(row))
            self.assertEqual(resumable_cases(root, {"model": "custom-openai/qwen3.8-max"}, "custom-openai/qwen3.8-max"), ["minimal-search"])
            with self.assertRaisesRegex(ValueError, "identical model"):
                resumable_cases(root, {"model": "custom-openai/glm-5.2"}, "custom-openai/qwen3.8-max")


if __name__ == "__main__":
    unittest.main()
