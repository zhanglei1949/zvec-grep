#!/usr/bin/env python3
"""Build the pinned zvec native read-only fix; do not vendor a second SDK."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(*args, cwd=None, env=None):
    subprocess.run(args, cwd=cwd, env=env, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True, help="initialized zvec checkout at the pinned revision")
    parser.add_argument("--build", type=Path, required=True)
    parser.add_argument("--simde-source", type=Path)
    parser.add_argument("--jobs", type=int, default=14)
    parser.add_argument("--oss-mirror", action="store_true")
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    source, build = args.source.resolve(), args.build.resolve()
    patches = Path(__file__).resolve().parents[1] / "patches"
    manifest = json.loads((patches / "zvec-readonly-mapping.json").read_text())
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
    if revision != manifest["revision"]:
        parser.error("source revision differs from the pinned native SDK")
    changed = subprocess.check_output(
        ["git", "diff", "HEAD", "--name-only", "--ignore-submodules=all"], cwd=source, text=True
    ).splitlines()
    if any(name != manifest["source_file"] for name in changed):
        parser.error("source has unrelated tracked modifications; use a separate checkout")
    patch = patches / "zvec-readonly-mapping.patch"
    if digest(patch) != manifest["patch_sha256"]:
        parser.error("native patch digest mismatch")
    target = source / manifest["source_file"]
    if digest(target) == manifest["before_sha256"]:
        run("git", "apply", "--check", str(patch), cwd=source)
        run("git", "apply", str(patch), cwd=source)
    if digest(target) != manifest["after_sha256"]:
        parser.error("source file differs from both the pinned base and reviewed patch")
    submodules = subprocess.check_output(["git", "submodule", "status", "--recursive"], cwd=source, text=True)
    if any(line.startswith(("-", "+", "U")) for line in submodules.splitlines()):
        parser.error("initialize nested submodules at their pinned revisions first")
    args_cmake = [
        "cmake", "-S", str(source), "-B", str(build),
        "-DCMAKE_BUILD_TYPE=Release", "-DBUILD_C_BINDINGS=ON",
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5", "-DZVEC_ENABLE_LTO=OFF",
    ]
    if args.simde_source:
        args_cmake.append("-DFETCHCONTENT_SOURCE_DIR_SIMDE=" + str(args.simde_source.resolve()))
    if args.oss_mirror:
        args_cmake.append("-DUSE_OSS_MIRROR=ON")
    env = os.environ.copy()
    # Keep compiler scratch/cache writes inside the selected build directory.
    env.setdefault("CCACHE_DIR", str(build / "ccache"))
    env["CMAKE_BUILD_PARALLEL_LEVEL"] = str(args.jobs)
    run(*args_cmake, env=env)
    run("cmake", "--build", str(build), "--target", "zvec_c_api", "--parallel", str(args.jobs), env=env)
    lib = build / "lib"
    dictionary = source / "thirdparty/cppjieba/cppjieba-5.6.7/dict"
    destination = lib / "data/jieba_dict"
    destination.mkdir(parents=True, exist_ok=True)
    for name in ["jieba.dict.utf8", "hmm_model.utf8"]:
        shutil.copyfile(dictionary / name, destination / name)
    if digest(target) != manifest["after_sha256"]:
        parser.error("native source changed during the build")
    record = {
        **manifest, "submodules": submodules.splitlines(),
        "runtime_sha256": {p.name: digest(p) for p in lib.iterdir() if p.suffix in (".dylib", ".so", ".dll")},
        "dictionary_sha256": {p.name: digest(p) for p in destination.iterdir()},
    }
    (lib / "zg-zvec-native.json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps({"ZVEC_LIB_DIR": str(lib), "ZVEC_INCLUDE_DIR": str(source / "src/include")}, indent=2))


if __name__ == "__main__":
    main()
