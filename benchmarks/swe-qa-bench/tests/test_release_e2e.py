"""Release control and accounting tests; no remote model or registry requests."""
import base64
import hashlib
import io
import json
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from types import SimpleNamespace
from pathlib import Path
from unittest.mock import patch

from zg_bench.release_packages import package_members, verify_integrity, prepare_release, verify_release, write_json
from zg_bench.release_analysis import GROUPS, METRICS, bootstrap, summarize, tool_attempts
from zg_bench.release_e2e import check_smoke, collect_job, make_smoke_task, release_command, run
from zg_bench.runner import load_suite

ROOT = Path(__file__).resolve().parents[1]


def archive(files):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as target:
        for name, value in files.items():
            info = tarfile.TarInfo(name); info.size = len(value)
            target.addfile(info, io.BytesIO(value))
    return output.getvalue()


class ReleasePackagesTests(unittest.TestCase):
    def test_registry_integrity_and_native_identity_are_required(self):
        platform_files = {"bin/zg": {"bytes": 6, "sha256": hashlib.sha256(b"native").hexdigest()}}
        payload_cache = {}
        def fake_fetch(url):
            platform = "linux-x64-gnu" in url
            name = "@zvec/zvec-grep" + ("-linux-x64-gnu" if platform else "")
            files = {"package/package.json": json.dumps({"name": name, "version": "0.3.1", "optionalDependencies": {"@zvec/zvec-grep-linux-x64-gnu": "0.3.1"}}).encode()}
            if platform:
                files.update({"package/checksums.json": json.dumps({"files": platform_files}).encode(), "package/bin/zg": b"native"})
            payload = payload_cache.setdefault(name, archive(files))
            if url.endswith(".tgz"):
                return payload
            return json.dumps({"name": name, "version": "0.3.1", "dist": {"tarball": f"https://registry.npmjs.org/{name}/package.tgz", "shasum": hashlib.sha1(payload).hexdigest(), "integrity": "sha512-" + base64.b64encode(hashlib.sha512(payload).digest()).decode()}}).encode()
        with tempfile.TemporaryDirectory() as temp, patch("zg_bench.release_packages.fetch", side_effect=fake_fetch):
            path = Path(temp)
            prepare_release(path)
            self.assertEqual(verify_release(path / "release.json")["version"], "0.3.1")
            (path / "platform.tgz").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "archive changed"):
                verify_release(path / "release.json")
        with self.assertRaisesRegex(ValueError, "integrity"):
            verify_integrity(b"payload", {"integrity": "bad", "shasum": "bad"})

    def test_archive_paths_and_links_cannot_escape(self):
        for name in ("/absolute", "package/../../escape", "other/file"):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "Unsafe"):
                package_members(archive({name: b"bad"}))


