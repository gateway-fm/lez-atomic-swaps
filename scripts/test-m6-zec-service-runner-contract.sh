#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  printf '%s\n' "M6 ZEC service runner contract failed: $*" >&2
  exit 1
}

readonly runner="scripts/run-m2-taker-sells-lez-poc.sh"
readonly wrapper="scripts/run-m6-zec-taker-service-poc.sh"

[[ -x "$wrapper" && ! -L "$wrapper" ]] || fail 'wrapper is not a regular executable'
bash -n "$runner" "$wrapper"

rg -Fq 'export M6_TAKER_SERVICE_MODE=1' "$wrapper" || fail 'wrapper does not select M6 service mode'
rg -Fq 'exec ./scripts/run-m2-taker-sells-lez-poc.sh "$@"' "$wrapper" || fail 'wrapper bypasses the proven local corridor'

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

printf '%s\n' 'M6 ZEC service runner contract passed'
