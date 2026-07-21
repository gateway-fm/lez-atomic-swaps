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

function_source() {
  local function_name="$1"
  sed -n "/^${function_name}() {$/,/^}$/p" "$runner"
}

for command_name in bash chmod id jq mkdir mktemp rg rm sed stat unlink wc; do
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
  and .implemented_execute_through == "evidence"
  and .actor_onboarding_implemented == true
  and .successful_claim_tail_implemented == false
  and .monero_launcher_implemented == true
  and .monero_launcher_reachable_in_execute == true
  and .monero_launcher_executed_in_certifying_replay == false
  and .monero_funding_implemented == true
  and .monero_funding_reachable_in_execute == true
  and .monero_funding_executed_in_certifying_replay == false
  and .monero_verification_implemented == true
  and .monero_verification_reachable_in_execute == true
  and .monero_verification_executed_in_certifying_replay == false
  and .role_sidecar_launcher_contract_green == true
  and .role_sidecar_launcher_reachable_in_execute == true
  and .agreement_helper_contract_green == true
  and .agreement_helper_implemented_through == "countersigned_stage_b"
  and .agreement_helper_submission_performed == false
  and .agreement_helper_reachable_in_execute == true
  and .journal_phase_started_before_agreement_helper == true
  and .tag13_runner_implemented == true
  and .tag13_runner_reachable_in_execute == true
  and .tag13_runner_executed_in_certifying_replay == false
  and .tag13_handoff_exporter_implemented == true
  and .tag13_handoff_exporter_reachable_in_execute == true
  and .tag13_handoff_exporter_executed_in_certifying_replay == false
  and .available_unwired_launchers == ["run-m4-lez-sidecar.sh"]
  and .composed_launchers == [
    "run-m4-lez-artifact-tests.sh", "run-lez-v02-stack.sh",
    "run-m4-lez-local-deployment.sh", "run-m4-lez-actor-onboarding.sh",
    "run-monero-e2e.sh", "run-m4-xmr-agreement.sh"
  ]
  and .monero_owned_volume_count == 4
  and .cleanup.exact_resource_ledger == true
  and .cleanup.pid_start_time_binary_binding == true
  and .cleanup.foreign_sentinel_required == true
  and .cleanup.exact_monero_volume_capture == true
  and .cleanup.monero_child_preregistered == true
  and .cleanup.monero_child_sentinel_fallback == true
  and .cleanup.ledger_validated_before_cleanup == true
  and .cleanup.sentinel_survival_required_for_pass == true
  and .cleanup.docker_labels_revalidated_before_delete == true
  and .cleanup.tag13_no_retry_latch_before_submission == true
  and .cleanup.broad_cleanup_forbidden == true
  and .phases == ["preflight","build","identity","lez_stack","deployment",
    "actor_onboarding","monero_stack","agreement","journals","tag13", "tag13_handoff",
    "monero_funding","sidecars","release","tag14_finality","tag15",
    "tag15_finality","extraction","monero_sweep","evidence","cleanup"]
' <<<"$contract" >/dev/null || fail "runner does not expose the required phase/safety contract"

ledger_fixture_root="${test_root}/resource-ledger"
mkdir -m 0700 "$ledger_fixture_root"
readonly ledger_fixture_root
readonly ledger_fixture_run_id="fixture-run"
readonly ledger_fixture_monero_run_id="${ledger_fixture_run_id}-xmr"
readonly ledger_fixture_sentinel="lez-atomic-swaps-m4-${ledger_fixture_run_id}-foreign-sentinel"
jq -cn --arg sentinel "$ledger_fixture_sentinel" --arg run_id "$ledger_fixture_run_id" '
  {schema_version:1,kind:"sentinel_network",identity:$sentinel,name:$sentinel,
   start_ticks:null,binary_sha256:null,run_label:$run_id}
