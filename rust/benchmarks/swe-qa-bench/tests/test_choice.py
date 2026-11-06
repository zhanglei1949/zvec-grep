from __future__ import annotations

import hashlib
import io
import json
import tarfile
import tempfile
import unittest
from collections import Counter
from contextlib import redirect_stdout
from copy import deepcopy
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

from choice_fixtures import catalogs, discovery, event_call, fixture, jsonl, scores
from zg_bench.swe_qa import augment_protocol as old
from zg_bench.swe_qa import choice, choice_report as reports
from zg_bench.swe_qa.choice_protocol import *


def package(path):
    binary = bytearray(64)
    binary[:4], binary[18:20] = b"\x7fELF", b"\x3e\x00"
    payloads = {"bin/zg": bytes(binary), "package.json": json.dumps({"os": ["linux"], "cpu": ["x64"]}).encode()}
    payloads["checksums.json"] = json.dumps({"files": {"bin/zg": {"bytes": len(binary), "sha256": hashlib.sha256(binary).hexdigest()}}}).encode()
    with tarfile.open(path, "w:gz") as archive:
        for name, value in payloads.items():
            info = tarfile.TarInfo("package/" + name); info.size = len(value)
            archive.addfile(info, io.BytesIO(value))


def accepted_smoke(root):
    candidate = root / "candidate.tgz"; package(candidate)
    smoke = root / "smoke"; smoke.mkdir()
    manifest = fixture(smoke, candidate=candidate)
    collection = reports.collect(smoke)
    scores(smoke, collection)
    status = reports.report(smoke)
    return candidate, smoke, manifest, collection, status


class ChoiceProtocolTests(unittest.TestCase):
    def test_matrix_rotates_positions_and_does_not_modify_legacy(self):
        before = deepcopy(old.configuration())
        self.assertEqual(len(matrix("smoke")), 15)
        rows = matrix("full")
        self.assertEqual(len(rows), 180)
        self.assertEqual(Counter(r["profile"] for r in rows), dict.fromkeys(PROFILES, 60))
        self.assertEqual(len({r["repository"] for r in rows}), 11)
        for task in tasks("full"):
            runs = [r for r in rows if r["task_id"] == task["task_id"]]
            for offset in range(3):
                self.assertEqual({runs[i]["profile"] for i in range(offset, 9, 3)}, set(PROFILES))
        config = configuration(); config["graph"]["seeds"] = 1
        self.assertEqual(old.configuration(), before)
        self.assertEqual(old.PROFILES, ("zg-off", "zg-augment"))
        self.assertEqual(len(old.matrix("full")), 120)

    def test_mac_dry_run_uses_real_package_validator_and_never_starts_docker(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); candidate = root / "candidate.tgz"; package(candidate)
            with patch("zg_bench.runner.prepare_setup_cache", side_effect=AssertionError("no Docker")), patch("zg_bench.runner.validate_profile_credentials", side_effect=AssertionError("no models")), redirect_stdout(io.StringIO()):
                choice.run(root / "experiment", "full", candidate, None, True)
            manifest = read_json(root / "experiment/manifest.json")
            commands = read_json(root / "experiment/commands.json")
            self.assertEqual(manifest["protocol"], PROTOCOL)
            self.assertEqual(len(commands), 180)
            for command in commands:
                self.assertIn("zg_bench.agents.choice_opencode:ChoiceOpenCode", command)
                config = json.loads(next(v.split("=", 1)[1] for v in command if v.startswith("opencode_config=")))
                profile = next(v.split("=", 1)[1] for v in command if v.startswith("choice_profile="))
                expected = "/tmp/choice_proxy.mjs" if profile == AUTO else "/tmp/augment_proxy.mjs"
                self.assertEqual(config["mcp"]["zvec_grep"]["command"][1], expected)
                self.assertEqual(config["tools"], {"task": False})
                self.assertEqual(command[command.index("--max-retries") + 1], "0")
            with self.assertRaises(FileExistsError), redirect_stdout(io.StringIO()):
                choice.run(root / "experiment", "full", candidate, None, True)

    def test_new_smoke_gate_rejects_old_protocol_and_changed_package(self):
        with tempfile.TemporaryDirectory() as temporary:
            candidate, smoke, manifest, _, status = accepted_smoke(Path(temporary))
            self.assertEqual(choice.validate_smoke(candidate, smoke), manifest)
            status["protocol"] = old.PROTOCOL
            (smoke / "report/report.json").write_text(json.dumps(status))
            with self.assertRaises(ValueError): choice.validate_smoke(candidate, smoke)
            status["protocol"] = PROTOCOL
            (smoke / "report/report.json").write_text(json.dumps(status))
            candidate.write_bytes(b"not the accepted candidate")
            with self.assertRaises((ValueError, tarfile.TarError)): choice.validate_smoke(candidate, smoke)

    def test_smoke_gate_rechecks_raw_evidence_collection_and_scores_without_rerunning(self):
        with tempfile.TemporaryDirectory() as temporary:
            candidate, smoke, _, collection, status = accepted_smoke(Path(temporary))
            row = collection["trials"][0]
            paths = [smoke / next(iter(row["evidence"])), smoke / "collection.json",
                     smoke / "scores" / (row["id"] + ".json")]
            with patch.object(reports, "_default_completion", side_effect=AssertionError("no model")), patch("zg_bench.runner.prepare_setup_cache", side_effect=AssertionError("no Docker")):
                choice.validate_smoke(candidate, smoke)
                for path in paths:
                    original = path.read_bytes()
                    for mutation in ("changed", "missing"):
                        with self.subTest(path=path.name, mutation=mutation):
                            try:
                                if mutation == "missing": path.unlink()
                                else: path.write_bytes(original + b" ")
                                with self.assertRaises((ValueError, OSError)):
                                    choice.validate_smoke(candidate, smoke)
                            finally:
                                path.write_bytes(original)
                report_path = smoke / "report/report.json"
                for field in ("collection_sha256", "score_evidence"):
                    with self.subTest(missing_report_field=field):
                        stale = deepcopy(status); stale.pop(field, None)
                        report_path.write_text(json.dumps(stale))
                        with self.assertRaises(ValueError): choice.validate_smoke(candidate, smoke)
                report_path.write_text(json.dumps(status))
                choice.validate_smoke(candidate, smoke)


class ChoiceAuditTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.raw, cls.projected = catalogs()

    def events(self, args=None):
        return discovery(AUTO, self.raw, self.projected) + event_call(AUTO, args or {"root": "/app", "query": "q"}, {"generation": "g"})

    def test_invalid_agent_parameters_are_errors_not_integrity_failures(self):
        events = self.events()
        events[-1]["response"] = {"id": 3, "error": {"code": -32602, "message": "invalid graph input"}}
        audit = reports.audit_mcp(events, AUTO)
        self.assertEqual(audit["issues"], [])
        self.assertEqual(audit["errors"], 1)
        rejected = self.events()[:-2] + [{"event": "rejected", "original": {"id": 3, "params": {"name": "zvec_grep_search", "arguments": {"root": "/app", "graph": None}}}, "response": {"id": 3, "error": {"code": -32602}}}]
        self.assertEqual(reports.audit_mcp(rejected, AUTO)["issues"], [])

    def test_parameter_rewriting_duplicate_ids_and_missing_responses_are_detected(self):
        events = self.events({"root": "/app", "query": "q", "graph": {"mode": "constrain", "scope": {"anchor": {"by": "name", "name": "f"}}}})
        events[-2]["forwarded"]["params"]["arguments"]["graph"] = {"mode": "off"}
        self.assertIn("Agent parameters changed by proxy", reports.audit_mcp(events, AUTO)["issues"])
        events = self.events()
        self.assertIn("missing MCP response", reports.audit_mcp(events[:-1], AUTO)["issues"])
        self.assertIn("duplicate MCP request ID", reports.audit_mcp(events + events[-2:], AUTO)["issues"])
        events[-1]["response"]["id"] = 999
        self.assertIn("unpaired MCP response", reports.audit_mcp(events, AUTO)["issues"])

    def test_partial_ambiguity_pagination_and_cancellation(self):
        args = {"root": "/app", "request": {"domain": "code", "cursor": "page-1", "operation": {"operation": "neighbors"}}}
        events = discovery(AUTO, self.raw, self.projected) + event_call(AUTO, args, {"generation": "g"}, name="zvec_grep_explore", status="partial", ambiguous=True, cursor="page-2")
        audit = reports.audit_mcp(events, AUTO)
        self.assertEqual(audit["partial"], 1)
        call = audit["calls"][0]
        self.assertEqual(call["cursor"], "page-1")
        self.assertEqual(call["graph"]["next_cursor"], "page-2")
        self.assertEqual(call["graph"]["locations"][0]["status"], "ambiguous")
        self.assertEqual(call["graph"]["evidence"], 0)  # structured and text are one result
        cancelled = events[:-1] + [{"event": "cancel", "request_id": 3}]
        self.assertEqual(reports.audit_mcp(cancelled, AUTO)["issues"], [])
        self.assertEqual(reports.audit_mcp(cancelled, AUTO)["calls"][0]["graph"]["status"], "cancelled")
        degraded = self.events({"root": "/app", "query": "q", "graph": {"mode": "augment"}})
        degraded[-1]["response"]["result"]["content"][0]["text"] = degraded[-1]["response"]["result"]["content"][0]["text"].replace('"complete"', '"degraded"')
        self.assertEqual(reports.audit_mcp(degraded, AUTO)["degraded"], 1)
        degraded[-1]["response"] = {"id": 3, "result": {"content": [{"type": "text", "text": "ordinary only"}]}}
        self.assertIn("requested graph result absent", reports.audit_mcp(degraded, AUTO)["issues"])

    def test_autonomous_guidance_and_schema_drift_are_rejected(self):
        events = self.events()
        events[1]["instructions"] = "search first always"
        self.assertIn("product guidance overridden", reports.audit_mcp(events, AUTO)["issues"])
        events = self.events()
        events[2]["tools"] = deepcopy(events[2]["tools"])
        events[2]["tools"][0]["description"] = "prefer graph"
        self.assertIn("product tool description overridden", reports.audit_mcp(events, AUTO)["issues"])

    def test_real_degraded_shape_is_counted_without_losing_strict_version_checks(self):
        value = {"status": "degraded", "view": None, "seeds": [], "skipped": [], "expansions": [],
                 "nodes": [], "relations": [], "evidence": [], "diagnostics": ["graph deadline exceeded"], "execution": None}
        for profile in ("zg-augment", AUTO):
            events = discovery(profile, self.raw, self.projected) + event_call(profile, {"root": "/app", "query": "q", "graph": {"mode": "augment"}} if profile == AUTO else {"root": "/app", "query": "q"}, {"generation": "g"})
            events[-1]["response"]["result"]["content"][0]["text"] = "ordinary\nGraph\n```json\n" + json.dumps(value) + "\n```"
            events[-1]["graph"] = {"status": "degraded", "view": None, "evidence": 0}
            with self.subTest(profile=profile):
                audit = reports.audit_mcp(events, profile)
                self.assertEqual(audit["issues"], [])
                self.assertEqual(audit["degraded"], 1)
                self.assertIsNone(audit["calls"][0]["graph"]["view"])
                self.assertEqual(audit["calls"][0]["graph"]["diagnostics"], value["diagnostics"])
        for status in ("complete", "partial"):
            bad = {**value, "status": status, "execution": {"stop_reason": None}}
            response = {"result": {"content": [{"type": "text", "text": "ordinary\nGraph\n```json\n" + json.dumps(bad) + "\n```"}]}}
            self.assertEqual(reports._observe(reports.SEARCH, response)["status"], "invalid")

    def test_locate_candidates_and_empty_neighbors_are_distinguished(self):
        for status, candidates, empty in (("unique", [{"id": "n"}], False), ("ambiguous", [{"id": "n"}, {"id": "m"}], False), ("not_found", [], True)):
            args = {"root": "/app", "request": {"domain": "code", "operation": {"operation": "locate", "target": {"by": "name", "name": "f"}}}}
            events = discovery(AUTO, self.raw, self.projected) + event_call(AUTO, args, {"generation": "g"}, name=reports.EXPLORE)
            value = events[-1]["response"]["result"]["structuredContent"]
            value["locations"] = [{"status": status, "candidates": candidates}]
            # The structured result is authoritative even if text is stale.
            with self.subTest(status=status):
                audit = reports.audit_mcp(events, AUTO)
                self.assertEqual(audit["issues"], [])
                self.assertEqual(audit["calls"][0]["graph"]["empty"], empty)
                text_only = {"result": {"content": [{"type": "text", "text": json.dumps(value)}]}}
                self.assertEqual(reports._observe(reports.EXPLORE, text_only, "locate")["empty"], empty)
            if status == "unique":
                self.assertTrue(reports._observe(reports.EXPLORE, {"result": {"structuredContent": value}}, "neighbors")["empty"])

    def test_mode_switches_are_counted_within_a_session(self):
        events = self.events()
        extra = event_call(AUTO, {"root": "/app", "query": "q", "graph": {"mode": "augment"}}, {"generation": "g"})
        extra[0]["original"]["id"] = extra[0]["forwarded"]["id"] = 4
        extra[1]["response"]["id"] = 4
        audit = reports.audit_mcp(events + extra, AUTO)
        self.assertEqual(audit["issues"], [])
        self.assertEqual(audit["mode_switches"], 1)


