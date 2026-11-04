#!/usr/bin/env python3
"""Version the reviewed v1 corrections using source bytes and CPython Call.func.

The original generator, input, retrieval questions and source revisions stay frozen.
This script never invokes zg or consumes candidate responses.
"""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess

BASE = Path(__file__).resolve().parents[1]
V1_SHA256 = "c4723452cfd313e4200ff6b90f91ba45f2e3d8b28f557e4cfe4245e1941c39c5"
BUDGET_CASES = {
    "ripgrep:G4-03": {"status": "product_error", "reason": "complete result exceeds output budget"},
    "ripgrep:G4-16": {"status": "partial", "stop_reason": "output_budget"},
    "requests:G4-13": {"status": "partial", "stop_reason": "output_budget"},
    "requests:G4-15": {"status": "partial", "stop_reason": "output_budget"},
    "requests:G4-16": {"status": "partial", "stop_reason": "output_budget"},
}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def generate(sources):
    original = (BASE / "data/g4-code-v1.json").read_bytes()
    assert digest(original) == V1_SHA256, "v1 input changed"
    inputs = json.loads(original)
    roots = {}
    for repo in inputs["repositories"]:
        root = sources / repo["repository"].replace("/", "__")
        actual = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
        assert actual == repo["commit"], (repo["repository"], actual)
        roots[repo["repository"]] = root
    for task in inputs["tasks"]:
        root = roots[task["repository"]]
        for evidence in task["truth"]["required_evidence"]:
            data = (root / evidence["path"]).read_bytes()
            assert digest(data) == evidence["sha256"]
            assert data[evidence["start"]:evidence["end"]].decode() == evidence["quote"]
        task["case_role"] = "budget_boundary" if task["task_id"] in BUDGET_CASES else "normal"
        if task["task_id"] in BUDGET_CASES:
            task["budget_expectation"] = BUDGET_CASES[task["task_id"]]
        if task["task_id"] in {"ripgrep:G4-05", "ripgrep:G4-19", "ripgrep:G4-20", "ripgrep:G4-23"}:
            operation = task["explore_request"]["operation"]
            for locator in [operation.get(key) for key in ("anchor", "start", "target")]:
                if locator and locator.get("name") == "escape":
                    locator["kind"] = "function"
            if operation.get("selector", {}).get("owner", {}).get("name") == "escape":
                operation["selector"]["owner"]["kind"] = "function"
        if task["task_id"] == "requests:G4-24":
            node = task["truth"]["nodes"][0]
            data = (root / node["path"]).read_bytes()
            lines = data.splitlines(keepends=True)
            calls = [n.func for n in ast.walk(ast.parse(data))
                     if isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id == "dict_class"
                     and node["span"]["start"] <= sum(map(len, lines[:n.lineno - 1])) + n.col_offset < node["span"]["end"]]
            assert len(calls) == 1, "expected one dict_class invocation in merge_setting"
            call = calls[0]
            start = sum(map(len, lines[:call.lineno - 1])) + call.col_offset
            end = sum(map(len, lines[:call.end_lineno - 1])) + call.end_col_offset
            site = dict(path=node["path"], sha256=digest(data), start=start, end=end, quote=data[start:end].decode())
            task["truth"]["reference_sites"] = [dict(evidence=site, target_known=False)]
            task["contract"]["must_not_resolve"] = [site]
    inputs["id"] = "g4-code-source-v2"
    inputs["previous_inputs_sha256"] = V1_SHA256
    inputs["annotation_tool_sha256"] = digest(Path(__file__).read_bytes())
    return inputs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sources", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=BASE / "data/g4-code-v2.json")
    args = parser.parse_args()
    data = (json.dumps(generate(args.sources), ensure_ascii=False, indent=2) + "\n").encode()
    with args.output.open("xb") as output:
        output.write(data)
    print(json.dumps(dict(sha256=digest(data), tasks=48, normal=43, budget_boundary=5)))


if __name__ == "__main__":
    main()
