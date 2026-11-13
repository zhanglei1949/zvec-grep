"""Preflight validation for the caller/callee diagnostic task set.

The frozen QA20 lock keeps its own validator; this suite owns its input
validation instead of adding dataset branches to the shared twenty-task rules.
"""

from __future__ import annotations

import hashlib
import re
from collections import Counter
from pathlib import Path
from typing import Any

from . import SweQaError
from .validation import (
    FULL_COMMIT_RE,
    _instruction_matches,
    _load_metadata,
    _object,
    _records,
    _text,
)

SUITE = "caller-callee"
EXPECTED_TASK_COUNT = 11
EXPECTED_CATEGORIES = ("what", "where", "how", "why")
EXPECTED_SCENARIOS = {"main": 6, "control": 1, "stress": 1, "anchor": 3}
SMOKE_TASK_ID = "requests:cc1"
AUTO_TASK_IDS = ["requests:cc1", "requests:45", "sympy:cc1"]
SELF_AUTHORED_SOURCE = "self-authored/caller-callee"
ANCHOR_ID_RE = re.compile(r"^[a-z0-9-]+:cc\d+$")


def validate_caller_callee_assets(
    *, selection_path: Path, references_path: Path, dataset_path: Path
) -> dict[str, Any]:
    """Validate the 8+3 caller/callee lock, judge references, and Harbor dataset."""
    selection = _object(selection_path, label="selection")
    references = _object(references_path, label="references")
    tasks = _records(selection, "tasks", label="selection")
    reference_records = _records(references, "references", label="references")

    if selection.get("suite") != SUITE:
        raise SweQaError(f"selection suite must be {SUITE!r}")
    for key in ("benchmark", "benchmark_revision", "index_base"):
        if selection.get(key) != references.get(key):
            raise SweQaError(f"selection and references disagree on {key}")
    if selection.get("index_base") != 0:
        raise SweQaError("selection index_base must be 0")
    if references.get("visibility") != "judge-only":
        raise SweQaError("references visibility must be judge-only")
    if len(tasks) != EXPECTED_TASK_COUNT:
        raise SweQaError(
            "selection must contain exactly "
            f"{EXPECTED_TASK_COUNT} tasks, found {len(tasks)}"
        )
    gate = selection.get("gate")
    if not isinstance(gate, dict) or gate.get("required_tasks") != EXPECTED_TASK_COUNT:
        raise SweQaError(
            f"selection gate.required_tasks must equal {EXPECTED_TASK_COUNT}"
        )

    ids: set[str] = set()
    slugs: set[str] = set()
    selected: dict[str, dict[str, Any]] = {}
    for index, task in enumerate(tasks):
        label = f"selection.tasks[{index}]"
        task_id = _text(task, "task_id", label=label)
        slug = _text(task, "task_slug", label=label)
        source_file = _text(task, "source_file", label=label)
        question = _text(task, "question", label=label)
        question_hash = _text(task, "question_hash", label=label)
        commit = _text(task, "repository_commit", label=label)
        provenance = _text(task, "provenance", label=label)
        scenario = _text(task, "scenario", label=label)
        role = _text(task, "role", label=label)
        if role not in {"smoke", "task"}:
            raise SweQaError(f"invalid role for {task_id}")
        if task_id in ids:
            raise SweQaError(f"selection contains duplicate task_id {task_id!r}")
        if slug in slugs:
            raise SweQaError(f"selection contains duplicate task_slug {slug!r}")
        ids.add(task_id)
        slugs.add(slug)
        selected[task_id] = task
        if slug != task_id.replace(":", "-"):
            raise SweQaError(f"non-canonical task_slug for {task_id}")
        source_index = task.get("source_index")
        if provenance == "upstream":
            if (
                isinstance(source_index, bool)
                or not isinstance(source_index, int)
                or source_index < 0
                or task_id != f"{Path(source_file).stem}:{source_index}"
            ):
                raise SweQaError(f"source index does not match task_id for {task_id}")
            if scenario == "anchor":
                raise SweQaError(f"upstream task cannot use the anchor scenario: {task_id}")
        elif provenance == "self-authored":
            if source_index is not None:
                raise SweQaError(f"self-authored task must omit source_index: {task_id}")
            if source_file != SELF_AUTHORED_SOURCE:
                raise SweQaError(f"self-authored source_file mismatch for {task_id}")
            if not ANCHOR_ID_RE.fullmatch(task_id):
                raise SweQaError(f"non-canonical anchor task_id {task_id!r}")
            if scenario != "anchor":
                raise SweQaError(f"self-authored task must use the anchor scenario: {task_id}")
        else:
            raise SweQaError(f"invalid provenance for {task_id}: {provenance!r}")
        actual_hash = hashlib.sha256(question.encode("utf-8")).hexdigest()
        if question_hash != actual_hash:
            raise SweQaError(f"question SHA256 mismatch for {task_id}")
        if not FULL_COMMIT_RE.fullmatch(commit):
            raise SweQaError(f"repository commit is not a full SHA for {task_id}")
        category = task.get("category")
        if category not in EXPECTED_CATEGORIES:
            raise SweQaError(f"invalid question category for {task_id}")
        if task.get("question_type") != category:
            raise SweQaError(f"question_type does not match category for {task_id}")

    scenario_counts = Counter(str(task.get("scenario")) for task in tasks)
    if dict(scenario_counts) != EXPECTED_SCENARIOS:
        raise SweQaError(
            "selection must contain 6 main, 1 control, 1 stress, and 3 anchor tasks"
        )
    smoke = [task for task in tasks if task.get("role") == "smoke"]
    if len(smoke) != 1 or smoke[0]["task_id"] != SMOKE_TASK_ID:
        raise SweQaError(f"selection must mark exactly {SMOKE_TASK_ID} as the smoke task")
    if gate.get("smoke_task") != SMOKE_TASK_ID:
        raise SweQaError("selection gate.smoke_task does not match the smoke task")
    if gate.get("auto_tasks") != AUTO_TASK_IDS:
        raise SweQaError(f"selection gate.auto_tasks must equal {AUTO_TASK_IDS}")
    if gate.get("full_tasks") != [task["task_id"] for task in tasks]:
        raise SweQaError("selection gate.full_tasks must list all tasks in order")

    if not dataset_path.is_dir():
        raise SweQaError(f"Harbor dataset does not exist: {dataset_path}")
    dataset_dirs = {
        path.name
        for path in dataset_path.iterdir()
        if path.is_dir() and not path.name.startswith(".")
    }
    if dataset_dirs != slugs:
        missing = sorted(slugs - dataset_dirs)
        extra = sorted(dataset_dirs - slugs)
        raise SweQaError(
            f"Harbor dataset task mismatch (missing={missing}, extra={extra})"
        )

    for task_id, task in selected.items():
        task_dir = dataset_path / str(task["task_slug"])
        instruction_path = task_dir / "instruction.md"
        try:
            instruction = instruction_path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as error:
            raise SweQaError(
                f"could not read Harbor instruction {instruction_path}: {error}"
            ) from error
        if not _instruction_matches(instruction, str(task["question"])):
            raise SweQaError(f"Harbor instruction does not match selection for {task_id}")

        metadata = _load_metadata(task_dir / "task.toml")
        expected_metadata = {
            "benchmark": selection["benchmark"],
            "benchmark_revision": selection["benchmark_revision"],
            "index_base": selection["index_base"],
            "task_id": task["task_id"],
            "question_hash": task["question_hash"],
            "repository": task["repository"],
            "repository_commit": task["repository_commit"],
            "role": task["role"],
            "scenario": task["scenario"],
            "category": task["category"],
            "question_type": task["question_type"],
            "provenance": task["provenance"],
        }
        if task["provenance"] == "upstream":
            expected_metadata["source_index"] = task["source_index"]
        for key, expected_value in expected_metadata.items():
            if metadata.get(key) != expected_value:
                raise SweQaError(f"Harbor metadata {key} mismatch for {task_id}")
        dockerfile = task_dir / "environment" / "Dockerfile"
        try:
            docker_text = dockerfile.read_text(encoding="utf-8")
        except OSError as error:
            raise SweQaError(f"could not read {dockerfile}: {error}") from error
        if str(task["repository_commit"]) not in docker_text:
            raise SweQaError(f"Dockerfile does not pin the selected commit for {task_id}")

    refs_by_id: dict[str, dict[str, Any]] = {}
    for index, reference in enumerate(reference_records):
        label = f"references.references[{index}]"
        task_id = _text(reference, "task_id", label=label)
        if task_id in refs_by_id:
            raise SweQaError(f"references contains duplicate task_id {task_id!r}")
        refs_by_id[task_id] = reference
    if set(refs_by_id) != ids:
        raise SweQaError("references task IDs do not exactly match selection")
    for task_id, task in selected.items():
        reference = refs_by_id[task_id]
        for key in (
            "question",
            "question_hash",
            "source_file",
            "source_index",
            "repository",
            "repository_commit",
            "role",
            "scenario",
            "category",
            "provenance",
        ):
            if reference.get(key) != task.get(key):
                raise SweQaError(f"reference {key} mismatch for {task_id}")
        _text(reference, "reference_answer", label=f"references[{task_id}]")

    dataset_files = sorted(path for path in dataset_path.rglob("*") if path.is_file())
    file_contents: list[tuple[Path, bytes]] = []
    for path in dataset_files:
        try:
            file_contents.append((path, path.read_bytes()))
        except OSError as error:
            raise SweQaError(f"could not scan Harbor dataset file {path}: {error}") from error
    for task_id, reference in refs_by_id.items():
        needle = str(reference["reference_answer"]).encode("utf-8")
        for path, contents in file_contents:
            if needle in contents:
                raise SweQaError(
                    f"judge-only reference answer for {task_id} leaked into "
                    f"Harbor dataset file {path}"
                )

    return {
        "schema_version": 1,
        "valid": True,
        "suite": SUITE,
        "task_count": len(tasks),
        "task_ids": [str(task["task_id"]) for task in tasks],
        "auto_task_ids": list(AUTO_TASK_IDS),
        "dataset_files_scanned": len(dataset_files),
        "references_are_judge_only": True,
    }