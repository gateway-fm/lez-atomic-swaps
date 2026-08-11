#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly runner="scripts/run-m4-actual-claim-poc.sh"
readonly actor="crates/maker-node/src/bin/xmr-maker-actor.rs"
readonly supervisor="crates/maker-node/src/actor_supervisor/runtime.rs"
readonly tag15="crates/xmr-reference-actor/src/bin/xmr-reference-tag15.rs"
readonly activation="crates/xmr-reference-actor/src/lib.rs"
readonly process_test="crates/maker-node/tests/maker_xmr_tag17_supervisor.rs"

fail() {
  echo "M7 Maker Tag15 process-kill contract failed: $*" >&2
  exit 1
}

for binding in \
  M7_XMR_TAG15_PROCESS_KILL_AFTER_SUBMISSION \
  run_m7_maker_tag15_process_kill \
  m7-tag15-process-kill.json \
  m7_tag15_maker_finality \
  classify_tag15_finality \
  'claim --id'; do
  rg -Fq "$binding" "$runner" || fail "runner omits ${binding}"
done

for binding in \
  'Claim(ClaimOutput)' \
  'XmrWorkflowStep::ClaimLezTag15' \
  'claim_lez_tag15' \
  'pause_after_submitted_if_armed'; do
  rg -Fq "$binding" "$actor" || fail "real Maker actor omits ${binding}"
done

for binding in \
  'execute_effect_child' \
  'load_xmr_effect_child_plan_fd_for' \
  'XMR_EFFECT_FINALIZED_SIGNATURE_FD' \
  'lez_xmr_tag15_claim_v1'; do
  rg -Fq "$binding" "$tag15" || fail "sealed Tag15 sender omits ${binding}"
done

rg -Fq 'ActivateMakerClaimWorkflow' "$activation" ||
  fail "Maker Claim branch lacks evidence-driven Tag15 activation"

for binding in \
  '("claim", "claim_lez_tag15")' \
  '(MakerActorKindV1::Monero, ActorEffectCommand::Claim)'; do
  rg -Fq "$binding" "$supervisor" || fail "supervisor omits ${binding}"
done

for binding in \
  'provision_maker_claim' \
  'killed_tag15_actor_reconciles_durable_submission_without_resend'; do
  rg -Fq "$binding" "$process_test" || fail "real-process regression omits ${binding}"
done

echo "M7 Maker Tag15 process-kill contract passed"
