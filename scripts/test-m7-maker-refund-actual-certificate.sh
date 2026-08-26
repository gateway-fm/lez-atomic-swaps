#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-actual-maker-refund-7cd3a9c-20260805.json"

fail() {
  echo "M7 actual Maker-refund certificate test failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

[[ -f "$certificate" && ! -L "$certificate" ]] || fail "checked certificate is missing or unsafe"

jq -e '
  .schema_version == 1
  and .kind == "m7_actual_local_maker_refund_poc"
  and .result == "passed"
  and .run_id == "m7refund-7cd3a9c-a"
  and .repository_commit == "7cd3a9c16f716543cd130f4caab20be909e35cb0"
  and .swap_id == "587418e3dfaf763a47c463469fa1110304224b5cf6b3dcd83a30000e78303b5a"
  and .agreement_commitment == "f0aea75f39ff56a9e8d58374d953b3ccfe42195886391a430a078587392b5d25"
  and .activation_commitment == "b408f20c0070951df3634ea92034a62f922a78d07cdbca757d47dad2afcde89e"
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
  and .tag13.effect == "fund"
  and .tag13.submission_outcome == "accepted"
  and .tag13.automatic_retry == false
  and .tag16.effect == "refund"
  and .tag16.submission_outcome == "accepted"
  and .tag16.automatic_retry == false
  and .tag16.finalized_timestamp_ms >= .tag16.refund_at_ms
  and .tag16.boundary_margin_ms == (.tag16.finalized_timestamp_ms - .tag16.refund_at_ms)
  and .tag16.refund_authority_only_signature == true
  and .tag16.aggregate_signature_present == true
  and .tag16.terminal_metadata_state == "refunded"
  and .tag16.terminal_custody_balance == "0"
  and .monero.version == "0.18.5.1"
  and .monero.network == "Regtest"
  and .monero.funding.role == "maker"
  and .monero.funding.confirmations == 10
  and .monero.refund.role == "maker"
  and .monero.refund.semantic_send_count == 1
  and .monero.refund.confirmations == .monero.refund.required_confirmations
  and .monero.refund.confirmations == 10
  and .monero.refund.confirmation_blocks_mined == 10
  and .monero.refund.finality_observer_sent_transaction == false
  and .monero.refund.received_amount_piconero > 0
  and .supervisor.owner_action_was_replay == false
  and .supervisor.attempt_count == 4
  and .supervisor.lease_generation == 4
  and .supervisor.submitted_state_retained == true
  and .supervisor.terminal_schedule_state == "terminal"
  and .supervisor.terminal_manual_action_state == "completed"
  and .supervisor.terminal_observation_state == "active"
  and .supervisor.terminal_observation_phase == "refunded"
  and .supervisor.terminal_observation_revision == 2
  and .supervisor.terminal_next_action == "complete"
  and .supervisor.daemon_restart_after_submission_proven == false
  and .atomicity.proof_scope == "joined_refund_branch"
  and .atomicity.finalized_lez_refund_precedes_monero_refund_send == true
  and .atomicity.taker_lez_returned == true
  and .atomicity.maker_monero_returned == true
  and .atomicity.double_collection_observed == false
  and .retention.result == "passed"
  and .retention.destination_mode == "0600"
  and .retention.destination_link_count == 1
  and .retention.no_replace_publication == true
  and .retention.private_source_removed_by_cleanup == true
  and .retention.retained_receipt_sha256 == .evidence_sha256.refund_finalized_retained
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
  and .evidence_sha256.maker_effect_provision == "a8e0da7c560109dc1c5a63b149f55f355d44c03ad9e156d458670afa489d6d65"
  and ([.evidence_sha256[] | test("^[0-9a-f]{64}$")] | all)
' "$certificate" >/dev/null || fail "certificate invariants are incomplete or inconsistent"

if rg -q -e '"(exact_bytes|private_key|secret|capability|credential|rpc_url|proof_path|binary_path)"' "$certificate"; then
  fail "certificate exposes a private or run-local field"
fi

rg -Fq './scripts/test-m7-maker-refund-actual-certificate.sh' scripts/run-ci-quality-gates.sh ||
  fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-maker-refund-actual-certificate.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI hardening does not pin the certificate contract"

echo "M7 actual Maker-refund certificate test passed"
