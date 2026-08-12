#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  echo "M7 R1 taker-first actual baseline failed: $*" >&2
  exit 1
}

for command_name in jq rg cargo; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

readonly btc="docs/evidence/m3-local-two-direction-poc-20260715.json"
readonly xmr="docs/evidence/m4-actual-claim-poc-20260721.json"
readonly zec="docs/evidence/m2-canonical-local-certification-20260714.json"
readonly zec_reorg="docs/evidence/m7-actual-zec-application-reorg-297f09a-20260812.json"
readonly decision="docs/architecture/0207-certify-literal-reliability-acceptance.md"

for evidence in "$btc" "$xmr" "$zec" "$zec_reorg"; do
  [[ -f "$evidence" && ! -L "$evidence" ]] || fail "retained evidence is missing or unsafe: ${evidence}"
done

jq -e '
  .result == "passed"
  and (.directions | length) == 2
  and all(.directions[]; .result == "completed"
    and .pre_effect_gate.strict_order ==
      "presignatures_before_first_effect_before_dual_lock_finality_before_reveal"
    and .atomicity.adaptor_secret_release_waited_for_both_locks == true)
  and (.directions[] | select(.direction == "TakerSellsForeign") |
    .bitcoin.lock.submitted_by == "taker"
    and .lez.transactions[1].submitted_by == "maker"
    and .bitcoin.lock.confirmations_when_gate_opened >= 1)
  and (.directions[] | select(.direction == "TakerSellsLez") |
    .lez.transactions[1].submitted_by == "taker"
    and .bitcoin.lock.submitted_by == "maker"
    and .lez.transactions[1].bedrock_status == "Finalized")
' "$btc" >/dev/null || fail "BTC retained ordering evidence is incomplete"

jq -e '
  .result == "passed_working_tree_checkpoint"
  and .role_and_atomicity_evidence.taker_first_lez_lock_finalized_before_monero_funding == true
  and .role_and_atomicity_evidence.tag14_published_only_after_exact_monero_output_confirmation == true
  and [.ordered_effects[].order] == [1,2,3,4,5,6]
  and .ordered_effects[1].role == "taker"
  and .ordered_effects[2].role == "maker_funding_boundary"
  and .ordered_effects[2].required_confirmations == 10
' "$xmr" >/dev/null || fail "XMR retained ordering evidence is incomplete"

jq -e '
  .assertions.required_atomic_order_observed == true
  and .assertions.both_supported_directions_completed == true
  and .atomicity_evidence.both_zcash_locks_had_two_confirmations_before_lez_reveal == true
  and .atomicity_evidence.observed_order_in_both_directions == [
    "zcash_funded_and_confirmed",
    "lez_revealing_claim_submitted",
    "zcash_followup_claim_submitted_and_confirmed"]
' "$zec" >/dev/null || fail "ZEC retained ordering evidence is incomplete"

jq -e '
  .result == "passed"
  and .application.funding_removed == true
  and .application.exact_transaction_reused == true
  and .application.funding_remined == true
  and .application.swap_resumed == true
  and .atomicity.dependent_maker_lock_existed == false
' "$zec_reorg" >/dev/null || fail "ZEC replacement/reconfirmation evidence is incomplete"

cargo test --quiet -p lez-bridge-adapter --test btc_current_first_lock

rg -Fq 'R1 is GREEN at the private-local functional boundary' "$decision" ||
  fail "ADR 0207 does not record the R1 decision"

echo "M7 R1 taker-first actual baseline passed"
