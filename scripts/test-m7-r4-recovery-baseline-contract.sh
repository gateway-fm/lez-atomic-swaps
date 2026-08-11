#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly recovery_test="zec_sdk_recovery"
readonly -a claim_tests=(
  claim_reopen_rejects_wrong_key_id_or_material_without_state_mutation
  corrupt_or_future_protected_claim_payloads_fail_closed_on_reopen
  orphaned_duplicate_holey_or_drifted_claim_journals_fail_closed
  claim_transition_failures_roll_back_every_coupled_sqlite_effect
  claim_retry_observes_exact_durable_bytes_before_any_rebroadcast
)

for test_name in "${claim_tests[@]}"; do
  cargo test --locked -p lez-swap-store --test "$recovery_test" \
    "$test_name" -- --exact
done

./scripts/test-m7-maker-refund-process-kill-actual-certificate.sh
./scripts/test-m7-taker-claim-process-kill-actual-certificate.sh

echo "M7 R4 recovery baseline contract passed"
