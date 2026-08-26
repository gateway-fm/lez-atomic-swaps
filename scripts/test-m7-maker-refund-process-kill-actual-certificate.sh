#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-actual-maker-refund-process-kill-f8bee63-20260808.json"

fail() {
  echo "M7 actual Maker-refund process-kill certificate test failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

[[ -f "$certificate" && ! -L "$certificate" ]] || fail "checked certificate is missing or unsafe"

jq -e '
  .schema_version == 1
  and .kind == "m7_actual_local_maker_refund_process_kill"
  and .result == "passed"
  and .run_id == "m7refundkill-f8bee63-d"
  and .repository_commit == "f8bee63f0279e0362713bb6af752c5595a6a98e0"
  and .swap_id == "cee3575262030c34a43819c8c24ddc5c2ed4e1ad70571b7a38116278b26ca176"
  and .agreement_commitment == "86e6c6f49b03f8a9d59fa49cdaeee79913cf20abcc86c49d0c75bc6f236c6ac4"
  and .activation_commitment == "155f2f9276d016559f2044e19cb637530b8ddf5f6aebd8265f2fae33921f2df2"
  and .guest.recursive_tests_passed == 5
  and .guest.recursive_tests_total == 5
  and .guest.elf_sha256 == "ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee"
  and .guest.image_id == "b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"
  and .deployment.lez_source_commit == "a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
  and .deployment.chain_id == "b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"
  and .deployment.genesis_block_hash == "2a10c609c46d2381bd099455683262e397f5b7c723918c0ce84d66967e036c44"
  and .deployment.exact_elf_pre_window_occurrences == 0
  and .deployment.exact_elf_post_window_occurrences == 1
  and .deployment.canonical_window_occurrences == 1
  and .deployment.send_attempts == 1
  and .deployment.bedrock_status == "Finalized"
  and .tag13.effect == "fund"
  and .tag13.transaction_id == "9dffe743ebb537e0507bb9d9300f7ae2df51eb425320c46a90ce53755a066beb"
  and .tag13.submission_outcome == "accepted"
  and .tag13.automatic_retry == false
  and .tag16.effect == "refund"
  and .tag16.transaction_id == "b4f765ff5762080c89b74d39a5c89a09fdfbd51ed04761ed50bbf2cfa384187e"
  and .tag16.submission_outcome == "accepted"
  and .tag16.automatic_retry == false
  and .tag16.finalized_transaction_block_id == 169
  and .tag16.finalized_tip_id == 181
  and .tag16.finalized_timestamp_ms >= .tag16.refund_at_ms
  and .tag16.boundary_margin_ms == (.tag16.finalized_timestamp_ms - .tag16.refund_at_ms)
  and .tag16.refund_authority_only_signature == true
  and .tag16.aggregate_signature_present == true
  and .tag16.terminal_metadata_state == "refunded"
  and .tag16.terminal_custody_balance == "0"
  and .monero.version == "0.18.5.1"
  and .monero.network == "Regtest"
  and .monero.peer_count == 0
  and .monero.funding.role == "maker"
  and .monero.funding.transaction_id == "552521ea60b059a084c627bfe9b1fc778add3f4da29d26d82626799c547b2475"
  and .monero.funding.confirmations == 10
  and .monero.refund.role == "maker"
  and .monero.refund.semantic_send_count == 1
  and .monero.refund.transaction_id == "30a7926eff9cc1aba3a660781cb233aaec6b2147c17ebbfcfa9f925d04f51ce7"
  and .monero.refund.confirmations == .monero.refund.required_confirmations
  and .monero.refund.confirmations == 10
  and .monero.refund.confirmation_blocks_mined == 10
  and .monero.refund.finality_observer_sent_transaction == false
  and .monero.refund.received_amount_piconero == 998191600000
  and .process_kill.schema == "lez_v02_m7_monero_refund_process_kill_v1"
  and .process_kill.crash_boundary == "submitted_before_actor_stdout"
  and .process_kill.kill_order == "daemon_then_actor"
  and .process_kill.crashed_generation == 4
  and .process_kill.recovered_generation == 6
  and .process_kill.recovered_generation > .process_kill.crashed_generation
  and .process_kill.abandoned_generation_transferred == true
  and .process_kill.post_restart_route == "observe_only_pending"
  and .process_kill.submission.sha256 == "3e3857e9e7117f5aefd966837c2e0807a11bdb405c8a9ee61e1853d7a0adf4da"
  and .process_kill.submission.transaction_id == .monero.refund.transaction_id
  and .process_kill.submission.unchanged_after_restart == true
  and .process_kill.automatic_submission_retry == false
  and .process_kill.confirmations_mined_before_restart == 0
  and .process_kill.old_process_identities_absent == true
  and .supervisor.submitted.attempt_count == 4
  and .supervisor.submitted.lease_generation == 4
  and .supervisor.submitted.manual_action_state == "leased"
  and .supervisor.submitted.observation_revision == 0
  and .supervisor.recovered.attempt_count == 6
  and .supervisor.recovered.lease_generation == 6
  and .supervisor.recovered.manual_action_state == "leased"
  and .supervisor.recovered.observation_phase == "maker_recovery_available"
  and .supervisor.recovered.observation_revision == 1
  and .supervisor.terminal.attempt_count == 7
  and .supervisor.terminal.lease_generation == 7
  and .supervisor.terminal.schedule_state == "terminal"
  and .supervisor.terminal.manual_action_state == "completed"
  and .supervisor.terminal.observation_phase == "refunded"
  and .supervisor.terminal.observation_revision == 2
  and .supervisor.terminal.next_action == "complete"
  and .ordering.tag16_finality_phase_index == 39
  and .ordering.process_kill_started_phase_index == 44
  and .ordering.process_kill_completed_phase_index == 45
  and .ordering.supervisor_completed_phase_index == 46
  and .ordering.confirmations_mined_only_after_restart == true
  and .atomicity.proof_scope == "joined_refund_branch_after_ordered_process_kill"
  and .atomicity.finalized_lez_refund_precedes_monero_refund_send == true
  and .atomicity.taker_lez_returned == true
  and .atomicity.maker_monero_returned == true
  and .atomicity.double_collection_observed == false
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
  and ([.evidence_sha256[] | test("^[0-9a-f]{64}$")] | all)
' "$certificate" >/dev/null || fail "certificate invariants are incomplete or inconsistent"

if rg -q -e '"(exact_bytes|private_key|secret|capability|credential|rpc_url|proof_path|binary_path|filesystem_identity|process_id|start_ticks|socket|password|username|shared_address|destination_address)"' "$certificate"; then
  fail "certificate exposes a private or run-local field"
fi

rg -Fq './scripts/test-m7-maker-refund-process-kill-actual-certificate.sh' scripts/run-ci-quality-gates.sh ||
  fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-maker-refund-process-kill-actual-certificate.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI hardening does not pin the certificate contract"

echo "M7 actual Maker-refund process-kill certificate test passed"