class ChoiceReportTests(unittest.TestCase):
    def make(self, root):
        manifest = fixture(root)
        collection = reports.collect(root)
        self.assertTrue(all(not r["issues"] for r in collection["trials"]), [r["issues"] for r in collection["trials"]])
        scores(root, collection)
        return manifest, collection

    def test_three_arm_collection_report_and_legacy_statistics_agree(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); _, collection = self.make(root)
            result = reports.report(root)
            self.assertTrue(result["integrity"]["passed"], result["integrity"])
            self.assertTrue(result["scoring_complete"] and result["usage_complete"])
            self.assertEqual(set(result["comparisons"]), {c[0] for c in COMPARISONS})
            quality = result["comparisons"]["choice_vs_off"]["aggregate"]["quality"]
            self.assertEqual((quality["baseline_mean"], quality["treatment_mean"], quality["delta"]), (50, 70, 20))
            self.assertEqual(quality["ci95"], [20., 20.])
            baseline = reports.legacy.summarize([{"metrics": {"quality": {"off": 50, "augment": 60}}}] * 5, "quality")
            pair = result["comparisons"]["augment_vs_off"]["aggregate"]["quality"]
            self.assertEqual(pair["delta"], baseline["delta"])
            self.assertEqual(pair["ci95"], baseline["ci95"])
            self.assertEqual(result["diagnostics"][AUTO]["modes"], {"off": 1, "augment": 1, "constrain": 1, "explore": 2})
            self.assertEqual(result["diagnostics"][AUTO]["continuation_calls"], 1)
            self.assertTrue(all(c["original"] is not None for r in collection["trials"] for c in r["mcp"]["calls"]))
            key = read_json(root / "report/review-key.json")
            self.assertEqual(set(next(iter(key.values())).values()), set(PROFILES))
            with self.assertRaises(FileExistsError): reports.report(root)

    def test_invalid_operation_names_remain_tool_errors_and_do_not_break_report(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); manifest = fixture(root)
            row = next(r for r in manifest["matrix"] if r["profile"] == AUTO and r["task_id"] == tasks("smoke")[3]["task_id"])
            agent = root / "jobs" / row["job"] / "trial/agent"
            raw, projected = catalogs(); events = discovery(AUTO, raw, projected)
            invalid_names = [["neighbors"], {"operation": "neighbors"}, 42, True, None, [], {}, "", "unknown-operation"]
            calls = []
            for index, operation in enumerate(invalid_names, 3):
                args = {"root": "/app", "request": {"domain": "code", "operation": {"operation": operation}}}
                wire = event_call(AUTO, args, {"generation": "g"}, name=reports.EXPLORE)
                wire[0]["original"]["id"] = wire[0]["forwarded"]["id"] = index
                wire[1]["response"] = {"id": index, "error": {"code": -32602, "message": "invalid operation"}}
                wire[1]["graph"] = {"status": "error", "evidence": 0}
                events.extend(wire)
                calls.append({"function_name": reports.EXPLORE, "arguments": args, "tool_call_id": str(index)})
            jsonl(agent / "choice-mcp.jsonl", events)
            trace = read_json(agent / "trajectory.json"); trace["steps"][0]["tool_calls"] = calls
            (agent / "trajectory.json").write_text(json.dumps(trace))
            collection = reports.collect(root)
            selected = next(r for r in collection["trials"] if r["id"] == row["job"])
            self.assertEqual(selected["issues"], [])
            self.assertEqual(selected["mcp"]["errors"], len(invalid_names))
            self.assertEqual([c["original"]["params"]["arguments"]["request"]["operation"]["operation"] for c in selected["mcp"]["calls"]], invalid_names)
            scores(root, collection)
            result = reports.report(root)
            self.assertTrue(result["integrity"]["passed"])
            self.assertEqual(result["diagnostics"][AUTO]["errors"], len(invalid_names))
            self.assertEqual(result["diagnostics"][AUTO]["operations"], {"neighbors": 1, "unknown-operation": 1})

    def test_failed_trials_keep_provider_costs_independently_of_trajectory_and_audits(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); manifest = fixture(root)
            scenarios = ("interrupted", "broken-trajectory", "broken-mcp", "missing-setup", "broken-opencode", "broken-result", "missing-opencode")
            expected = {}
            for row, scenario in zip(manifest["matrix"], scenarios):
                agent = root / "jobs" / row["job"] / "trial/agent"
                if scenario == "interrupted":
                    jsonl(agent / "opencode.txt", [{"type": "error", "error": {"name": "interrupted before step_finish"}}])
                    (agent / "trajectory.json").unlink()
                    result = read_json(agent.parent / "result.json")
                    result["exception_info"] = {"exception_type": "AgentTimeoutError"}
                    result["agent_result"] = {"n_input_tokens": None, "n_output_tokens": None}
                    (agent.parent / "result.json").write_text(json.dumps(result))
                elif scenario == "broken-trajectory": (agent / "trajectory.json").write_text("{")
                elif scenario == "broken-mcp": (agent / "choice-mcp.jsonl").write_text("{")
                elif scenario == "missing-setup": (agent / "zvec-grep-setup.json").unlink()
                elif scenario == "broken-opencode": (agent / "opencode.txt").write_text("{")
                elif scenario == "broken-result": (agent.parent / "result.json").write_text("{")
                elif scenario == "missing-opencode": (agent / "opencode.txt").unlink()
                # Provider totals can exceed the last complete OpenCode step after failure.
                tokens = 32 if scenario in ("interrupted", "broken-opencode", "missing-opencode") else 79
                events = [json.loads(s) for s in (agent / "choice-provider-usage.jsonl").read_text().splitlines()]
                events[-1]["usage"]["prompt_tokens"] = tokens
                jsonl(agent / "choice-provider-usage.jsonl", events)
                expected[row["job"]] = (scenario, tokens)
            collection = reports.collect(root)
            for row in collection["trials"]:
                if row["id"] not in expected: continue
                scenario, tokens = expected[row["id"]]
                with self.subTest(scenario=scenario):
                    self.assertEqual(row["metrics"]["input_tokens"], tokens)
                    self.assertEqual(row["metrics"]["output_tokens"], 10)
                    self.assertEqual(row["metrics"]["total_tokens"], tokens + 10)
                    self.assertEqual(row["provider_usage"]["calls"], 1)
                    self.assertFalse(row["usage_complete"])
                    self.assertTrue(row["issues"])
            scores(root, collection)
            result = reports.report(root)
            self.assertFalse(result["usage_complete"] or result["integrity"]["passed"])
            self.assertEqual(result["comparisons"]["choice_vs_off"]["interpretation"], "incomplete")
            interrupted_id = next(key for key, value in expected.items() if value[0] == "interrupted")
            self.assertEqual(next(r for r in result["trials"] if r["id"] == interrupted_id)["quality"], 0)

    def test_invalid_provider_usage_keeps_opencode_costs_and_unavailable_usage_stays_na(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); manifest = fixture(root)
            for index, row in enumerate(manifest["matrix"][:3]):
                agent = root / "jobs" / row["job"] / "trial/agent"
                if index == 0: (agent / "choice-provider-usage.jsonl").unlink()
                else: (agent / "choice-provider-usage.jsonl").write_text("{")
                if index == 2:
                    jsonl(agent / "opencode.txt", [{"type": "error", "error": {"name": "no observed usage"}}])
                    (agent / "trajectory.json").unlink()
            collection = reports.collect(root)
            for index, row in enumerate(collection["trials"][:3]):
                with self.subTest(index=index):
                    self.assertEqual(row["metrics"]["input_tokens"], None if index == 2 else 32)
                    self.assertEqual(row["metrics"]["output_tokens"], None if index == 2 else 10)
                    self.assertEqual(row["metrics"]["total_tokens"], None if index == 2 else 42)
                    self.assertFalse(row["usage_complete"])
                    self.assertEqual(row["status"], "invalid")
                    self.assertTrue(row["issues"])

    def test_zero_denominators_and_repetitions_are_task_paired(self):
        pair = reports.legacy.summarize([{"metrics": {"tool_calls": {"off": 0, "augment": 2}}}], "tool_calls")
        self.assertIsNone(pair["change_pct"])
        self.assertEqual(pair["delta"], 2)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); manifest, collection = self.make(root)
            # Missing score stays N/A rather than becoming a zero-quality answer.
            row = collection["trials"][0]
            (root / "scores" / (row["id"] + ".json")).unlink()
            result = reports.report(root)
            self.assertFalse(result["scoring_complete"])
            self.assertEqual(result["comparisons"]["choice_vs_off"]["interpretation"], "incomplete")

    def test_real_degradation_and_successful_locate_keep_report_integrity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); manifest = fixture(root)
            task_id = tasks("smoke")[1]["task_id"]
            degraded = {"status": "degraded", "view": None, "seeds": [], "skipped": [], "expansions": [],
                        "nodes": [], "relations": [], "evidence": [], "diagnostics": ["graph deadline exceeded"], "execution": None}
            for row in manifest["matrix"]:
                if row["task_id"] == task_id and row["profile"] in ("zg-augment", AUTO):
                    path = root / "jobs" / row["job"] / "trial/agent/choice-mcp.jsonl"
                    events = [json.loads(s) for s in path.read_text().splitlines()]
                    events[-1]["response"]["result"]["content"][0]["text"] = "ordinary\nGraph\n```json\n" + json.dumps(degraded) + "\n```"
                    events[-1]["graph"] = {"status": "degraded", "view": None, "evidence": 0}
                    jsonl(path, events)
            row = next(r for r in manifest["matrix"] if r["profile"] == AUTO and r["task_id"] == tasks("smoke")[3]["task_id"])
            agent = root / "jobs" / row["job"] / "trial/agent"
            args = {"root": "/app", "request": {"domain": "code", "operation": {"operation": "locate", "target": {"by": "name", "name": "f"}}}}
            events = [json.loads(s) for s in (agent / "choice-mcp.jsonl").read_text().splitlines()]
            for event in events:
                for key in ("original", "forwarded", "request"):
                    if key in event: event[key]["params"]["arguments"] = deepcopy(args)
            value = events[-1]["response"]["result"]["structuredContent"]
            value["locations"] = [{"status": "unique", "candidates": [{"id": "n"}]}]
            jsonl(agent / "choice-mcp.jsonl", events)
            trajectory = read_json(agent / "trajectory.json")
            trajectory["steps"][0]["tool_calls"][0]["arguments"] = args
            (agent / "trajectory.json").write_text(json.dumps(trajectory))
            collection = reports.collect(root); scores(root, collection)
            result = reports.report(root)
            self.assertTrue(result["integrity"]["passed"], result["integrity"])
            self.assertTrue(result["scoring_complete"] and result["usage_complete"])
            self.assertEqual(result["diagnostics"]["zg-augment"]["degraded"], 1)
            self.assertEqual(result["diagnostics"][AUTO]["degraded"], 1)
            self.assertEqual(result["diagnostics"][AUTO]["empty_graph_responses"], 3)
            self.assertEqual(result["comparisons"]["choice_vs_off"]["aggregate"]["quality"]["delta"], 20)

    def test_quality_interval_including_zero_stays_uncertain(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); _, collection = self.make(root)
            for row in collection["trials"]:
                if row["profile"] == AUTO:
                    path = root / "scores" / (row["id"] + ".json")
                    score = read_json(path)
                    score["judge"].update(total=50, scores=dict.fromkeys(reports.SCORE_KEYS, 10))
                    path.write_text(json.dumps(score))
            result = reports.report(root)
            comparison = result["comparisons"]["choice_vs_off"]
            self.assertEqual(comparison["aggregate"]["quality"]["ci95"], [0., 0.])
            self.assertEqual(comparison["interpretation"], "uncertain")

    def test_native_only_auto_trial_is_valid_and_missing_final_answer_is_zero(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); manifest = fixture(root)
            row = next(r for r in manifest["matrix"] if r["profile"] == AUTO)
            agent = root / "jobs" / row["job"] / "trial/agent"
            raw, projected = catalogs()
            jsonl(agent / "choice-mcp.jsonl", discovery(AUTO, raw, projected))
            trace = read_json(agent / "trajectory.json")
            trace["steps"][0]["tool_calls"] = [trace["steps"][0]["tool_calls"][1]]
            (agent / "trajectory.json").write_text(json.dumps(trace))
            collection = reports.collect(root)
            selected = next(r for r in collection["trials"] if r["id"] == row["job"])
            self.assertEqual(selected["issues"], [])
            self.assertEqual(selected["metrics"]["zg_calls"], 0)
            # A failed Agent can still have observed usage; no final answer counts as zero.
            trace["steps"][-1]["message"] = ""
            (agent / "trajectory.json").write_text(json.dumps(trace))
            (root / "collection.json").unlink()
            collection = reports.collect(root)
            scores(root, collection)
            result = reports.report(root)
            self.assertTrue(result["integrity"]["passed"])
            self.assertEqual(next(r for r in result["trials"] if r["id"] == row["job"])["quality"], 0)

    def test_mutated_index_and_missing_usage_remain_explicit(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); manifest = fixture(root)
            row = manifest["matrix"][0]
            agent = root / "jobs" / row["job"] / "trial/agent"
            (agent / "choice-index-after.json").write_text('{"index":"changed"}')
            events = [json.loads(s) for s in (agent / "choice-provider-usage.jsonl").read_text().splitlines()]
            events[-1]["usage"].pop("completion_tokens")
            jsonl(agent / "choice-provider-usage.jsonl", events)
            collection = reports.collect(root)
            selected = next(r for r in collection["trials"] if r["id"] == row["job"])
            self.assertIn("missing or changed post-query index inventory", selected["issues"])
            self.assertFalse(selected["usage_complete"])
            self.assertEqual(selected["metrics"]["input_tokens"], 32)  # observed OpenCode usage retained
            self.assertEqual(selected["status"], "invalid")

    def test_view_index_and_tool_contract_drift_block_integrity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); manifest = fixture(root)
            row = next(r for r in manifest["matrix"] if r["profile"] == "zg-augment")
            path = root / "jobs" / row["job"] / "trial/agent/choice-mcp.jsonl"
            events = [json.loads(s) for s in path.read_text().splitlines()]
            response = events[-1]["response"]["result"]["content"][0]
            response["text"] = response["text"].replace('"text_revision": 1', '"text_revision": 2')
            jsonl(path, events)
            collection = reports.collect(root)
            self.assertIn("graph/text view drift", next(r for r in collection["trials"] if r["id"] == row["job"])["issues"])
            scores(root, collection)
            result = reports.report(root)
            self.assertFalse(result["integrity"]["passed"])

    def test_internally_unchanged_but_different_arm_snapshots_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); manifest = fixture(root)
            row = next(r for r in manifest["matrix"] if r["profile"] == AUTO)
            agent = root / "jobs" / row["job"] / "trial/agent"
            setup = read_json(agent / "zvec-grep-setup.json")
            setup["index_inventory"]["graph"] = "different-arm-snapshot"
            (agent / "zvec-grep-setup.json").write_text(json.dumps(setup))
            (agent / "choice-index-after.json").write_text(json.dumps(setup["index_inventory"]))
            collection = reports.collect(root)
            self.assertTrue(all(not r["issues"] for r in collection["trials"]))
            scores(root, collection)
            result = reports.report(root)
            self.assertIn("canonical index or view differs across arms", result["integrity"]["issues"])
            self.assertEqual(result["comparisons"]["choice_vs_off"]["interpretation"], "incomplete")

    def test_tampered_evidence_wrong_protocol_and_missing_trials_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); manifest, collection = self.make(root)
            row = collection["trials"][0]
            path = root / next(iter(row["evidence"]))
            path.write_bytes(path.read_bytes() + b" ")
            with self.assertRaisesRegex(ValueError, "evidence changed"): reports.report(root)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); _, collection = self.make(root)
            collection["trials"].pop()
            (root / "collection.json").write_text(json.dumps(collection))
            with self.assertRaisesRegex(ValueError, "missing or duplicate"): reports.report(root)
            manifest = read_json(root / "manifest.json"); manifest["protocol"] = old.PROTOCOL
            (root / "manifest.json").write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "unknown protocol"): reports.collect(root)

    def test_judge_uses_old_blind_prompt_and_failure_is_pending(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); fixture(root); collection = reports.collect(root)
            calls = []
            def completion(**kwargs):
                prompt = kwargs["messages"][-1]["content"]
                calls.append(prompt)
                self.assertNotIn(AUTO, prompt)
                self.assertNotIn("tool_calls", prompt)
                return {"choices": [{"message": {"content": json.dumps(dict.fromkeys(reports.SCORE_KEYS, 12))}}], "usage": {"prompt_tokens": 10, "completion_tokens": 5}}
            with patch.dict("os.environ", {"GLM_API_KEY": "fixture"}), patch.object(reports, "_default_completion", return_value=completion):
                reports.judge(root)
            self.assertEqual(len(calls), 15)
            self.assertTrue(reports.report(root)["scoring_complete"])
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); fixture(root); reports.collect(root)
            with patch.dict("os.environ", {"GLM_API_KEY": "fixture"}), patch.object(reports, "_default_completion", return_value=lambda **_: (_ for _ in ()).throw(ValueError("offline judge failure"))):
                reports.judge(root)
            self.assertTrue(all(read_json(p)["status"] == "pending" for p in (root / "scores").glob('*.json') if p.name != "order.json"))
            self.assertFalse(reports.report(root)["scoring_complete"])


