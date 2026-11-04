#!/usr/bin/env python3
"""Generate the frozen four-language oracle from reviewed source declarations.

This tool reads source bytes and the authored spec, never zg responses. Lexical
block matching locates reviewed declarations; it does not infer call targets.
"""
import argparse
import hashlib
import json
import re
from collections import Counter
from pathlib import Path

BASE = Path(__file__).resolve().parents[1]
SPEC = BASE / "gold/g4-compiled-v1-spec.json"
TOKENS = re.compile(r'//[^\n]*|/\*.*?\*/|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'|`[^`]*`|[{}]', re.S)
CATEGORIES = {"behavior": 3, "calls": 3, "dependencies": 2, "chain": 2, "responsibility": 2}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode()


def relative_path(value):
    path = Path(value)
    assert value and not path.is_absolute() and "\\" not in value
    assert all(part not in ("", ".", "..") for part in value.split("/"))
    return path


def unique(data, text):
    value = text.encode()
    assert value and data.count(value) == 1, f"non-unique source selector: {text[:100]!r}"
    return data.index(value)


def block(data, declaration):
    start = unique(data, declaration["start"])
    if "text" in declaration:
        text = declaration["text"].encode()
        assert data[start:start + len(text)] == text
        return start, start + len(text)
    # Latin-1 keeps lexer offsets byte-for-byte, including UTF-8 and CRLF.
    depth = 0
    for token in TOKENS.finditer(data.decode("latin1"), start):
        if token.group() == "{":
            depth += 1
        elif token.group() == "}":
            depth -= 1
            if depth == 0:
                return start, token.end()
    raise ValueError(f"unclosed reviewed block: {declaration['start']}")


def generate(sources):
    spec_bytes = SPEC.read_bytes()
    spec = json.loads(spec_bytes)
    files = {}
    for repo in spec["repositories"]:
        for entry in repo["files"]:
            key = (repo["repository"], entry["path"])
            path = sources / repo["slug"] / relative_path(entry["path"])
            data = path.read_bytes()
            assert digest(data) == entry["sha256"], f"source drift: {path}"
            assert len(data) == entry["bytes"], f"source size drift: {path}"
            files[key] = data
    symbols = {}
    for key, declaration in spec["symbols"].items():
        data = files[(declaration["repository"], declaration["path"])]
        start, end = block(data, declaration)
        symbols[key] = {"path": declaration["path"], "name": declaration["name"],
                        "kind": declaration["kind"], "sha256": digest(data),
                        "span": {"start": start, "end": end}}

    def evidence(selector):
        declaration = spec["symbols"][selector["symbol"]]
        node = symbols[selector["symbol"]]
        data = files[(declaration["repository"], declaration["path"])]
        region = data[node["span"]["start"]:node["span"]["end"]]
        if "quote" in selector:
            quote = selector["quote"].encode()
            start = node["span"]["start"] + unique(region, selector["quote"])
        else:
            quote = region
            start = node["span"]["start"]
        return {"path": node["path"], "sha256": node["sha256"],
                "start": start, "end": start + len(quote), "quote": quote.decode()}

    tasks = []
    for authored in spec["tasks"]:
        required = [evidence(e) for e in authored["evidence"]]
        keys = list(dict.fromkeys(e["symbol"] for e in authored["evidence"]))
        assert all(spec["symbols"][key]["repository"] == authored["repository"] for key in keys)
        query = authored["query"]
        retrieval = {"query": query, "query_sha256": digest(query.encode()),
                     "augment": {"mode": "augment", "seeds": 5,
                                 "expansion": {"relations": authored["relations"],
                                               "direction": authored["direction"],
                                               "min_hops": 1, "max_hops": 2}},
                     "target_files": list(dict.fromkeys(e["path"] for e in required)),
                     "file_relevance": [{"path": p, "relevance": 2} for p in dict.fromkeys(e["path"] for e in required)],
                     "required_evidence": required}
        if "scope" in authored:
            scope = authored["scope"]
            anchor = symbols[scope["anchor"]]
            retrieval["constrain"] = {"mode": "constrain", "scope": {
                "anchor": {"by": "position", "source_id": anchor["path"],
                           "byte_offset": anchor["span"]["start"], "kind": anchor["kind"]},
                "traversal": {"relations": scope["relations"], "direction": "outgoing",
                              "min_hops": 1, "max_hops": scope["max_hops"], "include_start": True},
                "region": "full"}}
            allowed = [evidence({"symbol": key}) for key in scope["nodes"]]
            scoped = [e for e in required if any(e["path"] == a["path"] and a["start"] <= e["start"] and e["end"] <= a["end"] for a in allowed)]
            assert scoped, authored["task_id"]
            retrieval.update(allowed_regions=allowed, scope_required_evidence=scoped,
                             scope_target_files=list(dict.fromkeys(e["path"] for e in scoped)),
                             scope_anchor_label=anchor["name"])
        tasks.append({"task_id": authored["task_id"], "repository": authored["repository"],
                      "language": authored["language"], "category": authored["category"],
                      "case_role": "normal", "query_style": authored["query_style"],
                      "standard_answer": authored["answer"], "truth": {"nodes": [symbols[k] for k in keys],
                      "required_evidence": required, "exhaustive": False,
                      "unknown_relations": "Unlabelled evidence and relations remain unknown."},
                      "annotation": {"status": "source_reviewed", "human_reviewed": False,
                      "basis": "Agent source review; pinned Git blobs; independent lexical byte checks. No zg extractor or product responses."},
                      "retrieval": retrieval})
    assert len(tasks) == 48 and len({t["task_id"] for t in tasks}) == 48
    assert sum("constrain" in t["retrieval"] for t in tasks) == 32
    for language in ("go", "java", "c", "cpp"):
        selected = [t for t in tasks if t["language"] == language]
        assert Counter(t["category"] for t in selected) == CATEGORIES, language
        assert sum(t["query_style"] == "behavior_without_answer_name" for t in selected) >= 8
        assert sum(len(t["retrieval"]["required_evidence"]) >= 2 for t in selected) >= 6
        assert sum(len(t["retrieval"]["target_files"]) >= 2 for t in selected) >= 4
        assert sum("constrain" in t["retrieval"] for t in selected) == 8
    boundaries = []
    for case in spec["boundary_cases"]:
        result = {k: v for k, v in case.items() if k != "evidence"}
        result["evidence"] = [evidence(e) for e in case["evidence"]]
        boundaries.append(result)
    assert len(boundaries) == 8
    references = []
    for authored in spec["reference_annotations"]:
        context = evidence(authored["site"])
        callee = context["quote"].split("(", 1)[0]
        name = re.search(r"[A-Za-z_][A-Za-z_0-9]*$", callee)
        assert name is not None
        start = context["start"] + len(callee[:name.start()].encode())
        references.append({**{k: v for k, v in authored.items() if k not in ("site", "target")},
                           "site": {"path": context["path"], "sha256": context["sha256"],
                                    "start": start, "end": start + len(name.group().encode()),
                                    "quote": name.group()},
                           "expression_context": context,
                           "target": symbols[authored["target"]] if authored.get("target") else None})
    return {"id": "g4-compiled-source-v1", "schema_version": 7,
            "annotation_spec_sha256": digest(spec_bytes),
            "annotation_tool_sha256": digest(Path(__file__).read_bytes()),
            "scope": "Four pinned component slices; source-reviewed development oracle; not human-reviewed or full-repository ground truth.",
            "repositories": spec["repositories"], "tasks": tasks,
            "reference_annotations": references,
            "relation_annotations": [{**{k: v for k, v in r.items() if k not in ("source", "target", "evidence")},
                                      "source": symbols[r["source"]],
                                      "target": symbols[r["target"]] if r.get("target") else None,
                                      "evidence": [evidence(e) for e in r["evidence"]]}
                                     for r in spec["relation_annotations"]],
            "boundary_cases": boundaries,
            "oracle_limitations": spec["oracle_limitations"],
            "planned_retrieval_calls": 800, "evidence_scoring": "visible-bytes-v2"}


def render(inputs):
    lines = ["# 四语言查询题集与标准答案 v1", "",
             "由固定源码和独立标注生成，48 道主问题／32 个 Constrain 配对／8 组边界。未人工复核；未标注内容保持未知。", "",
             f"输入 SHA-256：`{digest(encoded(inputs))}`。精确原文、字节及范围以 `data/g4-compiled-v1.json` 为准。", ""]
    for task in inputs["tasks"]:
        lines += [f"## {task['task_id']}：{task['category']}", "", task["retrieval"]["query"], "",
                  task["standard_answer"], "", "必需证据：", ""]
        for e in task["truth"]["required_evidence"]:
            lines.append(f"- `{e['path']}`，UTF-8 字节 `[{e['start']}, {e['end']})`：")
            lines.extend(["", "```text", e["quote"], "```", ""])
        if "constrain" in task["retrieval"]:
            scope = task["retrieval"]["constrain"]["scope"]
            lines += [f"Constrain：`{task['retrieval']['scope_anchor_label']}`，来源 `{scope['anchor']['source_id']}` 字节 `{scope['anchor']['byte_offset']}`；范围内必需区间 {len(task['retrieval']['scope_required_evidence'])} 个。", ""]
    lines += ["## 边界用例（收益评分之外）", ""]
    for case in inputs["boundary_cases"]:
        lines += [f"- **{case['case_id']}**：{case['reason']}；预期 `{case['expected']}`。"]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sources", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=BASE / "data/g4-compiled-v1.json")
    parser.add_argument("--answers", type=Path, default=BASE / "gold/g4-compiled-v1-answers.md")
    args = parser.parse_args()
    inputs = generate(args.sources)
    # Frozen versions are never overwritten, including during reproduction.
    with args.output.open("xb") as output:
        output.write(encoded(inputs))
    with args.answers.open("x") as output:
        output.write(render(inputs))
    print(json.dumps({"sha256": digest(encoded(inputs)), "tasks": 48, "constrain": 32,
                      "boundaries": 8, "retrieval_calls": 800}))


if __name__ == "__main__":
    main()
