#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if [[ "${M3_ACTOR_CONTRACT_FAKE_ACTOR:-0}" == 1 ]]; then
  config=""
  command_name=""
  while (( $# > 0 )); do
    case "$1" in
      --config) config="$2"; shift 2 ;;
      recover | status) command_name="$1"; shift ;;
      *) echo "unexpected fake-actor argument: $1" >&2; exit 64 ;;
    esac
  done
  role="$(jq -er '.role' "$config")"
  printf '%s\t%s\n' "$role" "$command_name" >>"${FAKE_ACTOR_LOG}"
  case "$command_name:${FAKE_ACTOR_MODE}" in
    recover:lez-moving-tip-then-success)
      attempt="$(<"${FAKE_ACTOR_ATTEMPTS}")"
      attempt=$((attempt + 1))
      printf '%s\n' "$attempt" >"${FAKE_ACTOR_ATTEMPTS}"
      if (( attempt < 3 )); then
        echo "actor chain observation is unavailable" >&2
        exit 1
      fi
      printf '3\n' >"${FAKE_LEZ_SUBMISSION_COUNT}"
      jq -n --arg role "$role" --argjson revision "${FAKE_LEZ_PREDECESSOR}" '
        {schema_version:1,role:$role,command:"recover",outcome:"awaiting_observation",
         chain:"lez",revision:$revision}'
      ;;
    recover:lez-other-error)
      attempt="$(<"${FAKE_ACTOR_ATTEMPTS}")"
      printf '%s\n' "$((attempt + 1))" >"${FAKE_ACTOR_ATTEMPTS}"
      echo "actor durable state is unavailable" >&2
      exit 1
      ;;
    recover:lez-nonempty-typed)
      attempt="$(<"${FAKE_ACTOR_ATTEMPTS}")"
      printf '%s\n' "$((attempt + 1))" >"${FAKE_ACTOR_ATTEMPTS}"
      echo '{"unexpected":"stdout"}'
      echo "actor chain observation is unavailable" >&2
      exit 1
      ;;
    recover:lez-typed-after-send)
      attempt="$(<"${FAKE_ACTOR_ATTEMPTS}")"
      attempt=$((attempt + 1))
      printf '%s\n' "$attempt" >"${FAKE_ACTOR_ATTEMPTS}"
      if (( attempt == 1 )); then
        printf '3\n' >"${FAKE_LEZ_SUBMISSION_COUNT}"
        echo "actor chain observation is unavailable" >&2
      else
        echo "actor durable state is unavailable" >&2
      fi
      exit 1
      ;;
    recover:typed-unavailable)
      echo "actor chain observation is unavailable" >&2
      exit 1
      ;;
    recover:other-error)
      echo "actor durable state is unavailable" >&2
      exit 1
      ;;
    status:*)
      jq -n --arg role "$role" --arg phase "${FAKE_STATUS_PHASE}" \
        --arg next_action "${FAKE_STATUS_NEXT_ACTION}" \
        --argjson revision "${FAKE_STATUS_REVISION}" '
        {schema_version:1,role:$role,state:"active",phase:$phase,
         revision:$revision,next_action:$next_action}'
      ;;
    *) echo "unsupported fake-actor mode" >&2; exit 64 ;;
  esac
  exit 0
fi

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
stage_two_spec_source="$(sed -n '/^prepare_stage_two_spec() {$/,/^}$/p' "$direction_driver")"
[[ -n "$stage_two_spec_source" ]] || fail "direction boundary lacks stage-two agreement construction"
rg -Fq -- '--argjson maker_cutoff "$maker_cutoff"' <<<"$stage_two_spec_source" ||
  fail "stage-two agreement does not bind the journey-specific cutoff into jq"
rg -Fq 'maker_second_lock_cutoff_unix_seconds:$maker_cutoff' \
  <<<"$stage_two_spec_source" ||
  fail "stage-two agreement does not use the bound maker cutoff"
rg -Fq 'claim | survivor_claim)' <<<"$stage_two_spec_source" ||
  fail "happy/survivor journeys do not select a future maker-lock cutoff"
rg -Fq 'maker_cutoff=$((now + 1800))' <<<"$stage_two_spec_source" ||
  fail "happy/survivor maker-lock cutoff lacks a reproducible admission window"
rg -Fq 'refund)' <<<"$stage_two_spec_source" ||
  fail "two-lock refund journey does not select a maker-lock admission window"
rg -Fq 'maker_cutoff=$((now + 300))' <<<"$stage_two_spec_source" ||
  fail "two-lock refund cutoff does not precede its signed reaction margin"
rg -Fq 'first_lock_refund)' <<<"$stage_two_spec_source" ||
  fail "absent-maker journey does not select an immediate cutoff"
rg -Fq 'maker_cutoff="$now"' <<<"$stage_two_spec_source" ||
  fail "absent-maker journey no longer fixes cutoff at agreement preparation"
if rg -Fq 'maker_second_lock_cutoff_unix_seconds:$now' \
    <<<"$stage_two_spec_source"; then
  fail "stage-two agreement references an unbound jq cutoff variable"
fi

readonly lez_stack_driver="scripts/run-lez-v02-stack.sh"
[[ -x "$lez_stack_driver" ]] || fail "LEZ v0.2 stack runner is missing or not executable"
bash -n "$lez_stack_driver"
bedrock_renderer_source="$(sed -n \
  '/^render_bedrock_deployment_settings() {$/,/^}$/p' "$lez_stack_driver")"
[[ -n "$bedrock_renderer_source" ]] ||
  fail "LEZ stack lacks the pure audited Bedrock deployment-settings renderer"
count_fixed_source="$(sed -n '/^count_fixed_occurrences() {$/,/^}$/p' "$lez_stack_driver")"

bedrock_renderer_root="$(mktemp -d /tmp/lez-bedrock-render-contract.XXXXXX)"
cleanup_bedrock_renderer_root() {
  rm -rf -- "$bedrock_renderer_root"
}
trap cleanup_bedrock_renderer_root EXIT
readonly audited_genesis_hex="2c04626900000000"
readonly rendered_genesis_hex="0102030405060708"

write_bedrock_source() {
  local output="$1" genesis_count="${2:-1}" slot_count="${3:-1}" slot_line="${4:-  slot_duration: '1.0'}"
  printf '%s\n' 'version: 1' 'genesis:' >"$output"
  if [[ "$genesis_count" == 1 ]]; then
    printf "  inscription: 'prefix%s-suffix'\n" "$audited_genesis_hex" >>"$output"
  elif [[ "$genesis_count" == 2 ]]; then
    printf "  inscription: 'prefix%s-middle-%s-suffix'\n" \
      "$audited_genesis_hex" "$audited_genesis_hex" >>"$output"
  else
    printf '%s\n' "  inscription: 'prefix-no-audited-genesis-suffix'" >>"$output"
  fi
  printf '%s\n' 'time:' >>"$output"
  if [[ "$slot_count" == 1 ]]; then
    printf '%s\n' "$slot_line" >>"$output"
  elif [[ "$slot_count" == 2 ]]; then
    printf '%s\n%s\n' "$slot_line" "$slot_line" >>"$output"
  fi
  printf '%s\n' 'tail: preserved' >>"$output"
}

run_bedrock_renderer() {
  local source="$1" output="$2" genesis="$3" cadence="$4"
  bash -c '
    set -euo pipefail
    upstream_genesis_time_hex="$1"
    eval "$2"
    eval "$3"
    render_bedrock_deployment_settings "$4" "$5" "$6" "$7"
  ' bedrock-renderer "$audited_genesis_hex" "$count_fixed_source" \
    "$bedrock_renderer_source" "$source" "$output" "$genesis" "$cadence"
}

valid_bedrock_source="${bedrock_renderer_root}/valid.yaml"
write_bedrock_source "$valid_bedrock_source"
for cadence in 1.0 3.0; do
  output="${bedrock_renderer_root}/valid-${cadence}.yaml"
  run_bedrock_renderer "$valid_bedrock_source" "$output" \
    "$rendered_genesis_hex" "$cadence" ||
    fail "Bedrock renderer rejected exact allowed slot duration ${cadence}"
  [[ -f "$output" && ! -L "$output" ]] || fail "Bedrock renderer did not create a regular output"
  [[ "$(rg -Fo "$audited_genesis_hex" "$output" | wc -l | tr -d '[:space:]')" == 0 ]] ||
    fail "Bedrock renderer retained the audited stale genesis"
  [[ "$(rg -Fo "$rendered_genesis_hex" "$output" | wc -l | tr -d '[:space:]')" == 1 ]] ||
    fail "Bedrock renderer did not replace exactly one genesis"
  [[ "$(rg -Foc "  slot_duration: '${cadence}'" "$output")" == 1 ]] ||
    fail "Bedrock renderer did not replace exactly one slot duration"
  rg -Fq 'tail: preserved' "$output" || fail "Bedrock renderer changed unrelated settings"
done

expect_bedrock_render_rejected() {
  local name="$1" source="$2" genesis="$3" cadence="$4"
  local output="${bedrock_renderer_root}/rejected-${name}.yaml"
  if run_bedrock_renderer "$source" "$output" "$genesis" "$cadence" \
      >/dev/null 2>&1; then
    fail "Bedrock renderer accepted invalid fixture: ${name}"
  fi
  [[ ! -e "$output" && ! -L "$output" ]] ||
    fail "rejected Bedrock render created output: ${name}"
}

for cadence in 0.5 2.0 3 03.0 '3.0 ' $'3.0\ninjected: true'; do
  expect_bedrock_render_rejected "slot-${RANDOM}" "$valid_bedrock_source" \
    "$rendered_genesis_hex" "$cadence"
done
for genesis in 0102 ABCDEF0123456789 2c04626900000000 $'01020304\n05060708'; do
  expect_bedrock_render_rejected "genesis-${RANDOM}" "$valid_bedrock_source" \
    "$genesis" 3.0
done

duplicate_genesis_source="${bedrock_renderer_root}/duplicate-genesis.yaml"
missing_genesis_source="${bedrock_renderer_root}/missing-genesis.yaml"
duplicate_slot_source="${bedrock_renderer_root}/duplicate-slot.yaml"
missing_slot_source="${bedrock_renderer_root}/missing-slot.yaml"
malformed_slot_source="${bedrock_renderer_root}/malformed-slot.yaml"
write_bedrock_source "$duplicate_genesis_source" 2 1
write_bedrock_source "$missing_genesis_source" 0 1
write_bedrock_source "$duplicate_slot_source" 1 2
write_bedrock_source "$missing_slot_source" 1 0
write_bedrock_source "$malformed_slot_source" 1 1 '  slot_duration: 1.0'
for fixture in duplicate-genesis missing-genesis duplicate-slot missing-slot malformed-slot; do
  source_variable="${fixture//-/_}_source"
  expect_bedrock_render_rejected "$fixture" "${!source_variable}" "$rendered_genesis_hex" 3.0
done

existing_output="${bedrock_renderer_root}/existing.yaml"
printf '%s\n' sentinel >"$existing_output"
if run_bedrock_renderer "$valid_bedrock_source" "$existing_output" \
    "$rendered_genesis_hex" 3.0 >/dev/null 2>&1; then
  fail "Bedrock renderer overwrote an existing output"
