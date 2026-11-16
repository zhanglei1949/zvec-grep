"""Audit the actual outbound request payloads without inferring missing context."""
import hashlib
import json
from pathlib import Path


def audit_model_requests(path: Path, surface: dict, calls: list[dict]) -> dict:
    events = [json.loads(line) for line in path.read_text().splitlines()]
    if not events or events[0]["type"] != "started" or events[-1]["type"] != "closed":
        raise ValueError("model request recorder did not complete")
    requests = [event for event in events if event["type"] == "request"]
    if not requests:
        raise ValueError("no outbound model requests recorded")
    expected = {"zvec_grep_" + tool["name"] for tool in surface["tools"]}
    session_by_call = {call["call_id"]: call["session_id"] for call in calls}
    rows = []
    for event in requests:
        body = event["body"]
        names = sorted(tool["function"]["name"] for tool in body.get("tools", []))
        texts = []
        guidance_roles = []
        sessions = set()
        for message in body["messages"]:
            content = message.get("content")
            message_text = ""
            if isinstance(content, str):
                texts.append(content)
                message_text = content
            elif isinstance(content, list):
                parts = [part["text"] for part in content if part.get("type") == "text"]
                texts.extend(parts)
                message_text = "\n".join(parts)
            if surface["guidance"].strip() in message_text:
                guidance_roles.append(message["role"])
            for call in message.get("tool_calls", []):
                if call["id"] in session_by_call:
                    sessions.add(session_by_call[call["id"]])
        actual = {name for name in names if name.startswith("zvec_grep_")}
        rows.append({
            "id": event["id"], "time": event["time"], "model": body["model"],
            "tool_names": names, "tool_choice": body.get("tool_choice"),
            "missing_zg_tools": sorted(expected - actual), "extra_zg_tools": sorted(actual - expected),
            "guidance_present": surface["guidance"].strip() in "\n".join(texts),
            "guidance_roles": guidance_roles,
            "messages_sha256": hashlib.sha256(json.dumps(body["messages"], ensure_ascii=False, separators=(",", ":")).encode()).hexdigest(),
            "body_sha256": event["body_sha256"], "session_ids_from_call_history": sorted(sessions),
        })
    with_tools = [row for row in rows if row["tool_names"]]
    invalid = [call for call in calls if call["name"] == "invalid" and isinstance(call["input"], dict) and call["input"].get("tool", "").startswith("zvec_grep_")]
    return {
        "complete": True, "raw_file": "agent/model-requests.jsonl",
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "upstream": events[0]["upstream"], "requests": rows,
        "tool_bearing_requests": len(with_tools),
        "no_tool_bearing_requests": not with_tools,
        "context_issue_request_ids": [row["id"] for row in (with_tools or rows) if row["missing_zg_tools"] or row["extra_zg_tools"] or not row["guidance_present"]],
        "transport_events": [event for event in events if event["type"] not in ("request", "started", "closed")],
        "invalid_zg_attempts": invalid,
    }
