#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-actual-zec-first-lock-refund-8981e32-20260812.json"

fail() {
  echo "M7 actual reverse-ZEC first-lock refund certificate test failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

[[ -f "$certificate" && ! -L "$certificate" ]] ||
  fail "checked certificate is missing or unsafe"

jq -e '
  .schema_version == 1
  and .kind == "m7_actual_local_zec_first_lock_refund"
  and .result == "passed"
  and .run_id == "m7zecfirstrefund8981e32a"
  and .repository_commit == "8981e324b300038401cec93844c60f659ab87901"
  and .swap_id == "m7zecfirstrefund8981e32a-swap"
  and .guest.elf_sha256 == "ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee"
  and .guest.image_id == "b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"
  and .deployment.lez_source_commit == "a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
  and .deployment.chain_id == "b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"
  and .deployment.genesis_block_hash == "99dbead360267a60d233a07a11fc347a067a1c9ffbd5b96397fc882e82d642e4"
  and .deployment.exact_elf_pre_window_occurrences == 0
  and .deployment.exact_elf_post_window_occurrences == 1
  and .deployment.canonical_window_occurrences == 1
  and .deployment.send_attempts == 1
  and .deployment.bedrock_status == "Finalized"
  and .onboarding.roles == ["maker", "taker"]
  and .onboarding.fresh_role_identities == true
  and .onboarding.finalized_claims == 2
  and .onboarding.submission_count == 2
  and .onboarding.automatic_submission_retry == false
  and .onboarding.swap_effects_started == false
  and .zcash.version == "5.2.0"
  and .zcash.network == "Regtest"
  and .zcash.peer_count == 0
  and .zcash.initial_tip == 104
  and .zcash.funding_confirmation_tip == 106
  and .zcash.signed_refund_height == 109
  and .zcash.eligible_tip == 109
  and .zcash.final_tip == 110
  and .zcash.funding_transaction_id == "f4111a1f7fb614ac4e7d760e9eccb39b7d5a6ba1fd7db1aa25da555172888db0"
  and .zcash.refund_transaction_id == "db066a94221e19dd4de8dd0de1377f51c394fa43d915991e0ce76255715ab470"
  and .zcash.refund_block_hash == "b5645d497f67d39e510d081d66472ac830e90aa15bf3270cfcbfdc298740f55e"
  and .zcash.refund_block_height == 110
  and .zcash.refund_occurrences == 1
  and .zcash.final_mempool_empty == true
  and .deadline.protocol_deadline_changed == false
  and .deadline.wall_clock_sleep_used == false
  and .absence.maker_supervisor_absent_before_effects == true
  and .absence.maker_effect_authority == "absent"
  and .absence.maker_observer_effect_authority == false
  and .absence.stable_owner_service_samples == 2
  and .absence.sample_state == "refund_available"
  and .absence.sample_generation == 1
  and .absence.maker_second_lock_submitted == false
  and ([.absence.maker_lez_submissions_before, .absence.maker_lez_submissions_after,
        .absence.taker_lez_submissions_before, .absence.taker_lez_submissions_after] | all(. == 0))
  and .application.direction == "taker_sells_foreign"
  and .application.zcash_funder == "taker"
  and .application.zcash_refunder == "taker"
  and .application.lez_refunder == null
  and .application.terminal_action_authority == "owner_taker_service"
  and .application.direct_taker_terminal_effects == false
  and .application.concurrent_direct_maker_effects == false
  and .application.transports_absent_through_terminal_state == true
  and .application.fresh_operator_restart_reports_terminal == true
  and .application.same_run_drive_retries == 0
  and .terminal.maker_phase == "refunded"
  and .terminal.maker_revision == 2
  and .terminal.taker_phase == "refunded"
  and .terminal.taker_revision == 2
  and .terminal.operator_history_phase == "refunded"
  and .terminal.operator_status_phase == "refunded"
  and .replay.was_replay == true
  and .replay.terminal_revision == 2
  and .replay.zebra_tip_before == 110
  and .replay.zebra_tip_after == 110
  and .replay.zebra_mempool_before == []
  and .replay.zebra_mempool_after == []
  and .replay.new_chain_effect == false
  and .ordering == [
    "zcash_funded_and_confirmed",
    "maker_lez_second_lock_absent",
    "zcash_first_lock_refund_submitted_and_confirmed"
  ]
  and .atomicity.proof_scope == "reverse_zec_first_lock_refund_after_maker_absence"
  and .atomicity.signed_deadline_preserved == true
  and .atomicity.only_funded_leg_was_zcash == true
  and .atomicity.maker_lez_effect_absent == true
  and .atomicity.refund_owner_was_original_zcash_funder == true
  and .atomicity.canonical_refund_confirmed_once == true
  and .atomicity.both_roles_terminal == true
  and .atomicity.double_collection_observed == false
  and .cleanup.result == "passed"
  and .cleanup.source_exit_status == 0
  and .cleanup.application_processes_absent == true
  and .cleanup.sidecar_processes_absent == true
  and .cleanup.exact_zebra_resources_absent == true
  and .cleanup.shared_effect_free_lez_stack_retained == true
  and .cleanup.private_run_material_removed == true
  and .cleanup.foreign_resources_targeted == false
  and .cleanup.broad_cleanup_used == false
  and .runtime_external_resources == []
  and .public_rpc_used == false
  and .faucet_used == false
  and .public_funds_used == false
  and .public_deployment == false
  and ([.evidence_sha256[] | test("^[0-9a-f]{64}$")] | all)
' "$certificate" >/dev/null || fail "certificate invariants are incomplete or inconsistent"

if rg -q -e '"(exact_bytes|private_key|secret|capability|credential|rpc_url|proof_path|binary_path|filesystem_identity|process_id|pid|start_ticks|socket|password|username)"' "$certificate"; then
  fail "certificate exposes a private or run-local field"
fi

rg -Fq './scripts/test-m7-zec-first-lock-refund-actual-certificate.sh' scripts/run-ci-quality-gates.sh ||
  fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-zec-first-lock-refund-actual-certificate.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI policy does not pin the certificate contract"

echo "M7 actual reverse-ZEC first-lock refund certificate test passed"