' >"${ledger_fixture_root}/valid.jsonl"
jq -s '. + . | .[]' "${ledger_fixture_root}/valid.jsonl" >"${ledger_fixture_root}/duplicate.jsonl"
printf '%s' '{"schema_version":1' >"${ledger_fixture_root}/truncated.jsonl"
: >"${ledger_fixture_root}/empty.jsonl"

if ! M4_LEDGER_FIXTURE_ROOT="$ledger_fixture_root" \
  M4_LEDGER_FIXTURE_RUN_ID="$ledger_fixture_run_id" \
  M4_LEDGER_FIXTURE_MONERO_RUN_ID="$ledger_fixture_monero_run_id" \
  bash -c '
    set -euo pipefail
    fixture_root="$M4_LEDGER_FIXTURE_ROOT"
    fixture_run_id="$M4_LEDGER_FIXTURE_RUN_ID"
    fixture_monero_run_id="$M4_LEDGER_FIXTURE_MONERO_RUN_ID"
    set -- contract
    # Source contract mode to exercise the exact production validator without
    # running preflight, cleanup, Docker, or any protocol effect.
    source scripts/run-m4-actual-claim-poc.sh >/dev/null
    run_id="$fixture_run_id"
    MONERO_RUN_ID="$fixture_monero_run_id"
    resource_ledger="${fixture_root}/valid.jsonl"
    materialize_validated_resource_ledger "${fixture_root}/valid.usv"
    [[ "$(wc -l <"${fixture_root}/valid.usv")" == 1 ]]
    for invalid in duplicate truncated empty; do
      resource_ledger="${fixture_root}/${invalid}.jsonl"
      if materialize_validated_resource_ledger "${fixture_root}/${invalid}.usv" 2>/dev/null; then
        echo "invalid ${invalid} resource ledger was accepted" >&2
        exit 1
      fi
      [[ ! -e "${fixture_root}/${invalid}.usv" ]]
    done
  '; then
  fail "resource-ledger validator did not reject corrupt/ambiguous cleanup input"
fi

for required in \
  run-m4-lez-artifact-tests.sh run-lez-v02-stack.sh \
  run-m4-lez-local-deployment.sh run-m4-lez-actor-onboarding.sh \
  run-m4-lez-sidecar.sh run-m4-xmr-agreement.sh run-monero-e2e.sh lez-v02-vault-claim-poc \
  lez-v02-bridge-poc lez-v02-xmr-tag13-export lez-v02-xmr-stage-a-compose lez-v02-xmr-stage-a-poc \
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
  'docker_resource_run_label_matches' \
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

record_resource_source="$(function_source record_resource)"
[[ -n "$record_resource_source" ]] || fail "resource-ledger writer is unavailable"
for run_label_boundary in \
  'run_label=' \
  '--arg run_label "$run_label"' \
  'run_label:' \
  'else $run_label end'; do
  rg -Fq -- "$run_label_boundary" <<<"$record_resource_source" ||
    fail "resource ledger does not accept and persist expected run label: ${run_label_boundary}"
done

for capture_function in capture_lez_resources capture_monero_resources; do
  capture_source="$(function_source "$capture_function")"
  [[ -n "$capture_source" ]] || fail "${capture_function} is unavailable"
  expected_label='$run_id'
  [[ "$capture_function" == capture_monero_resources ]] && expected_label='$MONERO_RUN_ID'
  for docker_kind in image network container; do
    resource_lines="$(rg -F "record_resource ${docker_kind}" <<<"$capture_source" || true)"
    rg -Fq "$expected_label" <<<"$resource_lines" ||
      fail "${capture_function} does not ledger ${docker_kind} with its expected run label"
  done
  if [[ "$capture_function" == capture_monero_resources ]]; then
    resource_lines="$(rg -F 'record_resource volume' <<<"$capture_source" || true)"
    rg -Fq "$expected_label" <<<"$resource_lines" ||
      fail "Monero volume ledger entry omits its expected child run label"
  fi
done

