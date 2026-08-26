#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly nightly="nightly-2026-07-01"
readonly expected_cargo_fuzz="cargo-fuzz 0.13.2"
readonly runs="${FUZZ_SMOKE_RUNS:-512}"
readonly max_len="${FUZZ_MAX_LEN:-512}"
readonly timeout_seconds="${FUZZ_TIMEOUT_SECONDS:-2}"

[[ "$runs" =~ ^[1-9][0-9]{0,5}$ ]] || {
  echo "FUZZ_SMOKE_RUNS must be an integer from 1 through 999999" >&2
  exit 2
}
[[ "$max_len" =~ ^[1-9][0-9]{0,4}$ ]] || {
  echo "FUZZ_MAX_LEN must be an integer from 1 through 99999" >&2
  exit 2
}
[[ "$timeout_seconds" =~ ^[1-9][0-9]{0,2}$ ]] || {
  echo "FUZZ_TIMEOUT_SECONDS must be an integer from 1 through 999" >&2
  exit 2
}

command -v cargo-fuzz >/dev/null || {
  echo "cargo-fuzz 0.13.2 is required" >&2
  exit 1
}
test "$(cargo fuzz --version)" = "$expected_cargo_fuzz"
rustc "+$nightly" --version >/dev/null

target_dir="${FUZZ_TARGET_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/lez-m5-fuzz-target.XXXXXX")}"
corpus_dir="$(mktemp -d "${TMPDIR:-/tmp}/lez-m5-fuzz-corpus.XXXXXX")"
artifact_dir="$(mktemp -d "${TMPDIR:-/tmp}/lez-m5-fuzz-artifacts.XXXXXX")"
cp -a fuzz/corpus/coordinator/. "$corpus_dir/"
cleanup() {
  rm -rf -- "$artifact_dir" "$corpus_dir"
  if [[ -z "${FUZZ_TARGET_DIR:-}" ]]; then
    rm -rf -- "$target_dir"
  fi
}
trap cleanup EXIT

set +e
cargo "+$nightly" fuzz run \
  --fuzz-dir fuzz \
  --target-dir "$target_dir" \
  coordinator "$corpus_dir" \
  -- \
  "-runs=$runs" \
  "-max_len=$max_len" \
  "-timeout=$timeout_seconds" \
  "-artifact_prefix=$artifact_dir/"
status=$?
set -e

if (( status != 0 )); then
  rm -rf -- "$corpus_dir"
  if [[ -z "${FUZZ_TARGET_DIR:-}" ]]; then
    rm -rf -- "$target_dir"
  fi
  trap - EXIT
  echo "coordinator fuzz failure artifact retained at $artifact_dir" >&2
  exit "$status"
fi
