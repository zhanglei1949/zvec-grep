"""Source-only checks for the frozen four-language oracle; no native runtime."""
import importlib.util
import json
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
BASE = ROOT / "benchmarks/zg-retrieval"
MODULE = importlib.util.spec_from_file_location("compiled_labels", BASE / "tools/g4-compiled-labels.py")
labels = importlib.util.module_from_spec(MODULE)
MODULE.loader.exec_module(labels)


class CompiledLabelsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix="zg-compiled-oracle-test-")
        cls.root = Path(cls.directory.name)
        with tarfile.open(BASE / "fixtures/g4-compiled/sources.tar.gz") as archive:
            archive.extractall(cls.root, filter="data")
        cls.inputs = labels.generate(cls.root / "sources")

    @classmethod
    def tearDownClass(cls):
        cls.directory.cleanup()

    def test_frozen_input_reproduces_from_archived_source_only(self):
        self.assertEqual(labels.encoded(self.inputs), (BASE / "data/g4-compiled-v1.json").read_bytes())
        self.assertEqual(labels.render(self.inputs), (BASE / "gold/g4-compiled-v1-answers.md").read_text())

    def test_source_drift_is_rejected_before_annotation(self):
        path = self.root / "sources/google__uuid/uuid.go"
        original = path.read_bytes()
        try:
            path.write_bytes(original.replace(b"panic", b"PANIC", 1))
            with self.assertRaisesRegex(AssertionError, "source drift"):
                labels.generate(self.root / "sources")
        finally:
            path.write_bytes(original)

    def test_lexer_keeps_utf8_crlf_offsets_and_ignores_quoted_braces(self):
        data = '前缀\r\nvoid f() { const char *x="}"; /* { */ if (x) { use(); } }\r\n'.encode()
        start, end = labels.block(data, {"start": "void f()"})
        self.assertEqual(data[start:end], b'void f() { const char *x="}"; /* { */ if (x) { use(); } }')

    def test_ambiguous_source_selector_and_unclosed_block_are_rejected(self):
        with self.assertRaisesRegex(AssertionError, "non-unique"):
            labels.unique(b"call(); call();", "call")
        with self.assertRaisesRegex(ValueError, "unclosed"):
            labels.block(b"void f() {", {"start": "void f()"})

    def test_48_questions_32_scopes_and_eight_independent_boundary_workspaces(self):
        self.assertEqual(len(self.inputs["tasks"]), 48)
        self.assertEqual(sum("constrain" in t["retrieval"] for t in self.inputs["tasks"]), 32)
        self.assertEqual(len(self.inputs["boundary_cases"]), 8)
        for case in self.inputs["boundary_cases"]:
            self.assertTrue(case["workspace_files"])
            self.assertFalse(case["human_reviewed"])
        self.assertEqual(self.inputs["planned_retrieval_calls"], 800)

    def test_scopes_use_source_positions_without_leaking_anchors_into_augment(self):
        for task in self.inputs["tasks"]:
            retrieval = task["retrieval"]
            self.assertEqual(set(retrieval["augment"]), {"mode", "seeds", "expansion"})
            if "constrain" not in retrieval:
                continue
            scope = retrieval["constrain"]["scope"]
            self.assertEqual(scope["anchor"]["by"], "position")
            self.assertIn(scope["anchor"]["kind"], {"class", "method", "function"})
            self.assertTrue(retrieval["scope_required_evidence"])

    def test_callee_sites_are_name_tokens_inside_the_reviewed_call_context(self):
        for ref in self.inputs["reference_annotations"]:
            site, context = ref["site"], ref["expression_context"]
            self.assertLessEqual(context["start"], site["start"])
            self.assertLessEqual(site["end"], context["end"])
            self.assertRegex(site["quote"], r"^[A-Za-z_][A-Za-z_0-9]*$")

    def test_protocol_retains_model_requests_timeouts_and_scoring(self):
        before = json.loads((BASE / "configs/g4-code-v9.json").read_text())
        after = json.loads((BASE / "configs/g4-compiled-v1.json").read_text())
        for key in ["model", "device", "retrieval", "limit", "repetitions", "quality_repetition", "request", "execution", "evidence_scoring"]:
            self.assertEqual(before[key], after[key], key)


if __name__ == "__main__":
    unittest.main()
