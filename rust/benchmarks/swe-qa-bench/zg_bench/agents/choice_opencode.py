"""Three-arm Agent adapter; legacy protocol adapters remain unchanged."""

from __future__ import annotations

import json
import shlex
import tempfile
import time
from datetime import UTC, datetime
from pathlib import Path

from .zvec_opencode import ZvecOpenCode
from ..settings import OPENCODE_DASHSCOPE_BASE_URL
from ..swe_qa.augment_protocol import INSTRUCTIONS as FIXED_INSTRUCTIONS
from ..swe_qa.choice_protocol import (
    GRAPH, EXECUTION, PROFILES, PROXY, PROTOCOL, AUTO, READONLY_INSTRUCTIONS,
    inventory, read_json, write_json,
)


class ChoiceOpenCode(ZvecOpenCode):
    def __init__(self, *args, choice_profile: str, graph_seed_dir: str,
                 repository_commit: str, **kwargs):
        if choice_profile not in PROFILES:
            raise ValueError("invalid graph profile")
        super().__init__(*args, **kwargs)
        if not self._zvec_grep_package_sha256:
            raise ValueError("Choice protocol requires a hashed local Rust package")
        self._choice_profile = choice_profile
        self._expected_commit = repository_commit
        self._index_seed_root = Path(graph_seed_dir).resolve()
        self._index_seed_root.mkdir(parents=True, exist_ok=True)

    async def setup(self, environment):
        with tempfile.TemporaryDirectory() as temporary:
            graph = Path(temporary) / "graph.json"
            write_json(graph, {"scope": "code"})
            await environment.upload_file(graph, "/tmp/zg-choice-graph.json")
        for name in ("choice_proxy.mjs", "augment_proxy.mjs", "augment_files.mjs", "augment_usage.mjs"):
            await environment.upload_file(PROXY.with_name(name), "/tmp/" + name)
        with tempfile.TemporaryDirectory() as temporary:
            instructions = Path(temporary) / "instructions.md"
            instructions.write_text((READONLY_INSTRUCTIONS if self._choice_profile == AUTO else FIXED_INSTRUCTIONS) + "\n", encoding="utf-8")
            await environment.upload_file(instructions, "/tmp/zg-choice-instructions.md")
        await super().setup(environment)

    @staticmethod
    def _index_command(embedding_model):
        return (f"zg --index --embedding {shlex.quote(embedding_model)} "
                "--graph-options /tmp/zg-choice-graph.json")

    async def _index_seed_identity(self, environment, workdir, *, zvec_grep_version):
        result = await self.exec_as_agent(environment, command="git rev-parse HEAD", cwd=workdir)
        if result.stdout.strip() != self._expected_commit or workdir != "/app":
            raise RuntimeError("frozen repository or workspace root mismatch")
        return {
            "protocol": PROTOCOL, "commit": self._expected_commit,
            "package": self._zvec_grep_package_sha256,
            "embedding": self._embedding_model, "endpoint": self._embedding_endpoint,
            "platform": "linux/amd64", "workdir": workdir, "graph_options": {"scope": "code"},
        }

    async def _restore_index_seed(self, environment, workdir, *, seed_key):
        seed = self._index_seed_root / seed_key
        if not seed.exists():
            return False
        if inventory(seed / "workspace") != read_json(seed / "inventory.json"):
            raise RuntimeError("canonical graph index checksum mismatch")
        await environment.upload_dir(seed / "workspace", workdir + "/.zvec-grep")
        return True

    async def _restored_index_is_ready(self, environment, workdir, *, supports_ready_check):
        result = await self.exec_as_agent(environment, command="zg --status --check-ready", cwd=workdir)
        return result, True

    async def _save_index_seed(self, environment, workdir, *, seed_key, identity):
        seed = self._index_seed_root / seed_key
        seed.mkdir()
        await environment.download_dir(workdir + "/.zvec-grep", seed / "workspace")
        write_json(seed / "inventory.json", inventory(seed / "workspace"))
        write_json(seed / "identity.json", identity)
        return True

    async def _setup_mcp(self, environment, workdir, metadata, *, supports_ready_check):
        started = time.monotonic()
        fixed = {"profile": self._choice_profile, "graph": GRAPH,
                 "graph_execution": EXECUTION, "instructions": FIXED_INSTRUCTIONS,
                 "log": "/logs/agent/choice-mcp.jsonl"}
        autonomous = {"profile": AUTO, "instructions": READONLY_INSTRUCTIONS,
                      "log": "/logs/agent/choice-mcp.jsonl"}
        config = autonomous if self._choice_profile == AUTO else fixed
        probe = {**fixed, "profile": "zg-augment", "log": "/logs/agent/choice-preflight.jsonl"}
        catalog = {**autonomous, "log": "/logs/agent/choice-catalog.jsonl"}
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for name, value in (("proxy", config), ("probe", probe), ("catalog", catalog)):
                path = directory / (name + ".json")
                write_json(path, value)
                await environment.upload_file(path, "/tmp/zg-choice-" + name + ".json")
        before = await self.exec_as_agent(environment,
            command="node /tmp/augment_files.mjs /app/.zvec-grep", cwd=workdir)
        graph = await self.exec_as_agent(environment,
            command="node /tmp/augment_proxy.mjs /tmp/zg-choice-probe.json --probe", cwd=workdir)
        tools = await self.exec_as_agent(environment,
            command="node /tmp/choice_proxy.mjs /tmp/zg-choice-catalog.json --probe", cwd=workdir)
        after = await self.exec_as_agent(environment,
            command="node /tmp/augment_files.mjs /app/.zvec-grep", cwd=workdir)
        if json.loads(before.stdout) != json.loads(after.stdout):
            raise RuntimeError("preflight mutated the immutable index")
        identity = await self._index_seed_identity(environment, workdir, zvec_grep_version="")
        seed_key = self._index_seed_key(identity)
        if json.loads(before.stdout) != read_json(self._index_seed_root / seed_key / "inventory.json"):
            raise RuntimeError("trial index differs from canonical graph seed")
        catalog_info = json.loads(tools.stdout)
        if not catalog_info.get("ready") or set(catalog_info.get("tool_names", [])) != {"zvec_grep_search", "zvec_grep_explore"}:
            raise RuntimeError("required product tools unavailable")
        metadata.update({"choice_profile": self._choice_profile, "graph_seed_key": seed_key,
                         "graph_preflight": json.loads(graph.stdout), "catalog_preflight": catalog_info,
                         "index_inventory": json.loads(before.stdout), "protocol": PROTOCOL,
                         "graph_preflight_seconds": time.monotonic() - started})
        await self.exec_as_agent(environment, command=(
            "node /tmp/augment_usage.mjs --launch " + shlex.quote(OPENCODE_DASHSCOPE_BASE_URL)
            + " /logs/agent/choice-provider-usage.jsonl"))
        await self.exec_as_agent(environment, command="node --input-type=module -e " + shlex.quote(
            "const r = await fetch('http://127.0.0.1:7998/health'); if (!r.ok || await r.text() !== 'ready') process.exit(1);"))

    async def run(self, instruction, environment, context):
        started = datetime.now(UTC)
        try:
            return await super().run(instruction, environment, context)
        finally:
            finished = datetime.now(UTC)
            write_json(Path(self.logs_dir) / "choice-timing.json", {
                "started_at": started.isoformat(), "finished_at": finished.isoformat(),
                "wall_seconds": (finished - started).total_seconds(),
            })
            try:
                result = await self.exec_as_agent(environment,
                    command="node /tmp/augment_files.mjs /app/.zvec-grep", cwd="/app")
                audit = json.loads(result.stdout)
            except BaseException as error:
                # Keep the original Agent exception/cancellation; collection flags a failed audit.
                audit = {"audit_error": type(error).__name__}
            write_json(Path(self.logs_dir) / "choice-index-after.json", audit)
