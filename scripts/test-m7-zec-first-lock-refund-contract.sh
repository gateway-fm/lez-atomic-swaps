#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export LC_ALL=C

readonly runner="scripts/run-m2-taker-sells-lez-poc.sh"
readonly handoff="scripts/run-m5-zec-chat-handoff.sh"
readonly wrapper="scripts/run-m7-zec-taker-first-lock-refund-poc.sh"
readonly facade="crates/maker-node/src/taker_facade.rs"

fail() {
  echo "M7 ZEC first-lock refund contract failed: $*" >&2
  exit 1
}

[[ -x "$wrapper" ]] || fail 'reproducible wrapper is absent or not executable'

for required in \
  'export POC_DIRECTION=taker_sells_foreign' \
  'export M5_APPLICATION_MODE=1' \
  'export M6_TAKER_SERVICE_MODE=1' \
  'export M6_ZEC_JOURNEY=first_lock_refund'; do
  rg -Fq -- "$required" "$wrapper" || fail "wrapper is missing: ${required}"
done

[[ "$(rg -Fc 'supported_direction: SwapDirection::TakerSellsForeign' "$facade")" == 2 ]] ||
  fail 'Taker service capability table does not expose BTC plus reverse ZEC routes'

for required in \
  '--direction "$POC_DIRECTION"' \
  'first_lock_refund)' \
  'M6_ZEC_JOURNEY must be claim, refund, or first_lock_refund' \
  'wait_for_m7_zcash_first_lock_refund_window' \
  'm7_zec_first_lock_refund=1' \
  'm7-maker-second-lock-absence.json' \
  'm7-zebra-first-lock-refund-inclusion.json' \
  'm7-taker-first-lock-refund-terminal-replay.json' \
  'm7-maker-first-lock-refund-terminal.json'; do
  rg -Fq -- "$required" "$runner" || fail "runner is missing: ${required}"
done

for required in \
  '--direction DIRECTION' \
  '--direction) direction=' \
  'taker_sells_foreign)' \
  "direction_cli='taker-sells-foreign'" \
  "maker_claim_preimage_file=''" \
  'maker_claim_preimage_arguments=()' \
  '--direction "$direction_cli"'; do
  rg -Fq -- "$required" "$handoff" || fail "handoff is missing: ${required}"
done

for required in \
  'maker_claim_preimage_arguments=()' \
  'if [[ "$POC_DIRECTION" == taker_sells_lez ]]'; do
  rg -Fq -- "$required" "$runner" || fail "runner is missing role-correct preimage custody: ${required}"
done

if rg -Fq 'currently requires POC_DIRECTION=taker_sells_lez' "$runner"; then
  fail 'application runner still rejects the reverse ZEC direction'
fi

printf '%s\n' 'M7 ZEC first-lock refund contract passed'
