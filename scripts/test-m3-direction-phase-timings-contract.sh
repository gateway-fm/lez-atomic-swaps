#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly runner="scripts/run-m3-actor-direction.sh"
readonly outer_runner="scripts/run-m3-actor-local-poc.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/m3-direction-phase-timings.XXXXXX")"
readonly test_root
trap 'rm -rf -- "$test_root"' EXIT

fail() {
  echo "M3 direction phase-timings contract failed: $*" >&2
  exit 1
}

extract_function() {
  local name="$1" source
  source="$(sed -n "/^${name}() {$/,/^}$/p" "$runner")"
  [[ -n "$source" ]] || fail "direction runner is missing ${name}"
  printf '%s\n' "$source"
}

readonly extracted="${test_root}/direction-timing-functions.sh"
for function_name in parse_direction_proc_uptime_ms read_direction_monotonic_ms \
  expected_direction_phase_timings_json initialize_direction_phase_timings \
  direction_phase_begin direction_phase_end finalize_direction_phase_timings; do
  extract_function "$function_name" >>"$extracted"
done
# shellcheck source=/dev/null
source "$extracted"

extract_outer_function() {
  local name="$1" source
  source="$(sed -n "/^${name}() {$/,/^}$/p" "$outer_runner")"
  [[ -n "$source" ]] || fail "outer runner is missing ${name}"
  printf '%s\n' "$source"
}

readonly outer_extracted="${test_root}/direction-timing-binding-functions.sh"
for function_name in expected_actor_direction_phase_ids_json \
  validate_actor_direction_phase_timing_for_run_evidence \
  actor_direction_phase_timings_hash_stable; do
  extract_outer_function "$function_name" >>"$outer_extracted"
done
# shellcheck source=/dev/null
source "$outer_extracted"

parsed_ms=""
parse_direction_proc_uptime_ms "123.45" parsed_ms ||
  fail "canonical uptime did not parse"
[[ "$parsed_ms" == 123450 ]] || fail "canonical uptime conversion is wrong"
parse_direction_proc_uptime_ms "9007199254740.991" parsed_ms ||
  fail "maximum exact JSON uptime did not parse"
[[ "$parsed_ms" == 9007199254740991 ]] ||
  fail "maximum exact JSON uptime conversion is wrong"
for malformed in "" 1 -1.0 01.0 1e3 1. "1.0 extra" \
  9007199254740.992 9007199254741.0; do
  if parse_direction_proc_uptime_ms "$malformed" parsed_ms >/dev/null 2>&1; then
    fail "malformed uptime parsed: ${malformed}"
  fi
done
read_direction_monotonic_ms || fail "live monotonic read failed"
# shellcheck disable=SC2154 # Assigned by the extracted production function.
first_live_ms="$direction_timing_now_ms"
read_direction_monotonic_ms || fail "second live monotonic read failed"
(( direction_timing_now_ms >= first_live_ms )) || fail "live monotonic clock regressed"

prepare_case() {
  local label="$1" selected_direction="${2:-taker_sells_foreign}"
  local selected_asset="${3:-custom_token}" selected_journey="${4:-claim}"
  local selected_mode="${5:-sequential}"
  case_root="${test_root}/${label}"
  M3_POC_RUN_ID="direction-timing-${label}"
  M3_POC_DIRECTION="$selected_direction"
  M3_POC_JOURNEY="$selected_journey"
  # shellcheck disable=SC2034 # Read by extracted production functions.
  asset_mode="$selected_asset"
  # shellcheck disable=SC2034 # Read by the extracted outer-runner functions.
  m5_btc_application_mode=0
  direction_timing_execution_mode="$selected_mode"
  M3_POC_DIRECTION_ROOT="${case_root}/private/directions/${selected_direction}"
  M3_POC_EVIDENCE_DIR="${case_root}/evidence"
  direction_timing_dir="${M3_POC_DIRECTION_ROOT}/timings"
  direction_timing_journal="${direction_timing_dir}/actor.ndjson.partial"
  direction_timing_evidence="${M3_POC_EVIDENCE_DIR}/${selected_direction}-actor-phase-timings.json"
  mkdir -p "$M3_POC_DIRECTION_ROOT" "$M3_POC_EVIDENCE_DIR"
  chmod 0700 "$M3_POC_DIRECTION_ROOT" "$M3_POC_EVIDENCE_DIR"
  jq -n --arg direction "$selected_direction" '
    {schema_version:1,direction:$direction,bitcoin_effect_ids:["btc"],
     lez_effect_ids:["lez"],expected_unique_effects:{bitcoin:1,lez:1}}
  ' >"${M3_POC_EVIDENCE_DIR}/${selected_direction}-actual-effects.json"
  chmod 0600 "${M3_POC_EVIDENCE_DIR}/${selected_direction}-actual-effects.json"
  initialize_direction_phase_timings || fail "${label} initialization failed"
}

