#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly runner="scripts/run-m3-actor-local-poc.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/m3-phase-timings-contract.XXXXXX")"
readonly test_root
trap 'rm -rf -- "$test_root"' EXIT

fail() {
  echo "M3 phase-timings contract failed: $*" >&2
  exit 1
}

extract_function() {
  local name="$1"
  local source
  source="$(sed -n "/^${name}() {$/,/^}$/p" "$runner")"
  [[ -n "$source" ]] || fail "runner is missing ${name}"
  printf '%s\n' "$source"
}

readonly extracted="${test_root}/phase-timing-functions.sh"
for function_name in parse_proc_uptime_ms read_monotonic_ms \
  expected_phase_timings_json initialize_phase_timings phase_timing_begin \
  phase_timing_end finalize_phase_timings validate_phase_timings_for_run_evidence \
  phase_timings_hash_stable; do
  extract_function "$function_name" >>"$extracted"
done
# shellcheck source=/dev/null
source "$extracted"

parsed_ms=""
parse_proc_uptime_ms "123.45" parsed_ms || fail "canonical uptime did not parse"
[[ "$parsed_ms" == 123450 ]] || fail "canonical uptime conversion is wrong"
parse_proc_uptime_ms "1.2" parsed_ms || fail "short fractional uptime did not parse"
[[ "$parsed_ms" == 1200 ]] || fail "fractional uptime padding is wrong"
parse_proc_uptime_ms "0.0019" parsed_ms || fail "fine uptime did not parse"
[[ "$parsed_ms" == 1 ]] || fail "uptime truncation to milliseconds is wrong"
parse_proc_uptime_ms "9007199254740.991" parsed_ms ||
  fail "maximum exact JSON integer uptime did not parse"
[[ "$parsed_ms" == 9007199254740991 ]] ||
  fail "maximum exact JSON integer uptime conversion is wrong"
for malformed_uptime in "" 1 -1.0 01.0 1e3 1. "1.0 extra" \
  9007199254740.992 9007199254741.0 \
  9223372036854776.0 9223372036854775.808; do
  if parse_proc_uptime_ms "$malformed_uptime" parsed_ms >/dev/null 2>&1; then
    fail "malformed or overflowing uptime parsed: ${malformed_uptime}"
  fi
done
read_monotonic_ms || fail "first live monotonic read failed"
# shellcheck disable=SC2154 # Assigned by the extracted production function.
first_live_ms="$phase_timing_now_ms"
read_monotonic_ms || fail "second live monotonic read failed"
[[ "$phase_timing_now_ms" -ge "$first_live_ms" ]] ||
  fail "live monotonic reads regressed"

prepare_case() {
  local label="$1" selected_asset_mode="${2:-custom_token}"
  local selected_schedule="${3:-sequential}"
  case_root="${test_root}/${label}"
  timing_dir="${case_root}/private/timings"
  evidence_dir="${case_root}/evidence"
  phase_timing_journal="${timing_dir}/outer.ndjson.partial"
  phase_timings_evidence="${evidence_dir}/m3-phase-timings.json"
  run_id="timing-${label}"
  relative_run_root=".e2e/${run_id}/m3-actor-poc"
  # shellcheck disable=SC2034 # Read by the extracted production functions.
  journey=claim
  # shellcheck disable=SC2034 # Read by the extracted production functions.
  schedule="$selected_schedule"
  # shellcheck disable=SC2034 # Read by the extracted production functions.
  asset_mode="$selected_asset_mode"
  mkdir -p "${case_root}/private" "$evidence_dir"
  chmod 0700 "${case_root}/private" "$evidence_dir"
  initialize_phase_timings || fail "${label} timing initialization failed"
}

