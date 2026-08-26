#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-actual-tag17-a23a314-20260804.json"

fail() {
  echo "M7 actual Tag-17 certificate test failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

[[ -f "$certificate" && ! -L "$certificate" ]] || fail "checked certificate is missing or unsafe"

jq -e '
  .schema_version == 1
  and .kind == "m7_actual_local_tag17_poc"
  and .result == "passed"
  and .run_id == "m7tag17a23a314a"
  and .repository_commit == "a23a314cfa71c82b0272a04f97fd6e60510510c0"
  and .guest.recursive_tests_passed == 5
  and .guest.recursive_tests_total == 5
  and .guest.elf_sha256 == "ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee"
  and .guest.image_id == "b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"
  and .deployment.program_id_is_current_guest_image_id == true
  and .deployment.canonical_window_occurrences == 1
  and .deployment.send_attempts == 1
  and .deployment.bedrock_status == "Finalized"
  and .tag17.prepared_before_submission == true
  and .tag17.preboundary_outcome == "uncertain"
  and .tag17.preboundary_finalized_timestamp_ms < .tag17.punish_at_ms
  and .tag17.release_request_equals_transaction_id == true
  and .tag17.release_outcome == "accepted"
  and .tag17.release_performed == true
  and .tag17.automatic_retry == false
  and .tag17.finalized_transaction_timestamp_ms >= .tag17.punish_at_ms
  and .tag17.boundary_margin_ms == (.tag17.finalized_transaction_timestamp_ms - .tag17.punish_at_ms)
  and .tag17.finality_page_blocks == 8
  and .tag17.finality_page_is_pagination_not_confirmation_depth == true
  and .tag17.effect == "punish"
  and .tag17.claimant_only_signature == true
  and .tag17.aggregate_signature_absent == true
  and .tag17.terminal_metadata_state == "claimed"
  and .tag17.terminal_custody_balance == "0"
  and .tag17.maker_taker_finalized_facts_equal == true
  and .monero.version == "0.18.5.1"
  and .monero.network == "Regtest"
  and .monero.role == "agreement_identity_only"
  and .monero.funding_or_spend_effect == false
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

if rg -q -e '"(exact_bytes|private_key|secret|capability|credential|rpc_url|proof_path|binary_path)"' "$certificate"; then
  fail "certificate exposes a private or run-local field"
fi

rg -Fq './scripts/test-m7-tag17-actual-certificate.sh' scripts/run-ci-quality-gates.sh ||
  fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-tag17-actual-certificate.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI hardening does not pin the certificate contract"

echo "M7 actual Tag-17 certificate test passed"
