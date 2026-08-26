#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-actual-zebra-competing-fork-087c37f-20260812.json"

fail() {
  echo "M7 actual Zebra competing-fork certificate test failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

[[ -f "$certificate" && ! -L "$certificate" ]] ||
  fail "checked certificate is missing or unsafe"

jq -e '
  .schema_version == 1
  and .kind == "m7_actual_zebra_competing_fork"
  and .result == "passed"
  and .run_id == "m7reorg087c37fa"
  and .repository_commit == "087c37fc68b5863814f5a8c8edb8b69ec9e2d79d"
  and .source_evidence_sha256 == "895d403a0607905a9318b0d1e1f42eba3865c13c2a1579360dbaa18df0db3343"
  and .zebra.version == "5.2.0"
  and .zebra.image == "docker.io/zfnd/zebra:5.2.0@sha256:477e65add4dacf52074ba04da8d763c89c26cc57f911dba2127401f8e1da597d"
  and .zebra.network == "Regtest"
  and .zebra.node_count == 2
  and .zebra.common_height == 116
  and .zebra.automatic_submission_retry == false
  and .old_branch.block_count == 3
  and .old_branch.first_height == 117
  and .old_branch.tip_height == 119
  and .replacement_branch.block_count == 4
  and .replacement_branch.first_height == 117
  and .replacement_branch.tip_height == 120
  and ([.old_branch.first_hash, .old_branch.tip_hash,
        .replacement_branch.first_hash, .replacement_branch.tip_hash,
        .transactions.detached_claim,
        .transactions.canonical_conflicting_refund,
        .transactions.canonical_shared_refund] | all(test("^[0-9a-f]{64}$")))
  and .transactions.detached_claim_lookup == "indexed_detached"
  and .outcome.old_branch_detached == true
  and .outcome.detached_claim_is_not_active == true
  and .outcome.replacement_branch_canonical == true
  and .outcome.shared_refund_survived_reorg == true
  and .outcome.conflicting_refund_replaced_claim == true
  and .restart_prerequisite.canonical_funding_requeried_after_real_removal == true
  and .competing_fork_test.raw_blocks_relayed_over_loopback_rpc == true
  and .atomicity.proof_scope == "zcash_htlc_canonical_outcome_under_competing_fork_replacement"
  and .atomicity.claim_and_conflicting_refund_coexisted_on_canonical_chain == false
  and .atomicity.independent_refund_remained_canonical == true
  and .atomicity.automatic_resubmission_observed == false
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

rg -Fq './scripts/test-m7-zebra-reorg-actual-certificate.sh' scripts/run-ci-quality-gates.sh ||
  fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-zebra-reorg-actual-certificate.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI policy does not pin the certificate contract"

echo "M7 actual Zebra competing-fork certificate test passed"