fi
[[ "$(<"$existing_output")" == sentinel ]] || fail "Bedrock renderer mutated existing output"
cleanup_bedrock_renderer_root
trap - EXIT

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
  and .actor_owned_maker_lock_effects == true
  and .taker_first_lock_external_runner_submission == true
  and .maker_lock_submission_actor_output == "awaiting_observation"
  and .maker_lock_restart_never_resubmits == true
  and .runner_only_confirms_actor_submitted_maker_locks == true
  and .actor_owned_claim_effects == true
  and .actor_owned_survivor_claim_effects == true
  and .survivor_revealer_absent_until_follower_terminal == true
  and .survivor_fresh_follower_restarts == true
  and .survivor_intermediate_phase == "claim_evidence_available"
  and .survivor_intermediate_terminal == false
  and .journeys == ["claim", "survivor_claim", "refund", "first_lock_refund"]
  and .maker_lock_cutoff_schedule == {
    claim_and_survivor_seconds_after_preparation:1800,
    two_lock_refund_seconds_after_preparation:300,
    first_lock_refund_seconds_after_preparation:0,
    required_reaction_margin_seconds:600
  }
  and .default_journey == "claim"
  and .actor_owned_refund_effects == true
  and .actor_owned_first_lock_refund_effects == true
  and .first_lock_refund_terminal_revision == 2
  and .first_lock_refund_requires_signed_maker_cutoff == true
  and .first_lock_refund_requires_two_fresh_absence_and_unspent_reads == true
  and .first_lock_refund_lez_absence_window_reaches_current_finalized_tip == true
  and .first_lock_refund_bitcoin_cutoff_uses_stable_median_time == true
  and .first_lock_refund_owner_restart_never_resubmits == true
  and .first_lock_refund_fresh_maker_observer == true
  and .first_lock_refund_abandoned_maker_after_activation_until_finality == true
  and .first_lock_refund_taker_only_revision_one_and_refund_projection == true
  and .timeout_terminal_phase == "refunded"
  and .actor_config_schema_version == 4
  and .role_shaped_bitcoin_refund_authority == true
  and .secure_sidecar_state_root_required == true
  and .single_core_rpc_response_per_call == true
  and .anchor_height_uses_allowed_blockchain_info == true
  and .prelock_policy_response_retained == true
  and .role_allowed_block_and_mempool_observation == true
  and .bounded_read_only_observation_retries_never_resubmit == true
  and .bounded_pending_observation_retries == true
  and .bounded_prepared_bitcoin_claim_reconciliation == true
  and .actor_lez_bridge_request_timeout_millis == 30000
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
foreign_claim_plan="$("$direction_driver" effect-plan taker_sells_foreign claim)"
[[ "$(jq -S -c . <<<"$foreign_plan")" == "$(jq -S -c . <<<"$foreign_claim_plan")" ]] ||
  fail "foreign direction default effect plan is not the claim journey"
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
lez_claim_plan="$("$direction_driver" effect-plan taker_sells_lez claim)"
[[ "$(jq -S -c . <<<"$lez_plan")" == "$(jq -S -c . <<<"$lez_claim_plan")" ]] ||
  fail "LEZ direction default effect plan is not the claim journey"
jq -e '
  .schema_version == 1
  and .direction == "taker_sells_lez"
  and .before_first_effect == ["finalize_agreement","prepare_exact_lez_claim",
    "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"]
  and .public_effect_order == ["lez_initialize_by_taker","lez_fund_by_taker",
    "bitcoin_lock_by_maker","dual_lock_gate","bitcoin_claim_by_taker","lez_claim_by_maker"]
  and .terminal == {maker_revision:4,taker_revision:4}
' <<<"$lez_plan" >/dev/null || fail "LEZ direction effect plan is not role-correct"

foreign_survivor_plan="$("$direction_driver" effect-plan taker_sells_foreign survivor_claim)"
jq -e '
  .schema_version == 1 and .journey == "survivor_claim"
  and .direction == "taker_sells_foreign"
  and .public_effect_order == ["bitcoin_lock_by_taker","lez_initialize_by_maker",
    "lez_fund_by_maker","dual_lock_gate","lez_claim_by_taker",
    "fresh_maker_observes_reveal","maker_revision_three_nonterminal",
    "fresh_maker_bitcoin_claim","delayed_taker_observation_only_catchup"]
  and .survivor == {revealer:"taker",follower:"maker",revealing_chain:"lez",
    followup_chain:"bitcoin",intermediate_phase:"claim_evidence_available",
    intermediate_terminal:false}
  and .terminal == {maker_revision:4,taker_revision:4}
' <<<"$foreign_survivor_plan" >/dev/null ||
  fail "foreign direction survivor plan is not role-correct"

lez_survivor_plan="$("$direction_driver" effect-plan taker_sells_lez survivor_claim)"
jq -e '
  .schema_version == 1 and .journey == "survivor_claim"
  and .direction == "taker_sells_lez"
  and .public_effect_order == ["lez_initialize_by_taker","lez_fund_by_taker",
    "bitcoin_lock_by_maker","dual_lock_gate","bitcoin_claim_by_taker",
    "fresh_maker_observes_reveal","maker_revision_three_nonterminal",
    "fresh_maker_lez_claim","delayed_taker_observation_only_catchup"]
  and .survivor == {revealer:"taker",follower:"maker",revealing_chain:"bitcoin",
    followup_chain:"lez",intermediate_phase:"claim_evidence_available",
    intermediate_terminal:false}
  and .terminal == {maker_revision:4,taker_revision:4}
' <<<"$lez_survivor_plan" >/dev/null ||
  fail "LEZ direction survivor plan is not role-correct"

foreign_refund_plan="$("$direction_driver" effect-plan taker_sells_foreign refund)"
jq -e '
  .schema_version == 1
  and .direction == "taker_sells_foreign"
  and .before_first_effect == ["finalize_agreement","prepare_exact_lez_claim",
    "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"]
  and .public_effect_order == ["bitcoin_lock_by_taker","lez_initialize_by_maker",
    "lez_fund_by_maker","dual_lock_gate","lez_refund_by_maker",
    "bitcoin_refund_by_taker"]
  and .terminal == {maker_revision:4,taker_revision:4,phase:"refunded"}
' <<<"$foreign_refund_plan" >/dev/null ||
  fail "foreign direction refund effect plan is not role-correct"

lez_refund_plan="$("$direction_driver" effect-plan taker_sells_lez refund)"
jq -e '
  .schema_version == 1
  and .direction == "taker_sells_lez"
  and .before_first_effect == ["finalize_agreement","prepare_exact_lez_claim",
    "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"]
  and .public_effect_order == ["lez_initialize_by_taker","lez_fund_by_taker",
    "bitcoin_lock_by_maker","dual_lock_gate","bitcoin_refund_by_maker",
    "lez_refund_by_taker"]
  and .terminal == {maker_revision:4,taker_revision:4,phase:"refunded"}
' <<<"$lez_refund_plan" >/dev/null ||
  fail "LEZ direction refund effect plan is not role-correct"

foreign_first_lock_refund_plan="$("$direction_driver" effect-plan \
  taker_sells_foreign first_lock_refund)"
