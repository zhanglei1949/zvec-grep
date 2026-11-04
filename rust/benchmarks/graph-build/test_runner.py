import subprocess
import tempfile
from pathlib import Path
import unittest

import runner


class ExplicitRoots(unittest.TestCase):
    def test_inventory_retains_cpp_case_and_go_module_inputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            for name in ["main.C", "header.H", "go.mod", "ignored.mod"]:
                (root / name).write_text("fixture\n")
            self.assertEqual([r["path"] for r in runner.inventory(root)],
                             ["go.mod", "header.H", "main.C"])

    def test_generated_policy_leaves_contract_selection_to_each_binary(self):
        policy = runner.graph_options(Path("."), [{"path": "sample/main.py"}])
        self.assertNotIn("rules", policy)
        self.assertEqual(policy["module_roots"][0]["language"], "python")

    def test_rust_fixture_overrides_the_python_benchmark_root(self):
        with tempfile.TemporaryDirectory() as root:
            records = [{"path": p} for p in [
                "rust/crates/example/Cargo.toml", "rust/crates/example/src/lib.rs",
                "benchmarks/zg-graph/corpus/rust/src/lib.rs",
                "benchmarks/zg-graph/corpus/rust/src/calc.rs", "benchmarks/check.py",
            ]]
            roots = runner.graph_options(Path(root), records)["module_roots"]
            for entry in records:
                path = Path(entry["path"])
                if path.suffix != ".rs":
                    continue
                matched = max((r for r in roots if path.is_relative_to(r["root"])),
                              key=lambda r: len(Path(r["root"]).parts))
                self.assertEqual(matched["language"], "rust")
            self.assertEqual(next(r for r in roots if r["namespace"] == "fixture-rust")["root"],
                             "benchmarks/zg-graph/corpus/rust/src")


if __name__ == "__main__":
    unittest.main()
