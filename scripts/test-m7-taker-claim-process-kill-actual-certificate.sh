#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-actual-taker-claim-process-kill-507a38b-20260811.json"

fail() {
  echo "M7 Taker-claim process-kill certificate test failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

[[ -f "$certificate" && ! -L "$certificate" ]] || fail "checked certificate is missing or unsafe"

jq -e '
  .schema_version == 1
  and .kind == "m7_actual_local_taker_claim_process_kill"
  and .result == "passed"
  and .run_id == "m7claimkill507a38ba"
  and .repository_commit == "507a38b16b591742cc9a9aa2a25561fd3b05f74b"
  and .guest.recursive_tests_passed == 5
  and .guest.recursive_tests_total == 5
  and .guest.elf_sha256 == "ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee"
  and .guest.image_id == "b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"
  and .application_replay.second_typed_blocked_projection == true
  and .application_replay.production_default_reobservation_seconds == 3600
  and .application_replay.configured_reobservation_seconds == 1
  and .application_replay.test_acceleration_used == true
  and .crash_recovery.boundary == "invoked_before_cli_stdout"
  and .crash_recovery.killed_process == "taker_cli"
  and .crash_recovery.old_process_identity_absent == true
  and .crash_recovery.post_restart_route == "observe_only"
  and .crash_recovery.first_recovered_state == "observe_only"
  and .crash_recovery.release_journal_unchanged_after_restart == true
  and .crash_recovery.automatic_submission_retry == false
  and .tag14.owner_sidecar_role == "taker"
  and .tag14.classifier_target == "exact"
  and (.tag14.transaction_id | test("^[0-9a-f]{64}$"))
  and .tag14.containing_block_id >= .tag14.scan_start_height
  and .tag14.containing_block_id < (.tag14.scan_start_height + .tag14.scan_max_blocks)
  and .tag14.finalized_clock_height >= .tag14.containing_block_id
  and .tag14.automatic_retry == false
  and .tag15.observer_sidecar_role == "taker"
  and .tag15.classifier_target == "discover_by_terms"
  and (.tag15.transaction_id | test("^[0-9a-f]{64}$"))
  and .tag15.containing_block_id > .tag14.containing_block_id
  and .tag15.finalized_clock_height >= .tag15.containing_block_id
  and .tag15.metadata_state == "claimed"
  and .tag15.terminal_custody_balance == "0"
  and .tag15.automatic_retry == false
  and .monero.version == "0.18.5.1"
  and .monero.network == "Regtest"
  and .monero.peer_count == 0
  and (.monero.genesis_hash | test("^[0-9a-f]{64}$"))
  and .monero.sweeping_role == "taker"
  and (.monero.transaction_id | test("^[0-9a-f]{64}$"))
  and .monero.confirmations == 10
  and .monero.confirmations == (.monero.stable_tip_height - .monero.containing_block_height + 1)
  and .monero.funded_amount_piconero == (.monero.received_amount_piconero + .monero.fee_piconero)
  and .atomicity.proof_scope == "successful_claim_path_conditional_atomicity_with_ambiguous_tag14_result"
  and .atomicity.distributed_cross_chain_transaction_claimed == false
  and .atomicity.future_reorg_immunity_claimed == false
  and .atomicity.claim_sweep_binding_passed == true
  and (.atomicity.argument | length) > 300
  and (.runtime_external_resources | to_entries | all(.value == false))
  and .cleanup.result == "passed"
  and .cleanup.source_exit_status == 0
  and .cleanup.exact_run_resources_absent == true
  and .cleanup.sidecar_processes_absent == true
  and .cleanup.sidecar_ports_closed == true
  and .cleanup.foreign_sentinel_survived_exact_cleanup == true
  and .cleanup.broad_cleanup_used == false
  and .cleanup.foreign_resources_targeted == false
  and .cleanup.tag13_no_retry_latch_preserved == true
' "$certificate" >/dev/null || fail "certificate invariants are incomplete or inconsistent"

if rg -q -e '"(exact_bytes|private_key|secret|capability|credential|rpc_url|proof_path|binary_path|process_id|start_ticks|filesystem_identity|release_journal_sha256)"' "$certificate"; then
  fail "certificate exposes a private or run-local field"
fi

rg -Fq './scripts/test-m7-taker-claim-process-kill-actual-certificate.sh' scripts/run-ci-quality-gates.sh ||
  fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-taker-claim-process-kill-actual-certificate.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI hardening does not pin the certificate contract"

echo "M7 Taker-claim process-kill certificate test passed"