record_expected_phases() {
  local expected phase_id direction
  expected="$(expected_phase_timings_json)" || fail "expected phase set failed"
  while IFS=$'\t' read -r phase_id direction; do
    phase_timing_begin "$phase_id" "$direction" ||
      fail "phase begin failed for ${phase_id}"
    phase_timing_end "$phase_id" || fail "phase end failed for ${phase_id}"
  done < <(jq -r '.[] | [.phase_id, (.direction // "")] | @tsv' <<<"$expected")
}

prepare_case valid
record_expected_phases
finalize_phase_timings || fail "valid timing publication failed"
[[ -f "$phase_timings_evidence" && ! -L "$phase_timings_evidence" ]] ||
  fail "valid timing evidence is missing or a symlink"
[[ "$(stat -c '%a' "$phase_timings_evidence")" == 600 ]] ||
  fail "valid timing evidence is not owner-private"
jq -e '
  .schema_version == 1
  and .kind == "m3_monotonic_phase_timings"
  and .result == "execution_passed_pre_cleanup"
  and .clock == {source:"linux_proc_uptime",unit:"milliseconds",
    resolution_ms:10,includes_suspend:true,wall_clock_used_for_duration:false}
  and .coverage == {starts_after_run_directory_initialization:true,
    ends_before_run_evidence_publication:true,
    cleanup_in_separate_attestation:true}
  and (.phases | length) == 18
  and [.phases[].phase_id] == ["contract_validation","prebuild",
    "identities_stage_one","node_startup","bitcoin_funding","lez_bootstrap",
    "f7_fixture",
    "direction_taker_sells_foreign_reserve_funding",
    "direction_taker_sells_foreign_stage_two",
    "direction_taker_sells_foreign_actor_flow",
    "direction_taker_sells_foreign_terminal_replay",
    "direction_taker_sells_foreign_terminal_balances",
    "direction_taker_sells_lez_reserve_funding",
    "direction_taker_sells_lez_stage_two",
    "direction_taker_sells_lez_actor_flow",
    "direction_taker_sells_lez_terminal_replay",
    "direction_taker_sells_lez_terminal_balances",
    "effect_validation"]
  and [.phases[].sequence] == [range(1;19)]
  and all(.phases[];
    .producer == "outer" and .outcome == "passed"
    and .start_offset_ms >= 0 and .end_offset_ms >= .start_offset_ms
    and .duration_ms == (.end_offset_ms - .start_offset_ms))
  and all(.phases[7:12][]; .direction == "taker_sells_foreign")
  and all(.phases[12:17][]; .direction == "taker_sells_lez")
  and all(.phases[0:7][]; .direction == null)
  and .phases[17].direction == null
  and .total_duration_ms >= ([.phases[].duration_ms] | add)
  and .unattributed_duration_ms ==
    (.total_duration_ms - ([.phases[].duration_ms] | add))
  and .private_material_disclosed == false
' "$phase_timings_evidence" >/dev/null || fail "valid timing schema is inconsistent"
phase_timing_summary=""
validate_phase_timings_for_run_evidence phase_timing_summary ||
  fail "valid timing evidence could not be summarized for the main packet"
expected_timing_sha="$(sha256sum "$phase_timings_evidence" | sed 's/ .*//')"
jq -e --arg path "${relative_run_root}/evidence/m3-phase-timings.json" \
  --arg sha "$expected_timing_sha" '
  .kind == "m3_monotonic_phase_timings"
  and .result == "execution_passed_pre_cleanup"
  and .evidence_path == $path
  and .evidence_sha256 == $sha
  and .clock.source == "linux_proc_uptime"
  and .clock.unit == "milliseconds"
  and .coverage.cleanup_in_separate_attestation == true
  and .total_duration_ms >= 0
  and .unattributed_duration_ms >= 0
  and .phase_count == 18
' <<<"$phase_timing_summary" >/dev/null ||
  fail "main-packet timing summary is incomplete"
phase_timings_hash_stable "$expected_timing_sha" ||
  fail "unchanged timing packet did not retain its hash"
