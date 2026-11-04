"""Frozen inputs for the independent search-Augment Agent experiment."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path

from ..settings import (
    OPENCODE_CUSTOM_GLM_BASE_URL, OPENCODE_DASHSCOPE_BASE_URL,
    ZVEC_GREP_EMBEDDING_ENDPOINT,
)

PROTOCOL = "sweqa20-zg-augment-agent-v1"
PROFILES = ("zg-off", "zg-augment")
ROOT = Path(__file__).resolve().parents[2]
DATA = Path(__file__).with_name("data")
PROXY = Path(__file__).with_name("augment_proxy.mjs")
SEED = 20261103
BUDGET = {
    "nodes": 2000, "edges": 10000, "candidates": 5000,
    "mapping_units": 5000, "output_bytes": 65536, "filter_bytes": 262144,
}
GRAPH = {
    "mode": "augment", "seeds": 5,
    "expansion": {
        "relations": ["calls"], "direction": "outgoing", "min_hops": 1,
        "max_hops": 2, "include_start": False,
        "boundary": {}, "result_filter": {},
    },
}
EXECUTION = {"request_timeout_ms": 30000, "graph_timeout_ms": 5000, "budget": BUDGET}
INSTRUCTIONS = (
    "Use zvec_grep_search for semantic workspace discovery and cross-file questions. "
    "Use native read and lexical tools to verify exact symbols and source evidence. "
    "The prepared workspace index is immutable; do not build, update, or delete it. "
    "Do not invoke zg through the shell or access its storage directly. "
    "Do not delegate this task to subagents. Stop searching when evidence is sufficient."
)


def read_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2, allow_nan=False)
        stream.write("\n")


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def inventory(root: Path) -> dict[str, str]:
    result = {}
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"index contains a symbolic link: {path}")
        if path.is_file():
            result[path.relative_to(root).as_posix()] = digest(path)
    if not result:
        raise ValueError(f"empty index: {root}")
    return result


def tasks(tier: str) -> list[dict]:
    selection = read_json(DATA / "selection.json")
    selected = selection["tasks"]
    if tier == "smoke":
        ids = selection["gate"]["auto_tasks"]
        selected = [task for task in selected if task["task_id"] in ids]
    elif tier != "full":
        raise ValueError(f"unknown tier: {tier}")
    return selected


def matrix(tier: str) -> list[dict]:
    rows = []
    for order, task in enumerate(tasks(tier)):
        for repeat in range(1, (1 if tier == "smoke" else 3) + 1):
            profiles = PROFILES if (order + repeat - 1) % 2 == 0 else PROFILES[::-1]
            for profile in profiles:
                rows.append({
                    "task_id": task["task_id"], "task_slug": task["task_slug"],
                    "repository": task["repository"], "commit": task["repository_commit"],
                    "repeat": repeat, "profile": profile,
                    "job": f"{task['task_slug']}-r{repeat}-{profile}",
                })
    return rows


def configuration() -> dict:
    return {
        "protocol": PROTOCOL, "profiles": list(PROFILES),
        "agent": "opencode", "agent_version": "1.18.4",
        "model": "dashscope/qwen3.7-max", "enable_thinking": False,
        "embedding": "qwen/qwen3.7-text-embedding", "judge": "openai/glm-5.2",
        "answer_endpoint": OPENCODE_DASHSCOPE_BASE_URL,
        "embedding_endpoint": ZVEC_GREP_EMBEDDING_ENDPOINT,
        "judge_endpoint": os.environ.get("GLM_BASE_URL", OPENCODE_CUSTOM_GLM_BASE_URL),
        "judge_temperature": 0, "self_judge": False,
        "agent_timeout_seconds": 1800, "concurrency": 1,
        "graph": GRAPH, "graph_execution": EXECUTION,
        "autoUpdate": False, "freshness": "eventual",
        "instructions": INSTRUCTIONS, "bootstrap_seed": SEED,
        "usage_accounting": "provider prompt_tokens (cache inclusive) reconciled with OpenCode input + cache.read + cache.write",
        "provider_stream_include_usage": True,
        "bootstrap_samples": 10000,
        "limitations": [
            "Remote model weights cannot be frozen.",
            "SWE-QA20 development subset; not a population-wide effectiveness claim.",
            "Augment adds context; this does not isolate graph structure from context volume.",
        ],
    }
