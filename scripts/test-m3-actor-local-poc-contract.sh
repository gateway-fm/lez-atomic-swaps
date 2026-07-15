#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly runner="scripts/run-m3-actor-local-poc.sh"
readonly direction_driver="scripts/run-m3-actor-direction.sh"
readonly bootstrap_driver="scripts/run-m3-lez-bootstrap.sh"
readonly require_binaries="${M3_ACTOR_CONTRACT_REQUIRE_BINARIES:-1}"

fail() {
  echo "M3 actor local-PoC contract failed: $*" >&2
  exit 1
}

case "$require_binaries" in
  0 | 1) ;;
  *) fail "M3_ACTOR_CONTRACT_REQUIRE_BINARIES must be exactly 0 or 1" ;;
esac

require_fixed() {
  local needle="$1"
  rg -Fq -- "$needle" "$runner" || fail "runner is missing: ${needle}"
}

[[ -x "$runner" ]] || fail "runner is missing or not executable"
bash -n "$runner"
[[ -x "$direction_driver" ]] || fail "direction boundary is missing or not executable"
bash -n "$direction_driver"

direction_contract="$($direction_driver contract)"
jq -e '
  .schema_version == 1
  and .kind == "m3_actor_direction_driver_contract"
  and .stage_two_spec_uses_actual_node_facts == true
  and .fresh_actor_process_per_command == true
  and .separate_role_state_and_signing_journals == true
  and .taker_first_effects == true
  and .dual_locks_before_scalar_use == true
  and .bitcoin_exact_signed_depth == true
  and .bitcoin_planned_funding_anchor_exact == true
  and .lez_exact_finalized_ancestry == true
  and .actor_owned_claim_effects == true
  and .submission_count_query == true
  and .owned_process_registry == true
  and .pre_lock_presignature_domains == ["bitcoin", "lez"]
  and .expected_unique_effects == {bitcoin: 2, lez: 3}
  and .submission_count_semantics == "unique_effects_plus_durable_one_shot_authority"
  and .runtime_backend == "repository_owned_actual_node_implementation"
' <<<"$direction_contract" >/dev/null || fail "direction boundary contract is incomplete"
if rg -n 'M3_ACTOR_DIRECTION_BACKEND|runtime backend is missing|real actor flow is not yet assembled' \
  "$direction_driver"; then
  fail "direction driver still delegates to an external runtime backend"
fi
if [[ "$require_binaries" == 1 ]]; then
  "$direction_driver" preflight
fi

foreign_plan="$("$direction_driver" effect-plan taker_sells_foreign)"
jq -e '
  .schema_version == 1
  and .direction == "taker_sells_foreign"
  and .before_first_effect == ["finalize_agreement","prepare_exact_lez_claim",
    "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"]
  and .public_effect_order == ["bitcoin_lock_by_taker","lez_initialize_by_maker",
    "lez_fund_by_maker","dual_lock_gate","lez_claim_by_taker","bitcoin_claim_by_maker"]
  and .terminal == {maker_revision:4,taker_revision:4}
' <<<"$foreign_plan" >/dev/null || fail "foreign direction effect plan is not role-correct"

lez_plan="$("$direction_driver" effect-plan taker_sells_lez)"
jq -e '
  .schema_version == 1
  and .direction == "taker_sells_lez"
  and .before_first_effect == ["finalize_agreement","prepare_exact_lez_claim",
    "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"]
  and .public_effect_order == ["lez_initialize_by_taker","lez_fund_by_taker",
    "bitcoin_lock_by_maker","dual_lock_gate","bitcoin_claim_by_taker","lez_claim_by_maker"]
  and .terminal == {maker_revision:4,taker_revision:4}
' <<<"$lez_plan" >/dev/null || fail "LEZ direction effect plan is not role-correct"

for behavior in prepare_final_transcript provision_signing_material run_signing_ceremony \
  write_actor_configs activate_actors submit_bitcoin_lock submit_lez_lock_pair \
  write_dual_lock_gate submit_actor_bitcoin_claim submit_actor_lez_claim \
  prove_lez_finalized_transaction write_actual_effect_manifest; do
  rg -Fq "${behavior}()" "$direction_driver" ||
    fail "actual direction implementation is missing behavior: ${behavior}"