record_expected_phases() {
  local expected phase_id
  expected="$(expected_direction_phase_timings_json)" || fail "phase plan failed"
  while IFS= read -r phase_id; do
    direction_phase_begin "$phase_id" || fail "begin failed for ${phase_id}"
    direction_phase_end "$phase_id" || fail "end failed for ${phase_id}"
  done < <(jq -r '.[].phase_id' <<<"$expected")
}

prepare_case valid
record_expected_phases
finalize_direction_phase_timings || fail "valid child timing publication failed"
[[ -f "$direction_timing_evidence" && ! -L "$direction_timing_evidence" ]] ||
  fail "valid child timing evidence is missing or unsafe"
[[ "$(stat -c '%a' "$direction_timing_evidence")" == 600 ]] ||
  fail "valid child timing evidence is not owner-private"
actual_effects_sha="$(sha256sum \
  "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actual-effects.json" | sed 's/ .*//')"
jq -e --arg sha "$actual_effects_sha" '
  .schema_version == 1
  and .kind == "m3_actor_direction_phase_timings"
  and .result == "actor_flow_passed"
  and .direction == "taker_sells_foreign"
  and .journey == "claim"
  and .asset_mode == "custom_token"
  and .execution_mode == "sequential"
  and .actual_effects_sha256 == $sha
  and .clock == {source:"linux_proc_uptime",unit:"milliseconds",
    resolution_ms:10,includes_suspend:true,wall_clock_used_for_duration:false}
  and .coverage == {starts_before_final_transcript:true,
    ends_after_actual_effect_manifest:true,
    excludes_outer_stage_two_replay_and_balances:true}
  and [.phases[].phase_id] == ["final_transcript","presign_and_activate",
    "first_lock_to_revision_one","second_lock_to_revision_two","dual_lock_gate",
    "revealing_claim_to_revision_three","followup_claim_to_revision_four",
    "terminal_evidence"]
  and [.phases[].sequence] == [range(1;9)]
  and all(.phases[];
    .producer == "direction_actor" and .outcome == "passed"
    and .start_offset_ms >= 0 and .end_offset_ms >= .start_offset_ms
    and .duration_ms == (.end_offset_ms - .start_offset_ms))
  and .unattributed_duration_ms ==
    (.total_duration_ms - ([.phases[].duration_ms] | add))
  and .private_material_disclosed == false
' "$direction_timing_evidence" >/dev/null ||
  fail "valid child timing schema is inconsistent"

for matrix_case in \
  foreign-native:taker_sells_foreign:native:claim:sequential:8 \
  lez-native:taker_sells_lez:native:claim:sequential:8 \
  lez-token:taker_sells_lez:custom_token:claim:sequential:8 \
  foreign-overlap:taker_sells_foreign:native:claim:overlap:11 \
  survivor:taker_sells_foreign:native:survivor_claim:sequential:7 \
  refund:taker_sells_lez:native:refund:sequential:7 \
  first-refund:taker_sells_lez:native:first_lock_refund:sequential:4; do
  IFS=: read -r label direction selected_asset selected_journey selected_mode count \
    <<<"$matrix_case"
  prepare_case "$label" "$direction" "$selected_asset" "$selected_journey" "$selected_mode"
  [[ "$(expected_direction_phase_timings_json | jq 'length')" == "$count" ]] ||
    fail "wrong phase-plan count for ${label}"
  record_expected_phases
  finalize_direction_phase_timings || fail "${label} publication failed"
done

prepare_invalid_case() {
  local label="$1"
  prepare_case "$label"
  record_expected_phases
}

prepare_invalid_case missing
sed -n '1,7p' "$direction_timing_journal" >"${direction_timing_journal}.new"
mv "${direction_timing_journal}.new" "$direction_timing_journal"
chmod 0600 "$direction_timing_journal"
if finalize_direction_phase_timings >/dev/null 2>&1; then
  fail "missing child phase produced evidence"
fi

prepare_invalid_case duplicate
sed -n '1p' "$direction_timing_journal" >>"$direction_timing_journal"
if finalize_direction_phase_timings >/dev/null 2>&1; then
  fail "duplicate child phase produced evidence"
fi

prepare_invalid_case extra-field
jq -c '. + {secret_sentinel:"DO_NOT_RECORD_ME"}' "$direction_timing_journal" \
  >"${direction_timing_journal}.new"
mv "${direction_timing_journal}.new" "$direction_timing_journal"
chmod 0600 "$direction_timing_journal"
if finalize_direction_phase_timings >/dev/null 2>&1; then
  fail "extra child field produced evidence"
fi

prepare_invalid_case regression
jq -c 'if .sequence == 8 then .end_offset_ms = (.start_offset_ms - 1) else . end' \
  "$direction_timing_journal" >"${direction_timing_journal}.new"
