#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fixture="compat/spel-zec-escrow/Cargo.toml"
spel_commit="73fc462eb8f0a4d00f1a846437c627ec2e523f83"
lez_commit="cf3639d8252040d13b3d4e933feb19b42c76e14a"
run_id="${RUN_ID:-local-$$}"

if [[ ! -f "$fixture" ]]; then
  echo "missing SPEL compatibility fixture: ${fixture}" >&2
  exit 1
fi

export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR="${TMPDIR:-/tmp}/lez-spel-compat-${run_id}"

cargo fmt --manifest-path "$fixture" -- --check
cargo clippy --locked --manifest-path "$fixture" --all-targets -- -D warnings
cargo test --locked --manifest-path "$fixture"
RUSTDOCFLAGS="-D warnings" cargo doc --locked --manifest-path "$fixture" --no-deps

lockfile="${fixture%Cargo.toml}Cargo.lock"
rg -Fq "#${spel_commit}" "$lockfile" || {
  echo "SPEL lockfile did not resolve ${spel_commit}" >&2
  exit 1
}
rg -Fq "#${lez_commit}" "$lockfile" || {
  echo "LEZ lockfile did not resolve ${lez_commit}" >&2
  exit 1
}

rsa_tree="$(cargo tree --locked --manifest-path "$fixture" -e features -i rsa@0.9.10)"
for dependency in "rzup v0.5.1" "risc0-zkvm v3.0.5" "nssa_core v0.1.0"; do
  if ! rg -Fq "$dependency" <<<"$rsa_tree"; then
    echo "reviewed RSA advisory path changed: missing ${dependency}" >&2
    exit 1
  fi
done
if rg -q 'rzup feature "(publish|install)"' <<<"$rsa_tree"; then
  echo "unsafe rzup private-key/install feature entered the compatibility graph" >&2
  exit 1
fi

tracing_tree="$(cargo tree --locked --manifest-path "$fixture" -e features -p tracing-subscriber@0.2.25)"
if rg -q 'tracing-subscriber feature "(fmt|ansi)"' <<<"$tracing_tree"; then
  echo "vulnerable tracing-subscriber formatter entered the compatibility graph" >&2
  exit 1
fi

tracing_reverse="$(cargo tree --locked --manifest-path "$fixture" -e features -i tracing-subscriber@0.2.25)"
for dependency in "ark-relations v0.5.1" "risc0-groth16 v3.0.4" "risc0-zkvm v3.0.5" "nssa_core v0.1.0"; do
  if ! rg -Fq "$dependency" <<<"$tracing_reverse"; then
    echo "reviewed tracing advisory path changed: missing ${dependency}" >&2
    exit 1
  fi
done
