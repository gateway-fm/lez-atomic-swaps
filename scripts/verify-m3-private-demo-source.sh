#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
repository_root="$(pwd -P)"
readonly repository_root

fail() {
  echo "M3 private demo source verification failed: $*" >&2
  exit 1
}

for dependency in jq realpath scriptreplay sha256sum stat; do
  command -v "$dependency" >/dev/null 2>&1 || fail "missing dependency: ${dependency}"
done

(( $# == 1 )) || fail "provide exactly one source recording manifest"
source_manifest="$1"
[[ -f "$source_manifest" && ! -L "$source_manifest" ]] ||
  fail "source recording manifest must be a regular non-symlink file"
source_manifest="$(realpath -e -- "$source_manifest")"
readonly source_manifest
[[ "$(stat -c '%a' "$source_manifest")" == 600 ]] ||
  fail "source recording manifest must have mode 0600"

jq -e '
  .schema_version == 1 and .kind == "m3_private_terminal_recording" and
  .result == "passed" and
  (.certification_mode == "live_actual_nodes" or .certification_mode == "test_contract") and
  .privacy == "private_local_stealth" and
  (.scenario == "happy" or .scenario == "refund" or .scenario == "concurrent") and
  (.run_id | test("^[a-zA-Z0-9][a-zA-Z0-9._-]{0,95}$")) and
  (.repository_commit | test("^[0-9a-f]{40}$")) and
  .networks.bitcoin_core.version == "31.1" and
  .networks.bitcoin_core.network == "regtest" and
  .networks.lez.version == "v0.2.0" and
  .networks.lez.network == "private_local" and
  .external_resources.public_rpc == false and
  .external_resources.faucet == false and
  .external_resources.public_funds == false and
  .external_resources.certification_success_depends_on_external_network == false and
  .recording.format == "util-linux-script-classic-v1" and
  .recording.output_file == "terminal.typescript" and
  .recording.timing_file == "terminal.timing" and
  (.recording.output_sha256 | test("^[0-9a-f]{64}$")) and
  (.recording.timing_sha256 | test("^[0-9a-f]{64}$")) and
  (.evidence.sha256 | test("^[0-9a-f]{64}$"))
' "$source_manifest" >/dev/null || fail "source recording manifest contract is invalid"

scenario="$(jq -er '.scenario' "$source_manifest")"
run_id="$(jq -er '.run_id' "$source_manifest")"
repository_commit="$(jq -er '.repository_commit' "$source_manifest")"
certification_mode="$(jq -er '.certification_mode' "$source_manifest")"
readonly scenario run_id repository_commit certification_mode

recording_dir="$(dirname "$source_manifest")"
terminal_output="${recording_dir}/$(jq -er '.recording.output_file' "$source_manifest")"
terminal_timing="${recording_dir}/$(jq -er '.recording.timing_file' "$source_manifest")"
for private_file in "$terminal_output" "$terminal_timing"; do
  [[ -s "$private_file" && ! -L "$private_file" ]] || fail "source terminal recording is missing"
  [[ "$(stat -c '%a' "$private_file")" == 600 ]] || fail "source terminal recording must have mode 0600"
done
[[ "$(sha256sum "$terminal_output" | cut -d ' ' -f 1)" == "$(jq -er '.recording.output_sha256' "$source_manifest")" ]] ||
  fail "source terminal output hash mismatch"
[[ "$(sha256sum "$terminal_timing" | cut -d ' ' -f 1)" == "$(jq -er '.recording.timing_sha256' "$source_manifest")" ]] ||
  fail "source terminal timing hash mismatch"
scriptreplay --summary --log-timing "$terminal_timing" --log-out "$terminal_output" >/dev/null ||
  fail "source terminal recording is not replayable"

evidence_path="$(jq -er '.evidence.packet' "$source_manifest")"
if [[ "$evidence_path" == /* ]]; then
  evidence_file="$evidence_path"
else
  evidence_file="${repository_root}/${evidence_path}"
fi
[[ -f "$evidence_file" && ! -L "$evidence_file" ]] || fail "source actual-node evidence is missing"
evidence_file="$(realpath -e -- "$evidence_file")"
readonly evidence_file
[[ "$(stat -c '%a' "$evidence_file")" == 600 ]] || fail "source actual-node evidence must have mode 0600"
[[ "$(sha256sum "$evidence_file" | cut -d ' ' -f 1)" == "$(jq -er '.evidence.sha256' "$source_manifest")" ]] ||
  fail "source actual-node evidence hash mismatch"
evidence_dir="$(dirname "$evidence_file")"
readonly evidence_dir

case "$scenario" in
  happy)
    expected_kind="m3_actor_two_direction_local_poc"
    expected_journey="claim"
    expected_schedule="sequential"
    expected_terminal="completed"
    scenario_assertion="terminal_claim_completion"
    ;;
  refund)
    expected_kind="m3_actor_two_direction_refund_local_poc"
    expected_journey="refund"
    expected_schedule="sequential"
    expected_terminal="refunded"
    scenario_assertion="ordered_timelock_refunds"
    ;;
  concurrent)
    expected_kind="m3_actor_overlapping_two_swap_local_poc"
    expected_journey="claim"
    expected_schedule="overlap"
    expected_terminal="completed"
    scenario_assertion="simultaneous_revision_two_overlap"
    ;;
esac
readonly expected_kind expected_journey expected_schedule expected_terminal scenario_assertion

jq -e \
  --arg kind "$expected_kind" --arg journey "$expected_journey" \
  --arg schedule "$expected_schedule" --arg terminal "$expected_terminal" \
  --arg run_id "$run_id" --arg commit "$repository_commit" '
    .schema_version == 1 and .kind == $kind and .journey == $journey and
    .schedule == $schedule and .result == "passed" and .run_id == $run_id and
    .repository_commit == $commit and .private_material_disclosed == false and
    .actor_process_model == "fresh_one_shot_process_per_command" and
    .actor_owned_effect_semantics == $journey and
    .replay_resubmission_count == 0 and
    .execution_provenance.repository_clean_exact_head == true and
    .execution_provenance.origin_main_equals_head == true and
    .execution_provenance.executable_hashes_stable_from_start_to_publication == true and
    ([.directions[].direction] | sort == ["taker_sells_foreign","taker_sells_lez"]) and
    all(.directions[];
      .terminal_revision == 4 and .terminal_phase == $terminal and
      .expected_unique_effects == {bitcoin:2,lez:3} and
      .maker_second_lock_effect_count == 1 and
      (.stage_two_evidence_sha256 | test("^[0-9a-f]{64}$"))) and
    (if $schedule == "overlap" then
      .concurrency.simultaneous_in_flight == true and
      .concurrency.overlap_revision == 2 and
      .concurrency.overlap_phase == "both_legs_locked" and
      .concurrency.distinct_funding_outpoints == true and
      .concurrency.distinct_agreements == true and
      .concurrency.distinct_actor_state_dbs == true and
      .concurrency.distinct_signing_journals == true and
      .concurrency.distinct_signer_sessions_per_domain == true and
      .concurrency.distinct_escrows == true and
      .concurrency.distinct_deadlines == true
    else .concurrency == null end)
  ' "$evidence_file" >/dev/null || fail "aggregate actual-node evidence does not prove the selected scenario"

source_entries=()
direction_entries=()

source_ref() {
  local file="$1"
  if [[ "$file" == "${repository_root}/"* ]]; then
    printf '%s' "${file#"${repository_root}/"}"
  else
    printf '%s' "$file"
  fi
}

add_source() {
  local kind="$1"
  local file="$2"
  [[ -s "$file" && ! -L "$file" ]] || fail "supporting source is missing: ${kind}"
  [[ "$(stat -c '%a' "$file")" == 600 ]] || fail "supporting source must have mode 0600: ${kind}"
  source_entries+=("$(jq -cn \
    --arg kind "$kind" --arg path "$(source_ref "$file")" \
    --arg sha256 "$(sha256sum "$file" | cut -d ' ' -f 1)" \
    '{kind:$kind,path:$path,sha256:$sha256}')")
}

add_source recording_manifest "$source_manifest"
add_source terminal_output "$terminal_output"
add_source terminal_timing "$terminal_timing"
add_source aggregate_actual_node_evidence "$evidence_file"

for direction in taker_sells_foreign taker_sells_lez; do
  stage_two="${evidence_dir}/${direction}-stage-two.json"
  effects="${evidence_dir}/${direction}-actual-effects.json"
  submissions="${evidence_dir}/${direction}-actual-submission-counts.json"
  maker_terminal="${evidence_dir}/${direction}-terminal-status-maker.json"
  taker_terminal="${evidence_dir}/${direction}-terminal-status-taker.json"
  for supporting in "$stage_two" "$effects" "$submissions" "$maker_terminal" "$taker_terminal"; do
    [[ -s "$supporting" && ! -L "$supporting" ]] || fail "direction evidence is missing: ${supporting##*/}"
    [[ "$(stat -c '%a' "$supporting")" == 600 ]] || fail "direction evidence must have mode 0600"
  done
  expected_stage_hash="$(jq -er --arg direction "$direction" '.directions[] | select(.direction == $direction) | .stage_two_evidence_sha256' "$evidence_file")"
  [[ "$(sha256sum "$stage_two" | cut -d ' ' -f 1)" == "$expected_stage_hash" ]] ||
    fail "stage-two evidence hash mismatch for ${direction}"
  jq -e --arg direction "$direction" '
    .schema_version == 1 and .direction == $direction and
    .agreement_revalidated == true and .private_material_disclosed == false and
    (.agreement_sha256 | test("^[0-9a-f]{64}$")) and
    (.bitcoin_funding_transaction_id | test("^[0-9a-f]{64}$"))
  ' "$stage_two" >/dev/null || fail "stage-two evidence contract failed for ${direction}"

  if [[ "$expected_journey" == claim ]]; then
    jq -e --arg direction "$direction" '
      .schema_version == 1 and .journey == "claim" and .direction == $direction and
      .expected_unique_effects == {bitcoin:2,lez:3} and
      ((.bitcoin_effect_ids | length) == 2 and (.bitcoin_effect_ids | unique | length) == 2) and
      ((.lez_effect_ids | length) == 3 and (.lez_effect_ids | unique | length) == 3) and
      all(.bitcoin_effect_ids[], .lez_effect_ids[]; test("^[0-9a-f]{64}$")) and
      .actor_owned_claims.bitcoin == .bitcoin_effect_ids[1] and
      .actor_owned_claims.lez == .lez_effect_ids[2]
    ' "$effects" >/dev/null || fail "claim effects are invalid for ${direction}"
    if [[ "$direction" == taker_sells_foreign ]]; then
      first_action="${evidence_dir}/${direction}-lez-revealing-claim-submit-taker.json"
      second_action="${evidence_dir}/${direction}-bitcoin-followup-claim-submit-maker.json"
      first_chain=lez
      second_chain=bitcoin
    else
      first_action="${evidence_dir}/${direction}-bitcoin-revealing-claim-submit-taker.json"
      second_action="${evidence_dir}/${direction}-lez-followup-claim-submit-maker.json"
      first_chain=bitcoin
      second_chain=lez
    fi
    jq -e --arg chain "$first_chain" '
      .schema_version == 1 and .role == "taker" and .command == "drive" and
      .outcome == "awaiting_observation" and .chain == $chain and
      .phase == "both_legs_locked" and .revision == 2
    ' "$first_action" >/dev/null || fail "Taker revealing-claim authority is invalid for ${direction}"
    jq -e --arg chain "$second_chain" '
      .schema_version == 1 and .role == "maker" and .command == "drive" and
      .outcome == "awaiting_observation" and .chain == $chain and
      .phase == "claim_evidence_available" and .revision == 3
    ' "$second_action" >/dev/null || fail "Maker follow-up-claim authority is invalid for ${direction}"
  else
    jq -e --arg direction "$direction" '
      .schema_version == 1 and .journey == "refund" and .direction == $direction and
      .expected_unique_effects == {bitcoin:2,lez:3} and
      ((.bitcoin_effect_ids | length) == 2 and (.bitcoin_effect_ids | unique | length) == 2) and
      ((.lez_effect_ids | length) == 3 and (.lez_effect_ids | unique | length) == 3) and
      all(.bitcoin_effect_ids[], .lez_effect_ids[]; test("^[0-9a-f]{64}$")) and
      .actor_owned_refunds.bitcoin == .bitcoin_effect_ids[1] and
      .actor_owned_refunds.lez == .lez_effect_ids[2] and
      .cooperative_claim_effects_present == false
    ' "$effects" >/dev/null || fail "refund effects are invalid for ${direction}"
    if [[ "$direction" == taker_sells_foreign ]]; then
      first_action="${evidence_dir}/${direction}-lez-maker-refund-submit-maker.json"
      second_action="${evidence_dir}/${direction}-bitcoin-taker-refund-submit-taker.json"
      first_chain=lez
      second_chain=bitcoin
    else
      first_action="${evidence_dir}/${direction}-bitcoin-maker-refund-submit-maker.json"
      second_action="${evidence_dir}/${direction}-lez-taker-refund-submit-taker.json"
      first_chain=bitcoin
      second_chain=lez
    fi
    jq -e --arg chain "$first_chain" '
      .schema_version == 1 and .role == "maker" and .command == "recover" and
      .outcome == "awaiting_observation" and .chain == $chain and
      .phase == "both_legs_locked" and .revision == 2
    ' "$first_action" >/dev/null || fail "Maker earlier-refund authority is invalid for ${direction}"
    jq -e --arg chain "$second_chain" '
      .schema_version == 1 and .role == "taker" and .command == "recover" and
      .outcome == "awaiting_observation" and .chain == $chain and
      .phase == "maker_leg_refunded" and .revision == 3
    ' "$second_action" >/dev/null || fail "Taker later-refund authority is invalid for ${direction}"
  fi
  jq -e --arg direction "$direction" --slurpfile effects "$effects" '
    .schema_version == 1 and .direction == $direction and .bitcoin == 2 and .lez == 3 and
    .measurement == "confirmed_unique_bitcoin_effects_and_exact_durable_lez_submissions" and
    .effect_ids.bitcoin == $effects[0].bitcoin_effect_ids and
    .effect_ids.lez == $effects[0].lez_effect_ids
  ' "$submissions" >/dev/null || fail "submission counts do not match effects for ${direction}"
  jq -e --arg terminal "$expected_terminal" '
    .schema_version == 1 and .role == "maker" and .state == "active" and
    .phase == $terminal and .revision == 4 and .next_action == "complete"
  ' "$maker_terminal" >/dev/null || fail "maker terminal state is invalid for ${direction}"
  jq -e --arg terminal "$expected_terminal" '
    .schema_version == 1 and .role == "taker" and .state == "active" and
    .phase == $terminal and .revision == 4 and .next_action == "complete"
  ' "$taker_terminal" >/dev/null || fail "taker terminal state is invalid for ${direction}"

  add_source "${direction}_stage_two" "$stage_two"
  add_source "${direction}_effects" "$effects"
  add_source "${direction}_submissions" "$submissions"
  add_source "${direction}_maker_terminal" "$maker_terminal"
  add_source "${direction}_taker_terminal" "$taker_terminal"
  add_source "${direction}_first_effect_action" "$first_action"
  add_source "${direction}_second_effect_action" "$second_action"

  refund_order='null'
  if [[ "$scenario" == refund && "$direction" == taker_sells_foreign ]]; then
    earlier_deadline="${evidence_dir}/${direction}-lez-maker-refund-deadline.json"
    earlier_finality="${evidence_dir}/${direction}-lez-maker-refund-finality.json"
    later_bound="${evidence_dir}/${direction}-bitcoin-taker-refund-later-bound-wall-clock.json"
    later_maturity="${evidence_dir}/${direction}-bitcoin-taker-refund-maturity.json"
    later_confirmed="${evidence_dir}/${direction}-bitcoin-taker-refund-confirmed.json"
    jq -n -e --slurpfile effects "$effects" --slurpfile deadline "$earlier_deadline" \
      --slurpfile finality "$earlier_finality" --slurpfile bound "$later_bound" \
      --slurpfile maturity "$later_maturity" --slurpfile confirmed "$later_confirmed" '
        $deadline[0].label == "lez-maker-refund" and $deadline[0].deadline_satisfied == true and
        $deadline[0].finalized_tip.timestamp_ms >= $deadline[0].deadline_ms and
        $finality[0].transaction_id == $effects[0].actor_owned_refunds.lez and
        $finality[0].occurrences == 1 and $finality[0].bedrock_status == "Finalized" and
        $finality[0].id_hash_lookups_equal == true and $finality[0].transaction_hash_revalidated == true and
        $bound[0].label == "bitcoin-taker-refund-later-bound" and $bound[0].bound_satisfied == true and
        $bound[0].observed_unix_seconds >= $bound[0].bound_unix_seconds and
        ($bound[0].bound_unix_seconds * 1000) > $deadline[0].finalized_tip.timestamp_ms and
        $maturity[0].label == "bitcoin-taker-refund" and $maturity[0].next_block_is_signed_refund_height == true and
        $confirmed[0].result.txid == $effects[0].actor_owned_refunds.bitcoin and
        $confirmed[0].result.confirmations >= 1 and $confirmed[0].result.vin[0].sequence == 144
        and $confirmed[0].result.blocktime >= $bound[0].bound_unix_seconds and
        ($confirmed[0].result.blocktime * 1000) > $deadline[0].finalized_tip.timestamp_ms
      ' >/dev/null || fail "ordered LEZ-maker then Bitcoin-taker refunds are not proven"
    for pair in \
      "refund_earlier_deadline:$earlier_deadline" "refund_earlier_finality:$earlier_finality" \
      "refund_later_bound:$later_bound" "refund_later_maturity:$later_maturity" \
      "refund_later_confirmation:$later_confirmed"; do add_source "${direction}_${pair%%:*}" "${pair#*:}"; done
    refund_order="$(jq -cn --slurpfile deadline "$earlier_deadline" --slurpfile bound "$later_bound" \
      '{earlier_actor:"maker",earlier_chain:"lez",later_actor:"taker",later_chain:"bitcoin",earlier_observed_ms:$deadline[0].finalized_tip.timestamp_ms,later_bound_ms:($bound[0].bound_unix_seconds*1000)}')"
  elif [[ "$scenario" == refund ]]; then
    earlier_maturity="${evidence_dir}/${direction}-bitcoin-maker-refund-maturity.json"
    earlier_confirmed="${evidence_dir}/${direction}-bitcoin-maker-refund-confirmed.json"
    later_deadline="${evidence_dir}/${direction}-lez-taker-refund-deadline.json"
    later_finality="${evidence_dir}/${direction}-lez-taker-refund-finality.json"
    jq -n -e --slurpfile effects "$effects" --slurpfile maturity "$earlier_maturity" \
      --slurpfile confirmed "$earlier_confirmed" --slurpfile deadline "$later_deadline" \
      --slurpfile finality "$later_finality" '
        $maturity[0].label == "bitcoin-maker-refund" and $maturity[0].next_block_is_signed_refund_height == true and
        $confirmed[0].result.txid == $effects[0].actor_owned_refunds.bitcoin and
        $confirmed[0].result.confirmations >= 1 and $confirmed[0].result.vin[0].sequence == 144 and
        $deadline[0].label == "lez-taker-refund" and $deadline[0].deadline_satisfied == true and
        $deadline[0].finalized_tip.timestamp_ms >= $deadline[0].deadline_ms and
        $deadline[0].deadline_ms > ($confirmed[0].result.blocktime * 1000) and
        $finality[0].transaction_id == $effects[0].actor_owned_refunds.lez and
        $finality[0].occurrences == 1 and $finality[0].bedrock_status == "Finalized" and
        $finality[0].id_hash_lookups_equal == true and $finality[0].transaction_hash_revalidated == true
      ' >/dev/null || fail "ordered Bitcoin-maker then LEZ-taker refunds are not proven"
    for pair in \
      "refund_earlier_maturity:$earlier_maturity" "refund_earlier_confirmation:$earlier_confirmed" \
      "refund_later_deadline:$later_deadline" "refund_later_finality:$later_finality"; do add_source "${direction}_${pair%%:*}" "${pair#*:}"; done
    refund_order="$(jq -cn --slurpfile confirmed "$earlier_confirmed" --slurpfile deadline "$later_deadline" \
      '{earlier_actor:"maker",earlier_chain:"bitcoin",later_actor:"taker",later_chain:"lez",earlier_observed_ms:($confirmed[0].result.blocktime*1000),later_bound_ms:$deadline[0].deadline_ms}')"
  fi

  direction_entries+=("$(jq -cn \
    --arg direction "$direction" --arg terminal "$expected_terminal" \
    --arg first_role "$(jq -er '.role' "$first_action")" \
    --arg first_chain "$first_chain" --arg first_phase "$(jq -er '.phase' "$first_action")" \
    --arg second_role "$(jq -er '.role' "$second_action")" \
    --arg second_chain "$second_chain" --arg second_phase "$(jq -er '.phase' "$second_action")" \
    --argjson effects "$(jq -c '.' "$effects")" --argjson refund_order "$refund_order" '
      {direction:$direction,terminal_revision:4,terminal_phase:$terminal,
       role_terminals:[{role:"maker",phase:$terminal,revision:4},{role:"taker",phase:$terminal,revision:4}],
       effects:$effects,
       actor_effect_actions:[
         {role:$first_role,chain:$first_chain,phase:$first_phase},
         {role:$second_role,chain:$second_chain,phase:$second_phase}
       ],refund_order:$refund_order}')")
done

source_inputs="$(printf '%s\n' "${source_entries[@]}" | jq -cs 'sort_by(.kind)')"
directions="$(printf '%s\n' "${direction_entries[@]}" | jq -cs 'sort_by(.direction)')"
readonly source_inputs directions
jq -n \
  --arg scenario "$scenario" --arg run_id "$run_id" \
  --arg repository_commit "$repository_commit" --arg certification_mode "$certification_mode" \
  --arg scenario_assertion "$scenario_assertion" \
  --argjson networks "$(jq -c '.networks' "$source_manifest")" \
  --argjson external_resources "$(jq -c '.external_resources' "$source_manifest")" \
  --argjson source_inputs "$source_inputs" --argjson directions "$directions" \
  --argjson concurrency "$(jq -c '.concurrency' "$evidence_file")" '
    {
      schema_version:1,kind:"m3_private_demo_proof",result:"passed",
      scenario:$scenario,run_id:$run_id,repository_commit:$repository_commit,
      certification_mode:$certification_mode,privacy:"private_local_stealth",
      scenario_assertion:$scenario_assertion,
      actor_process_model:"fresh_one_shot_process_per_command",
      replay_resubmission_count:0,networks:$networks,external_resources:$external_resources,
      directions:$directions,concurrency:$concurrency,source_inputs:$source_inputs
    }
' | jq -cS '.'