mv "${direction_timing_journal}.new" "$direction_timing_journal"
chmod 0600 "$direction_timing_journal"
if finalize_direction_phase_timings >/dev/null 2>&1; then
  fail "regressing child record produced evidence"
fi

prepare_invalid_case wrong-mode
chmod 0644 "$direction_timing_journal"
if finalize_direction_phase_timings >/dev/null 2>&1; then
  fail "wrong-mode child journal produced evidence"
fi

prepare_invalid_case no-clobber
: >"$direction_timing_evidence"
chmod 0600 "$direction_timing_evidence"
if finalize_direction_phase_timings >/dev/null 2>&1; then
  fail "child timing publisher overwrote existing evidence"
fi

prepare_invalid_case effects-tamper
printf '{' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actual-effects.json"
if finalize_direction_phase_timings >/dev/null 2>&1; then
  fail "malformed actual-effect evidence was bound"
fi

runner_source="$(<"$runner")"
for phase_id in final_transcript presign_and_activate first_lock_to_revision_one \
  second_lock_to_revision_two dual_lock_gate revealing_claim_to_revision_three \
  followup_claim_to_revision_four terminal_evidence; do
  rg -Fq -- "direction_phase_begin ${phase_id}" <<<"$runner_source" ||
    fail "direction runner omits semantic timing phase: ${phase_id}"
done

binding_root="${test_root}/binding"
run_id="direction-timing-binding"
journey="claim"
# shellcheck disable=SC2034 # Read by extracted outer production functions.
schedule="sequential"
# shellcheck disable=SC2034 # Read by extracted child and outer production functions.
asset_mode="custom_token"
# shellcheck disable=SC2034 # Read by extracted outer production functions.
m5_btc_application_mode=0
# shellcheck disable=SC2034 # Read by extracted outer production functions.
directions=(taker_sells_foreign taker_sells_lez)
# shellcheck disable=SC2034 # Read by extracted outer production functions.
relative_run_root=".e2e/${run_id}/m3-actor-poc"
evidence_dir="${binding_root}/evidence"
phase_timings_evidence="${evidence_dir}/m3-phase-timings.json"
mkdir -p "$evidence_dir"
chmod 0700 "$evidence_dir"

prepare_binding_direction() {
  local selected_direction="$1"
  # shellcheck disable=SC2034 # Read by extracted child production functions.
  M3_POC_RUN_ID="$run_id"
  M3_POC_DIRECTION="$selected_direction"
  # shellcheck disable=SC2034 # Read by extracted child production functions.
  M3_POC_JOURNEY="$journey"
  # shellcheck disable=SC2034 # Read by extracted child production functions.
  direction_timing_execution_mode="sequential"
  M3_POC_DIRECTION_ROOT="${binding_root}/private/directions/${selected_direction}"
  M3_POC_EVIDENCE_DIR="$evidence_dir"
  mkdir -p "$M3_POC_DIRECTION_ROOT"
  chmod 0700 "$M3_POC_DIRECTION_ROOT"
  jq -n --arg direction "$selected_direction" '
    {schema_version:1,direction:$direction,bitcoin_effect_ids:["btc"],
     lez_effect_ids:["lez"],expected_unique_effects:{bitcoin:1,lez:1}}
  ' >"${evidence_dir}/${selected_direction}-actual-effects.json"
  chmod 0600 "${evidence_dir}/${selected_direction}-actual-effects.json"
  initialize_direction_phase_timings || fail "binding child initialization failed"
  record_expected_phases
  finalize_direction_phase_timings || fail "binding child publication failed"
}

prepare_binding_direction taker_sells_foreign
prepare_binding_direction taker_sells_lez
jq -n --arg run_id "$run_id" '
  {schema_version:1,kind:"m3_monotonic_phase_timings",run_id:$run_id,
   journey:"claim",schedule:"sequential",asset_mode:"custom_token",
   phases:[
     {phase_id:"direction_taker_sells_foreign_actor_flow",duration_ms:100000},
     {phase_id:"direction_taker_sells_lez_actor_flow",duration_ms:100000}
   ]}
' >"$phase_timings_evidence"
chmod 0600 "$phase_timings_evidence"
phase_timing_sha="$(sha256sum "$phase_timings_evidence" | sed 's/ .*//')"

foreign_binding_summary=""
lez_binding_summary=""
validate_actor_direction_phase_timing_for_run_evidence \
  taker_sells_foreign foreign_binding_summary "$phase_timing_sha" ||
  fail "valid forward child timing did not bind"
validate_actor_direction_phase_timing_for_run_evidence \
  taker_sells_lez lez_binding_summary "$phase_timing_sha" ||
  fail "valid reverse child timing did not bind"
