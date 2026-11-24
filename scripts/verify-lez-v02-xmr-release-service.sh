#!/usr/bin/env bash
# Formats, tests, lints and documents the separately locked M4 XMR release
# service (compat/lez-v0_2-xmr-release-service) with the flags CI uses. The
# service reads sealed memfd descriptors and /proc, so it builds on Linux only;
# on any other system this script refuses to start Cargo.
set -euo pipefail

if [[ "$(uname -s)" != Linux ]]; then
  echo 'the M4 XMR release service requires Linux file-safety semantics; run this verifier on Linux' >&2
  exit 2
fi

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly repository_root
readonly crate_dir="${repository_root}/compat/lez-v0_2-xmr-release-service"
readonly manifest="${crate_dir}/Cargo.toml"

[[ -f "${manifest}" ]] || {
  echo "missing release service manifest: ${manifest}" >&2
  exit 2
}

cargo +1.96.0 fmt \
  --manifest-path "${manifest}" \
  --all \
  -- \
  --check
cargo +1.96.0 test \
  --manifest-path "${manifest}" \
  --locked \
  --all-targets \
  --all-features
cargo +1.96.0 clippy \
  --manifest-path "${manifest}" \
  --locked \
  --all-targets \
  --all-features \
  -- \
  -D warnings
cargo +1.96.0 rustdoc \
  --manifest-path "${manifest}" \
  --locked \
  --all-features \
  --lib \
  -- \
  -D warnings

echo 'M4 XMR release service verification: ok'
