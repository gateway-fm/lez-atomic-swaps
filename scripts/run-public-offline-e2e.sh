#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
offline_image="${LEZ_OFFLINE_RUST_IMAGE:-rust:1.96.0}"
cargo_registry="${LEZ_OFFLINE_CARGO_REGISTRY:-${CARGO_HOME:-${HOME}/.cargo}/registry}"
allow_emulation="${LEZ_OFFLINE_ALLOW_EMULATION:-false}"

fail() {
  echo "offline E2E failed: $*" >&2
  exit 1
}

command -v docker >/dev/null 2>&1 || fail "Docker is required"
docker image inspect "$offline_image" >/dev/null 2>&1 \
  || fail "cached image not found (the test never pulls): ${offline_image}"

normalize_arch() {
  case "$1" in
    aarch64) echo "arm64" ;;
    x86_64) echo "amd64" ;;
    *) echo "$1" ;;
  esac
}

image_arch="$(normalize_arch "$(docker image inspect "$offline_image" --format '{{.Architecture}}')")"
docker_arch="$(normalize_arch "$(docker info --format '{{.Architecture}}')")"
if [[ "$image_arch" != "$docker_arch" && "$allow_emulation" != "true" ]]; then
  fail "cached image ${offline_image} is ${image_arch}, but Docker is ${docker_arch}; use a cached native image via LEZ_OFFLINE_RUST_IMAGE or explicitly set LEZ_OFFLINE_ALLOW_EMULATION=true"
fi
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
    toolchain_dir=""
    for rustup_root in "${RUSTUP_HOME:-}" "${HOME:-}/.rustup" /usr/local/rustup; do
      test -n "$rustup_root" || continue
      candidate="$(find "$rustup_root/toolchains" -mindepth 1 -maxdepth 1 -type d -name "1.96.0-*" -print -quit 2>/dev/null || true)"
      if test -n "$candidate"; then
        toolchain_dir="$candidate"
        break
      fi
    done
    test -n "$toolchain_dir" || {
      echo "offline E2E failed: rustc 1.96.0 is not installed in ${offline_image}" >&2
      exit 1
    }
    export PATH="$toolchain_dir/bin:$PATH"
    rustc_version="$(rustc --version)"
    case "$rustc_version" in
      "rustc 1.96.0 "*) ;;
      *) echo "offline E2E failed: expected rustc 1.96.0, got ${rustc_version}" >&2; exit 1 ;;
    esac
    cargo test --locked --offline -p lez-maker-node \
      --lib \
      --test btc_chat_process \
      --test logos_offer_discovery \
      --test run_local_delivery
  '

echo "offline Chat and Delivery E2E passed with Docker networking disabled"
