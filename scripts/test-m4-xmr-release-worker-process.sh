#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
worker_manifest="${repo_root}/compat/lez-v0_2-xmr-release-service/Cargo.toml"
worker_name="lez-v0-2-xmr-release-service"
test_name="subprocess_worker_admits_once_and_restart_observes_only"

if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  if [[ "${CARGO_TARGET_DIR}" = /* ]]; then
    worker_target_dir="${CARGO_TARGET_DIR}"
  else
    worker_target_dir="${repo_root}/${CARGO_TARGET_DIR}"
  fi
else
  worker_target_dir="${repo_root}/compat/lez-v0_2-xmr-release-service/target"
fi

cargo_args=(--locked)
if [[ "${M4_RELEASE_PROCESS_OFFLINE:-0}" == "1" ]]; then
  cargo_args+=(--offline)
fi

CARGO_TARGET_DIR="${worker_target_dir}" cargo build \
  "${cargo_args[@]}" \
  --manifest-path "${worker_manifest}" \
  --bin "${worker_name}"

worker_binary="${worker_target_dir}/debug/${worker_name}"
[[ -x "${worker_binary}" ]] || {
  echo "M4 release process runner: built worker is unavailable" >&2
  exit 1
}

M4_XMR_RELEASE_WORKER_BIN="${worker_binary}" cargo test \
  "${cargo_args[@]}" \
  -p lez-xmr-release-authority \
  --test xmr_claim_release_public \
  "${test_name}" \
  -- \
  --ignored \
  --exact \
  --nocapture

echo "M4 typed-issuer release-process admission and restart proof passed"