class ChoiceAgentTests(unittest.IsolatedAsyncioTestCase):
    async def test_setup_uses_fixed_or_autonomous_proxy_and_checks_shared_inventory(self):
        from zg_bench.agents.choice_opencode import ChoiceOpenCode
        class Harness(ChoiceOpenCode):
            def __init__(self, root, profile):
                self._index_seed_root = root; self._choice_profile = profile
            async def _index_seed_identity(self, *args, **kwargs): return {"shared": True}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for profile in PROFILES:
                uploads = {}
                async def upload(path, target): uploads[target] = read_json(path)
                environment = SimpleNamespace(upload_file=AsyncMock(side_effect=upload))
                agent = Harness(root, profile)
                key = agent._index_seed_key({"shared": True})
                (root / key).mkdir(exist_ok=True)
                if not (root / key / "inventory.json").exists():
                    write_json(root / key / "inventory.json", {"graph": "snapshot"})
                async def execute(*args, command, **kwargs):
                    if "augment_files.mjs" in command: value = {"graph": "snapshot"}
                    elif "augment_proxy.mjs" in command: value = {"ready": True, "view": {"generation": "g"}}
                    elif "choice_proxy.mjs" in command: value = {"ready": True, "tool_names": [reports.SEARCH, reports.EXPLORE], "contract_hash": "hash"}
                    else: value = {}
                    return SimpleNamespace(stdout=json.dumps(value))
                agent.exec_as_agent = AsyncMock(side_effect=execute)
                metadata = {}
                await agent._setup_mcp(environment, "/app", metadata, supports_ready_check=True)
                config = uploads["/tmp/zg-choice-proxy.json"]
                self.assertEqual(config["profile"], profile)
                self.assertEqual(config["instructions"], READONLY_INSTRUCTIONS if profile == AUTO else old.INSTRUCTIONS)
                self.assertEqual("graph" in config, profile != AUTO)
                self.assertEqual(uploads["/tmp/zg-choice-probe.json"]["profile"], "zg-augment")
                self.assertEqual(uploads["/tmp/zg-choice-catalog.json"]["profile"], AUTO)
                self.assertEqual(metadata["graph_seed_key"], key)
                self.assertEqual(metadata["index_inventory"], {"graph": "snapshot"})
                self.assertEqual(metadata["protocol"], PROTOCOL)
                commands = [c.kwargs["command"] for c in agent.exec_as_agent.await_args_list]
                self.assertEqual(sum("7998" in c for c in commands), 1)
            # A preflight that mutates storage must stop before the Agent is started.
            agent.exec_as_agent = AsyncMock(side_effect=[
                SimpleNamespace(stdout='{"graph":"before"}'),
                SimpleNamespace(stdout='{"ready":true}'),
                SimpleNamespace(stdout='{"ready":true}'),
                SimpleNamespace(stdout='{"graph":"after"}'),
            ])
            with self.assertRaisesRegex(RuntimeError, "mutated"):
                await agent._setup_mcp(environment, "/app", {}, supports_ready_check=True)

    async def test_seed_identity_is_shared_and_corruption_is_not_rebuilt(self):
        from zg_bench.agents.choice_opencode import ChoiceOpenCode
        class Harness(ChoiceOpenCode):
            def __init__(self, root, profile):
                self._index_seed_root = root; self._choice_profile = profile
                self._expected_commit = "commit"; self._zvec_grep_package_sha256 = "package"
                self._embedding_model = "embedding"; self._embedding_endpoint = "endpoint"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); identities = []
            env = SimpleNamespace(upload_dir=AsyncMock(), download_dir=AsyncMock())
            for profile in PROFILES:
                agent = Harness(root, profile)
                agent.exec_as_agent = AsyncMock(return_value=SimpleNamespace(stdout="commit"))
                identities.append(await agent._index_seed_identity(env, "/app", zvec_grep_version="ignored"))
            self.assertTrue(all(v == identities[0] for v in identities))
            key = agent._index_seed_key(identities[0]); seed = root / key / "workspace"; seed.mkdir(parents=True)
            (seed / "index").write_bytes(b"text and graph")
            write_json(seed.parent / "inventory.json", inventory(seed))
            for profile in PROFILES:
                self.assertTrue(await Harness(root, profile)._restore_index_seed(env, "/app", seed_key=key))
            self.assertEqual(env.upload_dir.await_count, 3)
            (seed / "index").write_bytes(b"corrupt")
            with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
                await agent._restore_index_seed(env, "/app", seed_key=key)

    async def test_failed_agent_keeps_timing_and_failed_audit_does_not_mask_error(self):
        from zg_bench.agents.choice_opencode import ChoiceOpenCode
        class Harness(ChoiceOpenCode):
            def __init__(self, root): self.logs_dir = root
            async def exec_as_agent(self, *args, **kwargs): raise RuntimeError("audit failed")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch("zg_bench.agents.zvec_opencode.ZvecOpenCode.run", new=AsyncMock(side_effect=RuntimeError("Agent failed"))):
                with self.assertRaisesRegex(RuntimeError, "Agent failed"):
                    await Harness(root).run("q", object(), object())
            self.assertTrue((root / "choice-timing.json").is_file())
            self.assertEqual(read_json(root / "choice-index-after.json"), {"audit_error": "RuntimeError"})
