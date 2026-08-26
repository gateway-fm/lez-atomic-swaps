#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  echo "M7 ZEC concurrent-demo baseline failed: $*" >&2
  exit 1
}

for command_name in cargo jq; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

cargo test --quiet -p lez-maker-node --test daemon_actor_supervisor_process \
  daemon_runs_overlapping_actors_and_isolates_failing_peer_across_restart -- --exact

jq -e '
  .result == "passed"
  and .application.direction == "taker_sells_lez"
  and .application.transports_absent_after_first_lock == true
  and .atomicity.zcash_funding_confirmed_before_lez_reveal == true
  and .atomicity.lez_reveal_precedes_zcash_claim == true
  and .terminal.maker_phase == "completed"
  and .terminal.taker_phase == "completed"
' docs/evidence/m7-actual-zec-accepted-process-kill-820001b-20260811.json >/dev/null ||
  fail "actual-node ZEC Claim layer is incomplete"

jq -e '
  .result == "passed"
  and .application.direction == "taker_sells_foreign"
  and .application.transports_absent_through_terminal_state == true
  and .atomicity.only_funded_leg_was_zcash == true
  and .atomicity.canonical_refund_confirmed_once == true
  and .replay.new_chain_effect == false
' docs/evidence/m7-actual-zec-first-lock-refund-8981e32-20260812.json >/dev/null ||
  fail "actual-node ZEC Refund layer is incomplete"

echo "M7 ZEC concurrent-demo layered baseline passed"
