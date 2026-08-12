#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-actual-zec-application-reorg-297f09a-20260812.json"

fail() {
  echo "M7 actual ZEC application reorg certificate test failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

[[ -f "$certificate" && ! -L "$certificate" ]] ||
  fail "checked certificate is missing or unsafe"

jq -e '
  .schema_version == 1
  and .kind == "m7_actual_zec_application_reorg_continuation"
  and .result == "passed"
  and .run_id == "m7appreorg297f09aa"
  and .repository_commit == "297f09a43895d8970cd2d686290ee6f92484760b"
  and .source_evidence_sha256 == "dc79c28a432c443dd5a841f45a9ec52ab3e28a99ef34026d9e81754913b9bfd4"
  and .zebra.version == "5.2.0"
  and .zebra.network == "Regtest"
  and .zebra.node_count == 2
  and .zebra.initial_inclusion_height == 105
  and .zebra.replacement_tip_before_remine == 106
  and .zebra.remined_height == 107
  and (.application.transaction_id | test("^[0-9a-f]{64}$"))
  and .application.funding_removed == true
  and .application.mempool_survived_detach == false
  and .application.rebroadcast_used == true
  and .application.exact_transaction_reused == true
  and .application.funding_remined == true
  and .application.swap_resumed == true
  and .application.phase_after_removal == "offered"
  and .application.phase_after_restore == "taker_lock_confirmed"
  and .application.removal_revision == 2
  and .application.restored_revision == 3
  and .application.journal_events == 3
  and .application.restart_replay_was_replay == true
  and .application.restart_replay_appended_event == false
  and .application.automatic_submission_retry == false
  and .atomicity.proof_scope == "pre_dependent_zec_funding_reorg_and_exact_reappearance"
  and .atomicity.observation_and_projection_committed_together == true
  and .atomicity.revision_sequence == [1, 2, 3]
  and .atomicity.different_funding_identity_accepted == false
  and .atomicity.dependent_maker_lock_existed == false
  and .atomicity.duplicate_journal_event_observed == false
  and .atomicity.future_reorganization_immunity_claimed == false
  and .cleanup.result == "passed"
  and .cleanup.source_exit_status == 0
  and .cleanup.exact_containers_absent == true
  and .cleanup.exact_network_absent == true
  and .cleanup.exact_image_absent == true
  and .cleanup.exact_processes_absent == true
  and .cleanup.private_run_material_removed == true
  and .cleanup.foreign_resources_targeted == false
  and .cleanup.broad_cleanup_used == false
  and .runtime_external_resources == []
  and .public_rpc_used == false
  and .faucet_used == false
  and .public_funds_used == false
  and .public_deployment == false
' "$certificate" >/dev/null || fail "certificate invariants are incomplete or inconsistent"

if rg -q -e '"(exact_bytes|private_key|secret|capability|credential|rpc_url|proof_path|binary_path|filesystem_identity|process_id|pid|start_ticks|socket|password|username)"' "$certificate"; then
  fail "certificate exposes a private or run-local field"
fi

rg -Fq './scripts/test-m7-zebra-application-reorg-actual-certificate.sh' scripts/run-ci-quality-gates.sh ||
  fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-zebra-application-reorg-actual-certificate.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI policy does not pin the certificate contract"

echo "M7 actual ZEC application reorg certificate test passed"
