#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fixture="compat/lez-v0.2-provisional/Cargo.toml"
spel_commit="df17acd98436be4f09c55877dae1fe2e73cbcdca"
lez_commit="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
compat_test_sha256="51533c89142a4bad6d71a7bf4370e4f5812a7351b5f176b9397b536a595a221e"
run_id="${RUN_ID:-local-$$}"
if [[ ! "$run_id" =~ ^[a-z0-9][a-z0-9_-]*$ ]]; then
  echo "RUN_ID must contain only lowercase letters, numbers, underscores, or hyphens" >&2
  exit 1
fi

export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${TMPDIR:-/tmp}/lez-v02-provisional-target-${run_id}}"
export LEZ_V02_TOOL_DIR="${LEZ_V02_TOOL_DIR:-${TMPDIR:-/tmp}/lez-v02-provisional-tools-${run_id}}"
mkdir -p "$CARGO_TARGET_DIR" "$LEZ_V02_TOOL_DIR"
export PATH="$LEZ_V02_TOOL_DIR:$PATH"

command -v unzip >/dev/null || {
  echo "unzip is required by the pinned rust-rapidsnark build" >&2
  exit 1
}

cargo fmt --manifest-path "$fixture" -- --check
cargo clippy --locked --manifest-path "$fixture" --all-targets -- -D warnings
cargo test --locked --manifest-path "$fixture"
RUSTDOCFLAGS="-D warnings" cargo doc --locked --manifest-path "$fixture" --no-deps

lockfile="compat/lez-v0.2-provisional/Cargo.lock"
rg -Fq "?rev=${spel_commit}#${spel_commit}" "$lockfile" || {
  echo "provisional lockfile did not resolve exact SPEL PR head ${spel_commit}" >&2
  exit 1
}
rg -Fq "?tag=v0.2.0#${lez_commit}" "$lockfile" || {
  echo "provisional lockfile did not resolve LEZ v0.2.0 to ${lez_commit}" >&2
  exit 1
}
if rg -q 'logos-execution-zone\.git\?rev=' "$lockfile"; then
  echo "LEZ revision source would duplicate PR #238's tag-based lee_core types" >&2
  exit 1
fi
while IFS= read -r source; do
  if [[ "$source" != *"?tag=v0.2.0#${lez_commit}"* ]]; then
    echo "unexpected LEZ source identity: ${source}" >&2
    exit 1
  fi
done < <(rg 'source = "git\+https://github.com/logos-blockchain/logos-execution-zone\.git' "$lockfile")

rsa_tree="$(cargo tree --locked --manifest-path "$fixture" -e features -i rsa@0.9.10)"
for dependency in "rzup v0.5.1" "risc0-zkvm v3.0.5" "lee_core v0.1.0"; do
  if ! rg -Fq "$dependency" <<<"$rsa_tree"; then
    echo "reviewed RSA advisory path changed: missing ${dependency}" >&2
    exit 1
  fi
done
if rg -q 'rzup feature "(publish|install)"' <<<"$rsa_tree"; then
  echo "unsafe rzup private-key/install feature entered the provisional graph" >&2
  exit 1
fi

tracing_tree="$(cargo tree --locked --manifest-path "$fixture" -e features -p tracing-subscriber@0.2.25)"
if rg -q 'tracing-subscriber feature "(fmt|ansi)"' <<<"$tracing_tree"; then
  echo "vulnerable tracing-subscriber formatter entered the provisional graph" >&2
  exit 1
fi
tracing_reverse="$(cargo tree --locked --manifest-path "$fixture" -e features -i tracing-subscriber@0.2.25)"
for dependency in "ark-relations v0.5.1" "risc0-groth16 v3.0.4" "risc0-zkvm v3.0.5" "lee_core v0.1.0"; do
  if ! rg -Fq "$dependency" <<<"$tracing_reverse"; then
    echo "reviewed tracing advisory path changed: missing ${dependency}" >&2
    exit 1
  fi
done

hickory_tree="$(cargo tree --locked --manifest-path "$fixture" -e features -p hickory-proto@0.25.0-alpha.5)"
if rg -q 'hickory-proto feature "dnssec-(ring|aws-lc-rs)"' <<<"$hickory_tree"; then
  echo "DNSSEC validation entered the advisory-constrained Hickory graph" >&2
  exit 1
fi
hickory_reverse="$(cargo tree --locked --manifest-path "$fixture" -e features -i hickory-proto@0.25.0-alpha.5)"
for dependency in "hickory-resolver v0.25.0-alpha.5" "libp2p-dns v0.43.0" "sequencer_service v0.1.0"; do
  if ! rg -Fq "$dependency" <<<"$hickory_reverse"; then
    echo "reviewed Hickory advisory path changed: missing ${dependency}" >&2
    exit 1
  fi
done

compat_test="compat/lez-v0.2-provisional/tests/compatibility.rs"
actual_compat_test_sha256="$(sha256sum "$compat_test" | cut -d ' ' -f 1)"
if [[ "$actual_compat_test_sha256" != "$compat_test_sha256" ]]; then
  echo "compile-only compatibility test changed; review Hickory advisory exceptions" >&2
  exit 1
fi
rg -Fq 'drop(standalone);' "$compat_test" || {
  echo "compile-only standalone future is no longer explicitly dropped" >&2
  exit 1
}
for forbidden in '#[tokio::test' '.await' 'block_on(' 'check_health(' 'send_transaction('; do
  if rg -Fq "$forbidden" "$compat_test"; then
    echo "compile-only Hickory exception invalidated by executable test pattern: ${forbidden}" >&2
    exit 1
  fi
done
