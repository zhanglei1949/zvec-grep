"""Validate the proxy against catalog fixtures captured through real Rust MCP tools/list."""
from __future__ import annotations

import json
import subprocess
import unittest
from pathlib import Path

from jsonschema import ValidationError, validators

ROOT = Path(__file__).resolve().parents[1]
PROJECT = """
import { publicTool } from './zg_bench/swe_qa/augment_proxy.mjs';
let input = '';
for await (const chunk of process.stdin) input += chunk;
process.stdout.write(JSON.stringify(publicTool(JSON.parse(input))));
"""


class ProductToolSchemaTests(unittest.TestCase):
    def test_real_product_examples_and_ordinary_arguments_remain_valid(self):
        catalog = json.loads((ROOT / "tests" / "fixtures" / "mcp-search-graphdb.json").read_text())
        self.assertTrue(catalog["graphdb_enabled"])
        tool = catalog["search_tool"]
        result = subprocess.run(
            ["node", "--input-type=module", "-e", PROJECT],
            cwd=ROOT, input=json.dumps(tool), text=True, capture_output=True, check=True,
        )
        projected = json.loads(result.stdout)["inputSchema"]
        validator_type = validators.validator_for(projected)
        validator_type.check_schema(projected)
        validator = validator_type(projected)
        for example in projected.get("examples", []):
            validator.validate(example)
        request = {"root": "/app", "query": "redirect cookie handling", "limit": 5, "preview": "full"}
        validator.validate(request)
        validator.validate({"root": "/app", "fts": ["resolve_redirects"], "fuse": True})
        hidden_inputs = {
            "graph": {"mode": "off"}, "graphExecution": {"request_timeout_ms": 1000},
            "autoUpdate": False, "freshness": "eventual", "apiKey": "fixture-only",
        }
        for hidden, value in hidden_inputs.items():
            with self.assertRaises(ValidationError):
                validator.validate({**request, hidden: value})
        for invalid_limit in (0, 51):
            with self.assertRaises(ValidationError):
                validator.validate({**request, "limit": invalid_limit})
        with self.assertRaises(ValidationError):
            validator.validate({"query": "root is required"})
        hidden_examples = [e for e in tool["inputSchema"]["examples"] if "graph" in e]
        self.assertEqual({e["graph"]["mode"] for e in hidden_examples}, {"augment", "constrain"})
        for example in hidden_examples:
            with self.assertRaises(ValidationError):
                validator.validate(example)


if __name__ == "__main__":
    unittest.main()
