"""SWE-QA Augment adapter; existing benchmark agents retain their behavior."""

from __future__ import annotations

import json
import shlex
import tempfile
import time
from datetime import UTC, datetime
from pathlib import Path

from .zvec_opencode import ZvecOpenCode
from ..settings import OPENCODE_DASHSCOPE_BASE_URL
from ..swe_qa.augment_protocol import (
    GRAPH, EXECUTION, INSTRUCTIONS, PROFILES, PROXY, PROTOCOL,
    inventory, read_json, write_json,
)


class AugmentOpenCode(ZvecOpenCode):
    def __init__(self, *args, graph_profile: str, graph_seed_dir: str,
                 repository_commit: str, **kwargs):
        if graph_profile not in PROFILES:
            raise ValueError("invalid graph profile")
        super().__init__(*args, **kwargs)
        if not self._zvec_grep_package_sha256:
            raise ValueError("Augment protocol requires a hashed local Rust package")
        self._graph_profile = graph_profile
        self._expected_commit = repository_commit
        self._index_seed_root = Path(graph_seed_dir).resolve()
        self._index_seed_root.mkdir(parents=True, exist_ok=True)

    async def setup(self, environment):
        with tempfile.TemporaryDirectory() as temporary:
            graph = Path(temporary) / "graph.json"
            write_json(graph, {"scope": "code"})
            await environment.upload_file(graph, "/tmp/zg-augment-graph.json")
        await environment.upload_file(PROXY, "/tmp/zg-augment-proxy.mjs")
        await environment.upload_file(PROXY.with_name("augment_files.mjs"), "/tmp/zg-augment-files.mjs")
        await environment.upload_file(PROXY.with_name("augment_usage.mjs"), "/tmp/zg-augment-usage.mjs")
        with tempfile.TemporaryDirectory() as temporary:
            instructions = Path(temporary) / "instructions.md"
            instructions.write_text(INSTRUCTIONS + "\n", encoding="utf-8")
            await environment.upload_file(instructions, "/tmp/zg-augment-instructions.md")
        await super().setup(environment)

    @staticmethod
    def _index_command(embedding_model):
        return (f"zg index --embedding {shlex.quote(embedding_model)} "
                "--graph-options /tmp/zg-augment-graph.json")

    async def _index_seed_identity(self, environment, workdir, *, zvec_grep_version):
        result = await self.exec_as_agent(environment, command="git rev-parse HEAD", cwd=workdir)
        if result.stdout.strip() != self._expected_commit or workdir != "/app":
            raise RuntimeError("frozen repository or workspace root mismatch")
        return {
            "protocol": PROTOCOL, "commit": self._expected_commit,
            "package": self._zvec_grep_package_sha256,
            "embedding": self._embedding_model, "endpoint": self._embedding_endpoint,
            "platform": "linux/amd64", "workdir": workdir,
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
        result = await self.exec_as_agent(environment, command="zg status --check-ready", cwd=workdir)
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
        config = {
            "profile": self._graph_profile, "graph": GRAPH,
            "graph_execution": EXECUTION, "instructions": INSTRUCTIONS,
            "log": "/logs/agent/augment-mcp.jsonl",
        }
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            write_json(directory / "proxy.json", config)
            await environment.upload_file(directory / "proxy.json", "/tmp/zg-augment-proxy.json")
            probe = {**config, "profile": "zg-augment", "log": "/logs/agent/augment-preflight.jsonl"}
            write_json(directory / "probe.json", probe)
            await environment.upload_file(directory / "probe.json", "/tmp/zg-augment-probe.json")
        before = await self.exec_as_agent(environment,
            command="node /tmp/zg-augment-files.mjs /app/.zvec-grep", cwd=workdir)
        result = await self.exec_as_agent(environment,
            command="node /tmp/zg-augment-proxy.mjs /tmp/zg-augment-probe.json --probe", cwd=workdir)
        after = await self.exec_as_agent(environment,
            command="node /tmp/zg-augment-files.mjs /app/.zvec-grep", cwd=workdir)
        if json.loads(before.stdout) != json.loads(after.stdout):
            raise RuntimeError("graph preflight mutated the immutable index")
        identity = await self._index_seed_identity(environment, workdir, zvec_grep_version="")
        seed_key = self._index_seed_key(identity)
        if json.loads(before.stdout) != read_json(self._index_seed_root / seed_key / "inventory.json"):
            raise RuntimeError("trial index differs from canonical graph seed")
        metadata.update({
            "graph_profile": self._graph_profile, "graph_seed_key": seed_key,
            "graph_preflight": json.loads(result.stdout),
            "index_inventory": json.loads(before.stdout),
            "graph_preflight_seconds": time.monotonic() - started,
            "protocol": PROTOCOL,
        })
        await self.exec_as_agent(environment, command=(
            "node /tmp/zg-augment-usage.mjs --launch " + shlex.quote(OPENCODE_DASHSCOPE_BASE_URL)
            + " /logs/agent/augment-provider-usage.jsonl"
        ))
        await self.exec_as_agent(environment, command=(
            "node --input-type=module -e " + shlex.quote(
                "const r = await fetch('http://127.0.0.1:7998/health'); "
                "if (!r.ok || await r.text() !== 'ready') process.exit(1);"
            )
        ))

    async def run(self, instruction, environment, context):
        started = datetime.now(UTC)
        try:
            return await super().run(instruction, environment, context)
        finally:
            finished = datetime.now(UTC)
            write_json(Path(self.logs_dir) / "augment-timing.json", {
                "started_at": started.isoformat(), "finished_at": finished.isoformat(),
                "wall_seconds": (finished - started).total_seconds(),
            })
            result = await self.exec_as_agent(environment,
                command="node /tmp/zg-augment-files.mjs /app/.zvec-grep", cwd="/app")
            write_json(Path(self.logs_dir) / "augment-index-after.json", json.loads(result.stdout))
