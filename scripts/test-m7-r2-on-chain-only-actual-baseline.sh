#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  echo "M7 R2 on-chain-only actual baseline failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

readonly btc="docs/evidence/m3-local-two-direction-poc-20260715.json"
readonly xmr_refund="docs/evidence/m7-actual-maker-refund-process-kill-f8bee63-20260808.json"
readonly xmr_claim="docs/evidence/m7-actual-taker-claim-sweep-process-kill-997bd6b-20260811.json"
readonly zec_claim="docs/evidence/m7-actual-zec-accepted-process-kill-820001b-20260811.json"
readonly zec_refund="docs/evidence/m7-actual-zec-first-lock-refund-8981e32-20260812.json"
readonly decision="docs/architecture/0207-certify-literal-reliability-acceptance.md"

jq -e '
  .result == "passed"
  and (.directions | length) == 2
  and all(.directions[];
    .pre_effect_gate.post_lock_delivery_or_chat_configured == false
    and .atomicity.opposite_claim_completed_unilaterally_from_persisted_role_state == true)
' "$btc" >/dev/null || fail "BTC post-lock local-state evidence is incomplete"

jq -e '
  .result == "passed"
  and .process_kill.crash_boundary == "submitted_before_actor_stdout"
  and .process_kill.post_restart_route == "observe_only_pending"
  and .process_kill.automatic_submission_retry == false
  and .atomicity.taker_lez_returned == true
  and .atomicity.maker_monero_returned == true
' "$xmr_refund" >/dev/null || fail "Maker XMR recovery evidence is incomplete"

jq -e '
  .result == "passed"
  and .crash_recovery.post_restart_route == "observe_only"
  and .crash_recovery.automatic_submission_retry == false
  and .atomicity.claim_sweep_binding_passed == true
  and .monero.sweeping_role == "taker"
' "$xmr_claim" >/dev/null || fail "Taker XMR recovery evidence is incomplete"

jq -e '
  .result == "passed"
  and .application.transports_absent_after_first_lock == true
  and .process_kill.abandoned_generation_transferred == true
  and .process_kill.automatic_resubmission_observed == false
  and .terminal.maker_phase == "completed"
  and .terminal.taker_phase == "completed"
' "$zec_claim" >/dev/null || fail "ZEC accepted-claim recovery evidence is incomplete"

jq -e '
  .result == "passed"
  and .application.transports_absent_through_terminal_state == true
  and .application.terminal_action_authority == "owner_taker_service"
  and .absence.maker_second_lock_submitted == false
  and .atomicity.canonical_refund_confirmed_once == true
  and .replay.new_chain_effect == false
' "$zec_refund" >/dev/null || fail "ZEC owner-only refund evidence is incomplete"

rg -Fq 'R2 is GREEN at the private-local functional boundary' "$decision" ||
  fail "ADR 0207 does not record the R2 decision"

echo "M7 R2 on-chain-only actual baseline passed"