jq -e "
  .schema_version == 1
  and .journey == \"first_lock_refund\"
  and .direction == \"taker_sells_foreign\"
  and .public_effect_order == [\"bitcoin_lock_by_taker\",\"predecessor_one_pending\",
    \"signed_maker_second_lock_cutoff\",\"fresh_lez_maker_lock_absence_twice\",
    \"fresh_bitcoin_first_lock_unspent_twice\",\"bitcoin_refund_by_taker\",
    \"fresh_maker_observer\"]
  and .expected_unique_effects == {bitcoin:2,lez:0}
  and .maker_second_lock_effect_count == 0
  and .actor_availability == {maker_offline_after_activation_until_refund_finality:true,
    taker_only_revision_one_and_refund_projection:true}
  and .terminal == {maker_revision:2,taker_revision:2,phase:\"refunded\"}
" <<<"$foreign_first_lock_refund_plan" >/dev/null ||
  fail "foreign direction first-lock refund plan is not role-correct"

lez_first_lock_refund_plan="$("$direction_driver" effect-plan \
  taker_sells_lez first_lock_refund)"
jq -e "
  .schema_version == 1
  and .journey == \"first_lock_refund\"
  and .direction == \"taker_sells_lez\"
  and .public_effect_order == [\"lez_initialize_by_taker\",\"lez_fund_by_taker\",
    \"predecessor_one_pending\",\"signed_maker_second_lock_cutoff\",
    \"fresh_bitcoin_maker_lock_absence_twice\",\"fresh_lez_first_lock_unspent_twice\",
    \"lez_refund_by_taker\",\"fresh_maker_observer\"]
  and .expected_unique_effects == {bitcoin:0,lez:3}
  and .maker_second_lock_effect_count == 0
  and .actor_availability == {maker_offline_after_activation_until_refund_finality:true,
    taker_only_revision_one_and_refund_projection:true}
  and .terminal == {maker_revision:2,taker_revision:2,phase:\"refunded\"}
" <<<"$lez_first_lock_refund_plan" >/dev/null ||
  fail "LEZ direction first-lock refund plan is not role-correct"

if "$direction_driver" effect-plan taker_sells_foreign invalid-journey >/dev/null 2>&1; then
  fail "direction effect plan accepted an invalid journey"
fi

for behavior in prepare_final_transcript provision_signing_material run_signing_ceremony \
  write_actor_configs activate_actors submit_taker_bitcoin_first_lock \
  submit_taker_lez_first_lock_pair submit_actor_maker_bitcoin_second_lock \
  submit_actor_maker_lez_second_lock_pair \
  actor_invoke_observation_retry \
  actor_reconcile_bitcoin_claim_submission \
  write_dual_lock_gate submit_actor_bitcoin_claim submit_actor_lez_claim \
  prove_lez_finalized_transaction write_actual_effect_manifest; do
  rg -Fq "${behavior}()" "$direction_driver" ||
    fail "actual direction implementation is missing behavior: ${behavior}"
done
for behavior in submit_actor_bitcoin_refund submit_actor_lez_refund run_actor_refund_flow; do
  rg -Fq "${behavior}()" "$direction_driver" ||
    fail "actual direction implementation is missing refund behavior: ${behavior}"
done
for behavior in submit_actor_bitcoin_claim_effect submit_actor_lez_claim_effect \
  write_survivor_recovering_evidence assert_survivor_maker_terminal \
  write_survivor_completion_evidence run_actor_survivor_claim_flow; do
  rg -Fq "${behavior}()" "$direction_driver" ||
    fail "actual direction implementation is missing survivor behavior: ${behavior}"
done
bitcoin_claim_effect_source="$(sed -n \
  '/^submit_actor_bitcoin_claim_effect() {$/,/^}$/p' "$direction_driver")"
lez_claim_effect_source="$(sed -n \
  '/^submit_actor_lez_claim_effect() {$/,/^}$/p' "$direction_driver")"
bitcoin_claim_wrapper_source="$(sed -n \
  '/^submit_actor_bitcoin_claim() {$/,/^}$/p' "$direction_driver")"
lez_claim_wrapper_source="$(sed -n \
  '/^submit_actor_lez_claim() {$/,/^}$/p' "$direction_driver")"
if rg -Fq 'project_both_to_revision' <<<"$bitcoin_claim_effect_source" ||
   rg -Fq 'project_both_to_revision' <<<"$lez_claim_effect_source"; then
  fail "survivor effect-only claim helper still projects both actors"
fi
rg -Fq 'project_both_to_revision' <<<"$bitcoin_claim_wrapper_source" ||
  fail "ordinary Bitcoin claim wrapper lost both-role projection"
rg -Fq 'project_both_to_revision' <<<"$lez_claim_wrapper_source" ||
  fail "ordinary LEZ claim wrapper lost both-role projection"

survivor_flow_source="$(sed -n \
  '/^run_actor_survivor_claim_flow() {$/,/^}$/p' "$direction_driver")"
[[ -n "$survivor_flow_source" ]] || fail "survivor journey has no isolated flow branch"
if rg -Fq 'project_both_to_revision' <<<"$survivor_flow_source"; then
  fail "survivor journey erases the protected disappearance interval"
fi
for term in \
  'submit_actor_lez_claim_effect taker 3 survivor-lez-reveal' \
  'project_role_to_revision maker 3 lez survivor-maker-observe-reveal' \
  'submit_actor_bitcoin_claim_effect maker 4 survivor-bitcoin-followup' \
  'project_role_to_revision maker 4 bitcoin survivor-maker-project-followup' \
  'submit_actor_bitcoin_claim_effect taker 3 survivor-bitcoin-reveal' \
  'project_role_to_revision maker 3 bitcoin survivor-maker-observe-reveal' \
  'submit_actor_lez_claim_effect maker 4 survivor-lez-followup' \
  'project_role_to_revision maker 4 lez survivor-maker-project-followup' \
  'assert_survivor_maker_terminal' \
  'survivor_taker_absence_guard=0' \
  'project_role_to_revision taker 3 "$reveal_chain"' \
  'project_role_to_revision taker 4 "$followup_chain"'; do
  rg -Fq "$term" <<<"$survivor_flow_source" ||
    fail "survivor journey is missing ordered step: ${term}"
done
previous_line=0
for step in \
  'submit_actor_lez_claim_effect taker 3 survivor-lez-reveal' \
  'project_role_to_revision maker 3 lez survivor-maker-observe-reveal' \
  'write_survivor_recovering_evidence' \
  'submit_actor_bitcoin_claim_effect maker 4 survivor-bitcoin-followup'; do
  step_line="$(rg -n -F "$step" <<<"$survivor_flow_source" | head -1 | cut -d: -f1)"
  [[ "$step_line" =~ ^[0-9]+$ && "$step_line" -gt "$previous_line" ]] ||
    fail "foreign-first survivor steps are not in protocol order: ${step}"
  previous_line="$step_line"
done
previous_line=0
for step in \
  'submit_actor_bitcoin_claim_effect taker 3 survivor-bitcoin-reveal' \
  'project_role_to_revision maker 3 bitcoin survivor-maker-observe-reveal' \
  'write_survivor_recovering_evidence' \
  'submit_actor_lez_claim_effect maker 4 survivor-lez-followup'; do
  step_line="$(rg -n -F "$step" <<<"$survivor_flow_source" | tail -1 | cut -d: -f1)"
  [[ "$step_line" =~ ^[0-9]+$ && "$step_line" -gt "$previous_line" ]] ||
    fail "LEZ-first survivor steps are not in protocol order: ${step}"
  previous_line="$step_line"
done
maker_terminal_line="$(rg -n -F 'assert_survivor_maker_terminal' \
  <<<"$survivor_flow_source" | cut -d: -f1)"
guard_release_line="$(rg -n -F 'survivor_taker_absence_guard=0' \
  <<<"$survivor_flow_source" | cut -d: -f1)"
taker_catchup_line="$(rg -n -F 'project_role_to_revision taker 3 "$reveal_chain"' \
  <<<"$survivor_flow_source" | cut -d: -f1)"
if [[ ! "$maker_terminal_line" =~ ^[0-9]+$ || ! "$guard_release_line" =~ ^[0-9]+$ ||
      ! "$taker_catchup_line" =~ ^[0-9]+$ ||
      "$maker_terminal_line" -ge "$guard_release_line" ||
      "$guard_release_line" -ge "$taker_catchup_line" ]]; then
  fail "survivor revealer is re-enabled before follower terminality"
fi
survivor_recovering_source="$(sed -n \
  '/^write_survivor_recovering_evidence() {$/,/^}$/p' "$direction_driver")"
for term in 'gettxout' 'gettxspendingprevout' \
  'write_finalized_witnessed_funding_observation' \
  'before_signed_later_refund_boundary' \
  'lifecycle_disposition:"recovering"' 'terminal:false' \
  'followup_effect_present:false'; do
  rg -Fq "$term" <<<"$survivor_recovering_source" ||
    fail "survivor recovering evidence omits exact invariant: ${term}"
done
survivor_completion_source="$(sed -n \
  '/^write_survivor_completion_evidence() {$/,/^}$/p' "$direction_driver")"
for term in 'bitcoin_mempool_before_sha256' 'bitcoin_mempool_after_sha256' \
  'actor_observation_sha256' 'successful_resubmission_count' \
  'outcome:"observed_then_projected"' 'completion_boundary'; do
  rg -Fq "$term" <<<"$survivor_completion_source" ||
    fail "survivor completion evidence omits bound per-chain fact: ${term}"
done
native_observation_source="$(sed -n '/^write_native_escrow_observation() {$/,/^}$/p' "$direction_driver")"
[[ -n "$native_observation_source" ]] ||
  fail "native escrow admission observation is missing"
rg -Fq '.amount | strings | select(test("^[1-9][0-9]*$"))' \
  <<<"$native_observation_source" ||
  fail "native escrow admission does not preserve the canonical nonzero u128 decimal string"
if rg -Fq '.amount | numbers' <<<"$native_observation_source"; then
  fail "native escrow admission incorrectly drops the canonical string amount"
fi
finalized_funding_source="$(sed -n '/^write_finalized_witnessed_funding_observation() {$/,/^}$/p' "$direction_driver")"
[[ -n "$finalized_funding_source" ]] ||
  fail "first-lock admission is missing the finalized witnessed-funding observer"
rg -Fq 'observe-finalized-witnessed-funding' <<<"$finalized_funding_source" ||
  fail "first-lock admission does not reuse the typed finalized witnessed-funding operation"
rg -Fq 'target:{mode:"exact",funding_transaction_id:$funding}' \
  <<<"$finalized_funding_source" ||
  fail "first-lock witnessed-funding observation is not bound to the exact persisted transaction"
rg -Fq 'window:{start_height:$start,max_blocks:$blocks}' \
  <<<"$finalized_funding_source" ||
  fail "first-lock witnessed-funding observation is not bound to the finalized lock window"
admission_sample_source="$(sed -n '/^write_first_lock_recovery_admission_sample() {$/,/^}$/p' "$direction_driver")"
foreign_admission_source="$(sed -n '/^    taker_sells_foreign)$/,/^      ;;/p' <<<"$admission_sample_source")"
lez_admission_source="$(sed -n '/^    taker_sells_lez)$/,/^      ;;/p' <<<"$admission_sample_source")"
rg -Fq 'write_native_escrow_observation' <<<"$foreign_admission_source" ||
  fail "foreign first-lock admission no longer proves witnessed-PDA absence"
rg -Fq 'write_finalized_witnessed_funding_observation' <<<"$lez_admission_source" ||
  fail "LEZ first-lock admission does not use aggregate-witness finalized funding evidence"
if rg -Fq 'write_native_escrow_observation' <<<"$lez_admission_source" ||
   rg -Fq '.amount | numbers' <<<"$lez_admission_source"; then
  fail "LEZ first-lock admission still uses hashlock terms or drops the canonical amount"
fi
[[ "$(rg -Fc 'write_native_escrow_observation' <<<"$admission_sample_source")" == 1 ]] ||
  fail "hashlock-only native observation is not isolated to the absent LEZ second lock"
for behavior in assert_first_lock_recovery_pending \
  refresh_first_lock_lez_absence_window advance_core_median_time_to_first_lock_cutoff \
  write_first_lock_recovery_admission_evidence assert_first_lock_owner_terminal_restart \
  assert_fresh_maker_first_lock_terminal run_actor_first_lock_refund_flow; do
  rg -Fq "${behavior}()" "$direction_driver" ||
    fail "actual direction implementation is missing first-lock refund behavior: ${behavior}"
done
lez_cutoff_refresh_source="$(sed -n \
  '/^refresh_first_lock_lez_absence_window() {$/,/^}$/p' "$direction_driver")"
[[ -n "$lez_cutoff_refresh_source" ]] || fail "LEZ first-lock cutoff refresh is missing"
rg -Fq 'for _ in {1..1200}; do' <<<"$lez_cutoff_refresh_source" ||
  fail "LEZ first-lock cutoff refresh does not wait within a fixed bound"
rg -Fq '(( tip_timestamp >= cutoff * 1000 )) && break' \
  <<<"$lez_cutoff_refresh_source" ||
  fail "LEZ first-lock cutoff refresh does not wait for finalized chain time"
rg -Fq 'finalized_cutoff_wait_iterations:$wait_iterations' \
  <<<"$lez_cutoff_refresh_source" ||
  fail "LEZ first-lock cutoff refresh does not retain its bounded wait evidence"

first_lock_refund_flow_source="$(sed -n \
  '/^run_actor_first_lock_refund_flow() {$/,/^}$/p' "$direction_driver")"
[[ -n "$first_lock_refund_flow_source" ]] || fail "first-lock refund has no isolated flow branch"
if rg -q 'submit_actor_(bitcoin|lez)_claim|write_dual_lock_gate' \
    <<<"$first_lock_refund_flow_source"; then
  fail "first-lock refund invokes a claim helper or dual-lock gate"
fi
rg -Fq 'write_first_lock_recovery_admission_evidence' <<<"$first_lock_refund_flow_source" ||
  fail "first-lock refund omits cutoff, absence, or first-lock-unspent admission evidence"
rg -Fq 'assert_first_lock_recovery_pending' <<<"$first_lock_refund_flow_source" ||
  fail "first-lock refund omits predecessor-one pending proof"
foreign_first_lock_refund_source="$(sed -n \
  '/^    taker_sells_foreign)$/,/^      ;;/p' <<<"$first_lock_refund_flow_source")"
[[ -n "$foreign_first_lock_refund_source" ]] ||
  fail "foreign first-lock refund has no isolated direction branch"
[[ "$(rg -Fc 'refresh_first_lock_lez_absence_window' \
  <<<"$foreign_first_lock_refund_source")" == 2 ]] ||
  fail "foreign first-lock refund does not take exactly two fresh LEZ absence snapshots"
rg -U -Fq \
  $'project_role_to_revision taker 1 bitcoin bitcoin-first-lock\n      refresh_first_lock_lez_absence_window pre-maturity\n      assert_first_lock_recovery_pending bitcoin' \
  <<<"$foreign_first_lock_refund_source" ||
  fail "foreign first-lock pending recovery does not use a fresh pre-maturity LEZ window"
rg -U -Fq \
  $'mine_core_to_refund_eligibility bitcoin-taker-first-lock-refund\n      refresh_first_lock_lez_absence_window pre-admission\n      write_first_lock_recovery_admission_evidence' \
  <<<"$foreign_first_lock_refund_source" ||
  fail "foreign first-lock recovery admission does not refresh LEZ after maturity mining"
rg -Fq 'advance_core_median_time_to_first_lock_cutoff' <<<"$first_lock_refund_flow_source" ||
  fail "LEZ first-lock refund does not advance stable Core median time through cutoff"
rg -Fq 'project_role_to_revision taker 1' <<<"$first_lock_refund_flow_source" ||
  fail "first-lock branch does not project revision one in the taker alone"
if rg -Fq 'project_both_to_revision' <<<"$first_lock_refund_flow_source"; then
  fail "first-lock branch projects the abandoned maker before refund finality"
fi
pending_first_lock_source="$(sed -n \
  '/^assert_first_lock_recovery_pending() {$/,/^}$/p' "$direction_driver")"
if rg -Fq 'assert_recovery_pending_both' <<<"$pending_first_lock_source" ||
   rg -Fq 'actor_invoke maker' <<<"$pending_first_lock_source"; then
  fail "first-lock pending proof invokes the abandoned maker actor"
fi
rg -Fq 'assert_first_lock_owner_terminal_restart' <<<"$first_lock_refund_flow_source" ||
  fail "first-lock branch omits the taker owner terminal restart"
rg -Fq 'assert_fresh_maker_first_lock_terminal' <<<"$first_lock_refund_flow_source" ||
  fail "first-lock branch omits post-finality fresh maker convergence"
owner_restart_line="$(rg -n -F 'assert_first_lock_owner_terminal_restart' \
  <<<"$first_lock_refund_flow_source" | cut -d: -f1)"
maker_restart_line="$(rg -n -F 'assert_fresh_maker_first_lock_terminal' \
  <<<"$first_lock_refund_flow_source" | cut -d: -f1)"
if [[ ! "$owner_restart_line" =~ ^[0-9]+$ || ! "$maker_restart_line" =~ ^[0-9]+$ ||
      "$owner_restart_line" -ge "$maker_restart_line" ]]; then
  fail "fresh maker convergence does not follow taker terminal restart"
fi

refund_flow_source="$(sed -n '/^run_actor_refund_flow() {$/,/^}$/p' "$direction_driver")"
[[ -n "$refund_flow_source" ]] || fail "refund journey has no isolated flow branch"
rg -Fq 'submit_actor_bitcoin_refund' <<<"$refund_flow_source" ||
  fail "refund journey does not invoke the actor-owned Bitcoin recovery helper"
rg -Fq 'submit_actor_lez_refund' <<<"$refund_flow_source" ||
  fail "refund journey does not invoke the actor-owned LEZ recovery helper"
if rg -q 'submit_actor_(bitcoin|lez)_claim' <<<"$refund_flow_source"; then
  fail "refund journey invokes a claim helper"
fi
rg -Fq 'M3_POC_JOURNEY' "$direction_driver" ||
  fail "direction boundary does not select the claim or refund journey"

pending_contract_root="$(mktemp -d /tmp/m3-recovery-pending-contract.XXXXXX)"
cleanup_pending_contract_root() {
  rm -rf -- "$pending_contract_root"
}
trap cleanup_pending_contract_root EXIT

run_recovery_pending_fixture() {
  local case_name="$1" predecessor="$2" phase="$3" next_action="$4" mode="$5"
  local status_revision="${6:-$predecessor}" status_phase="${7:-$phase}"
  local status_next_action="${8:-$next_action}"
  local fixture_root="${pending_contract_root}/${case_name}" label="pending-${case_name}"
  local log="${fixture_root}/actor-calls.tsv" harness_error="${fixture_root}/harness.stderr"
  mkdir -p "${fixture_root}/actors/maker" "${fixture_root}/actors/taker" "${fixture_root}/evidence"
  jq -n '{role:"maker"}' >"${fixture_root}/actors/maker/actor-config.json"
  jq -n '{role:"taker"}' >"${fixture_root}/actors/taker/actor-config.json"
  : >"$log"
  M3_ACTOR_CONTRACT_FAKE_ACTOR=1 FAKE_ACTOR_MODE="$mode" FAKE_ACTOR_LOG="$log" \
    FAKE_STATUS_REVISION="$status_revision" FAKE_STATUS_PHASE="$status_phase" \
    FAKE_STATUS_NEXT_ACTION="$status_next_action" \
    bash -c '
      set -euo pipefail
      source "$1" contract >/dev/null
      export M3_POC_DIRECTION_ROOT="$2"
      export M3_POC_EVIDENCE_DIR="$2/evidence"
      export M3_POC_DIRECTION=taker_sells_foreign
      export M3_POC_ACTOR_BIN="$3"
      assert_recovery_pending_both lez "$4" "$5"
    ' pending-harness "$direction_driver" "$fixture_root" \
      "${PWD}/scripts/test-m3-actor-local-poc-contract.sh" "$predecessor" "$label" \
      2>"$harness_error"
}

assert_recovery_pending_fixture_evidence() {
  local case_name="$1" label="pending-$1" role call_log
  local fixture_root="${pending_contract_root}/${case_name}"
  call_log="${fixture_root}/actor-calls.tsv"
  for role in maker taker; do
    [[ "$(awk -F '\t' -v role="$role" '
      $1 == role && $2 == "recover" { count++ } END { print count + 0 }
    ' "$call_log")" == 1 ]] || fail "${role} pending recovery was not attempted exactly once"
    [[ "$(awk -F '\t' -v role="$role" '
      $1 == role && $2 == "status" { count++ } END { print count + 0 }
    ' "$call_log")" == 1 ]] || fail "${role} predecessor was not re-proved by offline status"
    mapfile -t retained_errors < <(find "${fixture_root}/evidence" -maxdepth 1 -type f \
      -name "*${label}*${role}*.stderr" -print)
    [[ "${#retained_errors[@]}" == 1 ]] ||
      fail "${role} typed observation-unavailable stderr was not retained exactly once"
    [[ "$(tr -d '\r\n' <"${retained_errors[0]}")" == \
      "actor chain observation is unavailable" ]] ||
      fail "${role} retained recovery stderr lost the exact typed error"
  done
}

if ! run_recovery_pending_fixture predecessor-two 2 both_legs_locked observe_revealing_claim \
    typed-unavailable; then
  pending_failure="$(tr '\r\n' '  ' \
    <"${pending_contract_root}/predecessor-two/harness.stderr")"
  fail "exact typed observation-unavailable was not accepted as safe pending recovery: ${pending_failure}"
fi
assert_recovery_pending_fixture_evidence predecessor-two

if ! run_recovery_pending_fixture predecessor-three 3 maker_leg_refunded recover_taker_leg \
    typed-unavailable; then
  fail "typed observation-unavailable was not accepted at the second refund predecessor"
fi
assert_recovery_pending_fixture_evidence predecessor-three

if run_recovery_pending_fixture other-error 2 both_legs_locked observe_revealing_claim other-error; then
  fail "pending recovery accepted a non-observation actor error"
fi

if run_recovery_pending_fixture wrong-predecessor 2 both_legs_locked observe_revealing_claim \
    typed-unavailable 3 maker_leg_refunded recover_taker_leg; then
  fail "pending recovery accepted offline status beyond its predecessor"
fi

cleanup_pending_contract_root
trap - EXIT

lez_retry_source="$(sed -n '/^actor_invoke_recovery_pending_retry() {$/,/^}$/p' \
  "$direction_driver")"
[[ -n "$lez_retry_source" ]] ||
  fail "LEZ refund submission lacks a bounded typed-error retry helper"
rg -q 'for attempt in \{1\.\.[0-9]+\}; do' <<<"$lez_retry_source" ||
  fail "LEZ refund submission retry is not statically bounded"
lez_submit_source="$(sed -n '/^submit_actor_lez_refund() {$/,/^}$/p' "$direction_driver")"
rg -Fq 'actor_invoke_recovery_pending_retry' <<<"$lez_submit_source" ||
  fail "LEZ refund submitter does not use its bounded typed-error retry helper"

lez_retry_contract_root="$(mktemp -d /tmp/m3-lez-refund-retry-contract.XXXXXX)"
cleanup_lez_retry_contract_root() {
  rm -rf -- "$lez_retry_contract_root"
}
trap cleanup_lez_retry_contract_root EXIT

run_lez_refund_retry_fixture() {
  local case_name="$1" mode="$2"
  local fixture_root="${lez_retry_contract_root}/${case_name}" label="lez-retry-${case_name}"
  local log="${fixture_root}/actor-calls.tsv" count="${fixture_root}/lez-submissions.count"
  local attempts="${fixture_root}/actor-attempts.count"
  mkdir -p "${fixture_root}/actors/maker" "${fixture_root}/evidence"
  jq -n '{role:"maker"}' >"${fixture_root}/actors/maker/actor-config.json"
  : >"$log"
  printf '2\n' >"$count"
  printf '0\n' >"$attempts"
  M3_ACTOR_CONTRACT_FAKE_ACTOR=1 FAKE_ACTOR_MODE="$mode" FAKE_ACTOR_LOG="$log" \
    FAKE_ACTOR_ATTEMPTS="$attempts" FAKE_LEZ_SUBMISSION_COUNT="$count" \
    FAKE_LEZ_PREDECESSOR=2 \
    bash -c '
      set -euo pipefail
      source "$1" contract >/dev/null
      export M3_POC_DIRECTION_ROOT="$2"
      export M3_POC_EVIDENCE_DIR="$2/evidence"
      export M3_POC_DIRECTION=taker_sells_foreign
      export M3_POC_ACTOR_BIN="$3"
      submission_count_file="$4"
      lez_successful_submission_count() { tr -d "\\r\\n" <"$submission_count_file"; }
      actor_invoke_recovery_pending_retry maker 2 lez "$5"
      printf "%s\\n" "$actor_last_output" >"$2/actor-last-output.path"
    ' lez-retry-harness "$direction_driver" "$fixture_root" \
      "${PWD}/scripts/test-m3-actor-local-poc-contract.sh" "$count" "$label" \
      2>"${fixture_root}/harness.stderr"
}

if ! run_lez_refund_retry_fixture moving-tip-success lez-moving-tip-then-success; then
  lez_retry_failure="$(tr '\r\n' '  ' \
    <"${lez_retry_contract_root}/moving-tip-success/harness.stderr")"
  fail "LEZ refund did not retry typed pre-send moving-tip unavailability: ${lez_retry_failure}"
fi
moving_tip_root="${lez_retry_contract_root}/moving-tip-success"
[[ "$(<"${moving_tip_root}/actor-attempts.count")" == 3 ]] ||
  fail "LEZ refund did not converge after the bounded fake moving-tip retries"
[[ "$(<"${moving_tip_root}/lez-submissions.count")" == 3 ]] ||
  fail "LEZ refund retry did not preserve exactly one durable submission"
final_lez_retry_output="$(<"${moving_tip_root}/actor-last-output.path")"
jq -e '
  .schema_version == 1 and .role == "maker" and .command == "recover"
  and .outcome == "awaiting_observation" and .chain == "lez" and .revision == 2
' "$final_lez_retry_output" >/dev/null ||
  fail "LEZ refund retry did not retain the predecessor awaiting-observation result"
for attempt in 1 2; do
  mapfile -t attempt_errors < <(find "${moving_tip_root}/evidence" -maxdepth 1 -type f \
    -name "*attempt-${attempt}.stderr" -print)
  [[ "${#attempt_errors[@]}" == 1 ]] ||
    fail "LEZ refund retry did not retain exactly one stderr for attempt ${attempt}"
  [[ "$(tr -d '\r\n' <"${attempt_errors[0]}")" == \
    "actor chain observation is unavailable" ]] ||
    fail "LEZ refund retry did not retain the exact typed moving-tip error"
  [[ ! -s "${attempt_errors[0]%.stderr}.json" ]] ||
    fail "LEZ refund retry accepted nonempty stdout from a failed typed attempt"
done

if run_lez_refund_retry_fixture other-error lez-other-error; then
  fail "LEZ refund retry accepted a non-observation actor error"
fi
[[ "$(<"${lez_retry_contract_root}/other-error/actor-attempts.count")" == 1 ]] ||
  fail "LEZ refund retry retried a non-observation actor error"
[[ "$(<"${lez_retry_contract_root}/other-error/lez-submissions.count")" == 2 ]] ||
  fail "rejected LEZ actor error changed the durable submission count"

if run_lez_refund_retry_fixture nonempty-typed lez-nonempty-typed; then
  fail "LEZ refund retry accepted typed failure with nonempty stdout"
fi
[[ "$(<"${lez_retry_contract_root}/nonempty-typed/actor-attempts.count")" == 1 ]] ||
  fail "LEZ refund retry retried typed failure carrying ambiguous stdout"

if run_lez_refund_retry_fixture typed-after-send lez-typed-after-send; then
  fail "LEZ refund retry accepted typed unavailability after a durable send"
fi
[[ "$(<"${lez_retry_contract_root}/typed-after-send/actor-attempts.count")" == 1 ]] ||
  fail "LEZ refund retry risked a duplicate after the durable count changed"
[[ "$(<"${lez_retry_contract_root}/typed-after-send/lez-submissions.count")" == 3 ]] ||
  fail "typed-after-send fixture did not expose the ambiguous durable submission"

if rg -Fq 'actor_invoke_lez_refund_restart_retry' <<<"$lez_submit_source" ||
   rg -Fq 'accepted-restart' <<<"$lez_submit_source"; then
  fail "LEZ refund restarts actors against the stale pre-finality discovery window"
fi

lez_preprojection_source="$(sed -n \
  '/^assert_lez_refund_preprojection_status_both() {$/,/^}$/p' "$direction_driver")"
[[ -n "$lez_preprojection_source" ]] ||
  fail "LEZ refund lacks offline predecessor proof after its durable submission"
rg -Fq 'actor_invoke "$role" status' <<<"$lez_preprojection_source" ||
  fail "LEZ refund predecessor proof is not offline actor status"
rg -Fq '.revision == $revision' <<<"$lez_preprojection_source" ||
  fail "LEZ refund offline status does not retain the predecessor revision"
rg -Fq 'lez_successful_submission_count' <<<"$lez_preprojection_source" ||
  fail "LEZ refund offline predecessor proof does not bind the durable submission count"
rg -q '==[[:space:]]*"?3"?' <<<"$lez_preprojection_source" ||
  fail "LEZ refund offline predecessor proof does not require exactly three durable submissions"

lez_source_line() {
  local needle="$1" line
  line="$(rg -n -m1 -F -- "$needle" <<<"$lez_submit_source" | cut -d: -f1)"
  [[ "$line" =~ ^[0-9]+$ ]] || fail "LEZ refund source is missing ordered step: ${needle}"
  printf '%s\n' "$line"
}
lez_count_three_line="$(lez_source_line '[[ "$after_count" == 3 ]]')"
lez_offline_line="$(lez_source_line 'assert_lez_refund_preprojection_status_both')"
lez_finality_line="$(lez_source_line 'prove_lez_finalized_transaction "$label" "$lez_refund_tx" "$refund_start"')"
lez_window_line="$(lez_source_line 'window_blocks=$((lez_proved_tip - refund_start))')"
lez_write_window_line="$(lez_source_line 'write_actor_configs "$((refund_start + 1))" "$window_blocks"')"
lez_project_line="$(lez_source_line 'project_both_refunds_to_revision "$expected_revision" lez "$phase" "$label"')"
if (( lez_count_three_line >= lez_offline_line ||
      lez_offline_line >= lez_finality_line ||
      lez_finality_line >= lez_window_line ||
      lez_window_line >= lez_write_window_line ||
      lez_write_window_line >= lez_project_line )); then
  fail "LEZ refund must prove count/offline predecessor, finality, and new window before projection"
fi
pre_finality_restart_source="$(sed -n \
  "${lez_offline_line},${lez_finality_line}p" <<<"$lez_submit_source")"
if rg -q 'actor_invoke([^[:alnum:]_]|_.*recover)|recover.*accepted-restart' \
    <<<"$pre_finality_restart_source"; then
  fail "LEZ refund invokes an actor recovery between offline proof and exact finality"
fi

cleanup_lez_retry_contract_root
trap - EXIT

finalized_funding_observation_source="$(sed -n \
  '/^write_finalized_witnessed_funding_observation() {$/,/^}$/p' "$direction_driver")"
[[ -n "$finalized_funding_observation_source" ]] ||
  fail "finalized witnessed-funding observation helper is unavailable"
rg -Fq 'for attempt in {1..120}; do' <<<"$finalized_funding_observation_source" ||
  fail "finalized witnessed-funding observation lacks a static retry bound"
rg -Fq 'bridge observation unavailable: moving_tip' \
  <<<"$finalized_funding_observation_source" ||
  fail "finalized witnessed-funding observation does not restrict retries to typed moving-tip"
rg -Fq 'new_request_id' <<<"$finalized_funding_observation_source" ||
  fail "finalized witnessed-funding retry does not allocate a fresh request id"
rg -Fq 'observation_only:true' <<<"$finalized_funding_observation_source" ||
  fail "finalized witnessed-funding retry evidence does not distinguish read-only observation"
operator_call_source="$(sed -n '/^operator_call() {$/,/^}$/p' "$direction_driver")"
rg -Fq 'return 1' <<<"$operator_call_source" ||
  fail "operator wrapper can mask a failed command when called from a retry conditional"
lez_operator_source="crates/lez-bridge-client/examples/m3_witnessed_lez_operator.rs"
rg -Fq 'ErrorCode::MovingTip' "$lez_operator_source" ||
  fail "witnessed operator erases the typed moving-tip category"
rg -Fq 'bridge observation unavailable: moving_tip' "$lez_operator_source" ||
  fail "witnessed operator lacks a stable secret-free moving-tip diagnostic"

rg -Fq 'planned_bitcoin_funding_anchor_height' "$direction_driver" ||
  fail "Bitcoin lock path does not bind the actual mined height to the planned anchor"
rg -Fq 'exact_transaction_occurrences:1' "$direction_driver" ||
  fail "Bitcoin lock path does not retain exact containing-block membership"
actor_config_source="$(sed -n '/^write_actor_configs() {$/,/^}$/p' "$direction_driver")"
rg -Fq 'schema_version:4,role:$role' <<<"$actor_config_source" ||
  fail "actor configs do not use schema version 4"
rg -Fq 'exact_funding_transaction_file:$maker_bitcoin_funding' \
  <<<"$actor_config_source" ||
  fail "Bitcoin Maker config does not name the exact signed funding transaction"
rg -Fq 'preparation_request_file:$maker_lez_request' <<<"$actor_config_source" ||
  fail "LEZ Maker config does not name the exact preparation request"
rg -Fq 'preparation_result_file:$maker_lez_result' <<<"$actor_config_source" ||
  fail "LEZ Maker config does not name the exact preparation result"
rg -Fq 'if $role == "maker" then {maker_lock:' <<<"$actor_config_source" ||
  fail "maker_lock is not restricted to the Maker config"

top_level_direction_contract_source="$(sed -n \
  '/^verify_direction_driver_contract() {$/,/^}$/p' "$runner")"
[[ -n "$top_level_direction_contract_source" ]] ||
  fail "top-level runner lacks its embedded direction-contract verifier"
rg -Fq '.actor_config_schema_version == 4' \
  <<<"$top_level_direction_contract_source" ||
  fail "top-level runner still rejects schema-4 actor configs"
for term in '.actor_owned_maker_lock_effects == true' \
  '.taker_first_lock_external_runner_submission == true' \
  '.maker_lock_restart_never_resubmits == true' \
  '.runner_only_confirms_actor_submitted_maker_locks == true'; do
  rg -Fq "$term" <<<"$top_level_direction_contract_source" ||
    fail "top-level runner does not verify direction contract field: ${term}"
done

bitcoin_first_lock_source="$(sed -n \
  '/^submit_taker_bitcoin_first_lock() {$/,/^}$/p' "$direction_driver")"
lez_first_lock_source="$(sed -n \
  '/^submit_taker_lez_first_lock_pair() {$/,/^}$/p' "$direction_driver")"
lez_external_submission_source="$(sed -n \
  '/^submit_lez_transaction_once() {$/,/^}$/p' "$direction_driver")"
bitcoin_maker_lock_source="$(sed -n \
  '/^submit_actor_maker_bitcoin_second_lock() {$/,/^}$/p' "$direction_driver")"
bitcoin_lock_confirmation_source="$(sed -n \
  '/^confirm_bitcoin_lock_after_submission() {$/,/^}$/p' "$direction_driver")"
lez_maker_lock_source="$(sed -n \
  '/^submit_actor_maker_lez_second_lock_pair() {$/,/^}$/p' "$direction_driver")"
maker_lock_awaiting_retry_source="$(sed -n \
  '/^actor_invoke_awaiting_retry() {$/,/^}$/p' "$direction_driver")"
[[ -n "$maker_lock_awaiting_retry_source" ]] ||
  fail "Maker-lock path lacks a bounded typed observation retry helper"
rg -Fq 'for attempt in {1..120}; do' <<<"$maker_lock_awaiting_retry_source" ||
  fail "Maker-lock observation retry is not statically bounded"
rg -Fq 'actor chain observation is unavailable' <<<"$maker_lock_awaiting_retry_source" ||
  fail "Maker-lock retry does not restrict itself to typed observation unavailability"
rg -Fq '[[ ! -s "$attempt_output" ]]' <<<"$maker_lock_awaiting_retry_source" ||
  fail "Maker-lock retry can continue after an ambiguous actor stdout"
for term in \
  'durable_count >= minimum_count && durable_count <= target_count' \
  '[[ "$(lez_successful_submission_count)" == "$target_count" ]]'; do
  rg -Fq "$term" <<<"$maker_lock_awaiting_retry_source" ||
    fail "Maker-lock retry does not enforce its durable submission bound: ${term}"
done
rg -Fq 'sendrawtransaction' <<<"$bitcoin_first_lock_source" ||
  fail "Taker Bitcoin first-lock helper no longer performs its external submission"
rg -Fq 'submit_lez_transaction_once taker' <<<"$lez_first_lock_source" ||
  fail "Taker LEZ first-lock helper no longer performs its external submissions"
rg -Fq '"$M3_POC_DIRECTION" == "taker_sells_lez" && "$role" == "taker"' \
  <<<"$lez_external_submission_source" ||
  fail "external LEZ submit helper is not fail-closed to the Taker first lock"
if rg -Fq 'sendrawtransaction' <<<"$bitcoin_maker_lock_source"; then
  fail "runner still submits the Maker Bitcoin second lock externally"
fi
if rg -Fq 'submit_lez_transaction_once' <<<"$lez_maker_lock_source" ||
   rg -Fq 'operator_call final maker submit-transaction' <<<"$lez_maker_lock_source"; then
  fail "runner still submits a Maker LEZ second-lock member externally"
fi
for source in "$bitcoin_maker_lock_source" "$lez_maker_lock_source"; do
  rg -q 'actor_invoke(_awaiting_retry)? maker drive' <<<"$source" ||
    fail "Maker second-lock path does not use a fresh Maker actor process"
  rg -Fq '.outcome == "awaiting_observation"' <<<"$source" ||
    fail "Maker second-lock path does not validate the settled actor output"
done
rg -Fq 'confirm_bitcoin_lock_after_submission maker taker' \
  <<<"$bitcoin_maker_lock_source" ||
  fail "Maker Bitcoin path does not hand the actor-submitted effect to confirmation"
rg -Fq 'mine_one_core_block' <<<"$bitcoin_lock_confirmation_source" ||
  fail "runner does not mine the actor-submitted Bitcoin maker lock"
rg -Fq 'prove_lez_finalized_transaction' <<<"$lez_maker_lock_source" ||
  fail "runner does not prove actor-submitted LEZ maker-lock finality"
rg -Fq 'lez_successful_submission_count' <<<"$lez_maker_lock_source" ||
  fail "LEZ maker-lock path does not retain exact submission counts"
for invocation in \
  'actor_invoke_awaiting_retry maker drive lez taker_lock_confirmed 1 0 1' \
  'actor_invoke_awaiting_retry maker drive lez taker_lock_confirmed 1 1 1' \
  'actor_invoke_awaiting_retry maker drive lez taker_lock_confirmed 1 1 2' \
  'actor_invoke_awaiting_retry maker drive lez taker_lock_confirmed 1 2 2'; do
  rg -Fq "$invocation" <<<"$lez_maker_lock_source" ||
    fail "LEZ Maker lock path is missing retry/count policy: ${invocation}"
done

maker_lez_source_line() {
  local needle="$1" line
  line="$(rg -n -m1 -F -- "$needle" <<<"$lez_maker_lock_source" | cut -d: -f1)"
  [[ "$line" =~ ^[0-9]+$ ]] ||
    fail "LEZ maker-lock source is missing ordered step: ${needle}"
  printf '%s\n' "$line"
}

lez_init_submit_line="$(maker_lez_source_line \
  'lez-maker-initialization-submit')"
lez_init_restart_line="$(maker_lez_source_line \
  'lez-maker-initialization-accepted-restart')"
lez_init_finality_line="$(maker_lez_source_line \
  'prove_lez_finalized_transaction lez-initialization')"
lez_init_window_line="$(maker_lez_source_line \
  'initialization_window_blocks=$((lez_proved_tip - initial_start))')"
lez_init_write_line="$(maker_lez_source_line \
  'write_actor_configs "$((initial_start + 1))" "$initialization_window_blocks"')"
lez_init_observe_line="$(maker_lez_source_line \
  'lez-maker-initialization-finalized-observe')"
lez_init_count_stable_line="$(maker_lez_source_line \
  '[[ "$(lez_successful_submission_count)" == 1 ]]')"
lez_funding_submit_line="$(maker_lez_source_line \
  'lez-maker-funding-submit')"
lez_funding_count_line="$(maker_lez_source_line '[[ "$after_count" == 2 ]]')"
lez_funding_restart_line="$(maker_lez_source_line \
  'lez-maker-funding-accepted-restart')"
lez_funding_finality_line="$(maker_lez_source_line \
  'prove_lez_finalized_transaction lez-funding')"
lez_pair_window_line="$(maker_lez_source_line \
  'lez_lock_window_blocks=$((lez_proved_tip - initial_start))')"
lez_pair_write_line="$(maker_lez_source_line \
  'write_actor_configs "$lez_lock_window_start" "$lez_lock_window_blocks"')"
lez_pair_assert_line="$(maker_lez_source_line 'assert_lez_pair_inside_actor_window')"

if (( lez_init_submit_line >= lez_init_restart_line ||
      lez_init_restart_line >= lez_init_finality_line ||
      lez_init_finality_line >= lez_init_window_line ||
      lez_init_window_line >= lez_init_write_line ||
      lez_init_write_line >= lez_init_observe_line ||
      lez_init_observe_line >= lez_init_count_stable_line ||
      lez_init_count_stable_line >= lez_funding_submit_line ||
      lez_funding_submit_line >= lez_funding_count_line ||
      lez_funding_count_line >= lez_funding_restart_line ||
      lez_funding_restart_line >= lez_funding_finality_line ||
      lez_funding_finality_line >= lez_pair_window_line ||
      lez_pair_window_line >= lez_pair_write_line ||
      lez_pair_write_line >= lez_pair_assert_line )); then
  fail "LEZ Maker lock must refresh the init window, observe canonical init, then fund and prove the full pair"
fi

lez_pair_assertion_source="$(sed -n \
  '/^assert_lez_pair_inside_actor_window() {$/,/^}$/p' "$direction_driver")"
[[ -n "$lez_pair_assertion_source" ]] ||
  fail "LEZ Maker lock lacks a full-pair actor-window assertion"
for term in 'initialization_block' 'funding_block' \
  'window_end=$((lez_lock_window_start + lez_lock_window_blocks - 1))' \
  'window_end == lez_proved_tip' \
  'initialization_block >= lez_lock_window_start' \
  'funding_block >= lez_lock_window_start' \
  'initialization_and_funding_inside_window:true'; do
  rg -Fq "$term" <<<"$lez_pair_assertion_source" ||
    fail "LEZ Maker full-pair window assertion is missing: ${term}"
done
rg -Fq 'taker_sells_foreign:taker|taker_sells_lez:maker' "$direction_driver" ||
  fail "Bitcoin refund authority is not direction/role shaped"
rg -Fq 'bitcoin_refund_key_file:$refund' "$direction_driver" ||
  fail "Bitcoin funder config does not carry its private refund key"
rg -Fq 'xxd -p -c 32 "$source" >"$refund_destination"' "$direction_driver" ||
  fail "raw provisioned refund material is not encoded for the actor boundary"
rg -Fq 'Bitcoin non-funder must not receive refund authority' "$direction_driver" ||
  fail "runner does not assert refund-authority separation"

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

invalid_journey_run_id="m3badjourney-$RANDOM-$$"
if invalid_journey_output="$(RUN_ID="$invalid_journey_run_id" \
  M3_ACTOR_POC_JOURNEY=invalid-journey M3_ACTOR_POC_MODE=contract "$runner" 2>&1)"; then
  fail "an invalid journey reached contract output"
fi
[[ "$invalid_journey_output" == *"M3_ACTOR_POC_JOURNEY must be claim, survivor_claim, refund, or first_lock_refund"* ]] ||
  fail "invalid journey did not fail with the bounded validation error"
[[ ! -e ".e2e/${invalid_journey_run_id}" && ! -L ".e2e/${invalid_journey_run_id}" ]] ||
  fail "invalid journey created run state"

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
  and .service_configuration.lez_v0_2.slot_duration_seconds == "1.0"
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
  and .survivor == null
  and .finality.bitcoin == "exact_signed_confirmation_depth"
  and .finality.lez == "exact_finalized_indexer_ancestry"
  and .terminal.required_revision == 4
  and .terminal.required_phase == "completed"
  and .terminal.required_next_action == "complete"
  and .replay.restart_both_roles == true
  and .replay.resubmission_count == 0
  and .isolation.dynamic_literal_loopback_ports == true
  and .isolation.secure_reservation_state == "exact_run_owned_tmp_root"
  and .isolation.foreign_resource_mutation == false
  and .cleanup.captured_exact_ids_only == true
  and .cleanup.secure_reservation_state_root_removed == true
  and .cleanup.process_exit_race_silent == true
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
  and .external_resources.bedrock_ntp == {
    endpoint:"pool.ntp.org:123/udp",
    attempted_by_pinned_component:true,
    required_for_certification:false
  }
' <<<"$contract_json" >/dev/null || fail "contract JSON does not prove the M3 invariants"

survivor_contract_run_id="m3survivorcontract-$RANDOM-$$"
survivor_contract_json="$(RUN_ID="$survivor_contract_run_id" \
  M3_ACTOR_POC_JOURNEY=survivor_claim M3_ACTOR_POC_MODE=contract "$runner")"
jq -e --arg run_id "$survivor_contract_run_id" '
  .schema_version == 1
  and .kind == "m3_actor_local_poc_contract"
  and .execution_performed == false
  and .run_id == $run_id
  and .journey == "survivor_claim"
  and .evidence_packet_kind == "m3_actor_two_direction_survivor_claim_local_poc"
  and .service_configuration.lez_v0_2.slot_duration_seconds == "1.0"
  and .ordering.dual_locks_before_scalar_use == true
  and .effect_semantics.actor_owned == "survivor_claim"
  and .effect_semantics.expected_unique_effects_by_direction == {
    taker_sells_foreign:{bitcoin:2,lez:3},
    taker_sells_lez:{bitcoin:2,lez:3}}
  and .survivor == {
    revealer:"taker",follower_role:"maker",
    revealer_absent_after_reveal_until_follower_terminal:true,
    fresh_follower_observes_revision_three:true,
    intermediate_phase:"claim_evidence_available",
    intermediate_lifecycle_disposition:"recovering",
    intermediate_terminal:false,
    remaining_leg_must_be_canonical_and_claimable:true,
    follower_restart_before_followup:true,
    delayed_revealer_catchup_observation_only:true}
  and (.survivor | has("follower") | not)
  and .terminal.required_revision == 4
  and .terminal.required_phase == "completed"
  and .replay.command == "drive"
  and .replay.resubmission_count == 0
' <<<"$survivor_contract_json" >/dev/null ||
  fail "survivor contract omits protected absence, nonterminal recovery, or delayed catch-up"

refund_contract_run_id="m3refundcontract-$RANDOM-$$"
refund_contract_json="$(RUN_ID="$refund_contract_run_id" M3_ACTOR_POC_JOURNEY=refund \
  M3_ACTOR_POC_MODE=contract "$runner")"
jq -e --arg run_id "$refund_contract_run_id" '
  .schema_version == 1
  and .kind == "m3_actor_local_poc_contract"
  and .execution_performed == false
  and .run_id == $run_id
  and .journey == "refund"
  and .service_configuration.lez_v0_2.slot_duration_seconds == "3.0"
  and .terminal.required_phase == "refunded"
' <<<"$refund_contract_json" >/dev/null ||
  fail "refund contract does not select the reproducible three-second LEZ cadence"

first_lock_contract_run_id="m3firstlockcontract-$RANDOM-$$"
first_lock_contract_json="$(RUN_ID="$first_lock_contract_run_id" \
  M3_ACTOR_POC_JOURNEY=first_lock_refund M3_ACTOR_POC_MODE=contract "$runner")"
jq -e --arg run_id "$first_lock_contract_run_id" '
  .schema_version == 1
  and .kind == "m3_actor_local_poc_contract"
  and .execution_performed == false
  and .run_id == $run_id
  and .journey == "first_lock_refund"
  and .evidence_packet_kind == "m3_actor_two_direction_first_lock_refund_local_poc"
  and .service_configuration.lez_v0_2.slot_duration_seconds == "3.0"
  and .ordering.dual_locks_before_scalar_use == false
  and .ordering.first_lock_refund_has_no_second_lock_or_dual_lock_gate == true
  and .effect_semantics.actor_owned == "first_lock_refund"
  and .effect_semantics.expected_unique_effects_by_direction == {
    taker_sells_foreign:{bitcoin:2,lez:0},
    taker_sells_lez:{bitcoin:0,lez:3}
  }
  and .effect_semantics.maker_second_lock_effect_count == 0
  and .first_lock_refund == {
    signed_maker_second_lock_cutoff_required:true,
    two_fresh_absence_and_first_lock_unspent_reads_required:true,
    lez_absence_window_reaches_current_finalized_tip:true,
    bitcoin_cutoff_clock:"stable_core_median_time",
    actor_internal_admission_is_authoritative:true,
    owner_restart_without_resubmission:true,
    fresh_maker_observer_terminal:true,
    maker_offline_after_activation_until_refund_finality:true,
    taker_only_revision_one_and_refund_projection:true
  }
  and .terminal.required_revision == 2
  and .terminal.required_phase == "refunded"
  and .terminal.required_next_action == "complete"
  and .replay.command == "recover"
  and .replay.restart_both_roles == true
  and .replay.resubmission_count == 0
  and .external_resources.public_rpc == false
  and .external_resources.faucet == false
  and .external_resources.public_funds == false
  and .external_resources.bedrock_ntp == {
    endpoint:"pool.ntp.org:123/udp",
    attempted_by_pinned_component:true,
    required_for_certification:false
  }
' <<<"$first_lock_contract_json" >/dev/null ||
  fail "first-lock refund contract omits exact effects, cutoff/race clocks, replay, or local resources"

rg -Fq 'LEZ_V02_SLOT_DURATION_SECONDS="$lez_slot_duration_seconds"' "$runner" ||
  fail "M3 runner does not pass the journey-selected cadence to the LEZ child"
rg -Fq 'slot_duration_seconds:$lez_slot_duration_seconds' "$runner" ||
  fail "M3 evidence does not record the selected LEZ slot cadence"
run_evidence_source="$(sed -n '/^write_run_evidence() {$/,/^}$/p' "$runner")"
[[ -n "$run_evidence_source" ]] || fail "outer runner is missing final run evidence"
rg -Fq 'revealer:$foreign_survivor.revealer' <<<"$run_evidence_source" ||
  fail "final survivor packet does not derive the revealer from validated evidence"
rg -Fq 'follower_role:$foreign_survivor.follower_role' <<<"$run_evidence_source" ||
  fail "final survivor packet does not derive the follower role from validated evidence"
if rg -Fq 'revealer:"taker",follower:"maker"' <<<"$run_evidence_source"; then
  fail "final survivor packet reintroduced the duplicate follower-key collision"
fi
survivor_validator_source="$(sed -n \
  '/^validate_survivor_direction_evidence() {$/,/^}$/p' "$runner")"
[[ -n "$survivor_validator_source" ]] ||
  fail "outer runner does not validate hashed survivor direction evidence"
for term in '--slurpfile recovering' '--slurpfile reveal_output' \
  '--slurpfile followup_output' '--slurpfile bitcoin_before' \
  '--slurpfile bitcoin_after' 'recovering_evidence_sha256' \
  'successful_resubmission_count'; do
  rg -Fq -- "$term" <<<"$survivor_validator_source" ||
    fail "survivor direction validator is missing bound input: ${term}"
done
rg -Fq 'validate_survivor_direction_evidence taker_sells_foreign lez bitcoin' \
  <<<"$run_evidence_source" ||
  fail "aggregate evidence does not validate the foreign-first survivor packet"
rg -Fq 'validate_survivor_direction_evidence taker_sells_lez bitcoin lez' \
  <<<"$run_evidence_source" ||
  fail "aggregate evidence does not validate the LEZ-first survivor packet"

survivor_validation_root="$(mktemp -d /tmp/m3-survivor-evidence-contract.XXXXXX)"
cleanup_survivor_validation_root() {
  rm -rf -- "$survivor_validation_root"
}
trap cleanup_survivor_validation_root EXIT

write_survivor_validation_fixture() {
  local direction="$1" reveal_chain="$2" followup_chain="$3"
  local reveal_tx followup_tx remaining_tx
  local maker_observation maker_revision_three followup_submit terminal_projection
  local maker_terminal reveal_output followup_output bitcoin_before bitcoin_after
  local maker_observation_sha maker_revision_three_sha followup_submit_sha
  local terminal_projection_sha maker_terminal_sha reveal_output_sha followup_output_sha
  local bitcoin_before_sha bitcoin_after_sha recovering recovering_sha completion boundary remaining
  reveal_tx="$(printf '%s' "${direction}-reveal" | sha256sum | cut -d' ' -f1)"
  followup_tx="$(printf '%s' "${direction}-followup" | sha256sum | cut -d' ' -f1)"
  remaining_tx="$(printf '%s' "${direction}-remaining" | sha256sum | cut -d' ' -f1)"
  maker_observation="${survivor_validation_root}/${direction}-survivor-maker-observe-reveal-maker.json"
  maker_revision_three="${survivor_validation_root}/${direction}-survivor-maker-revision-three-status-maker.json"
  followup_submit="${survivor_validation_root}/${direction}-survivor-${followup_chain}-followup-submit-maker.json"
  terminal_projection="${survivor_validation_root}/${direction}-survivor-maker-project-followup-maker.json"
  maker_terminal="${survivor_validation_root}/${direction}-survivor-maker-terminal-status-maker.json"
  reveal_output="${survivor_validation_root}/${direction}-survivor-delayed-taker-reveal-taker.json"
  followup_output="${survivor_validation_root}/${direction}-survivor-delayed-taker-followup-taker.json"
  bitcoin_before="${survivor_validation_root}/${direction}-survivor-catchup-bitcoin-mempool-before.json"
  bitcoin_after="${survivor_validation_root}/${direction}-survivor-catchup-bitcoin-mempool-after.json"
  jq -n --arg chain "$reveal_chain" \
    '{schema_version:1,role:"maker",revision:3,phase:"claim_evidence_available",
      chain:$chain,outcome:"observed_then_projected"}' >"$maker_observation"
  jq -n '{schema_version:1,role:"maker",revision:3,phase:"claim_evidence_available",
    next_action:"observe_followup_claim"}' >"$maker_revision_three"
  jq -n --arg chain "$followup_chain" \
    '{schema_version:1,role:"maker",revision:3,phase:"claim_evidence_available",
      chain:$chain,outcome:"awaiting_observation"}' >"$followup_submit"
  jq -n --arg chain "$followup_chain" \
    '{schema_version:1,role:"maker",revision:4,phase:"completed",
      chain:$chain,outcome:"observed_then_projected"}' >"$terminal_projection"
  jq -n '{schema_version:1,role:"maker",revision:4,phase:"completed",next_action:"complete"}' \
    >"$maker_terminal"
  jq -n --arg chain "$reveal_chain" \
    '{schema_version:1,role:"taker",revision:3,phase:"claim_evidence_available",
      chain:$chain,outcome:"observed_then_projected"}' >"$reveal_output"
  jq -n --arg chain "$followup_chain" \
    '{schema_version:1,role:"taker",revision:4,phase:"completed",
      chain:$chain,outcome:"observed_then_projected"}' >"$followup_output"
  jq -n '{jsonrpc:"2.0",id:1,error:null,result:[]}' >"$bitcoin_before"
  jq -n '{jsonrpc:"2.0",id:1,error:null,result:[]}' >"$bitcoin_after"
  maker_observation_sha="$(sha256sum "$maker_observation" | cut -d' ' -f1)"
  maker_revision_three_sha="$(sha256sum "$maker_revision_three" | cut -d' ' -f1)"
  followup_submit_sha="$(sha256sum "$followup_submit" | cut -d' ' -f1)"
  terminal_projection_sha="$(sha256sum "$terminal_projection" | cut -d' ' -f1)"
  maker_terminal_sha="$(sha256sum "$maker_terminal" | cut -d' ' -f1)"
  reveal_output_sha="$(sha256sum "$reveal_output" | cut -d' ' -f1)"
  followup_output_sha="$(sha256sum "$followup_output" | cut -d' ' -f1)"
  bitcoin_before_sha="$(sha256sum "$bitcoin_before" | cut -d' ' -f1)"
  bitcoin_after_sha="$(sha256sum "$bitcoin_after" | cut -d' ' -f1)"
  if [[ "$direction" == "taker_sells_foreign" ]]; then
    remaining="$(jq -cn --arg tx "$remaining_tx" '
      {chain:"bitcoin",transaction_id:$tx,canonical:true,unspent_or_funded:true,
       before_signed_later_refund_boundary:true}')"
    boundary='{"chain":"bitcoin","confirmed_tip_height":100,"signed_refund_height":200,"completed_before_signed_refund_boundary":true}'
  else
    remaining="$(jq -cn --arg tx "$remaining_tx" '
      {chain:"lez",transaction_id:$tx,amount:"1000",canonical:true,unspent_or_funded:true,
       before_signed_later_refund_boundary:true,metadata_status:"funded",custody_balance:"1000"}')"
    boundary="$(jq -cn --arg sha "$(printf finality | sha256sum | cut -d' ' -f1)" \
      --arg block_sha "$(printf block | sha256sum | cut -d' ' -f1)" '
      {chain:"lez",finalized_containing_block_timestamp_ms:100,signed_refund_at_ms:200,
       finality_evidence_sha256:$sha,containing_block_evidence_sha256:$block_sha,
       completed_before_signed_refund_boundary:true}')"
  fi
  recovering="${survivor_validation_root}/${direction}-survivor-recovering.json"
  jq -n --arg direction "$direction" --arg reveal_chain "$reveal_chain" \
    --arg reveal_tx "$reveal_tx" --arg followup_chain "$followup_chain" \
    --arg maker_observation_sha "$maker_observation_sha" \
    --arg maker_revision_three_sha "$maker_revision_three_sha" \
    --argjson remaining "$remaining" '
    {schema_version:1,journey:"survivor_claim",direction:$direction,
     reveal:{role:"taker",chain:$reveal_chain,transaction_id:$reveal_tx,canonical:true},
     continuation:{follower_role:"maker",canonical_reveal_observed_by_fresh_process:true,
       caller_supplied_secret:false,related_presignature_and_adaptor_point_validated:true,
       projected_revision:3},
     intermediate:{protocol_phase:"claim_evidence_available",lifecycle_disposition:"recovering",
       terminal:false,remaining_leg:$remaining,followup_effect_present:false,
       bitcoin_effect_count:(if $direction == "taker_sells_foreign" then 1 else 2 end),
       lez_effect_count:(if $direction == "taker_sells_foreign" then 3 else 2 end)},
     availability:{taker_invocations_after_reveal_before_maker_terminal:0,
       taker_absence_guard_enforced:true,follower_process_exited_at_revision_three:true},
     process_evidence:{maker_reveal_observation_sha256:$maker_observation_sha,
       maker_revision_three_status_sha256:$maker_revision_three_sha},
     secret_recorded:false,delivery_or_chat_used:false}
  ' >"$recovering"
  recovering_sha="$(sha256sum "$recovering" | cut -d' ' -f1)"
  completion="${survivor_validation_root}/${direction}-survivor-claim.json"
  jq -n --arg direction "$direction" --arg reveal_chain "$reveal_chain" \
    --arg followup_chain "$followup_chain" --arg reveal_tx "$reveal_tx" \
    --arg followup_tx "$followup_tx" --arg recovering_sha "$recovering_sha" \
    --arg maker_terminal_sha "$maker_terminal_sha" \
    --arg followup_submit_sha "$followup_submit_sha" \
    --arg terminal_projection_sha "$terminal_projection_sha" \
    --arg reveal_output_sha "$reveal_output_sha" --arg followup_output_sha "$followup_output_sha" \
    --arg bitcoin_before_sha "$bitcoin_before_sha" --arg bitcoin_after_sha "$bitcoin_after_sha" \
    --argjson boundary "$boundary" '
    {schema_version:1,journey:"survivor_claim",direction:$direction,
     reveal:{role:"taker",chain:$reveal_chain,transaction_id:$reveal_tx,canonical:true},
     continuation:{follower_role:"maker",canonical_reveal_observed_by_fresh_process:true,
       caller_supplied_secret:false,related_presignature_and_adaptor_point_validated:true,
       projected_revision:3},
     intermediate:{protocol_phase:"claim_evidence_available",lifecycle_disposition:"recovering",
       terminal:false,recovering_evidence_sha256:$recovering_sha},
     availability:{taker_invocations_after_reveal_before_maker_terminal:0,
       taker_absence_guard_enforced:true,follower_process_exited_at_revision_three:true,
       fresh_follower_process_submitted_followup:true,
       distinct_fresh_follower_process_projected_terminal:true,
       followup_submission_output_sha256:$followup_submit_sha,
       terminal_projection_output_sha256:$terminal_projection_sha},
     completion:{followup_role:"maker",chain:$followup_chain,transaction_id:$followup_tx,
       canonical:true,maker_revision:4,phase:"completed",
       maker_terminal_status_sha256:$maker_terminal_sha,boundary:$boundary},
     delayed_revealer_catchup:{began_after_maker_terminal:true,revisions:[3,4],observation_only:true,
       actor_observations:{
         reveal:{chain:$reveal_chain,revision:3,outcome:"observed_then_projected",sha256:$reveal_output_sha},
         followup:{chain:$followup_chain,revision:4,outcome:"observed_then_projected",sha256:$followup_output_sha}},
       per_chain:{
         bitcoin:{actor_observation_sha256:
             (if $reveal_chain == "bitcoin" then $reveal_output_sha else $followup_output_sha end),
           mempool_before_count:0,mempool_after_count:0,
           bitcoin_mempool_before_sha256:$bitcoin_before_sha,
           bitcoin_mempool_after_sha256:$bitcoin_after_sha,successful_resubmission_count:0},
         lez:{actor_observation_sha256:
             (if $reveal_chain == "lez" then $reveal_output_sha else $followup_output_sha end),
           durable_submission_count_before:3,durable_submission_count_after:3,
           successful_resubmission_count:0}},successful_resubmission_count:0},
     secret_recorded:false,delivery_or_chat_used:false}
  ' >"$completion"
}

validate_survivor_fixture() {
  local direction="$1" reveal_chain="$2" followup_chain="$3"
  bash -c '
    set -euo pipefail
    evidence_dir="$1"
    fail() { echo "isolated survivor validator failed: $*" >&2; exit 2; }
    eval "$2"
    validate_survivor_direction_evidence "$3" "$4" "$5"
  ' survivor-validator "$survivor_validation_root" "$survivor_validator_source" \
    "$direction" "$reveal_chain" "$followup_chain"
}

for fixture_spec in 'taker_sells_foreign lez bitcoin' 'taker_sells_lez bitcoin lez'; do
  read -r fixture_direction fixture_reveal fixture_followup <<<"$fixture_spec"
  write_survivor_validation_fixture "$fixture_direction" "$fixture_reveal" "$fixture_followup"
  validate_survivor_fixture "$fixture_direction" "$fixture_reveal" "$fixture_followup" \
    >/dev/null || fail "strict survivor validator rejected exact ${fixture_direction} evidence"
done

jq '.direction = "taker_sells_lez"' \
  "${survivor_validation_root}/taker_sells_foreign-survivor-claim.json" \
  >"${survivor_validation_root}/taker_sells_foreign-survivor-claim.json.partial"
mv "${survivor_validation_root}/taker_sells_foreign-survivor-claim.json.partial" \
  "${survivor_validation_root}/taker_sells_foreign-survivor-claim.json"
if validate_survivor_fixture taker_sells_foreign lez bitcoin >/dev/null 2>&1; then
  fail "survivor validator accepted a swapped completion direction"
fi
write_survivor_validation_fixture taker_sells_foreign lez bitcoin
jq '.intermediate.remaining_leg.canonical = false' \
  "${survivor_validation_root}/taker_sells_foreign-survivor-recovering.json" \
  >"${survivor_validation_root}/taker_sells_foreign-survivor-recovering.json.partial"
mv "${survivor_validation_root}/taker_sells_foreign-survivor-recovering.json.partial" \
  "${survivor_validation_root}/taker_sells_foreign-survivor-recovering.json"
mutated_recovering_sha="$(sha256sum \
  "${survivor_validation_root}/taker_sells_foreign-survivor-recovering.json" | cut -d' ' -f1)"
jq --arg sha "$mutated_recovering_sha" '.intermediate.recovering_evidence_sha256 = $sha' \
  "${survivor_validation_root}/taker_sells_foreign-survivor-claim.json" \
  >"${survivor_validation_root}/taker_sells_foreign-survivor-claim.json.partial"
mv "${survivor_validation_root}/taker_sells_foreign-survivor-claim.json.partial" \
  "${survivor_validation_root}/taker_sells_foreign-survivor-claim.json"
if validate_survivor_fixture taker_sells_foreign lez bitcoin >/dev/null 2>&1; then
  fail "survivor validator accepted a hash-consistent noncanonical remaining leg"
fi
write_survivor_validation_fixture taker_sells_lez bitcoin lez
jq '.result = ["unexpected-bitcoin-resubmission"]' \
  "${survivor_validation_root}/taker_sells_lez-survivor-catchup-bitcoin-mempool-after.json" \
  >"${survivor_validation_root}/taker_sells_lez-survivor-catchup-bitcoin-mempool-after.json.partial"
mv "${survivor_validation_root}/taker_sells_lez-survivor-catchup-bitcoin-mempool-after.json.partial" \
  "${survivor_validation_root}/taker_sells_lez-survivor-catchup-bitcoin-mempool-after.json"
mutated_mempool_sha="$(sha256sum \
  "${survivor_validation_root}/taker_sells_lez-survivor-catchup-bitcoin-mempool-after.json" | \
  cut -d' ' -f1)"
jq --arg sha "$mutated_mempool_sha" \
  '.delayed_revealer_catchup.per_chain.bitcoin.bitcoin_mempool_after_sha256 = $sha' \
  "${survivor_validation_root}/taker_sells_lez-survivor-claim.json" \
  >"${survivor_validation_root}/taker_sells_lez-survivor-claim.json.partial"
mv "${survivor_validation_root}/taker_sells_lez-survivor-claim.json.partial" \
  "${survivor_validation_root}/taker_sells_lez-survivor-claim.json"
if validate_survivor_fixture taker_sells_lez bitcoin lez >/dev/null 2>&1; then
  fail "survivor validator accepted a hash-consistent Bitcoin catch-up resubmission"
fi

cleanup_survivor_validation_root
trap - EXIT
rg -Fq 'observed_timeout_count:$bedrock_ntp_timeout_count' <<<"$run_evidence_source" ||
  fail "final evidence does not record the observed pinned-Bedrock NTP timeout count"
rg -Fq 'certification_success_depends_on_external_network:false' \
  <<<"$run_evidence_source" ||
  fail "final evidence does not distinguish attempted egress from a certification dependency"
[[ "$(rg -Foc 'render_bedrock_deployment_settings' "$lez_stack_driver")" -ge 2 ]] ||
  fail "LEZ stack defines but does not invoke the audited settings renderer"
rg -Fq 'LEZ_V02_SLOT_DURATION_SECONDS' "$lez_stack_driver" ||
  fail "LEZ stack does not accept and record the selected slot cadence"

manifest_validator_source="$(sed -n '/^validate_actual_effect_manifests() {$/,/^}$/p' "$runner")"
[[ -n "$manifest_validator_source" ]] ||
  fail "outer runner is missing journey-specific actual-effect manifest validation"
[[ "$(rg -Foc 'validate_actual_effect_manifests' "$runner")" -ge 2 ]] ||
  fail "outer runner does not invoke journey-specific actual-effect manifest validation"

manifest_validation_root="$(mktemp -d /tmp/m3-actor-manifest-contract.XXXXXX)"
cleanup_manifest_validation_root() {
  rm -rf -- "$manifest_validation_root"
}
trap cleanup_manifest_validation_root EXIT

write_effect_manifest_fixture() {
  local direction="$1" shape="$2" output btc_lock btc_terminal lez_initialization lez_funding lez_terminal
  output="${manifest_validation_root}/${direction}-actual-effects.json"
  btc_lock="$(printf '%s' "${direction}-${shape}-bitcoin-lock" | sha256sum | cut -d' ' -f1)"
  btc_terminal="$(printf '%s' "${direction}-${shape}-bitcoin-terminal" | sha256sum | cut -d' ' -f1)"
  lez_initialization="$(printf '%s' "${direction}-${shape}-lez-initialization" | sha256sum | cut -d' ' -f1)"
  lez_funding="$(printf '%s' "${direction}-${shape}-lez-funding" | sha256sum | cut -d' ' -f1)"
  lez_terminal="$(printf '%s' "${direction}-${shape}-lez-terminal" | sha256sum | cut -d' ' -f1)"
  case "$shape" in
    claim)
      jq -n --arg direction "$direction" --arg bitcoin_lock "$btc_lock" \
        --arg bitcoin_claim "$btc_terminal" --arg lez_initialization "$lez_initialization" \
        --arg lez_funding "$lez_funding" --arg lez_claim "$lez_terminal" '
        {schema_version:1,journey:"claim",direction:$direction,
         bitcoin_effect_ids:[$bitcoin_lock,$bitcoin_claim],
         lez_effect_ids:[$lez_initialization,$lez_funding,$lez_claim],
         expected_unique_effects:{bitcoin:2,lez:3},
         actor_owned_claims:{bitcoin:$bitcoin_claim,lez:$lez_claim}}
      ' >"$output"
      ;;
    survivor_claim)
      jq -n --arg direction "$direction" --arg bitcoin_lock "$btc_lock" \
        --arg bitcoin_claim "$btc_terminal" --arg lez_initialization "$lez_initialization" \
        --arg lez_funding "$lez_funding" --arg lez_claim "$lez_terminal" '
        {schema_version:1,journey:"survivor_claim",direction:$direction,
         bitcoin_effect_ids:[$bitcoin_lock,$bitcoin_claim],
         lez_effect_ids:[$lez_initialization,$lez_funding,$lez_claim],
         expected_unique_effects:{bitcoin:2,lez:3},
         actor_owned_claims:{bitcoin:$bitcoin_claim,lez:$lez_claim},
         survivor_evidence_file:($direction + "-survivor-claim.json"),
         revealer:"taker",follower:"maker",
         intermediate_phase:"claim_evidence_available",intermediate_terminal:false}
      ' >"$output"
      ;;
    refund)
      jq -n --arg direction "$direction" --arg bitcoin_lock "$btc_lock" \
        --arg bitcoin_refund "$btc_terminal" --arg lez_initialization "$lez_initialization" \
        --arg lez_funding "$lez_funding" --arg lez_refund "$lez_terminal" '
        {schema_version:1,journey:"refund",direction:$direction,
         bitcoin_effect_ids:[$bitcoin_lock,$bitcoin_refund],
         lez_effect_ids:[$lez_initialization,$lez_funding,$lez_refund],
         expected_unique_effects:{bitcoin:2,lez:3},
         actor_owned_refunds:{bitcoin:$bitcoin_refund,lez:$lez_refund},
         cooperative_claim_effects_present:false}
      ' >"$output"
      ;;
    first_lock_refund)
      case "$direction" in
        taker_sells_foreign)
          jq -n --arg direction "$direction" --arg bitcoin_lock "$btc_lock" \
            --arg bitcoin_refund "$btc_terminal" '
            {schema_version:1,journey:"first_lock_refund",direction:$direction,
             bitcoin_effect_ids:[$bitcoin_lock,$bitcoin_refund],lez_effect_ids:[],
             expected_unique_effects:{bitcoin:2,lez:0},
             actor_owned_refunds:{bitcoin:$bitcoin_refund},
             maker_second_lock:{chain:"lez",effect_count:0},
             cooperative_claim_effects_present:false,dual_lock_gate_opened:false}
          ' >"$output"
          ;;
        taker_sells_lez)
          jq -n --arg direction "$direction" --arg lez_initialization "$lez_initialization" \
            --arg lez_funding "$lez_funding" --arg lez_refund "$lez_terminal" '
            {schema_version:1,journey:"first_lock_refund",direction:$direction,
             bitcoin_effect_ids:[],
             lez_effect_ids:[$lez_initialization,$lez_funding,$lez_refund],
             expected_unique_effects:{bitcoin:0,lez:3},
             actor_owned_refunds:{lez:$lez_refund},
             maker_second_lock:{chain:"bitcoin",effect_count:0},
             cooperative_claim_effects_present:false,dual_lock_gate_opened:false}
          ' >"$output"
          ;;
      esac
      ;;
    *) fail "unsupported actual-effect fixture shape: ${shape}" ;;
  esac
}

