#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly runner="scripts/run-m4-actual-claim-poc.sh"
readonly taker="crates/maker-node/src/bin/lez-taker.rs"
readonly actor="crates/xmr-reference-actor/src/lib.rs"
readonly worker="crates/xmr-reference-actor/src/bin/xmr-reference-monero-refund.rs"
readonly observer="crates/xmr-reference-actor/src/bin/xmr-reference-monero-verify.rs"

fail() {
  echo "M7 Taker claim-sweep process-kill contract failed: $*" >&2
  exit 1
}

for binding in \
  M7_XMR_CLAIM_SWEEP_PROCESS_KILL_AFTER_SUBMISSION \
  run_m7_taker_claim_sweep_process_kill \
  m7-claim-sweep-process-kill.json; do
  rg -Fq "$binding" "$runner" || fail "runner omits ${binding}"
done
rg -Fq 'ActivateTakerClaimSweepWorkflow' "$actor" ||
  fail "claim sweep lacks evidence-driven durable activation"
rg -Fq 'XmrWorkflowStep::SweepMoneroClaim' "$taker" ||
  fail "real Taker command cannot select the prepared sweep"
for binding in 'ActorRole::Taker' 'XmrWorkflowStep::SweepMoneroClaim' \
  'lez_v02_m7_monero_claim_submission_v1'; do
  rg -Fq "$binding" "$worker" || fail "sealed sender omits ${binding}"
done
for binding in 'ActorRole::Taker' 'XmrWorkflowStep::SweepMoneroClaim' \
  'monero-claim-submission.json' 'monero-claim-finalized.json'; do
  rg -Fq "$binding" "$observer" || fail "read-only observer omits ${binding}"
done
for binding in 'MoneroTopologyVerifier' 'peer_count' 'foreign_wallet_version'; do
  rg -Fq "$binding" "$observer" || fail "read-only observer omits topology proof ${binding}"
done
for binding in 'validate_semantic_monero_claim_pair' +  'M7_MONERO_CLAIM_SUBMISSION_SCHEMA' 'M7_MONERO_CLAIM_FINALITY_SCHEMA' +  'finality.submission_sha256 == sha256_hex(sweep_bytes)'; do
  rg -Fq "$binding" "$actor" || fail "claim binder omits semantic pair check ${binding}"
done
rg -Fq 'LEZ_TAKER_TEST_PAUSE_AFTER_INVOKED_STEP=sweep_monero_claim' "$runner" ||
  fail "runner lacks the post-sender pre-stdout crash boundary"
rg -Fq 'mine_m7_claim_confirmations' "$runner" ||
  fail "runner does not keep confirmation generation outside sender and observer"

echo "M7 Taker claim-sweep process-kill contract passed"
