#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  printf '%s\n' "M6 ZEC service runner contract failed: $*" >&2
  exit 1
}

readonly runner="scripts/run-m2-taker-sells-lez-poc.sh"
readonly wrapper="scripts/run-m6-zec-taker-service-poc.sh"
readonly refund_wrapper="scripts/run-m6-zec-taker-service-refund-poc.sh"

[[ -x "$wrapper" && ! -L "$wrapper" ]] || fail 'wrapper is not a regular executable'
[[ -x "$refund_wrapper" && ! -L "$refund_wrapper" ]] ||
  fail 'refund wrapper is not a regular executable'
bash -n "$runner" "$wrapper" "$refund_wrapper"

rg -Fq 'export M6_TAKER_SERVICE_MODE=1' "$wrapper" || fail 'wrapper does not select M6 service mode'
rg -Fq 'export M6_ZEC_JOURNEY=claim' "$wrapper" || fail 'claim wrapper does not fix the claim journey'
rg -Fq 'exec ./scripts/run-m2-taker-sells-lez-poc.sh "$@"' "$wrapper" || fail 'wrapper bypasses the proven local corridor'
rg -Fq 'export M6_TAKER_SERVICE_MODE=1' "$refund_wrapper" ||
  fail 'refund wrapper does not select M6 service mode'
rg -Fq 'export M6_ZEC_JOURNEY=refund' "$refund_wrapper" ||
  fail 'refund wrapper does not fix the refund journey'
rg -Fq 'exec ./scripts/run-m2-taker-sells-lez-poc.sh "$@"' "$refund_wrapper" ||
  fail 'refund wrapper bypasses the proven local corridor'

handler_source="$(sed -n '/^handle_zcash_submission() {$/,/^}$/p' "$runner")"
[[ -n "$handler_source" ]] || fail 'Zcash submission handler is missing'

# Execute the production function with inert chain effects. A service-owned
# claim must stop after its one Zcash effect and must not be reclassified as
# the earlier LEZ revealing claim.
eval "$handler_source"
M6_TAKER_SERVICE_MODE=1
lez_revealing_claim_seen=1
expected_zcash_claimant_role=taker
expected_zcash_funder_role=maker
zcash_claim_mined=0
m6_zcash_claim_txid="$(printf 'a%.0s' {1..64})"
zcash_claim_submitter=''
lez_revealing_claim_submitter=maker
mine_blocks() { [[ "$1" == followup-claim && "$2" == 1 ]]; }

claim='{"schema_version":1,"action":"claim","was_replay":false,"m6_first_claim":true}'
handle_zcash_submission taker "$claim" || fail 'service claim fell through into LEZ reveal validation'
[[ "$zcash_claim_mined" == 1 && "$zcash_claim_submitter" == taker ]] ||
  fail 'service claim did not record exactly one Taker Zcash effect'
[[ "$lez_revealing_claim_seen" == 1 && "$lez_revealing_claim_submitter" == maker ]] ||
  fail 'service claim mutated prior LEZ-reveal evidence'

required_markers=(
  'readonly M6_ZEC_JOURNEY="${M6_ZEC_JOURNEY:-claim}"'
  'readonly M6_SERVICE_QUERY_TIMEOUT_MS=15000'
  'readonly M6_SERVICE_ACTION_TIMEOUT_MS=40000'
  'M6_ZEC_JOURNEY must be claim or refund'
  'MAX_CORRIDOR_SECONDS=130'
  'm6_claim_generation:$generation'
  'm6_zcash_claim_txid:$txid'
  'm6-zebra-mempool-before-claim.json'
  'm6-zebra-mempool-after-first-claim.json'
  'm6-zebra-mempool-after-claim-replay.json'
  'm6_claim_generation="$(jq -er'
  '.m6_claim_generation | numbers'
  '--argjson m6_taker_service_mode "$M6_TAKER_SERVICE_MODE"'
  'm6_taker_service_mode: ($m6_taker_service_mode == 1)'
  '"owner_taker_service"'
  'drive_m6_taker_refund()'
  'taker_swap_refund_v1'
  'action:"refund"'
  'taker_action_conflict'
  'm6-taker-service-refund-first.json'
  'm6-taker-service-refund-transients.ndjson'
  'm6-taker-service-refund-commit.json'
  'm6-taker-service-refund-replay.json'
  'm6-taker-service-refund-claim-exclusion.json'
  '"m6-refund-admission-${admission_attempt}" \'
  '"$refund_request" "$M6_SERVICE_ACTION_TIMEOUT_MS")"'
  '"m6-refund-replay-${round}" "$refund_request" "$M6_SERVICE_ACTION_TIMEOUT_MS"'
  'm6_taker_lez_refund_deadline_ms()'
  'wait_for_m6_lez_refund_window'
  'm6-taker-lez-refund-window.json'
  'm6-taker-lez-refund-finality.json'
  'm6-refund-maker-manual-action.json'
  'm6-zebra-mempool-zcash-refund.json'
  'm6_maker_supervisor_suppressed=1'
  'start_m6_refund_maker_supervisor'
  'direct Taker drive crossed the M6 service terminal-action boundary'
  '--arg journey "$M6_ZEC_JOURNEY"'
  'journey: $journey'
  '"lez_refund_finalized"'
  '"zcash_refund_submitted_and_confirmed"'
)
for required in "${required_markers[@]}"; do
  rg -Fq -- "$required" "$runner" || fail "runner is missing replay evidence propagation: ${required}"
done

printf '%s\n' 'M6 ZEC service runner contract passed'