write_effect_manifest_pair() {
  local foreign_shape="$1" lez_shape="${2:-$1}"
  write_effect_manifest_fixture taker_sells_foreign "$foreign_shape"
  write_effect_manifest_fixture taker_sells_lez "$lez_shape"
}

validate_effect_manifest_pair() {
  local selected_journey="$1"
  bash -c '
    set -euo pipefail
    journey="$1"
    evidence_dir="$2"
    directions=(taker_sells_foreign taker_sells_lez)
    fail() { echo "isolated actual-effect validator failed: $*" >&2; exit 2; }
    eval "$3"
    validate_actual_effect_manifests
  ' manifest-validator "$selected_journey" "$manifest_validation_root" "$manifest_validator_source"
}

write_effect_manifest_pair claim
validate_effect_manifest_pair claim >/dev/null ||
  fail "outer runner rejected two claim-shaped manifests for the claim journey"
if validate_effect_manifest_pair refund >/dev/null 2>&1; then
  fail "outer runner accepted claim-shaped manifests for the refund journey"
fi

write_effect_manifest_pair survivor_claim
validate_effect_manifest_pair survivor_claim >/dev/null ||
  fail "outer runner rejected two exact survivor-shaped manifests"
if validate_effect_manifest_pair claim >/dev/null 2>&1; then
  fail "outer runner accepted survivor manifests as ordinary claim evidence"
