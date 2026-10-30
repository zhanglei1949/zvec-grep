import tempfile
from pathlib import Path
import unittest

import runner


class ExplicitRoots(unittest.TestCase):
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
