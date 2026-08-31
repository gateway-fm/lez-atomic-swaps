#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

command -v docker >/dev/null || {
  echo "Docker is required for the supported Linux runtime check" >&2
  exit 1
}

repository="$(pwd -P)"
readonly repository
runtime_user="$(id -u):$(id -g)"
readonly runtime_user

docker run --rm \
  --user "$runtime_user" \
  --env CARGO_HOME=/tmp/cargo-home \
  --env CARGO_TARGET_DIR=/tmp/cargo-target \
  --volume "$repository:/workspace:ro" \
  --workdir /workspace \
  rust:1.96.0-bookworm \
  cargo check --locked -p lez-node-common -p lez-maker-node -p lez-taker-node --all-targets
