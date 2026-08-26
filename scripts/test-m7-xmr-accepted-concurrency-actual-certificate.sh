#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-actual-xmr-accepted-concurrency-d8efb7c-20260812.json"

fail() {
  echo "M7 actual XMR accepted-concurrency certificate test failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

[[ -f "$certificate" && ! -L "$certificate" ]] || fail "checked certificate is missing or unsafe"

jq -e '
  .schema_version == 1
  and .kind == "m7_actual_xmr_accepted_concurrency_poc"
  and .result == "passed"
  and .run_id == "m7xmrconc-d8efb7ca"
  and .repository_commit == "d8efb7c68fdd95932dcbea3a9f84524e069e2546"
  and (.artifact.phase_ledger_sha256 | test("^[0-9a-f]{64}$"))
  and .artifact.repository_clean_exact_head == true
  and .artifact.origin_main_equals_head == true
  and .application == {
    pair:"monero",
    direction:"taker_sells_lez",
    journey:"claim",
    accepted_swap_count:2,
    shared_daemon:true,
    shared_database:true,
    shared_delivery_directory:true,
    post_acceptance_restart:true,
    no_acceptance_replay:true
  }
  and .services.monero == {
    version:"0.18.5.1-release",
    network:"isolated_regtest",
    daemon_count:1,
    public_peers:0,
    genesis_hash:"418015bb9ae982a1975da7d79277c2705727a56894ba0fb246adaabb1f4632e3"
  }
  and .services.lez.version == "v0.2.0"
  and .services.lez.network == "private_local"
  and .services.lez.stack_count == 1
  and .services.lez.guest_sha256 == "ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee"
  and .services.lez.program_id == "b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"
  and (.swaps | length) == 2
  and ([.swaps[].swap_id] | unique | length) == 2
  and ([.swaps[].agreement_commitment] | unique | length) == 2
  and ([.swaps[].taker_identity] | sort) == ["taker-a","taker-b"]
  and ([.swaps[].lez_claim_transaction_id] | unique | length) == 2
  and ([.swaps[].monero_funding_transaction_id] | unique | length) == 2
  and all(.swaps[];
    (.swap_id | test("^[0-9a-f]{64}$"))
    and (.agreement_commitment | test("^[0-9a-f]{64}$"))
    and (.lez_claim_transaction_id | test("^[0-9a-f]{64}$"))
    and .lez_claim_height > 0
    and .lez_metadata_state == "claimed"
    and .lez_custody_balance == "0"
    and (.monero_funding_transaction_id | test("^[0-9a-f]{64}$"))
    and .monero_funding_confirmations == 10
    and .monero_sweep_finalized == true
    and .terminal_replay == true)
  and .concurrency.both_applications_accepted_before_actor_activation == true
  and .concurrency.both_swaps_in_flight_before_settlement == true
  and .concurrency.distinct_swap_ids == true
  and .concurrency.distinct_agreements == true
  and .concurrency.distinct_actor_stores == true
  and .concurrency.distinct_role_journals == true
  and .concurrency.distinct_taker_lez_identities == true
  and .concurrency.distinct_monero_outputs == true
  and .concurrency.distinct_lez_escrows == true
  and .concurrency.terminal_resubmission_count == 0
  and .atomicity.proof_scope == "two_independent_successful_xmr_claim_paths"
  and .atomicity.each_swap_conditionally_atomic == true
  and .atomicity.two_swaps_atomic_with_each_other == false
  and .atomicity.distributed_cross_chain_transaction_claimed == false
  and .atomicity.future_reorganization_immunity_claimed == false
  and (.atomicity.argument | length) > 400
  and .runtime_external_resources.public_rpc_used == false
  and .runtime_external_resources.public_peer_used == false
  and .runtime_external_resources.faucet_used == false
  and .runtime_external_resources.public_funds_used == false
  and .runtime_external_resources.public_deployment_used == false
  and .runtime_external_resources.certification_success_depends_on_external_network == false
  and .cleanup.result == "passed"
  and .cleanup.source_exit_status == 0
  and .cleanup.all_exact_run_resources_absent == true
  and .cleanup.sidecar_processes_absent == true
  and .cleanup.sidecar_ports_closed == true
  and .cleanup.foreign_sentinel_survived_exact_cleanup == true
  and .cleanup.broad_cleanup_used == false
  and .cleanup.foreign_resources_targeted == false
' "$certificate" >/dev/null || fail "certificate invariants are incomplete or inconsistent"

rg -Fq './scripts/test-m7-xmr-accepted-concurrency-actual-certificate.sh' \
  scripts/run-ci-quality-gates.sh || fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-xmr-accepted-concurrency-actual-certificate.sh' \
  scripts/test-ci-hardening-policy.sh || fail "CI hardening does not pin the certificate contract"

echo "M7 actual XMR accepted-concurrency certificate test passed"
