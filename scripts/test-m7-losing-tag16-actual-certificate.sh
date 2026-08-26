#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-actual-losing-tag16-930e3b4-20260807.json"

fail() {
  echo "M7 losing-Tag16 certificate test failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

[[ -f "$certificate" && ! -L "$certificate" ]] || fail "checked certificate is missing or unsafe"

jq -e '
  .schema_version == 1
  and .kind == "m7_actual_local_losing_tag16_after_tag17"
  and .result == "passed"
  and .run_id == "m7lose16-930e3b4-a"
  and .repository_commit == "930e3b4f5110c74959de8ec9aef92e8e3efbf8a0"
  and .swap_id == "e2a671e9b4b0a8ee509a904e43246ef50654ba82a7ef5bbb5d539036a0e751c2"
  and .agreement_commitment == "e985ad0a369acd4c7f8f1c52767958617afca96822d79c143ec7df018aa60ece"
  and .activation_commitment == "0ad326d692a67edb28d514d1a23437136e9b91ba5e66618369504a06d8b77eec"
  and .guest.recursive_tests_passed == 5
  and .guest.recursive_tests_total == 5
  and .guest.elf_sha256 == "ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee"
  and .guest.image_id == "b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"
  and .deployment.lez_source_commit == "a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
  and .deployment.chain_id == "b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"
  and .deployment.genesis_block_hash == "5cbaef6132f327812177ea5d7af1e3cf9bbf66e45ee367a6b6c69185954d57fb"
  and .deployment.exact_elf_pre_window_occurrences == 0
  and .deployment.exact_elf_post_window_occurrences == 1
  and .deployment.canonical_window_occurrences == 1
  and .deployment.send_attempts == 1
  and .deployment.bedrock_status == "Finalized"
  and .monero.version == "0.18.5.1"
  and .monero.network == "Regtest"
  and .monero.transaction_id == "f9612837be2c3322e1cdbf66cfb757b5c0288e3c8abcc3752f7f132642009844"
  and .monero.amount_piconero == 1000000000000
  and .monero.confirmations == 10
  and .monero.same_stage_a_output_before_and_after_tag17 == true
  and .monero.wallet_reported_available_after_tag17 == true
  and .monero.composite_key_image_unspent_authority_present == false
  and .monero.peer_count == 0
  and .tag17.transaction_id == "14cdfc8e3b3183288d351c806c803f506edae280fbf6164f691fc8cf5c5562dd"
  and .tag17.transport_outcome == "accepted"
  and .tag17.automatic_retry == false
  and .tag17.effect == "punish"
  and .tag17.terminal_state == "claimed"
  and .tag17.terminal_custody_balance == "0"
  and .tag17.original_facts_sha256 == .tag17.reobserved_facts_sha256
  and .tag17.facts_reobserved_equal == true
  and .tag16.completed_before_tag17_preparation == true
  and .tag16.process_exit_status == 0
  and .tag16.transport_admission == "accepted"
  and .tag16.transaction_id == "bf477091b8518bd53402cbe11b07d06ea0288e209d0a7440161cb5a7a544cc0e"
  and .tag16.submission_request_id == .tag16.transaction_id
  and .tag16.submission_outcome == "accepted"
  and .tag16.automatic_retry == false
  and .finalized_exclusion.pre_attempt_clock.height == 167
  and .finalized_exclusion.post_attempt_clock.height == 167
  and .finalized_exclusion.scan_start_height == 168
  and .finalized_exclusion.scan_blocks == 8
  and .finalized_exclusion.finalized_clock.height == 175
  and .finalized_exclusion.status == "absent"
  and .finalized_exclusion.terminal_rule == "refund_absent_only_when_claimed_zero_at_candidate_and_window_end"
  and .atomicity.proof_scope == "tag17_wins_over_late_tag16_in_finalized_window"
  and .atomicity.losing_refund_excluded == true
  and .atomicity.complete_attempt_interval_covered == true
  and .atomicity.distributed_cross_chain_transaction_claimed == false
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
  and .evidence_sha256.tag17_original_facts == .evidence_sha256.tag17_reobserved_facts
  and ([.evidence_sha256[] | test("^[0-9a-f]{64}$")] | all)
' "$certificate" >/dev/null || fail "certificate invariants are incomplete or inconsistent"

if rg -q -e '"(exact_bytes|private_key|secret|capability|credential|rpc_url|proof_path|binary_path|destination_address|shared_address)"' "$certificate"; then
  fail "certificate exposes a private or run-local field"
fi

rg -Fq './scripts/test-m7-losing-tag16-actual-certificate.sh' scripts/run-ci-quality-gates.sh ||
  fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-losing-tag16-actual-certificate.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI hardening does not pin the certificate contract"

echo "M7 losing-Tag16 certificate test passed"
