"""Official-release provisioning; experimental switches never reach zg."""
from __future__ import annotations

import fcntl
import copy
import hashlib
import json
import shlex
import time
from pathlib import Path

from .zvec_opencode import ZvecOpenCode
from ..release_packages import VERSION, verify_release, write_json

RESOURCES = Path(__file__).resolve().parents[1]
MODEL_PREFIX = "model2vec/minishlab--potion-code-16M-v2/e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b/"
MODEL_FILES = {
    "model.safetensors": "75cf7a6c2171b230ad19b1e7d8e0b1aee86da5a02af8e7cacedd9921d227623c",
    "tokenizer/tokenizer_config.json": "81336c4a13050c6487eee83879338202a16638fbf49b5b0231a9deee1231b0c1",
    "tokenizer/tokenizer.json": "107bbdcbad4bff1d299b7a4c3a2fb17c52890688b7dd0e4c9deab79d3c4f3d45",
}
Q = shlex.quote
NVM = 'if [ -s "$HOME/.nvm/nvm.sh" ]; then . "$HOME/.nvm/nvm.sh"; fi; '


class ReleaseOpenCode(ZvecOpenCode):
    def __init__(self, *args, release_bundle: str, round_number: int, technical_smoke: bool = False, **kwargs):
        self._release_path = Path(release_bundle).resolve()
        self._release = verify_release(self._release_path)
        if self._release["platform"] != "linux-x64-gnu":
            raise ValueError("Harbor release evaluation requires Linux x64")
        if round_number not in (0, 1, 2, 3):
            raise ValueError("Round 0 is reserved for technical smoke")
        if kwargs.get("guidance_variant") is not None or kwargs.get("fixed_search"):
            raise ValueError("Release evaluation preserves published guidance and defaults")
        if kwargs.get("mcp_target") != "opencode" or kwargs.get("mcp_toolset") not in ("agent", "relationships", "search"):
            raise ValueError("Release evaluation requires a declared OpenCode arm")
        self._round_number = round_number
        if not isinstance(technical_smoke, bool) or technical_smoke != (round_number == 0):
            raise ValueError("Technical smoke must use round zero")
        self._technical_smoke = technical_smoke
        kwargs.update(graph_index=True, collect_tool_calls=True, collect_session_usage=True, record_model_requests=True)
        kwargs["zvec_grep_package_sha256"] = self._release["packages"][0]["sha256"]
        super().__init__(*args, **kwargs)
        self._seed_lock = None

    async def exec_as_agent(self, environment, command, **kwargs):
        kwargs["env"] = {"ZVEC_GREP_DEVICE": "cpu", "ZVEC_GREP_MODEL_CACHE": "/root/.zvec-grep/models",
                         "ZVEC_GREP_EMBEDDING_CONCURRENCY": "2", **(kwargs.get("env") or {})}
        return await super().exec_as_agent(environment, command, **kwargs)

    async def setup(self, environment):
        for attempt in range(1, 4):
            try:
                await super().setup(environment)
                return
            except Exception as error:
                # Setup runs before any OpenCode session or paid request.
                path = self.logs_dir / "zvec-grep-setup.json"
                if path.exists():
                    shutil_copy = self.logs_dir / f"setup-attempt-{attempt}.json"
                    shutil_copy.write_bytes(path.read_bytes())
                write_json(self.logs_dir / f"setup-error-{attempt}.json", {"stage": "setup", "attempt": attempt, "error_type": type(error).__name__})
                if attempt == 3:
                    raise
            finally:
                if self._seed_lock is not None:
                    fcntl.flock(self._seed_lock, fcntl.LOCK_UN)
                    self._seed_lock.close()
                    self._seed_lock = None

    async def _install_zvec_grep(self, environment):
        await self.exec_as_agent(environment, command="mkdir -p /tmp/zg-official-release")
        for package in self._release["packages"]:
            await environment.upload_file(self._release_path.parent / package["file"], "/tmp/zg-official-release/" + package["file"])
        await environment.upload_file(self._release_path, "/tmp/zg-official-release/release.json")
        checks = " && ".join(f"echo {Q(package['sha256'] + '  /tmp/zg-official-release/' + package['file'])} | sha256sum -c -" for package in self._release["packages"])
        verify_js = """const fs=require('fs'),c=require('crypto'),p=require('path');
const manifest=JSON.parse(fs.readFileSync('/tmp/zg-official-release/release.json'));
const root=p.join(process.argv[1],'@zvec/zvec-grep-linux-x64-gnu');
const meta=p.join(process.argv[1],'@zvec/zvec-grep');
for(const [file,expected] of Object.entries(manifest.packages[1].payload)) {
 for(const installed of [p.join(root,file),p.join(meta,file==='bin/zg'?'bin/zg.exe':file)]) {
  const data=fs.readFileSync(installed);
  if(data.length!==expected.bytes || c.createHash('sha256').update(data).digest('hex')!==expected.sha256) throw Error('Installed native payload mismatch');
 }
}
if(fs.realpathSync(process.argv[2])!==fs.realpathSync(p.join(meta,'bin/zg.exe')))throw Error('npm zg entry differs from materialized release');
fs.writeFileSync('/logs/agent/release-installed.json',JSON.stringify({version:manifest.version,packages:manifest.packages.map(x=>({name:x.name,sha256:x.sha256})),payload_verified:true}));
"""
        result = await self.exec_as_agent(environment, command=(
            "set -e; " + NVM + checks + "; "
            "npm install -g --offline --ignore-scripts --omit=optional --no-audit --no-fund "
            "/tmp/zg-official-release/platform.tgz /tmp/zg-official-release/main.tgz; "
            'node "$(npm root -g)/@zvec/zvec-grep/install.cjs"; '
            f"node -e {Q(verify_js)} \"$(npm root -g)\" \"$(command -v zg)\"; "
            'printf "ZG_RELEASE_BINARY=%s\\n" "$(npm root -g)/@zvec/zvec-grep/bin/zg.exe"'
        ))
        binary = self._parse_install_path(result.stdout, marker="ZG_RELEASE_BINARY")
        await self.exec_as_root(environment, command=f"ln -sf {Q(binary)} /usr/local/bin/zg")
        version = await self.exec_as_agent(environment, command="zg --version")
        if version.stdout.strip() != VERSION:
            raise RuntimeError("Installed runtime is not official release 0.3.1")
        await self.exec_as_agent(environment, command="zg --config model set local/potion-code-16m-v2 --device cpu")
        return VERSION, False

    async def _index_seed_identity(self, environment, workdir, *, zvec_grep_version):
        identity = await super()._index_seed_identity(environment, workdir, zvec_grep_version=zvec_grep_version)
        if identity is None or self._index_seed_root is None:
            raise RuntimeError("Release evaluation requires a commit-bound shared index snapshot")
        identity.update(release_platform_sha256=self._release["packages"][1]["sha256"], device="cpu")
        key = self._index_seed_key(identity)
        # Each Harbor process has one serial trial. The lock is host-side and
        # spans restore/build/save so other arms cannot create a second snapshot.
        self._seed_lock = (self._index_seed_root / (key + ".lock")).open("a")
        fcntl.flock(self._seed_lock, fcntl.LOCK_EX)
        return identity

    async def _restore_index_seed(self, environment, workdir, *, seed_key):
        seed = self._index_seed_dir(seed_key)
        if seed.exists():
            manifest = seed / "payload-sha256.json"
            if not manifest.exists() or json.loads(manifest.read_text()) != self._snapshot_hashes(seed / "workspace"):
                raise RuntimeError("Frozen index snapshot changed")
        return await super()._restore_index_seed(environment, workdir, seed_key=seed_key)

    async def _restored_index_is_ready(self, *args, **kwargs):
        result, ready = await super()._restored_index_is_ready(*args, **kwargs)
        if not ready:
            raise RuntimeError("Frozen release index is not ready; refusing to rebuild it")
        return result, ready

    async def _save_index_seed(self, environment, workdir, *, seed_key, identity):
        saved = await super()._save_index_seed(environment, workdir, seed_key=seed_key, identity=identity)
        if not saved:
            raise RuntimeError("Could not preserve the shared release index")
        seed = self._index_seed_dir(seed_key)
        write_json(seed / "payload-sha256.json", self._snapshot_hashes(seed / "workspace"))
        return True

    @staticmethod
    def _snapshot_hashes(root):
        return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in sorted(root.rglob("*")) if path.is_file()}

    def _mcp_install_environment(self):
        return None

    async def _setup_mcp(self, environment, workdir, metadata, *, supports_ready_check):
        started = time.monotonic()
        model_check = "const fs=require('fs'),c=require('crypto'),p=require('path');const checked={};" + \
            f"for(const [file,expected] of Object.entries({json.dumps(MODEL_FILES)})){{" + \
            f"const hash=c.createHash('sha256').update(fs.readFileSync(p.join(process.env.ZVEC_GREP_MODEL_CACHE,{json.dumps(MODEL_PREFIX)},file))).digest('hex');" + \
            "if(hash!==expected)throw Error('Embedding artifact differs from frozen model');checked[file]=hash;}" + \
            "fs.writeFileSync('/logs/agent/embedding-files.json',JSON.stringify(checked));"
        await self.exec_as_agent(environment, command=NVM + f"node -e {Q(model_check)}")
        installed = await self.exec_as_agent(environment, command="zg --install --target opencode --mcp-transport http --mcp-toolset agent --yes", cwd=workdir)
        metadata.update(mcp_setup_stdout=self._bounded_output(installed.stdout), mcp_setup_stderr=self._bounded_output(installed.stderr))
        await self.exec_as_agent(environment, command="zg --server status --check-ready", cwd=workdir)
        for name in ("release_mcp_proxy.mjs", "mcp_probe.mjs"):
            await environment.upload_file(RESOURCES / name, "/tmp/" + name)
        if self._technical_smoke:
            # Verify all three native tools before the first model session,
            # independently of the arm later exposed to that model.
            await self.exec_as_agent(environment, command=NVM + f"node /tmp/mcp_probe.mjs http://127.0.0.1:7999/mcp agent {Q(workdir)} > /logs/agent/tool-surface.native.json",
                                     cwd=workdir, env={"ZG_BENCH_RELEASE_PREFLIGHT": "1"})
        guidance_js = "const p=require('path'),o=require('os');console.log(p.join(process.env.XDG_CONFIG_HOME||p.join(o.homedir(),'.config'),'opencode','AGENTS.md'))"
        guidance = await self.exec_as_agent(environment, command=NVM + f"node -e {Q(guidance_js)}")
        guidance_path = guidance.stdout.strip().splitlines()[-1]
        if not guidance_path.startswith("/"):
            raise RuntimeError("Installed guidance path is not absolute")
        proxy = await self.exec_as_agent(environment, command=(
            "set -e; " + NVM + "nohup node /tmp/release_mcp_proxy.mjs http://127.0.0.1:7999/mcp "
            f"{self._mcp_toolset} /logs/agent/mcp-execution.jsonl /logs/agent/mcp-proxy.closed {Q(guidance_path)} "
            "> /logs/agent/mcp-proxy.json 2> /logs/agent/mcp-proxy.stderr < /dev/null & "
            "echo $! > /logs/agent/mcp-proxy.pid; "
            "for i in $(seq 1 100); do test -s /logs/agent/mcp-proxy.json && break; sleep 0.1; done; "
            "test -s /logs/agent/mcp-proxy.json; cat /logs/agent/mcp-proxy.json"
        ), cwd=workdir)
        control = json.loads(proxy.stdout)
        self._opencode_config["mcp"]["zvec_grep"]["url"] = control["url"]
        fixture = " " + Q(workdir) if self._technical_smoke else ""
        await self.exec_as_agent(environment, command=NVM + f"node /tmp/mcp_probe.mjs {Q(control['url'])} {self._mcp_toolset}{fixture} > /logs/agent/tool-surface.json",
                                 cwd=workdir, env={"ZG_BENCH_RELEASE_PREFLIGHT": "1"} if self._technical_smoke else {})
        metadata.update(mcp_toolset=self._mcp_toolset, graph_index=True, guidance_variant="published-0.3.1",
                        release_manifest=self._release, round=self._round_number,
                        mcp_setup_duration_seconds=time.monotonic() - started)

    async def _start_request_recorder(self, environment):
        for name in ("model_request_recorder.mjs", "release_request_contract.mjs"):
            await environment.upload_file(RESOURCES / name, "/tmp/" + name)
        contract_js = """const fs=require('fs');const s=JSON.parse(fs.readFileSync('/logs/agent/tool-surface.json'));
fs.writeFileSync('/logs/agent/request-contract.json',JSON.stringify({mode:'release',terminate_isolated_agent:true,
 guidance:s.guidance,instructions:s.instructions,tool_names:s.tools.map(t=>'zvec_grep_'+t.name),
 schemas:Object.fromEntries(s.tools.map(t=>['zvec_grep_'+t.name,t.inputSchema]))}));"""
        await self.exec_as_agent(environment, command=NVM + f"node -e {Q(contract_js)}")
        provider = self.model_name.split("/", 1)[0]
        upstream = self._opencode_config["provider"][provider]["options"]["baseURL"]
        result = await self.exec_as_agent(environment, command=(
            "set -e; " + NVM + f"nohup node /tmp/model_request_recorder.mjs {Q(upstream)} "
            "/logs/agent/model-requests.jsonl /logs/agent/model-recorder.closed /logs/agent/request-contract.json "
            "> /logs/agent/model-recorder.json 2> /logs/agent/model-recorder.stderr < /dev/null & "
            "echo $! > /logs/agent/model-recorder.pid; "
            "for i in $(seq 1 100); do test -s /logs/agent/model-recorder.json && break; sleep 0.1; done; "
            "test -s /logs/agent/model-recorder.json; cat /logs/agent/model-recorder.json"
        ))
        self._opencode_config = copy.deepcopy(self._opencode_config)
        self._opencode_config["provider"][provider]["options"]["baseURL"] = json.loads(result.stdout)["baseURL"]

    async def run(self, instruction, environment, context):
        try:
            await super().run(instruction, environment, context)
        finally:
            await self.exec_as_agent(environment, command=(
                'kill -TERM "$(cat /logs/agent/mcp-proxy.pid)"; '
                "for i in $(seq 1 50); do test -s /logs/agent/mcp-proxy.closed && break; sleep 0.1; done; "
                "test -s /logs/agent/mcp-proxy.closed"
            ), timeout_sec=20)
