#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly runner="scripts/run-m4-actual-claim-poc.sh"
readonly evidence_contract="scripts/jq/m4-actual-claim-evidence-contract.jq"

fail() {
  echo "M4 actual-claim runner contract test failed: $*" >&2
  exit 1
}

for command_name in bash jq mktemp rg rm sed; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done
[[ -f "$evidence_contract" && ! -L "$evidence_contract" ]] ||
  fail "evidence contract is missing or unsafe"

test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT
readonly test_root
valid_evidence="$test_root/valid-evidence.json"

jq -n '
  def h($character; $width): $character * $width;
  def effect($order; $chain; $name; $role; $tx):
    {order:$order,chain:$chain,effect:$name,role:$role,transaction_id:$tx,
     automatic_submission_retry:false};
  {
    schema:"lez-atomic-swaps-m4-actual-local-claim-poc",version:2,
    result:"passed_exact_committed_tree_replay",milestone:"M4",
    certification_status:"exact_committed_tree_replay_passed",
    m4_complete_tag_authorized:false,
    source_binding:{commit:h("a";40),expected_commit:h("a";40),clean_before:true,
      clean_after:true,exact_committed_tree_replay_completed:true,
      binary_sha256:{artifact:h("1";64),deployer:h("2";64),stage_a:h("3";64),
        fund:h("4";64),verify:h("5";64),release:h("6";64),classifier:h("7";64),
        tag15:h("8";64),sweep:h("9";64)}},
    agreement:{taker_claim_partial_committed_before_effects:true,
      taker_claim_partial_withheld_until_confirmed_monero_funding:true},
    ordered_effects:[
      effect(1;"lez";"initialize_native_xmr";"taker";h("a";64)) + {finalized_height:10},
      effect(2;"lez";"fund_native";"taker";h("b";64)) + {finalized_height:20},
      effect(3;"monero";"fund_stage_a_shared_address";"maker_funding_boundary";h("c";64))
        + {confirmations:10,required_confirmations:10},
      effect(4;"lez";"authorize_native_xmr_claim";"taker_release_worker";h("d";64))
        + {finalized_height:30},
      effect(5;"lez";"claim_native_xmr";"maker";h("e";64)) + {finalized_height:40},
      effect(6;"monero";"reconstructed_spend_key_sweep";"taker";h("f";64))
        + {confirmations:10,required_confirmations:10,funded_amount_piconero:1000,
           received_amount_piconero:900,fee_piconero:100}
    ],
    role_and_atomicity_evidence:{maker_consumed_canonical_finalized_tag14:true,
      tag15_finalized_signature_matched_maker_packet:true,
      maker_adaptor_share_extracted_only_after_finalized_tag15:true,
      successful_claim_branch_conditionally_atomic:true,
      distributed_cross_chain_transaction_claimed:false},
    resource_and_secret_boundary:{runtime_external_resources:[],public_rpc_used:false,
      peer_used:false,faucet_used:false,public_funds_used:false,
      external_finality_service_used:false,credentials_or_private_keys_in_packet:false,
      private_paths_in_packet:false,extracted_scalar_or_hash_in_packet:false},
    cleanup:{result:"passed",exact_run_resources_absent:true,sidecar_processes_absent:true,
      sidecar_ports_closed:true,foreign_sentinel_survived_exact_cleanup:true,
      foreign_resources_targeted:false,broad_cleanup_used:false}
  }
' >"$valid_evidence"

jq -e -f "$evidence_contract" "$valid_evidence" >/dev/null ||
  fail "valid exact-commit evidence fixture was rejected"

for mutation in wrong_order scalar_leak cleanup_incomplete public_rpc; do
  mutated="$test_root/${mutation}.json"
  case "$mutation" in
    wrong_order) jq '.ordered_effects[4].order = 4' "$valid_evidence" >"$mutated" ;;
    scalar_leak) jq '.resource_and_secret_boundary.extracted_scalar_or_hash_in_packet = true' \
      "$valid_evidence" >"$mutated" ;;
    cleanup_incomplete) jq '.cleanup.exact_run_resources_absent = false' \
      "$valid_evidence" >"$mutated" ;;
    public_rpc) jq '.resource_and_secret_boundary.public_rpc_used = true' \
      "$valid_evidence" >"$mutated" ;;
  esac
  if jq -e -f "$evidence_contract" "$mutated" >/dev/null; then
    fail "unsafe ${mutation} evidence fixture was accepted"
  fi
done

[[ -x "$runner" ]] || fail "runner is missing or not executable: ${runner}"
bash -n "$runner" || fail "runner shell syntax is invalid"

