#!/usr/bin/env python3
"""Build the six-language acceptance input from source-only declarations."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
BASE = ROOT / 'rust/crates/zg-engine/tests/fixtures/compiled-languages-v1.json'
assert hashlib.sha256(BASE.read_bytes()).hexdigest() == 'dbc82bbdb9ee2b29373755b5b01e6690fd5c435da3bc84f2461e2ce6649d6804'
value = json.loads(BASE.read_text())
value['version'] = 'm7-six-languages-v1'
value['scope'] = 'Six-language static subset; JS/TS graph deferred. No candidate output is read.'
value['graph']['module_roots'] += [
    {'root': 'rs', 'language': 'rust', 'namespace': 'fixture-rust', 'module_prefix': [], 'layout': 'flat'},
    {'root': 'py', 'language': 'python', 'namespace': 'fixture-python', 'module_prefix': [], 'layout': 'flat'},
]
new = {
    'rs/lib.rs': 'pub trait RustBase {}\npub trait RustChild: RustBase {}\npub struct RustWorker;\nimpl RustChild for RustWorker {}\nmod chain;\npub fn RustEntry(){chain::RustStep();}\n',
    'rs/chain.rs': 'pub fn RustStep(){RustLeaf();}\npub fn RustLeaf(){ /* rust_scope */ }\npub fn RustUnknown(){missing();}\n',
    'py/main.py': 'class PythonBase: pass\nclass PythonChild(PythonBase): pass\ndef PythonEntry():\n    PythonStep()\ndef PythonStep():\n    PythonLeaf()\ndef PythonLeaf():\n    pass # python_scope\ndef PythonUnknown(callback):\n    callback()\n',
    'docs/README.md': '# Workspace\nFrozen six-language graph acceptance.\n',
    'skipped/app.js': 'export function skippedJS(){ return "js_scope"; }\n',
    'skipped/app.ts': 'export function skippedTS(): string { return "ts_scope"; }\n',
}
for path, text in new.items():
    value['sources'].append({'path': path, 'text': text, 'sha256': hashlib.sha256(text.encode()).hexdigest()})
value['paths'] += [
    {'language': 'rust', 'anchor': 'RustEntry', 'target': 'RustLeaf', 'marker': 'rust_scope'},
    {'language': 'python', 'anchor': 'PythonEntry', 'target': 'PythonLeaf', 'marker': 'python_scope'},
]
for source, marker, expression, target, target_source in [
    ('rs/lib.rs', 'pub fn RustEntry(){', 'chain::RustStep', 'RustStep', 'rs/chain.rs'),
    ('rs/chain.rs', 'pub fn RustStep(){', 'RustLeaf', 'RustLeaf', 'rs/chain.rs'),
    ('py/main.py', 'def PythonEntry():\n    ', 'PythonStep', 'PythonStep', 'py/main.py'),
    ('py/main.py', 'def PythonStep():\n    ', 'PythonLeaf', 'PythonLeaf', 'py/main.py'),
]:
    text = new[source]
    start = text.index(expression, text.index(marker) + len(marker))
    byte = len(text[:start].encode())
    value['calls'].append({'source': source, 'span': {'start': byte, 'end': byte + len(expression.encode())}, 'text': expression, 'target': target, 'target_source': target_source})
value['type_relations'] += [
    {'anchor': 'RustChild', 'relations': ['extends'], 'targets': ['RustBase']},
    {'anchor': 'RustWorker', 'relations': ['implements'], 'targets': ['RustChild']},
    {'anchor': 'PythonChild', 'relations': ['extends'], 'targets': ['PythonBase']},
]
value['negative_cases'] += [
    {'source': 'rs/chain.rs', 'text': 'missing', 'status': 'unresolved', 'reason': 'missing lexical function'},
    {'source': 'py/main.py', 'text': 'callback', 'status': 'unresolved', 'reason': 'unknown runtime callable'},
]
value['deferred_languages'] = [{'source': 'skipped/app.js', 'symbol': 'skippedJS', 'marker': 'js_scope'}, {'source': 'skipped/app.ts', 'symbol': 'skippedTS', 'marker': 'ts_scope'}]
unsupported = ROOT / 'rust/thirdparty/graphdb/src/compiler/literals.cc'
value['unsupported_sources'] = [{'name': 'cpp-default-parameter', 'path': 'cpp/literals.cc', 'language': 'cpp', 'text': unsupported.read_text(), 'sha256': hashlib.sha256(unsupported.read_bytes()).hexdigest(), 'repository_commit': 'c1a77be7d3327fdd72b1be23be8b2aac1dc2f8ae', 'expected': 'not_ready', 'reason': 'fixed tree-sitter-cpp=0.23.4 missing type_identifier at byte 517; original representative v1 failure retained'}]
output = ROOT / 'benchmarks/zg-retrieval/data/m7-six-languages-v1.json'
output.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')
print(hashlib.sha256(output.read_bytes()).hexdigest())
