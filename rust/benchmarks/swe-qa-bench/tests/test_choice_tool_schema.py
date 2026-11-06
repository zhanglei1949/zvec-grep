"""Exercise the autonomous tool projection against real product schemas."""
import json
import subprocess
import unittest

from jsonschema import ValidationError, validators
from zg_bench.swe_qa.choice_protocol import ROOT


class ChoiceToolSchemaTests(unittest.TestCase):
    def test_real_instructions_schemas_and_four_query_shapes(self):
        raw = json.loads((ROOT / "tests/fixtures/mcp-agent-tools-graphdb.json").read_text())
        self.assertTrue(raw["graphdb_enabled"])
        self.assertIn("Agent", raw["instructions"])
        script = """
import { publicTool } from './zg_bench/swe_qa/choice_proxy.mjs';
let s='';for await(const c of process.stdin)s+=c;
process.stdout.write(JSON.stringify(JSON.parse(s).tools.map(publicTool)));
"""
        tools = json.loads(subprocess.run(["node", "--input-type=module", "-e", script], cwd=ROOT,
            input=json.dumps(raw), capture_output=True, text=True, check=True).stdout)
        self.assertEqual({t["name"] for t in tools}, {"zvec_grep_search", "zvec_grep_explore"})
        checks = {}
        for tool in tools:
            source = next(t for t in raw["tools"] if t["name"] == tool["name"])
            self.assertEqual(tool["description"], source["description"])
            schema = tool["inputSchema"]
            kind = validators.validator_for(schema)
            kind.check_schema(schema)
            checks[tool["name"]] = kind(schema)
            for example in schema.get("examples", []):
                checks[tool["name"]].validate(example)
            for key in ("autoUpdate", "freshness", "apiKey"):
                self.assertNotIn(key, schema["properties"])
        search = checks["zvec_grep_search"]
        base = {"root": "/app", "query": "implementation"}
        search.validate(base)
        search.validate({**base, "graph": {"mode": "off"}})
        search.validate({**base, "graph": {"mode": "augment", "seeds": 2, "expansion": {"direction": "incoming", "max_hops": 1}}, "graphExecution": {"budget": {"nodes": 50}}})
        search.validate({**base, "graph": {"mode": "constrain", "scope": {"anchor": {"by": "name", "name": "f"}}}})
        checks["zvec_grep_explore"].validate({"root": "/app", "request": {"domain": "code", "operation": {"operation": "neighbors", "anchor": {"by": "name", "name": "f"}, "relations": ["calls"], "direction": "incoming"}}})
        for graph in ({"mode": "constrain"}, {"mode": "augment", "seeds": 0}, {"mode": "augment", "expansion": {"max_hops": 3}}):
            with self.assertRaises(ValidationError):
                search.validate({**base, "graph": graph})