cleanup_source="$(function_source cleanup)"
[[ -n "$cleanup_source" ]] || fail "cleanup function is unavailable"
for docker_kind in container volume network image; do
  cleanup_case="$(sed -n "/^[[:space:]]*${docker_kind})$/,/;;/p" <<<"$cleanup_source")"
  [[ -n "$cleanup_case" ]] || fail "cleanup omits ${docker_kind} case"
  validation_line="$(rg -n -m1 -F 'docker_resource_run_label_matches' \
    <<<"$cleanup_case" || true)"
  validation_line="${validation_line%%:*}"
  deletion_line="$(rg -n -m1 "docker[[:space:]]+${docker_kind}[[:space:]]+rm" \
    <<<"$cleanup_case" || true)"
  deletion_line="${deletion_line%%:*}"
  [[ "$validation_line" =~ ^[0-9]+$ && "$deletion_line" =~ ^[0-9]+$ ]] ||
    fail "${docker_kind} cleanup lacks point-of-delete run-label validation"
  (( validation_line < deletion_line )) ||
    fail "${docker_kind} cleanup validates its run label after deletion"
  rg -Fq '"$run_label"' <<<"$cleanup_case" ||
    fail "${docker_kind} cleanup does not validate the ledgered expected run label"
  rg -q 'if[[:space:]]+!?[[:space:]]*docker_resource_run_label_matches' \
    <<<"$cleanup_case" ||
    fail "${docker_kind} deletion is not gated by point-of-delete label validation"
done

monero_child_case="$(sed -n '/^[[:space:]]*monero_child)$/,/;;/p' <<<"$cleanup_source")"
[[ -n "$monero_child_case" ]] || fail "cleanup omits preregistered Monero-child recovery"
rg -Fq '"$run_label"' <<<"$monero_child_case" ||
  fail "Monero-child recovery is not scoped to its ledgered child run label"

monero_start_source="$(function_source start_monero_child)"
[[ -n "$monero_start_source" ]] || fail "Monero child launcher function is unavailable"
preregister_count="$(rg -F -c 'record_resource monero_child' <<<"$monero_start_source" || true)"
[[ "$preregister_count" == 1 ]] ||
  fail "Monero child must be preregistered exactly once before launch"
preregister_line="$(rg -n -m1 -F 'record_resource monero_child' <<<"$monero_start_source")"
preregister_line="${preregister_line%%:*}"
monero_launch_line="$(rg -n -m1 -F '"$monero_runner"' <<<"$monero_start_source")"
monero_launch_line="${monero_launch_line%%:*}"
(( preregister_line < monero_launch_line )) ||
  fail "Monero child preregistration occurs after its launcher invocation"
rg -Fq '"$MONERO_RUN_ID"' \
  <<<"$(rg -F 'record_resource monero_child' <<<"$monero_start_source")" ||
  fail "Monero child preregistration does not persist the exact child run label"

execute_source="$(function_source execute_run)"
rg -Fq 'actor_onboarding' <<<"$execute_source" ||
  fail "execute omits actor onboarding"
rg -Fq 'start_monero_child' <<<"$execute_source" ||
  fail "execute omits the Monero child launcher"
rg -Fq 'compose_xmr_agreement' <<<"$execute_source" ||
  fail "execute omits the agreement helper"
rg -Fq 'submit_tag13' <<<"$execute_source" ||
  fail "execute omits the tag-13 runner"
readonly post_tag13_fail='fail "cleanup phase is not implemented; cross-chain evidence completed; do not retry this run"'
rg -Fq "$post_tag13_fail" <<<"$execute_source" ||
  fail "execute omits the post-tag13 fail-closed boundary"
execute_tag13_line="$(rg -n -m1 -F 'submit_tag13' <<<"$execute_source")"
execute_tag13_line="${execute_tag13_line%%:*}"
post_tag13_fail_line="$(rg -n -m1 -F "$post_tag13_fail" <<<"$execute_source")"
post_tag13_fail_line="${post_tag13_fail_line%%:*}"
(( execute_tag13_line < post_tag13_fail_line )) ||
  fail "post-tag13 no-retry boundary appears before the tag-13 call"
