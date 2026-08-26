#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly process_test="crates/maker-node/tests/zec_chat_process.rs"
readonly registry="crates/swap-store/src/taker_facade_registry.rs"
readonly service="crates/maker-node/src/taker_service_config.rs"
readonly decision="docs/architecture/0198-derive-zec-taker-direction-from-authenticated-offer.md"

fail() {
  echo "M7 ZEC Taker direction process failed: $*" >&2
  exit 1
}

for path in "$process_test" "$registry" "$service" "$decision"; do
  [[ -f "$path" ]] || fail "missing ${path}"
done

for token in \
  'actor_deployment_with_direction(' \
  'SwapDirection::TakerSellsForeign' \
  'config.is_local_zcash_funder()' \
  'maker_claim_rows, 0' \
  'fault proxy upstream error'; do
  rg -Fq -- "$token" "$process_test" || fail "process proof is missing ${token}"
done

rg -Fq 'self.route.pair() != Pair::Zcash' "$registry" ||
  fail 'registry no longer fixes the ZEC pair boundary'
rg -Fq 'let route = authenticated.offer().route();' "$service" ||
  fail 'prepared service does not derive its route from the authenticated offer'
rg -Fq 'sequenceDiagram' "$decision" || fail 'architecture sequence is missing'
rg -Fq '## Atomicity argument' "$decision" || fail 'atomicity argument is missing'

cargo test -p lez-maker-node --test zec_chat_process

echo "M7 ZEC Taker direction process passed"
