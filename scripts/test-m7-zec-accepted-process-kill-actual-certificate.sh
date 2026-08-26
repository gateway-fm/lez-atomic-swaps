#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-actual-zec-accepted-process-kill-820001b-20260811.json"

fail() {
  echo "M7 actual accepted-ZEC process-kill certificate test failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

[[ -f "$certificate" && ! -L "$certificate" ]] ||
  fail "checked certificate is missing or unsafe"

jq -e '
  .schema_version == 1
  and .kind == "m7_actual_local_accepted_zec_process_kill"
  and .result == "passed"
  and .run_id == "m7zecpk820001ba"
  and .repository_commit == "820001b0813b30568267725a6913c0d8b4cd5351"
  and .swap_id == "m7zecpk820001ba-swap"
  and .guest.recursive_tests_passed == 5
  and .guest.recursive_tests_total == 5
  and .guest.elf_sha256 == "ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee"
  and .guest.image_id == "b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"
  and .deployment.lez_source_commit == "a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
  and .deployment.chain_id == "b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"
  and .deployment.genesis_block_hash == "56799436438b5e984ba095983312c86184144910b5e84fb29f95c1f9987602cd"
  and .deployment.exact_elf_pre_window_occurrences == 0
  and .deployment.exact_elf_post_window_occurrences == 1
  and .deployment.canonical_window_occurrences == 1
  and .deployment.send_attempts == 1
  and .deployment.bedrock_status == "Finalized"
  and .onboarding.roles == ["maker", "taker"]
  and .onboarding.fresh_role_identities == true
  and .onboarding.finalized_claims == 2
  and .onboarding.swap_effects_started == false
  and .zcash.version == "5.2.0"
  and .zcash.network == "Regtest"
  and .zcash.peer_count == 0
  and .zcash.initial_tip == 104
  and .zcash.crash_boundary_tip == 104
  and .zcash.restart_tip == 104
  and .zcash.final_tip == 107
  and .zcash.funding_transaction_id == "f4111a1f7fb614ac4e7d760e9eccb39b7d5a6ba1fd7db1aa25da555172888db0"
  and .zcash.funding_transaction_stayed_single == true
  and .zcash.final_mempool_empty == true
  and .process_kill.crash_boundary == "zcash_fund_submitted_before_actor_stdout"
  and .process_kill.kill_order == "daemon_then_actor"
  and .process_kill.crashed_generation == 15
  and .process_kill.recovered_generation == 16
  and .process_kill.recovered_generation > .process_kill.crashed_generation
  and .process_kill.terminal_generation == 27
  and .process_kill.abandoned_generation_transferred == true
  and .process_kill.confirmations_mined_before_restart == 0
  and .process_kill.mempool_identity_preserved == true
  and .process_kill.tip_unchanged == true
  and .process_kill.old_process_identities_absent == true
  and .process_kill.automatic_resubmission_observed == false
  and .process_kill.production_binary_exposes_crash_hook == false
  and .process_kill.marker_wait_events == 30
  and .process_kill.marker_wait_status_polls == 0
  and .application.direction == "taker_sells_lez"
  and .application.maker_role == "daemon_supervisor"
  and .application.taker_role == "receipt_bound_cli"
  and .application.concurrent_direct_maker_effects == false
  and .application.direct_taker_claim_effects == false
  and .application.transports_absent_after_first_lock == true
  and .application.fresh_operator_replay_completed == true
  and .terminal.maker_phase == "completed"
  and .terminal.taker_phase == "completed"
  and .terminal.scheduler_state == "terminal"
  and .terminal.child_identity_absent == true
  and .ordering == [
    "zcash_funded_and_confirmed",
    "lez_revealing_claim_submitted",
    "zcash_followup_claim_submitted_and_confirmed"
  ]
  and .atomicity.proof_scope == "successful_claim_after_accepted_zec_process_kill"
  and .atomicity.zcash_funding_confirmed_before_lez_reveal == true
  and .atomicity.lez_reveal_precedes_zcash_claim == true
  and .atomicity.both_roles_terminal == true
  and .atomicity.double_collection_observed == false
  and .cleanup.result == "passed"
  and .cleanup.source_exit_status == 0
  and .cleanup.application_processes_absent == true
  and .cleanup.sidecar_processes_absent == true
  and .cleanup.exact_chain_resources_absent == true
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

rg -Fq './scripts/test-m7-zec-accepted-process-kill-actual-certificate.sh' scripts/run-ci-quality-gates.sh ||
  fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-zec-accepted-process-kill-actual-certificate.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI hardening does not pin the certificate contract"

echo "M7 actual accepted-ZEC process-kill certificate test passed"
