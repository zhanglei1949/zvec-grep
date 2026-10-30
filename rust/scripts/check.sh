#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
workspace_dir="$(cd -- "${script_dir}/.." && pwd)"
cd "${workspace_dir}"

cargo fmt --all --check
for package in zg-markdown zg-storage zg-knowledge zg-engine zg-daemon-protocol zg-cli zg-host-native zg-testkit zg-transport-mcp zg-daemon zg; do
  cargo check --package "${package}" --all-targets
done
cargo check --package zg-engine --features knowledge-graph --all-targets
cargo clippy --workspace --all-targets --features zg-engine/knowledge-graph -- -D warnings
cargo test --workspace
cargo test --package zg-storage --features zvec --all-targets
cargo test --package zg-engine --features knowledge-graph --test graph_facts
cargo metadata --no-deps --format-version 1 >/dev/null
node --test scripts/npm-local.test.mjs npm/meta/install.test.cjs