actor_direction_timing_summary="$(jq -cn \
  --argjson foreign "$foreign_binding_summary" \
  --argjson lez "$lez_binding_summary" \
  '{taker_sells_foreign:$foreign,taker_sells_lez:$lez}')"
jq -e '
  .taker_sells_foreign.direction == "taker_sells_foreign"
  and .taker_sells_lez.direction == "taker_sells_lez"
  and all(.[];
    .kind == "m3_actor_direction_phase_timings"
    and .result == "actor_flow_passed"
    and .journey == "claim" and .asset_mode == "custom_token"
    and .execution_mode == "sequential"
    and (.evidence_path | startswith(".e2e/"))
    and (.evidence_sha256 | test("^[0-9a-f]{64}$"))
    and (.actual_effects_sha256 | test("^[0-9a-f]{64}$"))
    and .phase_count == 8
    and .parent.phase_id == ("direction_" + .direction + "_actor_flow")
    and .parent.duration_ms == 100000
    and .parent.contains_child == true
    and .parent.residual_ms == (.parent.duration_ms - .total_duration_ms))
' <<<"$actor_direction_timing_summary" >/dev/null ||
  fail "bound child timing summaries are inconsistent"
actor_direction_phase_timings_hash_stable "$actor_direction_timing_summary" ||
  fail "fresh child timing bindings were not hash-stable"

foreign_child="${evidence_dir}/taker_sells_foreign-actor-phase-timings.json"
foreign_effects="${evidence_dir}/taker_sells_foreign-actual-effects.json"
cp "$foreign_child" "${foreign_child}.saved"
printf '\n' >>"$foreign_child"
if actor_direction_phase_timings_hash_stable \
  "$actor_direction_timing_summary" >/dev/null 2>&1; then
  fail "post-validation child timing tamper was accepted"
fi
mv "${foreign_child}.saved" "$foreign_child"
chmod 0600 "$foreign_child"

cp "$phase_timings_evidence" "${phase_timings_evidence}.saved"
child_total="$(jq -er '.total_duration_ms' "$foreign_child")"
jq --argjson duration "$((child_total - 1))" '
  .phases[0].duration_ms = $duration
' "$phase_timings_evidence" >"${phase_timings_evidence}.new"
mv "${phase_timings_evidence}.new" "$phase_timings_evidence"
chmod 0600 "$phase_timings_evidence"
if validate_actor_direction_phase_timing_for_run_evidence \
  taker_sells_foreign foreign_binding_summary \
  "$phase_timing_sha" >/dev/null 2>&1; then
  fail "changed parent timing packet was accepted against its bound hash"
fi
changed_phase_timing_sha="$(sha256sum "$phase_timings_evidence" | sed 's/ .*//')"
if validate_actor_direction_phase_timing_for_run_evidence \
  taker_sells_foreign foreign_binding_summary \
  "$changed_phase_timing_sha" >/dev/null 2>&1; then
  fail "child duration exceeding its parent phase was accepted"
fi
mv "${phase_timings_evidence}.saved" "$phase_timings_evidence"
chmod 0600 "$phase_timings_evidence"
[[ "$(sha256sum "$phase_timings_evidence" | sed 's/ .*//')" == "$phase_timing_sha" ]] ||
  fail "restored parent timing packet does not match its bound hash"

cp "$foreign_child" "${foreign_child}.saved"
jq '.execution_mode = "overlap"' "$foreign_child" >"${foreign_child}.new"
mv "${foreign_child}.new" "$foreign_child"
chmod 0600 "$foreign_child"
if validate_actor_direction_phase_timing_for_run_evidence \
  taker_sells_foreign foreign_binding_summary \
  "$phase_timing_sha" >/dev/null 2>&1; then
  fail "wrong child execution mode was accepted"
fi
mv "${foreign_child}.saved" "$foreign_child"
chmod 0600 "$foreign_child"

cp "$foreign_effects" "${foreign_effects}.saved"
printf '\n' >>"$foreign_effects"
if validate_actor_direction_phase_timing_for_run_evidence \
  taker_sells_foreign foreign_binding_summary \
  "$phase_timing_sha" >/dev/null 2>&1; then
  fail "changed actual-effect evidence was accepted"
fi
mv "${foreign_effects}.saved" "$foreign_effects"
chmod 0600 "$foreign_effects"

outer_source="$(<"$outer_runner")"
rg -Fq -- 'actor_direction_timings:$actor_direction_timing_summary' \
  <<<"$outer_source" || fail "main evidence does not bind child direction timings"
rg -Fq -- 'actor_direction_phase_timings_hash_stable' <<<"$outer_source" ||
  fail "main evidence does not rehash child direction timings"
rg -Fq -- 'foreign_actor_direction_timing_summary "$phase_timing_sha"' \
  <<<"$outer_source" ||
  fail "main evidence does not bind child summaries to the validated parent hash"

echo "M3 direction phase-timings contract passed"