contract="$($runner contract)"
jq -e '
  .schema_version == 1
  and .kind == "m4_actual_claim_poc_contract"
  and .milestone == "M4"
  and .source_binding == "clean_expected_commit"
  and .protocol_run_id_equals_lez_run_id == true
  and .monero_child_run_suffix == "-xmr"
  and .all_effect_outputs_create_new == true
  and .automatic_submission_retry == false
  and .dynamic_literal_loopback_ports == true
  and .public_runtime_resources == []
  and .implemented_execute_through == "actor_onboarding"
  and .actor_onboarding_implemented == true
  and .successful_claim_tail_implemented == false
  and .monero_launcher_implemented == true
  and .monero_launcher_reachable_in_execute == false
  and .monero_launcher_executed_in_certifying_replay == false
  and .role_sidecar_launcher_contract_green == true
  and .role_sidecar_launcher_reachable_in_execute == false
  and .agreement_helper_contract_green == true
  and .agreement_helper_implemented_through == "countersigned_stage_b"
  and .agreement_helper_submission_performed == false
  and .agreement_helper_reachable_in_execute == false
  and .available_unwired_launchers == [
    "run-m4-lez-sidecar.sh", "run-m4-xmr-agreement.sh"
  ]
  and .monero_owned_volume_count == 4
  and .cleanup.exact_resource_ledger == true
  and .cleanup.pid_start_time_binary_binding == true
  and .cleanup.foreign_sentinel_required == true
  and .cleanup.exact_monero_volume_capture == true
  and .cleanup.broad_cleanup_forbidden == true
  and .phases == ["preflight","build","identity","lez_stack","deployment",
    "actor_onboarding","monero_stack","agreement","journals","tag13",
    "monero_funding","sidecars","release","tag14_finality","tag15",
    "tag15_finality","extraction","monero_sweep","evidence","cleanup"]
' <<<"$contract" >/dev/null || fail "runner does not expose the required phase/safety contract"

for required in \
  run-m4-lez-artifact-tests.sh run-lez-v02-stack.sh \
  run-m4-lez-local-deployment.sh run-m4-lez-actor-onboarding.sh \
  run-m4-lez-sidecar.sh run-m4-xmr-agreement.sh run-monero-e2e.sh lez-v02-vault-claim-poc \
  lez-v02-xmr-stage-a-compose lez-v02-xmr-stage-a-poc \
  lez-v02-xmr-regtest-fund lez-v02-xmr-regtest-verify \
  lez-v02-xmr-release-prepare lez-v0-2-xmr-release-service \
  lez-v02-xmr-classify-finalized xmr-reference-tag15 \
  lez-adaptor-role-runner lez-v02-xmr-regtest-sweep \
  bind-finalized-claim-sweep M4_EXPECTED_COMMIT MONERO_RUN_ID; do
  rg -Fq -- "$required" "$runner" || fail "runner omits required boundary: ${required}"
done
for ownership_boundary in \
  'docker volume ls --quiet' \
  'label=org.logos-co.atomic-swaps.run=${MONERO_RUN_ID}' \
  'record_resource volume' \
  'process_is_owned "$identity" "$start_ticks" "$binary_sha256"'; do
  rg -Fq -- "$ownership_boundary" "$runner" ||
    fail "runner omits exact ownership boundary: ${ownership_boundary}"
done

monero_capture_source="$(sed -n '/^capture_monero_resources() {$/,/^}$/p' "$runner")"
volume_record_line="$(rg -n -F 'record_resource volume' <<<"$monero_capture_source")"
container_record_line="$(rg -n -F 'record_resource container' <<<"$monero_capture_source")"
volume_record_line="${volume_record_line%%:*}"
container_record_line="${container_record_line%%:*}"
[[ "$volume_record_line" =~ ^[0-9]+$ && "$container_record_line" =~ ^[0-9]+$ ]] ||
  fail "Monero ledger ordering lines are unavailable"
(( volume_record_line < container_record_line )) ||
  fail "Monero volumes must be recorded before containers for reverse cleanup"

execute_source="$(sed -n '/^execute_run() {$/,/^}$/p' "$runner")"
rg -Fq 'actor_onboarding' <<<"$execute_source" ||
  fail "execute omits actor onboarding"
rg -Fq 'fail "monero_stack phase is not implemented; no Monero or swap effect was started"' \
  <<<"$execute_source" || fail "execute omits the post-onboarding fail-closed boundary"
if rg -Fq 'start_monero_child' <<<"$execute_source"; then
  fail "execute can reach the Monero launcher before that phase is implemented"
fi

for forbidden in \
  'docker system prune' 'docker container prune' 'docker network prune' \
  'docker volume prune' 'docker image prune' 'killall ' 'pkill '; do
  if rg -Fq -- "$forbidden" "$runner"; then
    fail "runner contains forbidden broad cleanup: ${forbidden}"
  fi
done
for retained_example_port in 33145 33146 33147 36967 58993 39185 41189 46769 58393; do
  if rg -n "(^|[^0-9])${retained_example_port}([^0-9]|$)" "$runner" >/dev/null; then
    fail "runner hard-codes retained example port: ${retained_example_port}"
  fi
done

echo "M4 actual-claim runner contract passed"