if rg -n -i 'DO_NOT_RECORD_ME|private[_ -]?key|transaction[_ -]?id|account[_ -]?id|endpoint|argv|environment' \
    "$phase_timings_evidence" >/dev/null; then
  fail "timing evidence contains a forbidden secret-bearing field"
fi

prepare_case native-sequential native sequential
record_expected_phases
finalize_phase_timings || fail "native-sequential timing publication failed"
jq -e '
  (.phases | length) == 15
  and ([.phases[].phase_id] | index("f7_fixture")) == null
  and [.phases[6].phase_id,.phases[10].phase_id] ==
    ["direction_taker_sells_foreign_reserve_funding",
     "direction_taker_sells_lez_reserve_funding"]
  and ([.phases[].phase_id] | index(
    "direction_taker_sells_foreign_terminal_balances")) == null
' "$phase_timings_evidence" >/dev/null ||
  fail "native-sequential phase schema is wrong"

prepare_case native-overlap native overlap
record_expected_phases
finalize_phase_timings || fail "native-overlap timing publication failed"
jq -e '
  (.phases | length) == 8
  and ([.phases[].phase_id] | index("f7_fixture")) == null
  and .phases[6] == {
    schema_version:1,sequence:7,producer:"outer",
    phase_id:"directions_overlap",direction:null,
    start_offset_ms:.phases[6].start_offset_ms,
    end_offset_ms:.phases[6].end_offset_ms,
    duration_ms:.phases[6].duration_ms,outcome:"passed"
  }
' "$phase_timings_evidence" >/dev/null ||
  fail "native-overlap phase schema is wrong"

prepare_invalid_case() {
  local label="$1"
  prepare_case "$label"
  record_expected_phases
}

prepare_invalid_case missing
sed -n '1,9p' "$phase_timing_journal" >"${phase_timing_journal}.new"
mv "${phase_timing_journal}.new" "$phase_timing_journal"
chmod 0600 "$phase_timing_journal"
if finalize_phase_timings >/dev/null 2>&1; then
  fail "missing phase record produced timing evidence"
fi
[[ ! -e "$phase_timings_evidence" ]] || fail "missing record published evidence"

prepare_invalid_case duplicate
sed -n '1p' "$phase_timing_journal" >>"$phase_timing_journal"
if finalize_phase_timings >/dev/null 2>&1; then
  fail "duplicate phase record produced timing evidence"
fi

prepare_invalid_case unexpected
jq -c '. + {secret_sentinel:"DO_NOT_RECORD_ME"}' "$phase_timing_journal" \
  >"${phase_timing_journal}.new"
mv "${phase_timing_journal}.new" "$phase_timing_journal"
chmod 0600 "$phase_timing_journal"
if finalize_phase_timings >/dev/null 2>&1; then
  fail "unexpected timing field produced evidence"
fi
[[ ! -e "$phase_timings_evidence" ]] || fail "unexpected field published evidence"

prepare_invalid_case regression
jq -c 'if .sequence == 10 then .end_offset_ms = (.start_offset_ms - 1) else . end' \
  "$phase_timing_journal" >"${phase_timing_journal}.new"
mv "${phase_timing_journal}.new" "$phase_timing_journal"
chmod 0600 "$phase_timing_journal"
if finalize_phase_timings >/dev/null 2>&1; then
  fail "regressing timing record produced evidence"
fi

prepare_invalid_case wrong-mode
chmod 0644 "$phase_timing_journal"
if finalize_phase_timings >/dev/null 2>&1; then
  fail "wrong-mode timing journal produced evidence"
fi

prepare_invalid_case no-clobber
: >"$phase_timings_evidence"
chmod 0600 "$phase_timings_evidence"
if finalize_phase_timings >/dev/null 2>&1; then
  fail "timing publisher overwrote existing final evidence"
fi

prepare_invalid_case partial-no-clobber
printf '%s\n' DO_NOT_OVERWRITE >"${phase_timings_evidence}.partial"
chmod 0600 "${phase_timings_evidence}.partial"
if finalize_phase_timings >/dev/null 2>&1; then
  fail "timing publisher overwrote an existing partial evidence file"
