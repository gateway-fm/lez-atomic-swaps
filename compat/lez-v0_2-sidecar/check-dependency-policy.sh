#!/usr/bin/env bash
set -euo pipefail

crate_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
manifest="$crate_dir/Cargo.toml"
policy="$crate_dir/deny.toml"

export CARGO_NET_OFFLINE=true

dependency_features() {
  cargo +1.96.0 tree \
    --locked \
    --offline \
    --manifest-path "$manifest" \
    --edges features \
    --invert "$1"
}

hickory_features="$(dependency_features 'hickory-proto@0.25.0-alpha.5')"
[[ "$hickory_features" == *'hickory-proto v0.25.0-alpha.5'* ]]
[[ "$hickory_features" == *'libp2p-dns v0.43.0'* ]]
if [[ "$hickory_features" == *'hickory-proto feature "dnssec-'* ]]; then
  echo 'Hickory DNSSEC was activated; RUSTSEC-2026-0118 is reachable' >&2
  exit 1
fi

tracing_features="$(dependency_features 'tracing-subscriber@0.2.25')"
[[ "$tracing_features" == *'tracing-subscriber v0.2.25'* ]]
[[ "$tracing_features" == *'ark-relations v0.5.1'* ]]
for vulnerable_feature in ansi fmt; do
  if [[ "$tracing_features" == *"tracing-subscriber feature \"$vulnerable_feature\""* ]]; then
    echo "tracing-subscriber $vulnerable_feature was activated; RUSTSEC-2025-0055 is reachable" >&2
    exit 1
  fi
done

rsa_features="$(dependency_features 'rsa@0.9.10')"
[[ "$rsa_features" == *'rsa v0.9.10'* ]]
[[ "$rsa_features" == *'rzup v0.5.1'* ]]
[[ "$rsa_features" == *'risc0-build v3.0.5'* ]]

spin_features="$(dependency_features 'spin@0.9.8')"
[[ "$spin_features" == *'spin v0.9.8'* ]]
[[ "$spin_features" == *'astro-float-num v0.3.6'* ]]

# The reachability arguments for RUSTSEC-2023-0071 and RUSTSEC-2026-0119
# depend on this isolated process never directly invoking RSA/rzup or the
# transitive DNS/libp2p implementation. Fail if that boundary changes.
if rg --line-number \
  '\b(hickory|libp2p|rsa|rzup|spin|tracing_subscriber)\b' \
  "$crate_dir/src" "$crate_dir/tests"; then
  echo 'sidecar source directly references an excepted upstream dependency' >&2
  exit 1
fi

cargo deny \
  --log-level error \
  --manifest-path "$manifest" \
  --locked \
  --offline \
  --all-features \
  check \
  --config "$policy" \
  advisories bans licenses sources

echo 'LEZ v0.2 sidecar dependency policy: ok'