export_line="$(rg -n -m1 -F 'export_tag13_handoff' <<<"$execute_source")"
export_line="${export_line%%:*}"
sidecar_line="$(rg -n -m1 -F 'start_role_sidecars' <<<"$execute_source")"
sidecar_line="${sidecar_line%%:*}"
[[ "$export_line" =~ ^[0-9]+$ && "$sidecar_line" =~ ^[0-9]+$ ]] || fail "sidecar continuation boundaries are unavailable"
(( execute_tag13_line < export_line && export_line < sidecar_line && sidecar_line < post_tag13_fail_line )) || fail "tag13 handoff/sidecar readiness ordering is not fail-closed"

tag13_source="$(function_source submit_tag13)"
[[ -n "$tag13_source" ]] || fail "tag-13 submission function is unavailable"
for latch_boundary in \
  'tag13_submission_may_have_occurred' \
  'ln -- "$temporary" "$tag13_no_retry_latch"' \
  'sync -f "$tag13_no_retry_latch"' \
  'require_owner_file "$tag13_no_retry_latch"'; do
  rg -Fq -- "$latch_boundary" <<<"$tag13_source" ||
    fail "tag-13 no-retry latch omits durable create-new boundary: ${latch_boundary}"
done
latch_publish_line="$(rg -n -m1 -F 'ln -- "$temporary" "$tag13_no_retry_latch"' \
  <<<"$tag13_source")"
latch_publish_line="${latch_publish_line%%:*}"
latch_sync_line="$(rg -n -m1 -F 'sync -f "$tag13_no_retry_latch"' <<<"$tag13_source")"
latch_sync_line="${latch_sync_line%%:*}"
tag13_binary_line="$(rg -n -m1 -F '"$tag13_binary"' <<<"$tag13_source")"
tag13_binary_line="${tag13_binary_line%%:*}"
[[ "$latch_publish_line" =~ ^[0-9]+$ && "$latch_sync_line" =~ ^[0-9]+$ &&
  "$tag13_binary_line" =~ ^[0-9]+$ ]] || fail "tag-13 latch/submission ordering is unavailable"
(( latch_publish_line < latch_sync_line && latch_sync_line < tag13_binary_line )) ||
  fail "durable tag-13 no-retry latch is not published before binary invocation"

agreement_source="$(function_source compose_xmr_agreement)"
[[ -n "$agreement_source" ]] || fail "agreement composition function is unavailable"
journal_start_line="$(rg -n -m1 -F 'record_phase journals started' <<<"$agreement_source")"
journal_start_line="${journal_start_line%%:*}"
agreement_helper_line="$(rg -n -m1 -F '"$agreement_runner" execute' <<<"$agreement_source")"
agreement_helper_line="${agreement_helper_line%%:*}"
journal_complete_line="$(rg -n -m1 -F 'record_phase journals completed' <<<"$agreement_source")"
journal_complete_line="${journal_complete_line%%:*}"
agreement_complete_line="$(rg -n -m1 -F 'record_phase agreement completed' <<<"$agreement_source")"
agreement_complete_line="${agreement_complete_line%%:*}"
[[ "$journal_start_line" =~ ^[0-9]+$ && "$agreement_helper_line" =~ ^[0-9]+$ &&
  "$journal_complete_line" =~ ^[0-9]+$ && "$agreement_complete_line" =~ ^[0-9]+$ ]] ||
  fail "agreement/journal phase ordering lines are unavailable"
(( journal_start_line < agreement_helper_line &&
   agreement_helper_line < journal_complete_line &&
   journal_complete_line < agreement_complete_line )) ||
  fail "journal phase evidence does not bracket the combined agreement helper truthfully"


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
