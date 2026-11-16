import json
from pathlib import Path
import tempfile
import subprocess
import unittest
from unittest.mock import patch

from zg_bench.relationship_diagnostic import definitions, encoded, execute_readonly, parse_completion, replay_body


class RelationshipDiagnosticTests(unittest.TestCase):
    def test_replay_changes_only_system_suffix_and_preserves_original(self):
        original = {"messages": [{"role": "system", "content": "system"}, {"role": "tool", "content": "source"}], "tools": [{"function": {"name": "read"}}], "seed": 42, "tool_choice": "auto"}
        before = encoded(original)
        self.assertEqual(encoded(replay_body(original, [])), before)
        body = replay_body(original, ["ready", "directive"])
        self.assertEqual(body["messages"][0]["content"], "system\n\nready\ndirective")
        body["messages"][0] = original["messages"][0]
        self.assertEqual(body, original)
        self.assertEqual(encoded(original), before)

    def test_stream_reassembles_tool_calls_and_usage(self):
        events = [
            {"choices": [{"delta": {"reasoning_content": "reason", "tool_calls": [{"index": 0, "id": "call1", "function": {"name": "read", "arguments": '{"file'}}]}}]},
            {"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": 'Path":"/app/a.py"}'}}]}, "finish_reason": "tool_calls"}]},
            {"choices": [], "usage": {"prompt_tokens": 100}},
        ]
        raw = "\n".join("data: " + json.dumps(e) for e in events) + "\ndata: [DONE]\n"
        result = parse_completion(raw)
        self.assertEqual(json.loads(result["tool_calls"][0]["function"]["arguments"]), {"filePath": "/app/a.py"})
        self.assertEqual(result["usage"]["prompt_tokens"], 100)
        self.assertEqual(result["reasoning_content"], "reason")
        with self.assertRaises(ValueError):
            parse_completion('data: {"choices":[]}\n')

    def test_oracle_checks_scope_and_retains_source_calls(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a.py").write_text("class A:\n    def run(self):\n        self.save()\n")
            result = definitions(root, [("a.py", "A::run")])[0]
            self.assertEqual(result["line"], 2)
            self.assertEqual(result["calls"][0]["expression"], "self.save()")
            with self.assertRaises(ValueError):
                definitions(root, [("a.py", "Other::run")])

    def test_executor_rejects_writes_shell_execution_and_path_escape(self):
        root = Path("/app")
        for command in ["rm -rf /app", "rg --pre=touch x /app", "rg x /app | sh"]:
            result = execute_readonly("bash", {"command": command}, root, None)
            self.assertIn("not_executed", result)
        self.assertIn("not_executed", execute_readonly("write", {}, root, None))
        with self.assertRaises(ValueError):
            execute_readonly("read", {"filePath": "/etc/passwd"}, root, None)
        with self.assertRaises(ValueError):
            execute_readonly("zvec_grep_zvec_grep_callees", {"root": "/other"}, root, None)

    def test_readonly_head_forms_execute_the_saved_rg_arguments(self):
        stdout = "".join(f"{i}\n" for i in range(25))
        for suffix, limit in [("head", 10), ("head -20", 20), ("head -n 20", 20)]:
            with patch("zg_bench.relationship_diagnostic.subprocess.run", return_value=subprocess.CompletedProcess([], 0, stdout, "")) as run:
                value = execute_readonly("bash", {"command": f'rg -n "def update|def other" /app/a.py | {suffix}'}, Path("/app"), None)
                self.assertEqual(run.call_args.args[0], ["rg", "-n", "def update|def other", "/app/a.py"])
                self.assertEqual(len(value["stdout"].splitlines()), limit)