done
rg -Fq 'planned_bitcoin_funding_anchor_height' "$direction_driver" ||
  fail "Bitcoin lock path does not bind the actual mined height to the planned anchor"
rg -Fq 'exact_transaction_occurrences:1' "$direction_driver" ||
  fail "Bitcoin lock path does not retain exact containing-block membership"

[[ -x "$bootstrap_driver" ]] || fail "LEZ bootstrap driver is missing or not executable"
bash -n "$bootstrap_driver"
bootstrap_contract="$($bootstrap_driver contract)"
"$bootstrap_driver" self-test-finality-selector
jq -e '
  .schema_version == 1
  and .kind == "m3_lez_bootstrap_contract"
  and .verified_artifact_target_required == true
  and .canonical_guest_artifact_independently_hashed == true
  and .canonical_guest_source == "compat/lez-v0.2-provisional/escrow/methods/guest/src/bin/zec_escrow_v02.rs"
  and .finality_membership_variants == ["ProgramDeployment", "Public"]
  and .deployment_submission_count == 1
  and .fresh_identity_vault_claims == ["maker", "taker"]
  and .vault_claim_submission_count_per_role == 1
  and .finalized_read_retries == "bounded_read_only_never_resubmit"
  and .evidence_binds_script_binary_manifest_source == true
' <<<"$bootstrap_contract" >/dev/null || fail "LEZ bootstrap contract is incomplete"
guest_source="$(jq -er '.canonical_guest_source' <<<"$bootstrap_contract")"
[[ -f "$guest_source" && ! -L "$guest_source" ]] ||
  fail "LEZ bootstrap contract does not name a tracked canonical guest source"
git ls-files --error-unmatch -- "$guest_source" >/dev/null ||
  fail "LEZ bootstrap canonical guest source is not tracked"

invalid_run_id="M3 invalid $$"
if invalid_output="$(RUN_ID="$invalid_run_id" M3_ACTOR_POC_MODE=contract "$runner" 2>&1)"; then
  fail "an invalid run ID reached contract output"
fi
[[ "$invalid_output" == *"RUN_ID must be 8..48 lowercase"* ]] ||
  fail "invalid run ID did not fail with the bounded validation error"
[[ ! -e ".e2e/${invalid_run_id}" && ! -L ".e2e/${invalid_run_id}" ]] ||
  fail "invalid input created run state"

contract_run_id="m3contract-$RANDOM-$$"
contract_json="$(RUN_ID="$contract_run_id" M3_ACTOR_POC_MODE=contract "$runner")"
jq -e --arg run_id "$contract_run_id" '
  .schema_version == 1
  and .kind == "m3_actor_local_poc_contract"
  and .execution_performed == false
  and .run_id == $run_id
  and .run_root == (".e2e/" + $run_id + "/m3-actor-poc")
  and .service_runs.bitcoin_core == ($run_id + "-btc")
  and .service_runs.lez_v0_2 == ($run_id + "-lez")
  and .service_runs.bitcoin_core != .service_runs.lez_v0_2
  and .directions == ["taker_sells_foreign", "taker_sells_lez"]
  and .process_model.actor == "fresh_process_for_every_command_and_revision"
  and .process_model.roles == ["maker", "taker"]
  and .process_model.state == "separate_role_configs_state_dbs_and_signing_journals"
  and .ordering.stage_one_before_node_facts == true
  and .ordering.official_nssa_before_stage_two == true
  and .ordering.stage_two_after_actual_node_facts == true
  and .ordering.taker_first_effects == true
  and .ordering.dual_locks_before_scalar_use == true
  and .ordering.directions_are_sequential == true
  and .finality.bitcoin == "exact_signed_confirmation_depth"
  and .finality.lez == "exact_finalized_indexer_ancestry"
  and .terminal.required_revision == 4
  and .terminal.required_phase == "completed"
  and .terminal.required_next_action == "complete"
  and .replay.restart_both_roles == true
  and .replay.resubmission_count == 0
  and .isolation.dynamic_literal_loopback_ports == true
  and .isolation.foreign_resource_mutation == false
  and .cleanup.captured_exact_ids_only == true
  and .cleanup.runs_on_success_and_failure == true
  and .evidence.secret_safe_json == true
  and .evidence.cleanup_attestation == true
  and .evidence.executable_script_sha256s == ["outer_runner","direction_driver","lez_bootstrap"]
  and .build_prerequisites.rapidsnark_lib_dir == "explicit_absolute_canonical_verified_v0_0_8"
  and .build_prerequisites.rapidsnark_files ==
    ["librapidsnark.a","libgmp.a","libfq.a","libfr.a"]
  and .build_prerequisites.bindgen_extra_clang_args == "explicit_nonempty"
  and .build_prerequisites.inherited_by_offline_sidecar_build == true
  and .external_resources.public_rpc == false
  and .external_resources.faucet == false
  and .external_resources.public_funds == false
