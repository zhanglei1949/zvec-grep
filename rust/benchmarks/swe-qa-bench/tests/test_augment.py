from __future__ import annotations

import hashlib
import io
import json
import tarfile
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import AsyncMock, patch
from types import SimpleNamespace

from zg_bench.swe_qa.augment import commands, package_identity, run
from zg_bench.swe_qa.augment_protocol import (
    DATA, EXECUTION, GRAPH, INSTRUCTIONS, PROTOCOL,
    configuration, digest, inventory, matrix, read_json, write_json,
)
from zg_bench.swe_qa.augment_report import (
    audit_mcp, bootstrap, collect, judge, opencode_usage, provider_usage, report, summarize,
)
from zg_bench.swe_qa.judge import SCORE_KEYS


def proxy_events(profile):
    args = {"root": "/app", "fts": ["def"], "autoUpdate": False, "freshness": "eventual",
            "graph": {"mode": "off"} if profile == "zg-off" else GRAPH}
    if profile == "zg-augment":
        args["graphExecution"] = EXECUTION
    request = {"id": 3, "method": "tools/call", "params": {"name": "zvec_grep_search", "arguments": args}}
    return [
        {"event": "start", "config": {"profile": profile, "graph": GRAPH,
            "graph_execution": EXECUTION, "instructions": INSTRUCTIONS}},
        {"event": "tools", "tools": [{"name": "zvec_grep_search", "inputSchema": {}}], "instructions": INSTRUCTIONS},
        {"event": "request", "request": request},
        {"event": "response", "request": request, "response": {"id": 3, "result": {}}, "elapsed_ms": 5,
         "graph": {"status": "absent" if profile == "zg-off" else "complete", "evidence": 0}},
    ]


def jsonl(path, rows):
    path.write_text("".join(json.dumps(row) + "\n" for row in rows))


def fixture(experiment, *, missing_usage=False, no_answer=False):
    candidate = experiment / "candidate.tgz"
    candidate.write_bytes(b"fixture-only-package")
    manifest = {**configuration(), "tier": "smoke", "matrix": matrix("smoke"),
                "candidate": {"sha256": digest(candidate)},
                "assets": {name: digest(DATA / name) for name in ("selection.json", "references.json")}}
    write_json(experiment / "manifest.json", manifest)
    for row in manifest["matrix"]:
        trial = experiment / "jobs" / row["job"] / "trial"
        agent = trial / "agent"
        agent.mkdir(parents=True)
        result = {"exception_info": None, "agent_result": {"n_input_tokens": 30, "n_output_tokens": 10},
                  "task_name": row["task_slug"], "finished_at": "2026-11-03T00:00:04+00:00",
                  "agent_info": {"name": "opencode", "version": "1.18.4",
                                 "model_info": {"name": "qwen3.7-max", "provider": "dashscope"}},
                  "agent_execution": {"started_at": "2026-11-03T00:00:00+00:00", "finished_at": "2026-11-03T00:00:04+00:00"}}
        write_json(trial / "result.json", result)
        write_json(agent / "zvec-grep-setup.json", {
            "status": "ready", "graph_profile": row["profile"], "graph_seed_key": row["repository"],
            "graph_preflight": {"ready": True}, "index_inventory": {"file": "hash"},
            "protocol": PROTOCOL, "package_sha256": digest(candidate),
            "embedding_model": "qwen/qwen3.7-text-embedding",
            "index_seed_identity": {"commit": row["commit"], "package": digest(candidate)},
        })
        write_json(agent / "augment-index-after.json", {"file": "hash"})
        write_json(agent / "augment-timing.json", result["agent_execution"])
        jsonl(agent / "augment-preflight.jsonl", proxy_events("zg-augment"))
        jsonl(agent / "augment-mcp.jsonl", proxy_events(row["profile"]))
        finish = {"reason": "tool-calls" if no_answer else "stop",
                  "tokens": {"input": 25, "output": 10, "cache": {"read": 5, "write": 2}}}
        if missing_usage:
            del finish["tokens"]["input"]
        jsonl(agent / "opencode.txt", [{"type": "step_finish", "part": finish}])
        jsonl(agent / "augment-provider-usage.jsonl", [
            {"event": "request", "id": 1, "model": "qwen3.7-max"},
            {"event": "response", "id": 1, "status": 200, "usage": {
                "prompt_tokens": 32, "completion_tokens": 10, "prompt_tokens_details": {"cached_tokens": 5}}},
        ])
        write_json(agent / "trajectory.json", {"steps": [
            {"source": "agent", "message": "search", "tool_calls": [
                {"function_name": "zvec_grep_search", "arguments": {}, "tool_call_id": "one"},
                {"function_name": "read", "arguments": {}, "tool_call_id": "two"}]},
            {"source": "agent", "message": "Answer", "tool_calls": []},
        ]})
    return manifest