class ReleaseAccountingTests(unittest.TestCase):
    def rows(self):
        return [{"task": task, "group": group, "round": number, "repeat": repeat, "status": "completed", "usage_complete": True,
                 "judge": {"total": 80 + (group == "agent")}, **{metric: 10 for metric in METRICS}}
                for group in GROUPS for task in ("requests-16", "xarray-32") for number in (1, 2, 3) for repeat in range(1, 6)]

    def test_three_rounds_use_tasks_as_units_and_reject_duplicates(self):
        rows = self.rows()
        report = summarize(rows, ["requests-16", "xarray-32"])
        self.assertTrue(report["complete"])
        self.assertEqual(report["planned"], 90)
        self.assertEqual(report["comparisons"][0]["metrics"]["score"]["delta_ci95"], [1, 1])
        self.assertEqual(report["comparisons"][0]["metrics"]["score"]["tasks"], 2)
        with self.assertRaisesRegex(ValueError, "duplicate"):
            summarize(rows + rows[:1], ["requests-16", "xarray-32"])

    def test_failures_missing_usage_and_missing_slots_are_not_zero_scores(self):
        rows = self.rows()
        rows[0].update(status="failed", judge=None, usage_complete=False, input_tokens=50)
        report = summarize(rows[:-1], ["requests-16", "xarray-32"])
        self.assertFalse(report["complete"])
        self.assertIsNone(report["groups"]["search"]["task_equal_score"])
        self.assertEqual(report["comparisons"][0]["paired_tasks"], ["xarray-32"])
        self.assertEqual(len(report["missing_slots"]), 1)
        self.assertEqual(report["groups"]["search"]["usage_incomplete_trials"], 1)
        self.assertEqual(bootstrap([(0, 0)], draws=10)["change_pct"], None)

    def test_raw_sse_retains_malformed_arguments_not_seen_by_mcp(self):
        with tempfile.TemporaryDirectory() as temp:
            agent = Path(temp); (agent / "model-responses").mkdir()
            value = {"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"name": "zvec_grep_zvec_grep_callees", "arguments": '{"globs": lib.rs}'}}]}}]}
            (agent / "model-responses/1.raw").write_text("data: " + json.dumps(value) + "\n\ndata: [DONE]\n\n")
            attempts = tool_attempts(agent)
            self.assertFalse(attempts[0]["valid_json"])
            self.assertEqual(attempts[0]["arguments_raw"], '{"globs": lib.rs}')

    def test_smoke_does_not_count_direct_probe_as_agent_use(self):
        row = {"group": "relationships", "status": "completed", "model_attempts": [], "calls": [], "surface": {"preflight_calls": [{"name": "callers"}]}}
        self.assertIn("callers_not_executed_by_both_main_and_child", check_smoke(row))
        with tempfile.TemporaryDirectory() as temp:
            rows = collect_job(Path(temp), task="requests-16", group="search", number=1, expected=5)
            self.assertEqual(len(rows), 5)
            self.assertTrue(all(row["status"] == "missing" and row["judge"] is None for row in rows))

    def test_harbor_commands_pin_release_and_disable_agent_retries(self):
        command = release_command(load_suite("swe-qa-bench", tier="full"), Path("release.json"), Path("jobs"), "relationships", 2, overlay=Path("compose.json"))
        self.assertEqual(command[command.index("--max-retries") + 1], "0")
        self.assertEqual(command[command.index("--n-attempts") + 1], "5")
        self.assertIn("round_number=2", command)
        self.assertIn("zg_bench.agents.release_opencode:ReleaseOpenCode", command)
        self.assertFalse(any("guidance_variant=" in item or "fixed_search=" in item for item in command))

    def test_development_fixture_is_separate_from_qa20_and_gold(self):
        with tempfile.TemporaryDirectory() as temp:
            suite = make_smoke_task(Path(temp), "relationships")
            prompt = (suite.path / "release-dev/instruction.md").read_text()
            self.assertIn('"globs":"lib.rs"', prompt)
            self.assertIn("subagent_type explore", prompt)
            self.assertNotIn("Coordinates", prompt)

    def test_failed_smoke_cannot_start_formal_qa20(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "fresh-output"
            def failed_job(*args, **kwargs):
                return [{"group": kwargs["group"], "status": "failed", "calls": [], "model_attempts": []}]
            with patch("zg_bench.release_e2e.verify_release", return_value={"platform": "linux-x64-gnu"}), \
                 patch("zg_bench.release_e2e.shutil.which", return_value="docker"), \
                 patch("zg_bench.release_e2e.resolve_opencode_model"), \
                 patch("zg_bench.release_e2e.subprocess.check_output", return_value="a" * 40), \
                 patch("zg_bench.release_e2e.prepare_overlay", return_value=Path(temp) / "compose.json"), \
                 patch("zg_bench.release_e2e.run_job", side_effect=failed_job) as jobs:
                code = run(SimpleNamespace(release=Path(temp) / "release.json", output=output, execute_full=True))
            self.assertEqual(code, 2)
            self.assertEqual(jobs.call_count, 1)
            self.assertTrue(all(call.kwargs["number"] == 0 for call in jobs.call_args_list))
            self.assertFalse((output / "round-1").exists())
            gate = json.loads((output / "smoke-gate.json").read_text())
            self.assertFalse(gate["passed"])
            self.assertEqual(gate["not_started_groups"], ["agent", "relationships"])

    def test_release_workflow_keeps_source_flow_and_uploads_failed_smoke(self):
        import yaml
        workflow = yaml.load((ROOT.parents[1] / ".github/workflows/swe-qa-bench.yml").read_text(), Loader=yaml.BaseLoader)
        self.assertIn("!inputs.release_e2e", workflow["jobs"]["package-candidate"]["if"])
        steps = workflow["jobs"]["release-e2e"]["steps"]
        run_step = next(step for step in steps if step.get("name") == "Technical smokes then three formal rounds")
        self.assertIn("--execute-full", run_step["run"])
        upload = next(step for step in steps if step.get("name") == "Preserve release evidence including blocked smoke")
        self.assertIn("always()", upload["if"])
        self.assertEqual(upload["with"]["include-hidden-files"], "true")
        self.assertEqual(workflow["jobs"]["run-pair"]["steps"][-1]["name"], "Save this task's populated zvec index seed")

    @unittest.skipUnless(shutil.which("node"), "requires Node for local transport tests")
    def test_node_transport_contracts(self):
        result = subprocess.run(["node", "--test", str(ROOT / "tests/release_contracts.test.mjs")], capture_output=True, text=True, timeout=45)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
