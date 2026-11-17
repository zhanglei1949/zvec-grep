"""Download and verify the two immutable npm packages used by release E2E."""
from __future__ import annotations

import base64
import hashlib
import io
import json
import os
import subprocess
import tarfile
from pathlib import Path, PurePosixPath
from urllib.parse import quote, urlparse
from urllib.request import urlopen

VERSION = "0.3.1"
SOURCE_COMMIT = "40b6005bd9adc784cf24c41afd2e753c883e44d4"
REGISTRY = "https://registry.npmjs.org"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def fetch(url: str) -> bytes:
    parsed = urlparse(url)
    if parsed.scheme != "https" or parsed.hostname != "registry.npmjs.org" or parsed.username or parsed.password:
        raise ValueError("Release downloads must come from the official npm registry")
    with urlopen(url, timeout=120) as response:
        if urlparse(response.url).hostname != "registry.npmjs.org":
            raise ValueError("Unexpected npm download redirect")
        return response.read()


def verify_integrity(payload: bytes, dist: dict) -> None:
    expected = "sha512-" + base64.b64encode(hashlib.sha512(payload).digest()).decode()
    if dist.get("integrity") != expected or dist.get("shasum") != hashlib.sha1(payload).hexdigest():
        raise ValueError("npm tarball integrity mismatch")


def package_members(payload: bytes) -> dict[str, bytes]:
    files = {}
    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:gz") as archive:
        for member in archive.getmembers():
            path = PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts or path.parts[0] != "package" or member.issym() or member.islnk():
                raise ValueError("Unsafe npm archive member")
            if member.isfile():
                if member.name in files:
                    raise ValueError("Duplicate npm archive member")
                files[member.name] = archive.extractfile(member).read()
    return files


def prepare_release(output: Path, *, platform: str = "linux-x64-gnu", version: str = VERSION) -> dict:
    if version != VERSION or platform not in ("linux-x64-gnu", "darwin-arm64"):
        raise ValueError("This protocol pins release 0.3.1 and a supported CPU platform")
    output.mkdir(parents=True, exist_ok=True)
    records = []
    for role, name in (("main", "@zvec/zvec-grep"), ("platform", f"@zvec/zvec-grep-{platform}")):
        metadata = json.loads(fetch(f"{REGISTRY}/{quote(name, safe='')}/{version}"))
        if metadata.get("name") != name or metadata.get("version") != version:
            raise ValueError("npm metadata identity mismatch")
        payload = fetch(metadata["dist"]["tarball"])
        verify_integrity(payload, metadata["dist"])
        files = package_members(payload)
        identity = json.loads(files["package/package.json"])
        if identity["name"] != name or identity["version"] != version:
            raise ValueError("npm package identity mismatch")
        target = output / f"{role}.tgz"
        target.write_bytes(payload)
        write_json(output / f"{role}.metadata.json", metadata)
        record = {"role": role, "name": name, "version": version, "file": target.name,
                  "sha256": sha256(target), "integrity": metadata["dist"]["integrity"],
                  "shasum": metadata["dist"]["shasum"], "tarball": metadata["dist"]["tarball"]}
        if role == "platform":
            checksums = json.loads(files["package/checksums.json"])
            for relative, expected in checksums["files"].items():
                actual = files.get("package/" + relative)
                if actual is None or len(actual) != expected["bytes"] or hashlib.sha256(actual).hexdigest() != expected["sha256"]:
                    raise ValueError(f"Native payload mismatch: {relative}")
            record["payload"] = checksums["files"]
        records.append(record)
    manifest = {"schema_version": 1, "source": "official-npm-release", "version": version,
                "platform": platform, "expected_source_commit": SOURCE_COMMIT, "packages": records}
    write_json(output / "release.json", manifest)
    return verify_release(output / "release.json")


def verify_release(path: Path) -> dict:
    manifest = json.loads(path.read_text())
    if manifest.get("source") != "official-npm-release" or manifest.get("version") != VERSION:
        raise ValueError("Wrong release manifest")
    if manifest.get("expected_source_commit") != SOURCE_COMMIT:
        raise ValueError("Wrong release source pin")
    platform = manifest["platform"]
    if platform not in ("linux-x64-gnu", "darwin-arm64"):
        raise ValueError("Unsupported platform")
    if [item["role"] for item in manifest["packages"]] != ["main", "platform"]:
        raise ValueError("Expected main and platform packages")
    for record in manifest["packages"]:
        expected_name = "@zvec/zvec-grep" + (f"-{platform}" if record["role"] == "platform" else "")
        if record["name"] != expected_name or record["file"] != record["role"] + ".tgz" or record["version"] != VERSION:
            raise ValueError("Unexpected release package identity")
        archive = path.parent / record["file"]
        payload = archive.read_bytes()
        if sha256(archive) != record["sha256"]:
            raise ValueError("Release archive changed")
        verify_integrity(payload, record)
        files = package_members(payload)
        identity = json.loads(files["package/package.json"])
        if identity["name"] != record["name"] or identity["version"] != VERSION:
            raise ValueError("Release archive identity changed")
        if record["role"] == "main" and identity["optionalDependencies"].get(f"@zvec/zvec-grep-{platform}") != VERSION:
            raise ValueError("Main/platform release mismatch")
        if record["role"] == "platform":
            declared = json.loads(files["package/checksums.json"])["files"]
            if record.get("payload") != declared:
                raise ValueError("Native checksum manifest changed")
            for relative, expected in declared.items():
                actual = files.get("package/" + relative)
                if actual is None or len(actual) != expected["bytes"] or hashlib.sha256(actual).hexdigest() != expected["sha256"]:
                    raise ValueError("Native payload changed")
    return manifest


def verify_install(path: Path, prefix: Path) -> dict:
    """Exercise the official installer without an Agent or any model call."""
    manifest = verify_release(path)
    prefix = prefix.resolve()
    if prefix.exists():
        raise ValueError("Installation check requires a fresh isolated prefix")
    prefix.mkdir(parents=True)
    environment = {**os.environ, "ZG_INSTALL_SKIP_AGENT_UPGRADE": "1"}
    with (prefix / "install.log").open("w") as log:
        subprocess.run(["npm", "install", "--global", "--prefix", str(prefix), "--offline", "--ignore-scripts", "--omit=optional", "--no-audit", "--no-fund",
                        str(path.parent / "platform.tgz"), str(path.parent / "main.tgz")], env=environment, stdout=log, stderr=log, check=True)
        meta = prefix / "lib/node_modules/@zvec/zvec-grep"
        subprocess.run(["node", str(meta / "install.cjs")], env=environment, stdout=log, stderr=log, check=True)
    checked = {}
    for name, expected in manifest["packages"][1]["payload"].items():
        target = meta / ("bin/zg.exe" if name == "bin/zg" else name)
        if target.stat().st_size != expected["bytes"] or sha256(target) != expected["sha256"]:
            raise ValueError("Materialized release payload differs from verified platform package")
        checked[name] = expected["sha256"]
    binary = prefix / "bin/zg"
    if binary.resolve() != (meta / "bin/zg.exe").resolve():
        raise ValueError("npm executable does not point to the verified payload")
    version = subprocess.check_output([str(binary), "--version"], env=environment, text=True).strip()
    if version != VERSION:
        raise ValueError("Materialized release runtime version mismatch")
    result = {"version": version, "platform": manifest["platform"], "official_postinstall_executed": True,
              "npm_entry_verified": True, "payload_sha256": checked, "model_requests": 0}
    write_json(path.parent / "installation.json", result)
    return result
