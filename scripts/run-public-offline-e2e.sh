#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
offline_image="${LEZ_OFFLINE_RUST_IMAGE:-rust:1.96.0}"
cargo_registry="${LEZ_OFFLINE_CARGO_REGISTRY:-${CARGO_HOME:-${HOME}/.cargo}/registry}"

fail() {
  echo "offline E2E failed: $*" >&2
  exit 1
}

command -v docker >/dev/null 2>&1 || fail "Docker is required"
docker image inspect "$offline_image" >/dev/null 2>&1 \
  || fail "cached image not found (the test never pulls): ${offline_image}"
[[ -d "$cargo_registry/cache" ]] \
  || fail "cached Cargo archives not found: ${cargo_registry}/cache"
[[ -d "$cargo_registry/index" ]] \
  || fail "cached Cargo index not found: ${cargo_registry}/index"

task_tmp="$(mktemp -d "${TMPDIR:-/tmp}/lez-public-offline.XXXXXX")"
container_name="lez-public-offline-${$}-$(date +%s)"
cleanup() {
  case "$task_tmp" in
    */lez-public-offline.*)
      # Docker Desktop can release a read-only bind mount just after the
      # container exits. Retry only this validated per-run directory.
      for _ in 1 2 3; do
        rm -rf -- "$task_tmp" 2>/dev/null && return
        sleep 1
      done
      rm -rf -- "$task_tmp"
      ;;
    *) echo "refusing unexpected cleanup path: ${task_tmp}" >&2 ;;
  esac
}
trap cleanup EXIT

mkdir -p "$task_tmp/cargo-home/registry/src" "$task_tmp/target"

docker run --rm --pull never --name "$container_name" \
  --user "$(id -u):$(id -g)" \
  --network none --cap-drop ALL --security-opt no-new-privileges \
  --read-only --tmpfs /tmp:rw,exec,nosuid,nodev,size=1g \
  -v "$repo_root:/workspace:ro" \
  -v "$task_tmp/cargo-home:/cargo-home" \
  -v "$cargo_registry/cache:/cargo-home/registry/cache:ro" \
  -v "$cargo_registry/index:/cargo-home/registry/index:ro" \
  -v "$task_tmp/target:/target" \
  -e CARGO_HOME=/cargo-home \
  -e CARGO_NET_OFFLINE=true \
  -e CARGO_TARGET_DIR=/target \
  -w /workspace \
  "$offline_image" /bin/bash -c '
    set -euo pipefail
    toolchain_dir="$(find /usr/local/rustup/toolchains -mindepth 1 -maxdepth 1 -type d -name "1.96.0-*" -print -quit)"
    test -n "$toolchain_dir"
    export PATH="$toolchain_dir/bin:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    cargo test --locked --offline -p lez-maker-node \
      --lib \
      --test btc_chat_process \
      --test logos_offer_discovery \
      --test run_local_delivery
  '

echo "offline Chat and Delivery E2E passed with Docker networking disabled"