fi

write_effect_manifest_pair survivor_claim
jq '.intermediate_terminal = true' \
  "${manifest_validation_root}/taker_sells_foreign-actual-effects.json" \
  >"${manifest_validation_root}/taker_sells_foreign-actual-effects.json.partial"
mv "${manifest_validation_root}/taker_sells_foreign-actual-effects.json.partial" \
  "${manifest_validation_root}/taker_sells_foreign-actual-effects.json"
if validate_effect_manifest_pair survivor_claim >/dev/null 2>&1; then
  fail "outer runner accepted terminal revision-three survivor evidence"
fi

write_effect_manifest_pair refund
validate_effect_manifest_pair refund >/dev/null ||
  fail "outer runner rejected two refund-shaped manifests for the refund journey"
if validate_effect_manifest_pair claim >/dev/null 2>&1; then
  fail "outer runner accepted refund-shaped manifests for the claim journey"
fi

write_effect_manifest_pair refund claim
if validate_effect_manifest_pair refund >/dev/null 2>&1; then
  fail "outer runner accepted a mixed-journey pair of actual-effect manifests"
fi

write_effect_manifest_pair refund
jq 'del(.actor_owned_refunds)' \
  "${manifest_validation_root}/taker_sells_foreign-actual-effects.json" \
  >"${manifest_validation_root}/taker_sells_foreign-actual-effects.json.partial"
