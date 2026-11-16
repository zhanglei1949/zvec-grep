"""Keep the small revision comparison pinned without relaxing historical guards."""
import hashlib
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

from zg_bench.tool_experiment import ROOT, aggregate, audit_calls, experiment_tasks, validate_revision_surface, write_json


class GuidanceRevisionTests(unittest.TestCase):
    def setUp(self):
        self.frozen = json.loads((ROOT / 'fixtures/caller-callee-guidance-revision-v1.json').read_text())['candidates']

    def test_candidate_surface_cannot_be_substituted(self):
        old, new = self.frozen
        for candidate, item in self.frozen.items():
            for group, digest in item['groups'].items():
                validate_revision_surface(candidate, group, {'sha256': digest})
                with self.assertRaisesRegex(ValueError, 'frozen revision candidate'):
                    validate_revision_surface(candidate, group, {'sha256': 'changed'})
        with self.assertRaisesRegex(ValueError, 'frozen revision candidate'):
            validate_revision_surface(old, 'agent', {'sha256': self.frozen[new]['groups']['agent']})
        with self.assertRaisesRegex(ValueError, 'frozen revision candidate'):
            validate_revision_surface('unknown', 'agent', {'sha256': 'unknown'})

    def test_complete_zero_call_cohort_and_missing_trial_rules(self):
        tasks = experiment_tasks('qa20', 'guidance-revision')
        self.assertEqual(tasks, ['matplotlib-37', 'sympy-38', 'xarray-32'])
        candidate = next(iter(self.frozen))
        trial = {'trial_name': 'fixture', 'judge': {'total': 50}, 'input_tokens': 100,
                 'output_tokens': 20, 'tool_calls': 0, 'agent_wall_seconds': 1,
                 'cost_usd': None, 'audit': audit_calls([]), 'model_requests': {
                     'tool_bearing_requests': 1, 'context_issue_request_ids': [], 'invalid_zg_attempts': []}}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for group in ('relationships', 'agent'):
                for task in tasks:
                    report = {'protocol': 'caller-callee-guidance-revision-v1', 'candidate': candidate,
                              'harness': 'same-harness', 'phase': 'guidance-revision', 'expected_trials': 2,
                              'model': 'custom-openai/glm-5.2', 'endpoint_sha256': 'endpoint',
                              'group': group, 'task': task, 'task_set': 'qa20', 'guidance_variant': 'direct',
                              'graph_index': True, 'surface_sha256': self.frozen[candidate]['groups'][group],
                              'trials': [trial] * 2}
                    path = root / 'input' / group / task / 'task-report.json'
                    write_json(path, report)
                    path.with_suffix('.sha256').write_text(hashlib.sha256(path.read_bytes()).hexdigest())
            args = SimpleNamespace(reports=root / 'input', output=root / 'out', phase='guidance-revision', group='all', task_set='qa20')
            aggregate(args)
            result = json.loads((root / 'out/report.json').read_text())
            self.assertTrue(result['complete'])
            self.assertEqual(sum(len(r['trials']) for r in result['reports']), 12)
            self.assertEqual(result['usage_rates']['relationships']['tools']['callers']['used_trials'], 0)
            path.unlink()
            with self.assertRaisesRegex(ValueError, '1 missing'):
                aggregate(args)


if __name__ == '__main__':
    unittest.main()
