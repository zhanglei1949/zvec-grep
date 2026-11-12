import json
import sqlite3
import unittest
import subprocess
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch
from zg_bench.agents.opencode_usage import CALL_AUDIT_SQL, session_tool_calls
from zg_bench.agents.zvec_opencode import ZvecOpenCode
from zg_bench.tool_experiment import audit_calls, comparisons, render


class ProbeRuntimeTests(unittest.IsolatedAsyncioTestCase):
    async def test_task_probe_uses_existing_node_without_requiring_python(self):
        with tempfile.TemporaryDirectory() as temp:
            agent = ZvecOpenCode(logs_dir=Path(temp), embedding_model="local/potion-code-16m-v2", mcp_target="opencode", mcp_toolset="search", graph_index=True)
            environment = SimpleNamespace(upload_file=AsyncMock())
            with patch.object(agent, "exec_as_agent", new=AsyncMock(return_value=subprocess.CompletedProcess([], 0, stdout="", stderr=""))) as execute:
                await agent._setup_mcp(environment, "/workspace", {}, supports_ready_check=True)
            environment.upload_file.assert_awaited_once()
            command = execute.call_args.kwargs["command"]
            self.assertIn("node /tmp/zg-bench-mcp-probe.mjs", command)
            self.assertNotIn("python", command)


class ToolExperimentTests(unittest.TestCase):
    def test_large_quality_gains_are_retained_and_missing_cost_is_unknown(self):
        def report(group, score, tokens):
            return {"group": group, "task": "fixture", "trials": [{
                "judge": {"total": score}, "input_tokens": tokens,
                "output_tokens": 10, "tool_calls": 1, "agent_wall_seconds": 2,
                "cost_usd": None,
            }]}
        result = comparisons([report("search", 30, 100), report("agent", 90, 50)])[0]
        self.assertEqual(result["judge_delta"], 60)
        self.assertEqual(result["paired_tasks"], ["fixture"])
        self.assertEqual(result["metrics"]["input_tokens"]["change_pct"], -50)
        self.assertIsNone(result["metrics"]["cost_usd"]["change_pct"])

    def test_recursive_call_export_retains_payload_and_excludes_other_sessions(self):
        db = sqlite3.connect(":memory:")
        db.row_factory = sqlite3.Row
        db.executescript("CREATE TABLE session(id TEXT, parent_id TEXT); CREATE TABLE message(id TEXT, session_id TEXT, data TEXT); CREATE TABLE part(id TEXT, session_id TEXT, message_id TEXT, data TEXT);")
        for session in ("root", "child", "unrelated"):
            db.execute("INSERT INTO part VALUES (?, ?, ?, ?)", (session, session, "m", json.dumps({
                "type": "tool", "tool": "zvec_grep_zvec_grep_callers", "callID": session,
                "state": {"status": "completed", "input": {"symbol": "leaf"}, "output": "No matches.", "time": {"start": 1, "end": 2}},
            })))
        rows = [dict(row) for row in db.execute(CALL_AUDIT_SQL)]
        calls = session_tool_calls(rows + rows, {"root", "child"})
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[0]["input"], {"symbol": "leaf"})
        self.assertEqual(calls[0]["output"], "No matches.")
        self.assertEqual(audit_calls(calls)["tools"]["callers"]["attempted"], 2)

    def test_failure_unknown_and_shell_are_distinct(self):
        calls = [
            {"id": "1", "name": "zvec_grep_zvec_grep_search", "status": "error", "error": "provider failure"},
            {"id": "2", "name": "zvec_grep_zvec_grep_callees", "status": "running"},
            {"id": "3", "name": "bash", "input": {"command": "zg --callers leaf"}, "status": "completed"},
        ]
        audit = audit_calls(calls)
        self.assertEqual(audit["tools"]["search"]["failed"], 1)
        self.assertEqual(audit["tools"]["callees"]["unknown"], 1)
        self.assertEqual(audit["tools"]["callers"]["attempted"], 0)
        self.assertEqual(audit["shell_zg_call_ids"], ["3"])
        self.assertIn("no score/token outlier exclusions", render([]))
