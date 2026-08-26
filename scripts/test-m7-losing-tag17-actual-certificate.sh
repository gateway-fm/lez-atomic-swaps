#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
readonly certificate="docs/evidence/m7-actual-losing-tag17-63a9496-20260807.json"
fail() { echo "M7 losing-Tag17 certificate test failed: $*" >&2; exit 1; }

[[ -f "$certificate" && ! -L "$certificate" ]] || fail "checked certificate is missing or unsafe"
jq -e '
  .schema_version==1 and .kind=="m7_actual_local_losing_tag17_after_tag16"
  and .result=="passed" and .run_id=="m7lose17-63a9496-b"
  and .repository_commit=="63a9496e472b216e69c05125855ec370644ea0bc"
  and .swap_id=="bd42c86043aeff155bbd5dfe310247578d0fc7a8e4609604bf81883aec6a0f38"
  and .agreement_commitment=="cf9793eb18c0ca1446126061dfd758d5a864a79fded3510eca775f38c5eb0db2"
  and .activation_commitment=="989490740e0e7ca7c96d9e98bcbe0effad14f6fd3b4e1488dee71dde6cd7c6f9"
  and .guest.recursive_tests_passed==5 and .guest.recursive_tests_total==5
  and .guest.elf_sha256=="ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee"
  and .guest.image_id=="b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"
  and .deployment.lez_source_commit=="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
  and .deployment.chain_id=="b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"
  and .deployment.genesis_block_hash=="6efb6b1056e855e0f458055f18f0f40f0a9efe0d12774b2d4b62d7f9d9526ab6"
  and .deployment.exact_elf_pre_window_occurrences==0
  and .deployment.exact_elf_post_window_occurrences==1
  and .deployment.canonical_window_occurrences==1 and .deployment.send_attempts==1
  and .deployment.bedrock_status=="Finalized"
  and .monero.version=="0.18.5.1" and .monero.network=="Regtest"
  and .monero.transaction_id=="0c940c930a362b37d2f2797ec8b6ef6589eba157eece0aaa3cf650e167fcdf8c"
  and .monero.confirmations==10 and .monero.peer_count==0
  and .ordering.tag17_prepared_phase_index==35 and .ordering.tag16_submitted_phase_index==39
  and .ordering.tag16_finalized_phase_index==41 and .ordering.late_tag17_phase_index==42
  and .ordering.tag17_prepared_before_tag16 and .ordering.tag16_finalized_before_late_tag17
  and .tag17.transaction_id=="324f6a8319d8abf6b897aef7848ee12b6d3ee22d79f1de416a5d19ed74d271c7"
  and .tag17.transport_admission=="accepted" and .tag17.process_exit_status==0
  and .tag17.submission_request_id==.tag17.transaction_id
  and .tag17.submission_outcome=="accepted" and .tag17.automatic_retry==false
  and .tag16.transaction_id=="170239738630c006c4df9ed5e2958c72f6d133ac2e79a26ef7a24a0535a63aa5"
  and .tag16.effect=="refund" and .tag16.terminal_state=="refunded"
  and .tag16.terminal_custody_balance=="0"
  and .tag16.original_facts_sha256==.tag16.reobserved_facts_sha256
  and .tag16.facts_reobserved_equal==true and .tag16.reobservation_attempts==2
  and .finalized_exclusion.pre_attempt_clock.height==218
  and .finalized_exclusion.post_attempt_clock.height==218
  and .finalized_exclusion.scan_start_height==219
  and .finalized_exclusion.scan_blocks==8
  and .finalized_exclusion.finalized_clock.height==226
  and .finalized_exclusion.status=="absent"
  and .finalized_exclusion.terminal_rule=="punish_absent_only_when_refunded_zero_at_candidate_and_window_end"
  and .atomicity.proof_scope=="tag16_wins_over_late_tag17_in_finalized_window"
  and .atomicity.losing_punish_excluded and .atomicity.complete_attempt_interval_covered
  and (.atomicity.distributed_cross_chain_transaction_claimed|not)
  and (.atomicity.future_reorg_immunity_claimed|not)
  and .cleanup.result=="passed" and .cleanup.source_exit_status==0
  and .cleanup.exact_run_resources_absent and .cleanup.sidecar_processes_absent
  and .cleanup.sidecar_ports_closed and .cleanup.foreign_sentinel_survived
  and (.cleanup.foreign_resources_targeted|not) and (.cleanup.broad_cleanup_used|not)
  and .runtime_external_resources==[] and (.public_rpc_used|not)
  and (.faucet_used|not) and (.public_funds_used|not) and (.public_deployment|not)
  and ([.evidence_sha256[]|test("^[0-9a-f]{64}$")]|all)
' "$certificate" >/dev/null || fail "certificate invariants are incomplete or inconsistent"

rg -q -e '"(exact_bytes|private_key|secret|capability|credential|rpc_url|proof_path|binary_path|destination_address|shared_address)"' "$certificate" &&
  fail "certificate exposes a private or run-local field"
rg -Fq './scripts/test-m7-losing-tag17-actual-certificate.sh' scripts/run-ci-quality-gates.sh || fail "quality runner wiring is missing"
rg -Fq './scripts/test-m7-losing-tag17-actual-certificate.sh' scripts/test-ci-hardening-policy.sh || fail "CI policy wiring is missing"
echo "M7 losing-Tag17 certificate test passed"