fi
[[ "$(cat "${phase_timings_evidence}.partial")" == DO_NOT_OVERWRITE ]] ||
  fail "existing partial timing evidence was modified"

prepare_invalid_case symlink
mv "$phase_timing_journal" "${phase_timing_journal}.real"
ln -s "${phase_timing_journal}.real" "$phase_timing_journal"
if finalize_phase_timings >/dev/null 2>&1; then
  fail "symlink timing journal produced evidence"
fi

prepare_case tampered-final
record_expected_phases
finalize_phase_timings || fail "tampered-final setup publication failed"
jq '.total_duration_ms += 1' "$phase_timings_evidence" >"${phase_timings_evidence}.new"
mv "${phase_timings_evidence}.new" "$phase_timings_evidence"
chmod 0600 "$phase_timings_evidence"
if validate_phase_timings_for_run_evidence phase_timing_summary >/dev/null 2>&1; then
  fail "tampered finalized timing evidence was accepted for the main packet"
fi

prepare_case object-phases
record_expected_phases
finalize_phase_timings || fail "object-phases setup publication failed"
jq '.phases |= with_entries(.key |= tostring) | .phases |=
  reduce to_entries[] as $entry ({}; .[$entry.key] = $entry.value)' \
  "$phase_timings_evidence" >"${phase_timings_evidence}.new"
mv "${phase_timings_evidence}.new" "$phase_timings_evidence"
chmod 0600 "$phase_timings_evidence"
if validate_phase_timings_for_run_evidence phase_timing_summary >/dev/null 2>&1; then
  fail "object-valued phase collection was accepted as an array"
fi

prepare_case invalid-calendar
record_expected_phases
finalize_phase_timings || fail "invalid-calendar setup publication failed"
jq '.started_at_utc = "2026-99-99T99:99:99Z"' "$phase_timings_evidence" \
  >"${phase_timings_evidence}.new"
mv "${phase_timings_evidence}.new" "$phase_timings_evidence"
chmod 0600 "$phase_timings_evidence"
if validate_phase_timings_for_run_evidence phase_timing_summary >/dev/null 2>&1; then
  fail "calendar-invalid timing timestamp was accepted"
fi

prepare_case wrong-final-mode
record_expected_phases
finalize_phase_timings || fail "wrong-final-mode setup publication failed"
chmod 0644 "$phase_timings_evidence"
if validate_phase_timings_for_run_evidence phase_timing_summary >/dev/null 2>&1; then
  fail "non-private finalized timing evidence was accepted for the main packet"
fi

write_run_evidence_source="$(sed -n '/^write_run_evidence() {$/,/^}$/p' "$runner")"
for binding_term in 'validate_phase_timings_for_run_evidence phase_timing_summary' \
  '--argjson phase_timing_summary "$phase_timing_summary"' \
  'performance:{phase_timings:$phase_timing_summary}' \
  'phase_timings_hash_stable "$phase_timing_sha"'; do
  rg -Fq -- "$binding_term" <<<"$write_run_evidence_source" ||
    fail "main run packet omits timing binding: ${binding_term}"
done
runner_source="$(<"$runner")"
for direction_step in reserve_funding stage_two actor_flow terminal_replay \
  terminal_balances; do
  direction_phase_probe='direction_${direction}_'"$direction_step"
  rg -Fq -- "$direction_phase_probe" <<<"$runner_source" ||
    fail "runner omits direction timing step: ${direction_step}"
done

prepare_case sentinel
if phase_timing_begin 'secret=DO_NOT_RECORD_ME' '' >/dev/null 2>&1; then
  fail "unallowlisted phase identifier was accepted"
fi
[[ ! -s "$phase_timing_journal" ]] || fail "rejected phase leaked into journal"

echo "M3 phase-timings contract passed"