class ProtocolTests(unittest.TestCase):
    def test_full_dry_run_freezes_120_commands_without_starting_docker(self):
        with tempfile.TemporaryDirectory() as temporary:
            package = Path(temporary) / "candidate.tgz"
            package.write_bytes(b"fixture package")
            output = Path(temporary) / "planned"
            with (
                patch("zg_bench.swe_qa.augment.package_identity", return_value={"sha256": digest(package)}),
                patch("zg_bench.runner.prepare_setup_cache", side_effect=AssertionError("must not prepare Docker")),
                redirect_stdout(io.StringIO()),
            ):
                run(output, "full", package, None, True)
            self.assertEqual(read_json(output / "manifest.json")["planned_runs"], 120)
            self.assertEqual(len(read_json(output / "commands.json")), 120)
            self.assertFalse((output / "jobs").exists())

    def test_matrix_and_adjacent_counterbalanced_pairs(self):
        self.assertEqual(len(matrix("full")), 120)
        self.assertEqual(len(matrix("smoke")), 10)
        rows = matrix("full")
        for offset in range(0, len(rows), 2):
            left, right = rows[offset:offset + 2]
            self.assertEqual((left["task_id"], left["repeat"]), (right["task_id"], right["repeat"]))
            self.assertNotEqual(left["profile"], right["profile"])
        self.assertEqual(sum(row["profile"] == "zg-off" for row in rows), 60)
        self.assertEqual(rows[0]["profile"], "zg-off")
        self.assertEqual(rows[2]["profile"], "zg-augment")
        self.assertEqual(len({row["repository"] for row in rows}), 11)

    def test_commands_use_same_agent_tools_and_model_with_no_retry(self):
        with tempfile.TemporaryDirectory() as temporary:
            result = commands(Path(temporary), "smoke", "/tmp/candidate.tgz", "a" * 64)
        self.assertEqual(len(result), 10)
        contracts = []
        for command in result:
            self.assertIn("zg_bench.agents.augment_opencode:AugmentOpenCode", command)
            self.assertIn("dashscope/qwen3.7-max", command)
            self.assertEqual(command[command.index("--n-attempts") + 1], "1")
            self.assertEqual(command[command.index("--max-retries") + 1], "0")
            config = next(value for value in command if value.startswith("opencode_config="))
            contracts.append(json.loads(config.split("=", 1)[1]))
        self.assertTrue(all(config == contracts[0] for config in contracts))
        self.assertEqual(contracts[0]["mcp"]["zvec_grep"]["type"], "local")
        self.assertFalse(contracts[0]["provider"]["dashscope"]["models"]["qwen3.7-max"]["options"]["enable_thinking"])

    def test_package_rejects_wrong_platform_and_corrupt_native_payload(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "candidate.tgz"
            def pack(platform="linux", corrupt=False):
                binary = b"\x7fELF" + b"\0" * 14 + b"\x3e\x00" + b"fixture"
                entries = {"package.json": json.dumps({"os": [platform], "cpu": ["x64"]}).encode(),
                           "bin/zg": binary,
                           "checksums.json": json.dumps({"files": {"bin/zg": {
                               "bytes": len(binary), "sha256": "bad" if corrupt else hashlib.sha256(binary).hexdigest()}}}).encode()}
                with tarfile.open(path, "w:gz") as archive:
                    for name, data in entries.items():
                        info = tarfile.TarInfo("package/" + name)
                        info.size = len(data)
                        archive.addfile(info, io.BytesIO(data))
            pack()
            self.assertEqual(package_identity(path)["sha256"], digest(path))
            pack("darwin")
            with self.assertRaisesRegex(ValueError, "Linux x64"):
                package_identity(path)
            pack(corrupt=True)
            with self.assertRaisesRegex(ValueError, "checksum"):
                package_identity(path)


class UsageAndAuditTests(unittest.TestCase):
    def test_cache_counted_once_and_harbor_reconciliation_value_is_separate(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "events"
            jsonl(path, [{"type": "step_finish", "part": {"reason": "stop", "tokens": {
                "input": 100, "output": 15, "cache": {"read": 40, "write": 8}}}}])
            usage = opencode_usage(path)
        self.assertEqual(usage["input_tokens"], 148)
        self.assertEqual(usage["total_tokens"], 163)
        self.assertEqual(usage["harbor_input_tokens"], 140)
        self.assertTrue(usage["complete"])

    def test_missing_usage_and_cache_are_not_zero(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "events"
            jsonl(path, [{"type": "step_finish", "part": {"reason": "stop", "tokens": {"output": 5}}}])
            with self.assertRaisesRegex(ValueError, "missing input/output"):
                opencode_usage(path)
            jsonl(path, [{"type": "step_finish", "part": {"reason": "stop", "tokens": {"input": 2, "output": 5}}}])
            usage = opencode_usage(path)
            self.assertIsNone(usage["input_tokens"])
            self.assertFalse(usage["complete"])

    def test_provider_totals_are_cache_inclusive_and_missing_calls_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "provider"
            events = [{"event": "request", "id": 1, "model": "qwen3.7-max"}, {"event": "response", "id": 1, "status": 200,
                       "usage": {"prompt_tokens": 140, "completion_tokens": 10,
                                 "prompt_tokens_details": {"cached_tokens": 40}}}]
            jsonl(path, events)
            self.assertEqual(provider_usage(path)["total_tokens"], 150)
            jsonl(path, events[:1])
            with self.assertRaisesRegex(ValueError, "missing provider"):
                provider_usage(path)

    def test_treatment_drift_is_detected_but_product_errors_are_kept(self):
        events = proxy_events("zg-augment")
        events[-1]["graph"] = {"status": "error", "evidence": 0}
        self.assertFalse(audit_mcp(events, "zg-augment")["issues"])
        events[2]["request"]["params"]["arguments"]["graph"] = {"mode": "off"}
        self.assertIn("incorrect experimental treatment", audit_mcp(events, "zg-augment")["issues"])

    def test_bootstrap_uses_task_values_and_zero_denominator_is_na(self):
        self.assertEqual(bootstrap([2.0] * 5), [2.0, 2.0])
        self.assertEqual(bootstrap([1.0, -2.0, 3.0]), bootstrap([1.0, -2.0, 3.0]))
        value = summarize([{"metrics": {"tool_calls": {"off": 0, "augment": 3}}}], "tool_calls")
        self.assertEqual(value["delta"], 3)
        self.assertIsNone(value["change_pct"])


class EvidenceAndReportTests(unittest.TestCase):
    def test_judge_is_blind_and_failure_is_pending_not_zero(self):
        from zg_bench.swe_qa import SweQaError
        with tempfile.TemporaryDirectory() as temporary:
            experiment = Path(temporary)
            fixture(experiment)
            collect(experiment)
            with (
                patch.dict("os.environ", {"GLM_API_KEY": "fixture-key"}),
                patch("zg_bench.swe_qa.augment_report._default_completion"),
                patch("zg_bench.swe_qa.augment_report._judge_candidate", side_effect=SweQaError("fixture failure")) as score,
            ):
                judge(experiment)
            self.assertEqual(score.call_count, 10)
            for call in score.call_args_list:
                self.assertNotIn("profile", call.kwargs)
                self.assertNotIn("trajectory", call.kwargs)
                self.assertEqual(call.kwargs["attempts"], 1)
            self.assertTrue(all(read_json(path)["status"] == "pending" for path in (experiment / "scores").glob("*zg-*.json")))
            result = report(experiment)
            self.assertFalse(result["scoring_complete"])
            self.assertIsNone(result["aggregate"]["quality"]["off"])

    def test_end_to_end_collection_and_offline_report(self):
        with tempfile.TemporaryDirectory() as temporary:
            experiment = Path(temporary)
            fixture(experiment)
            collected = collect(experiment)
            self.assertEqual(len(collected["trials"]), 10)
            self.assertTrue(all(not row["issues"] for row in collected["trials"]))
            self.assertEqual(collected["trials"][0]["metrics"]["input_tokens"], 32)
            for row in collected["trials"]:
                score = 10 if row["profile"] == "zg-off" else 12
                write_json(experiment / "scores" / (row["id"] + ".json"), {
                    "id": row["id"], "status": "scored",
                    "answer_sha256": hashlib.sha256(row["answer"].encode()).hexdigest(),
                    "judge": {"model": "glm-5.2", "self_judge": False,
                              "scores": dict.fromkeys(SCORE_KEYS, score), "total": score * 5,
                              "latency_seconds": .1, "usage": {"input_tokens": 12, "output_tokens": 5}},
                })
            result = report(experiment)
            self.assertTrue(result["integrity"]["passed"])
            self.assertEqual(result["aggregate"]["quality"]["delta"], 10)
            self.assertEqual(result["aggregate"]["quality"]["ci95"], [10, 10])
            self.assertEqual(result["judge"]["usage"]["input_tokens"], 120)
            self.assertIn("quality_improved", result["interpretation"])
            self.assertTrue((experiment / "report" / "review.md").is_file())

    def test_missing_trial_cannot_pass_and_pending_judge_not_zero(self):
        with tempfile.TemporaryDirectory() as temporary:
            experiment = Path(temporary)
            fixture(experiment)
            missing = experiment / "jobs" / matrix("smoke")[0]["job"] / "trial" / "result.json"
            missing.unlink()
            collect(experiment)
            result = report(experiment)
            self.assertFalse(result["integrity"]["passed"])
            self.assertFalse(result["scoring_complete"])
            self.assertIsNone(result["aggregate"]["quality"]["off"])
            self.assertEqual(result["interpretation"], "incomplete")

    def test_no_final_answer_is_zero_with_observed_costs_retained(self):
        with tempfile.TemporaryDirectory() as temporary:
            experiment = Path(temporary)
            fixture(experiment, no_answer=True)
            rows = collect(experiment)["trials"]
            self.assertTrue(all(row["status"] == "no_answer" for row in rows))
            self.assertEqual(rows[0]["metrics"]["total_tokens"], 42)
            result = report(experiment)
            self.assertEqual(result["aggregate"]["quality"]["off"], 0)
            self.assertEqual(result["diagnostics"]["no_answer"], 10)

    def test_changed_storage_or_evidence_blocks_integrity(self):
        with tempfile.TemporaryDirectory() as temporary:
            experiment = Path(temporary)
            fixture(experiment)
            trial = experiment / "jobs" / matrix("smoke")[0]["job"] / "trial"
            (trial / "agent" / "augment-index-after.json").write_text('{"file":"changed"}')
            collection = collect(experiment)
            self.assertTrue(any("inventory" in issue for issue in collection["trials"][0]["issues"]))
            (trial / "agent" / "opencode.txt").write_text("tampered\n")
            with self.assertRaisesRegex(ValueError, "evidence changed"):
                report(experiment)

    def test_original_protocol_profiles_are_unchanged(self):
        from zg_bench.runner import PROFILES
        from zg_bench.swe_qa.collect import PROFILES as collected_profiles
        self.assertEqual(PROFILES, ("baseline", "zvec-grep"))
        self.assertEqual(collected_profiles, PROFILES)


class AgentSeedTests(unittest.IsolatedAsyncioTestCase):
    async def test_seed_is_copied_and_corruption_is_not_rebuilt(self):
        from zg_bench.agents.augment_opencode import AugmentOpenCode
        class Harness(AugmentOpenCode):
            def __init__(self, root):
                self._index_seed_root = root
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "seed" / "workspace"
            source.mkdir(parents=True)
            (source / "graph").write_bytes(b"graph fixture")
            write_json(root / "seed" / "inventory.json", inventory(source))
            agent = Harness(root)
            environment = SimpleNamespace(upload_dir=AsyncMock())
            self.assertTrue(await agent._restore_index_seed(environment, "/app", seed_key="seed"))
            environment.upload_dir.assert_awaited_once_with(source, "/app/.zvec-grep")
            (source / "graph").write_bytes(b"changed")
            with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
                await agent._restore_index_seed(environment, "/app", seed_key="seed")
            self.assertEqual(environment.upload_dir.await_count, 1)

    async def test_failed_agent_retains_timing_and_post_index_audit(self):
        from zg_bench.agents.augment_opencode import AugmentOpenCode
        class Harness(AugmentOpenCode):
            def __init__(self, logs):
                self.logs_dir = logs
            async def exec_as_agent(self, environment, **kwargs):
                return SimpleNamespace(stdout='{"graph":"hash"}')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            agent = Harness(root)
            with patch("zg_bench.agents.zvec_opencode.ZvecOpenCode.run", new=AsyncMock(side_effect=RuntimeError("fixture failure"))):
                with self.assertRaisesRegex(RuntimeError, "fixture failure"):
                    await agent.run("question", object(), object())
            self.assertTrue((root / "augment-timing.json").is_file())
            self.assertEqual(read_json(root / "augment-index-after.json"), {"graph": "hash"})
