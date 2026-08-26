#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-actual-f7-custom-token-refund-062b6ba-20260808.json"

fail() {
  echo "M7 actual F7 custom-token refund certificate test failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

[[ -f "$certificate" && ! -L "$certificate" ]] ||
  fail "checked certificate is missing or unsafe"

jq -e '
  .schema_version == 1
  and .kind == "m7_actual_local_f7_custom_token_refund"
  and .result == "passed"
  and .run_id == "m7f7refund-062b6ba-h"
  and .repository_commit == "062b6ba0db97afddc3cf3d2b4a522089752cb38f"
  and .duration_ms == 3699310
  and .artifact.lez_source_commit == "a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
  and .artifact.guest_elf_sha256 ==
    "bc2ea18eaacb917727934fcf0366dd54c1f9a2b69b61ea53080c926850967fd7"
  and .artifact.program_id ==
    "f3ead24b95d316ce91980cb3531a70b83a27fd1640f47c1b857757aef26c244e"
  and .artifact.deployer_sha256 ==
    "c594ea1ec34fc0227e8e1b6ced9917ad4df5c5e4dfac7616565aae830d3f5cbd"
  and .artifact.independently_hashed == true
  and .deployment.profile == "m3_f7_checked_local"
  and .deployment.submission_count == 1
  and .deployment.maker_vault_claim_count == 1
  and .deployment.taker_vault_claim_count == 1
  and .deployment.both_vault_claims_finalized == true
  and .services.bitcoin == {version:"31.1",network:"regtest"}
  and .services.lez ==
    {version:"v0.2.0",network:"private_local",slot_duration_seconds:"3.0"}
  and (.directions | length) == 2
  and [.directions[].direction] == ["taker_sells_foreign","taker_sells_lez"]
  and all(.directions[];
    .asset.kind == "custom_token"
    and .asset.first_lock_order ==
      ["initialize_witnessed","create_custody_ata","fund"]
    and .expected_unique_effects == {bitcoin:2,lez:4}
    and (.bitcoin_effect_ids | length) == 2
    and (.bitcoin_effect_ids | unique | length) == 2
    and (.lez_effect_ids | length) == 4
    and (.lez_effect_ids | unique | length) == 4
    and .actor_owned_refunds.bitcoin == .bitcoin_effect_ids[1]
    and .actor_owned_refunds.lez == .lez_effect_ids[3]
    and .cooperative_claim_effects_present == false
    and .terminal_roles == {
      maker:{state:"active",phase:"refunded",revision:4,next_action:"complete"},
      taker:{state:"active",phase:"refunded",revision:4,next_action:"complete"}}
    and .lez_refund.deadline_satisfied == true
    and .lez_refund.containing_timestamp_ms >= .lez_refund.signed_refund_at_ms
    and .lez_refund.durable_total_lez_submissions == 4
    and .lez_refund.exact_finalized_projection == true
    and .lez_refund.actor_validated_refunded_metadata_zero_custody_and_immutable_depositor ==
      true
    and .bitcoin_refund.signed_refund_height == .bitcoin_refund.containing_block_height
    and .bitcoin_refund.signed_csv_sequence == 144
    and .bitcoin_refund.exact_countersigned_transaction == true
    and .bitcoin_refund.canonical_three_item_witness == true
    and .terminal_balances.conservation_total == 250
    and .terminal_balances.expected_total == 250
    and .terminal_balances.exact_direction_balances == true
    and .terminal_balances.owner_reader == "official_lez_v0_2_wallet"
    and .terminal_balances.finalized_before_read == true
    and .terminal_balances.wallet_operations_read_only == true
    and .terminal_balances.custody_finalized == true
    and .terminal_balances.custody_metadata_status == "refunded"
    and .terminal_balances.terminal_transfer_atomicity ==
      "single_on_chain_refund_transaction"
    and .replay_resubmission_count == 0)
  and .directions[0].refund_order == ["lez_maker","bitcoin_taker"]
  and .directions[0].actor_owned_refunds == {
    bitcoin:"c287cd2edffeedcb190df6d99dcc1955e8bdff76be38835bb2a87687dca553fe",
    lez:"9f607f1af37ffd97e1f38bda97731fc4e57b2a64a1232023aa638ea83cf195c6"}
  and .directions[0].bitcoin_refund.projected_revision == 4
  and .directions[0].terminal_balances.balances == {maker:250,taker:0,custody:0}
  and .directions[1].refund_order == ["bitcoin_maker","lez_taker"]
  and .directions[1].actor_owned_refunds == {
    bitcoin:"2fbc0c762024402168972ce5387877db7076f5221e137a4de046f5245774805e",
    lez:"71eba17372576554b4a0695e11b2c62f78819a54ca97a7cc453f7cdbe99d710e"}
  and .directions[1].bitcoin_refund.projected_revision == 3
  and .directions[1].terminal_balances.balances == {maker:0,taker:250,custody:0}
  and .atomicity.proof_scope == "two_independent_ordered_refund_directions"
  and .atomicity.forward_finalized_lez_refund_precedes_bitcoin_refund == true
  and .atomicity.reverse_bitcoin_refund_precedes_finalized_lez_refund == true
  and .atomicity.both_principals_returned == true
  and .atomicity.double_collection_observed == false
  and .atomicity.distributed_transaction_claimed == false
  and .atomicity.future_reorganization_immunity_claimed == false
  and .cleanup.result == "passed"
  and .cleanup.all_exact_run_resources_absent == true
  and .cleanup.foreign_resources_targeted == false
  and .cleanup.broad_cleanup_used == false
  and .runtime_external_resources == [{
    name:"Bedrock NTP",
    endpoint:"pool.ntp.org:123/udp",
    attempted:true,
    required:false,
    success_required:false,
    observed_timeout_count:244}]
  and .public_rpc_used == false
  and .faucet_used == false
  and .public_funds_used == false
  and .public_deployment == false
  and ([.evidence_sha256[] | test("^[0-9a-f]{64}$")] | all)
' "$certificate" >/dev/null || fail "certificate invariants are incomplete or inconsistent"

if rg -q -e '"(exact_bytes|private_key|secret|capability|credential|rpc_url|proof_path|binary_path)"' \
  "$certificate"; then
  fail "certificate exposes a private or run-local field"
fi

rg -Fq './scripts/test-m7-f7-custom-token-refund-actual-certificate.sh' \
  scripts/run-ci-quality-gates.sh ||
  fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-f7-custom-token-refund-actual-certificate.sh' \
  scripts/test-ci-hardening-policy.sh ||
  fail "CI hardening does not pin the certificate contract"

echo "M7 actual F7 custom-token refund certificate test passed"