' <<<"$contract_json" >/dev/null || fail "contract JSON does not prove the M3 invariants"

required_terms=(
  'scripts/run-bitcoin-core-e2e.sh'
  'scripts/run-lez-v02-stack.sh'
  'btc-local-poc-provision'
  'btc-reference-actor'
  'lez-adaptor-role-runner'
  'lez-v02-bridge-poc'
  'lez-v02-local-actor-identity'
  'lez-v02-account-id'
  'btc-core-p2tr-fixture'
  'BITCOIN_CORE_E2E_MODE=service'
  'BITCOIN_CORE_E2E_KEEP_RUNNING=1'
  'LEZ_V02_KEEP_RUNNING=1'
  'taker_sells_foreign'
  'taker_sells_lez'
  'run_stage_one'
  'run_official_nssa_mapping'
  'start_actual_nodes'
  'run_stage_two'
  'run_direction_actor_flow'
  'assert_terminal_and_replay'
  'write_cleanup_attestation'
  'capture_owned_resources'
  'remove_exact_resource_file'
  'all_exact_run_resources_absent'
  'verify_lez_bootstrap_contract'
  'bootstrap_lez_runtime'
  'LEZ_V02_ARTIFACT_TARGET_DIR'
  'LEZ_V02_MAKER_VAULT_ACCOUNT_ID'
  'LEZ_V02_TAKER_VAULT_ACCOUNT_ID'
  'capture_owned_containers'
  'assert_exact_owned_resource'
  'org.logos-co.atomic-swaps.run'
  '"$direction_driver" preflight'
)
for term in "${required_terms[@]}"; do
  require_fixed "$term"
done

stage_one_line="$(rg -n -F 'run_stage_one "$direction"' "$runner" | head -n1 | cut -d: -f1)"
nodes_line="$(rg -n -F 'start_actual_nodes' "$runner" | tail -n1 | cut -d: -f1)"
stage_two_line="$(rg -n -F 'run_stage_two "$direction"' "$runner" | head -n1 | cut -d: -f1)"
if [[ ! "$stage_one_line" =~ ^[0-9]+$ || ! "$nodes_line" =~ ^[0-9]+$ ||
      ! "$stage_two_line" =~ ^[0-9]+$ ]]; then
  fail "could not locate the stage-one/node/stage-two execution order"
fi
if (( stage_one_line >= nodes_line || nodes_line >= stage_two_line )); then
  fail "stage one must precede node facts and stage two must follow actual nodes"
fi

if rg -n 'docker[[:space:]]+(system[[:space:]]+prune|container[[:space:]]+prune|network[[:space:]]+prune|volume[[:space:]]+prune)|docker[[:space:]]+compose[[:space:]]+down|pkill|killall|rm[[:space:]]+-rf[[:space:]]+[^"$].*\.e2e' "$runner"; then
  fail "runner contains a broad or foreign-resource cleanup primitive"
fi

echo "M3 actor local-PoC orchestration contract is complete"
