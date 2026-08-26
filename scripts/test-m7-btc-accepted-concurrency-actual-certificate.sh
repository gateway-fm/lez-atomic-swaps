#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-actual-btc-accepted-concurrency-272788c-20260808.json"

fail() {
  echo "M7 actual BTC accepted-concurrency certificate test failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

[[ -f "$certificate" && ! -L "$certificate" ]] || fail "checked certificate is missing or unsafe"

jq -e '
  .schema_version == 1
  and .kind == "m7_actual_btc_accepted_concurrency_poc"
  and .result == "passed"
  and .run_id == "m7btcconc-272788c-a"
  and .repository_commit == "272788cbc83a8b7e839a4a2be1c1e3b5a0d8cdb4"
  and .artifact.packet_sha256 == "d318f45eaf4beecf341803848ed442930a639555b5dca45f607a94978d985585"
  and .application == {
    pair:"bitcoin",
    directions:["taker_sells_foreign","taker_sells_lez"],
    accepted_swap_count:2,
    shared_daemon:true,
    shared_database:true,
    post_acceptance_restart:true,
    no_acceptance_or_actor_replay:true
  }
  and .services.bitcoin_core == {version:"31.1",network:"regtest",public_peers:0}
  and .services.lez.version == "v0.2.0"
  and .services.lez.network == "private_local"
  and .services.lez.slot_duration_seconds == "1.0"
  and .services.lez.guest_sha256 == "ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee"
  and .services.lez.program_id == "b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"
  and [.directions[].direction] == ["taker_sells_foreign","taker_sells_lez"]
  and all(.directions[];
    .terminal_revision == 4 and .terminal_phase == "completed"
    and .expected_unique_effects == {bitcoin:2,lez:3}
    and .maker_second_lock_effect_count == 1
    and (.stage_two_evidence_sha256 | test("^[0-9a-f]{64}$"))
    and (.actor_timing_sha256 | test("^[0-9a-f]{64}$"))
    and (.actual_effects_sha256 | test("^[0-9a-f]{64}$"))
    and (.bitcoin_effect_ids | length) == 2
    and (.lez_effect_ids | length) == 3
    and ([.bitcoin_effect_ids[],.lez_effect_ids[]] | all(test("^[0-9a-f]{64}$"))))
  and ([.directions[].bitcoin_effect_ids[]] | unique | length) == 4
  and ([.directions[].lez_effect_ids[]] | unique | length) == 6
  and .concurrency.simultaneous_in_flight == true
  and .concurrency.overlap_revision == 2
  and .concurrency.overlap_phase == "both_legs_locked"
  and .concurrency.distinct_funding_outpoints == true
  and .concurrency.distinct_agreements == true
  and .concurrency.distinct_actor_state_dbs == true
  and .concurrency.distinct_signing_journals == true
  and .concurrency.distinct_signer_sessions_per_domain == true
  and .concurrency.distinct_escrows == true
  and .concurrency.distinct_deadlines == true
  and .concurrency.both_swaps_locked_before_settlement == true
  and .replay.command == "drive"
  and .replay.role_count == 4
  and .replay.resubmission_count == 0
  and .atomicity.proof_scope == "two_independent_successful_claim_paths"
  and .atomicity.each_swap_conditionally_atomic == true
  and .atomicity.two_swaps_atomic_with_each_other == false
  and .atomicity.distributed_cross_chain_transaction_claimed == false
  and .atomicity.future_reorg_immunity_claimed == false
  and (.atomicity.argument | length) > 300
  and .runtime_external_resources.public_rpc_used == false
  and .runtime_external_resources.public_peer_used == false
  and .runtime_external_resources.faucet_used == false
  and .runtime_external_resources.public_funds_used == false
  and .runtime_external_resources.public_deployment_used == false
  and .runtime_external_resources.certification_success_depends_on_external_network == false
  and .runtime_external_resources.bedrock_ntp_attempted == true
  and .runtime_external_resources.bedrock_ntp_required == false
  and .cleanup.result == "passed"
  and .cleanup.all_exact_run_resources_absent == true
  and .cleanup.broad_cleanup_used == false
  and .cleanup.foreign_resources_targeted == false
  and .not_proven.arbitrary_n_or_same_direction_scheduler == true
  and .not_proven.process_kill_or_crash_recovery == true
  and .not_proven.public_network_reliability == true
  and .not_proven.future_reorganization_immunity == true
' "$certificate" >/dev/null || fail "certificate invariants are incomplete or inconsistent"

if rg -q -e '"(exact_bytes|private_key|secret|capability|credential|rpc_url|proof_path|binary_path)"' "$certificate"; then
  fail "certificate exposes a private or run-local field"
fi

rg -Fq './scripts/test-m7-btc-accepted-concurrency-actual-certificate.sh' \
  scripts/run-ci-quality-gates.sh || fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-btc-accepted-concurrency-actual-certificate.sh' \
  scripts/test-ci-hardening-policy.sh || fail "CI hardening does not pin the certificate contract"

echo "M7 actual BTC accepted-concurrency certificate test passed"