mv "${manifest_validation_root}/taker_sells_foreign-actual-effects.json.partial" \
  "${manifest_validation_root}/taker_sells_foreign-actual-effects.json"
if validate_effect_manifest_pair refund >/dev/null 2>&1; then
  fail "outer runner accepted refund evidence without actor_owned_refunds"
fi

write_effect_manifest_pair refund
jq '.cooperative_claim_effects_present = true' \
  "${manifest_validation_root}/taker_sells_lez-actual-effects.json" \
  >"${manifest_validation_root}/taker_sells_lez-actual-effects.json.partial"
mv "${manifest_validation_root}/taker_sells_lez-actual-effects.json.partial" \
  "${manifest_validation_root}/taker_sells_lez-actual-effects.json"
if validate_effect_manifest_pair refund >/dev/null 2>&1; then
  fail "outer runner accepted refund evidence containing a cooperative claim effect"
fi

write_effect_manifest_pair first_lock_refund
validate_effect_manifest_pair first_lock_refund >/dev/null ||
  fail "outer runner rejected exact 2/0 and 0/3 first-lock manifests"
if validate_effect_manifest_pair refund >/dev/null 2>&1; then
  fail "outer runner accepted first-lock manifests as two-lock refunds"
fi

write_effect_manifest_pair first_lock_refund
jq '.maker_second_lock.effect_count = 1' \
  "${manifest_validation_root}/taker_sells_foreign-actual-effects.json" \
  >"${manifest_validation_root}/taker_sells_foreign-actual-effects.json.partial"
