"""Verify an installed candidate against the independent development fixture."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import shutil


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--opencode", type=Path, help="also run one development-only model session per group")
    args = parser.parse_args()
    binary = args.binary.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).resolve().parents[1] / "fixtures/caller-callee-dev"
    fixture = output / "workspace"
    fixture.mkdir(exist_ok=True)
    shutil.copyfile(source / "lib.rs", fixture / "lib.rs")
    subprocess.run([str(binary), "--index", "--graph", "--embedding", "local/potion-code-16m-v2", str(fixture)], check=True)
    probe = Path(__file__).with_name("mcp_probe.mjs")
    for port, group in enumerate(("relationships", "search", "agent"), 18991):
        root = output / group
        root.mkdir(exist_ok=True)
        env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "ZVEC_GREP_INSTALL_SKIP_SERVER": "1", "ZVEC_GREP_SERVER_URL": f"http://127.0.0.1:{port}/mcp"}
        subprocess.run([str(binary), "--install", "--target", "opencode", "--mcp-transport", "http", "--mcp-toolset", group, "--yes"], env=env, check=True)
        server = [str(binary), "--server"]
        subprocess.run([*server, "on", "--home", str(root / "daemon"), "--listen", f"127.0.0.1:{port}", "--mcp-toolset", group], env=env, check=True)
        try:
            evidence = subprocess.run(["node", str(probe), env["ZVEC_GREP_SERVER_URL"], group, str(fixture)], env=env, capture_output=True, text=True)
            (root / "probe-stderr.log").write_text(evidence.stderr)
            (root / "surface.json").write_text(evidence.stdout)
            evidence.check_returncode()
            print(json.dumps({"group": group, "verified": True, "evidence": str(root / "surface.json")}))
            if args.opencode is not None:
                from zg_bench.engines.opencode.config import build_opencode_config
                from zg_bench.engines.registry import resolve_opencode_model
                from zg_bench.agents.opencode_usage import CALL_AUDIT_SQL
                config = build_opencode_config(resolve_opencode_model("custom-openai/glm-5.2").configuration, profile="zvec-grep", tool_experiment=True)
                config["mcp"]["zvec_grep"]["url"] = env["ZVEC_GREP_SERVER_URL"]
                config.update(autoupdate=False, share="disabled", lsp=False)
                config_path = root / "config/opencode/opencode.json"
                config_path.write_text(json.dumps(config))
                env["OPENCODE_CONFIG"] = str(config_path)
                env["XDG_DATA_HOME"] = str(root / "data")
                env["PATH"] = str(binary.parent) + os.pathsep + os.environ["PATH"]
                questions = json.loads(source.with_suffix(".questions.json").read_text())["questions"]
                question = "Answer these questions about the current workspace using source evidence: " + " ".join(questions)
                with (root / "opencode.jsonl").open("w") as stdout, (root / "opencode-stderr.log").open("w") as stderr:
                    subprocess.run([str(args.opencode.resolve()), "run", "--dir", str(fixture), "--format", "json", "--model", "custom-openai/glm-5.2", question], cwd=fixture, env=env, stdout=stdout, stderr=stderr, check=True, timeout=600)
                with (root / "session-snapshot.json").open("w") as stdout:
                    subprocess.run([str(args.opencode.resolve()), "db", CALL_AUDIT_SQL, "--format", "json"], env=env, stdout=stdout, check=True)
        finally:
            subprocess.run([*server, "off", "--home", str(root / "daemon")], env=env, check=True)


if __name__ == "__main__":
    main()
