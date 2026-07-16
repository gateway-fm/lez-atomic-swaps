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
rg -Fq -- '--argjson maker_cutoff "$now"' <<<"$stage_two_spec_source" ||
  fail "stage-two agreement does not bind the current cutoff into jq"
rg -Fq 'maker_second_lock_cutoff_unix_seconds:$maker_cutoff' \
  <<<"$stage_two_spec_source" ||
  fail "stage-two agreement does not use the bound maker cutoff"
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
  and .actor_owned_claim_effects == true
  and .journeys == ["claim", "refund", "first_lock_refund"]
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
  and .actor_config_schema_version == 3
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
  write_actor_configs activate_actors submit_bitcoin_lock submit_lez_lock_pair \
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
for behavior in assert_first_lock_recovery_pending \
  refresh_first_lock_lez_absence_window advance_core_median_time_to_first_lock_cutoff \
  write_first_lock_recovery_admission_evidence assert_first_lock_owner_terminal_restart \
  assert_fresh_maker_first_lock_terminal run_actor_first_lock_refund_flow; do
  rg -Fq "${behavior}()" "$direction_driver" ||
    fail "actual direction implementation is missing first-lock refund behavior: ${behavior}"
done
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

rg -Fq 'planned_bitcoin_funding_anchor_height' "$direction_driver" ||
  fail "Bitcoin lock path does not bind the actual mined height to the planned anchor"
rg -Fq 'exact_transaction_occurrences:1' "$direction_driver" ||
  fail "Bitcoin lock path does not retain exact containing-block membership"
rg -Fq 'schema_version:3,role:$role' "$direction_driver" ||
  fail "actor configs do not use schema version 3"
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
[[ "$invalid_journey_output" == *"M3_ACTOR_POC_JOURNEY must be claim, refund, or first_lock_refund"* ]] ||
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
' <<<"$contract_json" >/dev/null || fail "contract JSON does not prove the M3 invariants"

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
' <<<"$first_lock_contract_json" >/dev/null ||
  fail "first-lock refund contract omits exact effects, cutoff/race clocks, replay, or local resources"

rg -Fq 'LEZ_V02_SLOT_DURATION_SECONDS="$lez_slot_duration_seconds"' "$runner" ||
  fail "M3 runner does not pass the journey-selected cadence to the LEZ child"
rg -Fq 'slot_duration_seconds:$lez_slot_duration_seconds' "$runner" ||
  fail "M3 evidence does not record the selected LEZ slot cadence"
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

echo "M3 actor local-PoC orchestration contract is complete"