mv "${manifest_validation_root}/taker_sells_foreign-actual-effects.json.partial" \
  "${manifest_validation_root}/taker_sells_foreign-actual-effects.json"
if validate_effect_manifest_pair first_lock_refund >/dev/null 2>&1; then
  fail "outer runner accepted a first-lock manifest with a maker second-lock effect"
fi

write_effect_manifest_pair first_lock_refund
jq '.dual_lock_gate_opened = true' \
  "${manifest_validation_root}/taker_sells_lez-actual-effects.json" \
  >"${manifest_validation_root}/taker_sells_lez-actual-effects.json.partial"
mv "${manifest_validation_root}/taker_sells_lez-actual-effects.json.partial" \
  "${manifest_validation_root}/taker_sells_lez-actual-effects.json"
if validate_effect_manifest_pair first_lock_refund >/dev/null 2>&1; then
  fail "outer runner accepted a dual-lock gate in the first-lock-only branch"
fi

write_effect_manifest_pair first_lock_refund
jq '.actor_owned_claims = {bitcoin:"1111111111111111111111111111111111111111111111111111111111111111"}' \
  "${manifest_validation_root}/taker_sells_foreign-actual-effects.json" \
  >"${manifest_validation_root}/taker_sells_foreign-actual-effects.json.partial"
