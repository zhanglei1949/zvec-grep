"""Frozen three-arm SWE-QA20 experiment; Augment v1 remains independent."""
from __future__ import annotations

from copy import deepcopy
from pathlib import Path

from . import augment_protocol as legacy

PROTOCOL = "sweqa20-zg-agent-choice-v1"
PROFILES = ("zg-off", "zg-augment", "zg-agent-choice")
AUTO = PROFILES[2]
ROOT, DATA, SEED = legacy.ROOT, legacy.DATA, legacy.SEED
GRAPH, EXECUTION = deepcopy(legacy.GRAPH), deepcopy(legacy.EXECUTION)
PROXY = Path(__file__).with_name("choice_proxy.mjs")
READONLY_INSTRUCTIONS = (
    "The prepared workspace index is immutable; do not build, update, or delete it. "
    "Do not invoke zg through the shell or access its storage directly. "
    "Do not delegate this task to subagents. Use native read and lexical tools for "
    "focused source verification. Stop when evidence is sufficient."
)
COMPARISONS = (
    ("choice_vs_off", AUTO, "zg-off"),
    ("augment_vs_off", "zg-augment", "zg-off"),
    ("choice_vs_augment", AUTO, "zg-augment"),
)
read_json, write_json = legacy.read_json, legacy.write_json
digest, inventory, tasks = legacy.digest, legacy.inventory, legacy.tasks


def matrix(tier: str) -> list[dict]:
    rows = []
    for order, task in enumerate(tasks(tier)):
        base = PROFILES if order % 2 == 0 else PROFILES[::-1]
        for repeat in range(1, (1 if tier == "smoke" else 3) + 1):
            shift = (order + repeat - 1) % len(base)
            for profile in base[shift:] + base[:shift]:
                rows.append({
                    "task_id": task["task_id"], "task_slug": task["task_slug"],
                    "repository": task["repository"], "commit": task["repository_commit"],
                    "repeat": repeat, "profile": profile,
                    "job": f"{task['task_slug']}-r{repeat}-{profile}",
                })
    return rows


def configuration() -> dict:
    config = deepcopy(legacy.configuration())
    config.update({
        "protocol": PROTOCOL, "profiles": list(PROFILES),
        "primary_comparison": COMPARISONS[0][0],
        "comparisons": [list(row) for row in COMPARISONS],
        "readonly_instructions": READONLY_INSTRUCTIONS,
        "profile_instructions": {p: READONLY_INSTRUCTIONS if p == AUTO else legacy.INSTRUCTIONS for p in PROFILES},
        "profile_tools": {p: ["zvec_grep_search", "zvec_grep_explore"] if p == AUTO else ["zvec_grep_search"] for p in PROFILES},
    })
    config["limitations"] += [
        "The choice arm changes available tools and product guidance together; this does not isolate routing policy.",
        "Tool use distributions are not tool-selection accuracy.",
    ]
    return config
