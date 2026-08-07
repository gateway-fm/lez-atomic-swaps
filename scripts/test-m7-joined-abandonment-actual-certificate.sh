#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-actual-joined-abandonment-a742c9f-20260807.json"

fail() {
  echo "M7 joined-abandonment certificate test failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

[[ -f "$certificate" && ! -L "$certificate" ]] || fail "checked certificate is missing or unsafe"

jq -e '
  .schema_version == 1
  and .kind == "m7_actual_local_joined_abandonment_poc"
  and .result == "passed"
  and .run_id == "m7abandon-a742c9f-a"
  and .repository_commit == "a742c9f62770a9e1a2b089c8fd4f28d3b6e38bea"
  and .swap_id == "a6167107c200acfee14fbebfbda1cea5be3e7760a7ace5e3ff1ed5e67925d8d0"
  and .agreement_commitment == "0da8d2fd328cff6e276af4e2bcda8d2cb8c2294b2117741df8393d7aad8fa21a"
  and .activation_commitment == "b3e762da68a5d0204edfc964d4e9070a2fd18ec0934c4cae8a1c37d9605ed93c"
  and .guest.recursive_tests_passed == 5
  and .guest.recursive_tests_total == 5
  and .guest.elf_sha256 == "ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee"
  and .guest.image_id == "b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"
  and .deployment.lez_source_commit == "a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
  and .deployment.exact_elf_pre_window_occurrences == 0
  and .deployment.exact_elf_post_window_occurrences == 1
  and .deployment.canonical_window_occurrences == 1
  and .deployment.send_attempts == 1
  and .deployment.bedrock_status == "Finalized"
  and .deployment.program_id_is_current_guest_image_id == true
  and .monero.version == "0.18.5.1"
  and .monero.network == "Regtest"
  and .monero.transaction_id == "cacbd3c2a1e30e5682a7d1dc8a3f669ded52da5c86d87f3ed3c20b533570ecf9"
  and .monero.amount_piconero == 1000000000000
  and .monero.confirmations_before_tag17 == 10
  and .monero.confirmations_after_tag17 == 10
  and .monero.same_transaction_before_and_after == true
  and .monero.same_agreement_before_and_after == true
  and .monero.same_containing_block_before_and_after == true
  and .monero.receipts_byte_identical == true
  and .monero.wallet_reported_available_after_tag17 == true
  and .monero.composite_key_image_unspent_authority_present == false
  and .monero.peer_count == 0
  and .tag17.transaction_id == "990cb9e44937eaef673fd88bf0e8c7a224e673a2da53f97111e3c04217910d94"
  and .tag17.prepared_before_submission == true
  and .tag17.preboundary_outcome == "uncertain"
  and .tag17.preboundary_finalized_timestamp_ms < .tag17.punish_at_ms
  and .tag17.release_request_equals_transaction_id == true
  and .tag17.release_outcome == "accepted"
  and .tag17.release_performed == true
  and .tag17.automatic_retry == false
  and .tag17.finalized_transaction_timestamp_ms >= .tag17.punish_at_ms
  and .tag17.boundary_margin_ms == (.tag17.finalized_transaction_timestamp_ms - .tag17.punish_at_ms)
  and .tag17.effect == "punish"
  and .tag17.claimant_only_signature == true
  and .tag17.terminal_metadata_state == "claimed"
  and .tag17.terminal_custody_balance == "0"
  and .tag17.maker_taker_finalized_facts_equal == true
  and .atomicity.proof_scope == "joined_penalty_branch"
  and .atomicity.same_fresh_agreement_across_both_chains == true
  and .atomicity.literal_both_refund_claimed == false
  and .atomicity.disclosed_penalty_model == true
  and .atomicity.distributed_cross_chain_transaction_claimed == false
  and .atomicity.losing_branch_injection_proven == false
  and .atomicity.future_reorg_immunity_claimed == false
  and .cleanup.result == "passed"
  and .cleanup.source_exit_status == 0
  and .cleanup.exact_run_resources_absent == true
  and .cleanup.sidecar_processes_absent == true
  and .cleanup.sidecar_ports_closed == true
  and .cleanup.foreign_sentinel_survived == true
  and .cleanup.foreign_resources_targeted == false
  and .cleanup.broad_cleanup_used == false
  and .runtime_external_resources == []
  and .public_rpc_used == false
  and .faucet_used == false
  and .public_funds_used == false
  and .public_deployment == false
  and .evidence_sha256.monero_before == .evidence_sha256.monero_after
  and ([.evidence_sha256[] | test("^[0-9a-f]{64}$")] | all)
' "$certificate" >/dev/null || fail "certificate invariants are incomplete or inconsistent"

if rg -q -e '"(exact_bytes|private_key|secret|capability|credential|rpc_url|proof_path|binary_path|destination_address)"' "$certificate"; then
  fail "certificate exposes a private or run-local field"
fi

rg -Fq './scripts/test-m7-joined-abandonment-actual-certificate.sh' scripts/run-ci-quality-gates.sh ||
  fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-joined-abandonment-actual-certificate.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI hardening does not pin the certificate contract"

echo "M7 joined-abandonment certificate test passed"