mv "${manifest_validation_root}/taker_sells_foreign-actual-effects.json.partial" \
  "${manifest_validation_root}/taker_sells_foreign-actual-effects.json"
if validate_effect_manifest_pair first_lock_refund >/dev/null 2>&1; then
  fail "outer runner accepted a claim helper effect in the first-lock refund branch"
fi

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
  'LEZ_V02_SLOT_DURATION_SECONDS="$lez_slot_duration_seconds"'
  'taker_sells_foreign'
  'taker_sells_lez'
  'M3_ACTOR_POC_JOURNEY'
  'M3_POC_JOURNEY'
  'run_stage_one'
  'run_official_nssa_mapping'
  'start_actual_nodes'
  'run_stage_two'
  'run_direction_actor_flow'
  'assert_terminal_and_replay'
  'write_cleanup_attestation'
  'capture_owned_resources'
  'remove_secure_state_root'
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

retained_survivor="docs/evidence/m3-local-two-direction-survivor-claim-poc-20260716.json"
[[ -f "$retained_survivor" && ! -L "$retained_survivor" ]] ||
  fail "clean survivor certification packet is missing or unsafe"
jq -e '
  .schema_version == 1 and .milestone == "M3" and .result == "passed"
  and .classification == "private_local_two_direction_post_reveal_survivor_claim_poc"
  and .run_id == "m3survivor-20260716c"
  and .provenance.repository_commit == "6e8b065c2247306b746743454e7816bab8285350"
  and .provenance.worktree_clean_during_run == true
  and .provenance.commit_pushed_before_run == true
  and .survivor_protocol.protected_absence.revealer_actor_invocation_count == 0
  and .survivor_protocol.intermediate.terminal == false
  and .survivor_protocol.intermediate.remaining_leg_canonical_and_claimable == true
  and (.directions | length) == 2
  and all(.directions[];
    .intermediate.protocol_phase == "claim_evidence_available"
    and .intermediate.terminal == false
    and .completion.maker_revision == 4 and .completion.phase == "completed"
    and .completion.boundary.completed_before_signed_refund_boundary == true
    and .delayed_revealer_catchup.per_chain.bitcoin.successful_resubmission_count == 0
    and .delayed_revealer_catchup.per_chain.lez.successful_resubmission_count == 0
    and .delayed_revealer_catchup.successful_resubmission_count == 0)
  and .terminal_replay.resubmission_count == 0
  and .exact_cleanup.all_exact_run_resources_absent == true
  and .exact_cleanup.foreign_resources_targeted == false
  and .runtime_external_resources.certification_success_depends_on_external_network == false
  and .secret_safety.private_material_disclosed == false
  and .open_scope[-1] == "This file alone does not authorize an M3 completion tag."
' "$retained_survivor" >/dev/null ||
  fail "clean survivor certification packet is incomplete or overclaims M3 closure"

echo "M3 actor local-PoC orchestration contract is complete"
