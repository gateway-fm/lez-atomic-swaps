#!/usr/bin/env bash
set -euo pipefail

[[ "${M3_RECORDING_TESTING:-0}" == 1 ]] || {
  echo "the M3 recording fixture is test-only" >&2
  exit 64
}

evidence_file="${M3_RECORDING_TEST_EVIDENCE_FILE:?missing test evidence file}"
evidence_dir="$(dirname "$evidence_file")"
mkdir -p -- "$evidence_dir"
umask 077

echo "M3 private demo: ${M3_RECORDING_SCENARIO}"
echo "actors: independent maker and taker"
echo "networks: Bitcoin Core Regtest and LEZ private-local"

if [[ "${M3_RECORDING_TEST_FAIL:-0}" == 1 ]]; then
  echo "intentional recording-driver failure" >&2
  exit 73
fi

case "${M3_RECORDING_SCENARIO}" in
  happy)
    packet_kind="m3_actor_two_direction_local_poc"
    journey="claim"
    schedule="sequential"
    slot_duration_seconds="1.0"
    terminal_phase="completed"
    ;;
  refund)
    packet_kind="m3_actor_two_direction_refund_local_poc"
    journey="refund"
    schedule="sequential"
    slot_duration_seconds="3.0"
    terminal_phase="refunded"
    ;;
  concurrent)
    packet_kind="m3_actor_overlapping_two_swap_local_poc"
    journey="claim"
    schedule="overlap"
    slot_duration_seconds="1.0"
    terminal_phase="completed"
    ;;
  *)
    echo "unsupported fixture scenario" >&2
    exit 64
    ;;
esac

write_direction_evidence() {
  local direction="$1"
  local bitcoin_lock bitcoin_terminal lez_initialization lez_funding lez_terminal
  local agreement_sha bitcoin_block_hash lez_block_hash
  local claim_or_refund owner_field terminal_effect_field terminal_effect_chain
  local bitcoin_owner lez_owner role
  case "$direction" in
    taker_sells_foreign)
      bitcoin_lock="$(printf '1%.0s' {1..64})"
      bitcoin_terminal="$(printf '2%.0s' {1..64})"
      lez_initialization="$(printf '3%.0s' {1..64})"
      lez_funding="$(printf '4%.0s' {1..64})"
      lez_terminal="$(printf '5%.0s' {1..64})"
      agreement_sha="$(printf 'a%.0s' {1..64})"
      bitcoin_block_hash="$(printf 'b%.0s' {1..64})"
      lez_block_hash="$(printf 'c%.0s' {1..64})"
      ;;
    taker_sells_lez)
      bitcoin_lock="$(printf '6%.0s' {1..64})"
      bitcoin_terminal="$(printf '7%.0s' {1..64})"
      lez_initialization="$(printf '8%.0s' {1..64})"
      lez_funding="$(printf '9%.0s' {1..64})"
      lez_terminal="$(printf 'd%.0s' {1..64})"
      agreement_sha="$(printf 'e%.0s' {1..64})"
      bitcoin_block_hash="$(printf 'f%.0s' {1..64})"
      lez_block_hash="$(printf '0%.0s' {1..64})"
      ;;
    *)
      echo "unsupported fixture direction" >&2
      exit 64
      ;;
  esac

  jq -n --arg direction "$direction" --arg agreement_sha "$agreement_sha" \
    --arg bitcoin_lock "$bitcoin_lock" --arg bitcoin_block_hash "$bitcoin_block_hash" '
    {schema_version:1,direction:$direction,agreement_revalidated:true,
     agreement_sha256:$agreement_sha,private_material_disclosed:false,
     bitcoin_funding_transaction_id:$bitcoin_lock,
     bitcoin:{funding_transaction_id:$bitcoin_lock,funding_output_index:0,
       claim_value_sat:100000},
     lez_terms:{refund_at_ms:(if $direction == "taker_sells_foreign" then 100000 else 200000 end)},
     recovery:{
       bitcoin_refund_height:(if $direction == "taker_sells_foreign" then 201 else 101 end),
       earlier_refund_latest_unix_seconds:101,
       later_refund_earliest_unix_seconds:201},
     signed_ordering:
       (if $direction == "taker_sells_foreign" then
          {earlier_chain:"lez",earlier_owner:"maker",later_chain:"bitcoin",later_owner:"taker"}
        else
          {earlier_chain:"bitcoin",earlier_owner:"maker",later_chain:"lez",later_owner:"taker"}
        end),
     fixture_chain_anchor_sha256:$bitcoin_block_hash}
  ' >"${evidence_dir}/${direction}-stage-two.json"
  chmod 0600 "${evidence_dir}/${direction}-stage-two.json"

  if [[ "$journey" == claim ]]; then
    claim_or_refund="claims"
    owner_field="actor_owned_claims"
    if [[ "$direction" == taker_sells_foreign ]]; then
      bitcoin_owner=maker
      lez_owner=taker
    else
      bitcoin_owner=taker
      lez_owner=maker
    fi
  else
    claim_or_refund="refunds"
    owner_field="actor_owned_refunds"
    if [[ "$direction" == taker_sells_foreign ]]; then
      bitcoin_owner=taker
      lez_owner=maker
    else
      bitcoin_owner=maker
      lez_owner=taker
    fi
  fi
  jq -n --arg direction "$direction" --arg journey "$journey" \
    --arg bitcoin_lock "$bitcoin_lock" --arg bitcoin_terminal "$bitcoin_terminal" \
    --arg lez_initialization "$lez_initialization" --arg lez_funding "$lez_funding" \
    --arg lez_terminal "$lez_terminal" --arg owner_field "$owner_field" \
    --arg bitcoin_owner "$bitcoin_owner" --arg lez_owner "$lez_owner" '
    {schema_version:1,journey:$journey,direction:$direction,
     bitcoin_effect_ids:[$bitcoin_lock,$bitcoin_terminal],
     lez_effect_ids:[$lez_initialization,$lez_funding,$lez_terminal],
     expected_unique_effects:{bitcoin:2,lez:3}}
    + {($owner_field):{bitcoin:$bitcoin_terminal,lez:$lez_terminal}}
    + {actor_owned_effect_roles:{bitcoin:$bitcoin_owner,lez:$lez_owner}}
    + (if $journey == "refund" then
         {cooperative_claim_effects_present:false}
       else {} end)
  ' >"${evidence_dir}/${direction}-actual-effects.json"
  chmod 0600 "${evidence_dir}/${direction}-actual-effects.json"

  jq -n --arg direction "$direction" --arg bitcoin_lock "$bitcoin_lock" \
    --arg bitcoin_terminal "$bitcoin_terminal" --arg lez_initialization "$lez_initialization" \
    --arg lez_funding "$lez_funding" --arg lez_terminal "$lez_terminal" '
    {schema_version:1,direction:$direction,bitcoin:2,lez:3,
     measurement:"confirmed_unique_bitcoin_effects_and_exact_durable_lez_submissions",
     effect_ids:{bitcoin:[$bitcoin_lock,$bitcoin_terminal],
       lez:[$lez_initialization,$lez_funding,$lez_terminal]}}
  ' >"${evidence_dir}/${direction}-actual-submission-counts.json"
  chmod 0600 "${evidence_dir}/${direction}-actual-submission-counts.json"

  for role in maker taker; do
    if [[ "$role" == "$bitcoin_owner" ]]; then
      terminal_effect_chain=bitcoin
    else
      terminal_effect_chain=lez
    fi
    terminal_effect_field="actor_owned_${claim_or_refund}"
    jq -n --arg direction "$direction" --arg role "$role" \
      --arg phase "$terminal_phase" --arg effect_field "$terminal_effect_field" \
      --arg effect_chain "$terminal_effect_chain" '
      {schema_version:1,direction:$direction,role:$role,command:"status",
       state:"active",revision:4,phase:$phase,next_action:"complete",
       replay_resubmission_count:0,
       ($effect_field):[$effect_chain],private_material_disclosed:false}
    ' >"${evidence_dir}/${direction}-terminal-status-${role}.json"
    chmod 0600 "${evidence_dir}/${direction}-terminal-status-${role}.json"
  done

  if [[ "$journey" == claim ]]; then
    if [[ "$direction" == taker_sells_foreign ]]; then
      first_action="${direction}-lez-revealing-claim-submit-taker.json"
      second_action="${direction}-bitcoin-followup-claim-submit-maker.json"
      first_chain=lez
      second_chain=bitcoin
    else
      first_action="${direction}-bitcoin-revealing-claim-submit-taker.json"
      second_action="${direction}-lez-followup-claim-submit-maker.json"
      first_chain=bitcoin
      second_chain=lez
    fi
    jq -n --arg chain "$first_chain" \
      '{schema_version:1,role:"taker",command:"drive",outcome:"awaiting_observation",chain:$chain,phase:"both_legs_locked",revision:2}' \
      >"${evidence_dir}/${first_action}"
    jq -n --arg chain "$second_chain" \
      '{schema_version:1,role:"maker",command:"drive",outcome:"awaiting_observation",chain:$chain,phase:"claim_evidence_available",revision:3}' \
      >"${evidence_dir}/${second_action}"
  else
    if [[ "$direction" == taker_sells_foreign ]]; then
      first_action="${direction}-lez-maker-refund-submit-maker.json"
      second_action="${direction}-bitcoin-taker-refund-submit-taker.json"
      first_chain=lez
      second_chain=bitcoin
    else
      first_action="${direction}-bitcoin-maker-refund-submit-maker.json"
      second_action="${direction}-lez-taker-refund-submit-taker.json"
      first_chain=bitcoin
      second_chain=lez
    fi
    jq -n --arg chain "$first_chain" \
      '{schema_version:1,role:"maker",command:"recover",outcome:"awaiting_observation",chain:$chain,phase:"both_legs_locked",revision:2}' \
      >"${evidence_dir}/${first_action}"
    jq -n --arg chain "$second_chain" \
      '{schema_version:1,role:"taker",command:"recover",outcome:"awaiting_observation",chain:$chain,phase:"maker_leg_refunded",revision:3}' \
      >"${evidence_dir}/${second_action}"
  fi
  chmod 0600 "${evidence_dir}/${first_action}" "${evidence_dir}/${second_action}"

  if [[ "$journey" != refund ]]; then return; fi

  case "$direction" in
    taker_sells_foreign)
      jq -n --arg hash "$lez_block_hash" '
        {schema_version:1,label:"lez-maker-refund",deadline_ms:100000,
         finalized_tip:{height:40,block_hash:$hash,timestamp_ms:100000},deadline_satisfied:true}
      ' >"${evidence_dir}/${direction}-lez-maker-refund-deadline.json"
      jq -n --arg tx "$lez_terminal" --arg hash "$lez_block_hash" '
        {schema_version:1,label:"lez-maker-refund",transaction_id:$tx,
         window:{start_height:40,finalized_tip:41},occurrences:1,
         containing_block_id:41,containing_block_hash:$hash,bedrock_status:"Finalized",
         id_hash_lookups_equal:true,transaction_hash_revalidated:true}
      ' >"${evidence_dir}/${direction}-lez-maker-refund-finality.json"
      jq -n '
        {schema_version:1,label:"bitcoin-taker-refund-later-bound",bound_unix_seconds:201,
         observed_unix_seconds:201,bound_satisfied:true}
      ' >"${evidence_dir}/${direction}-bitcoin-taker-refund-later-bound-wall-clock.json"
      jq -n --arg hash "$bitcoin_block_hash" '
        {schema_version:1,label:"bitcoin-taker-refund",previous_tip:199,mined_blocks:1,
         eligible_tip:200,eligible_tip_hash:$hash,next_block_is_signed_refund_height:true}
      ' >"${evidence_dir}/${direction}-bitcoin-taker-refund-maturity.json"
      jq -n --arg tx "$bitcoin_terminal" --arg hash "$bitcoin_block_hash" '
        {error:null,result:{txid:$tx,hash:$hash,confirmations:1,blocktime:202,
          vin:[{sequence:144}],
          fixture_order:{after_lez_maker_finality:true,later_bound_satisfied:true}}}
      ' >"${evidence_dir}/${direction}-bitcoin-taker-refund-confirmed.json"
      ;;
    taker_sells_lez)
      jq -n --arg hash "$bitcoin_block_hash" '
        {schema_version:1,label:"bitcoin-maker-refund",previous_tip:99,mined_blocks:1,
         eligible_tip:100,eligible_tip_hash:$hash,next_block_is_signed_refund_height:true}
      ' >"${evidence_dir}/${direction}-bitcoin-maker-refund-maturity.json"
      jq -n --arg tx "$bitcoin_terminal" --arg hash "$bitcoin_block_hash" '
        {error:null,result:{txid:$tx,hash:$hash,confirmations:1,blocktime:100,
          vin:[{sequence:144}],
          fixture_order:{before_lez_taker_deadline:true,earlier_bound_satisfied:true}}}
      ' >"${evidence_dir}/${direction}-bitcoin-maker-refund-confirmed.json"
      jq -n --arg hash "$lez_block_hash" '
        {schema_version:1,label:"lez-taker-refund",deadline_ms:200000,
         finalized_tip:{height:50,block_hash:$hash,timestamp_ms:200000},deadline_satisfied:true}
      ' >"${evidence_dir}/${direction}-lez-taker-refund-deadline.json"
      jq -n --arg tx "$lez_terminal" --arg hash "$lez_block_hash" '
        {schema_version:1,label:"lez-taker-refund",transaction_id:$tx,
         window:{start_height:50,finalized_tip:51},occurrences:1,
         containing_block_id:51,containing_block_hash:$hash,bedrock_status:"Finalized",
         id_hash_lookups_equal:true,transaction_hash_revalidated:true}
      ' >"${evidence_dir}/${direction}-lez-taker-refund-finality.json"
      ;;
  esac
  chmod 0600 "${evidence_dir}/${direction}"-*-refund-*.json
}

write_direction_evidence taker_sells_foreign
write_direction_evidence taker_sells_lez

foreign_stage_two_sha="$(sha256sum "${evidence_dir}/taker_sells_foreign-stage-two.json" | sed 's/ .*//')"
lez_stage_two_sha="$(sha256sum "${evidence_dir}/taker_sells_lez-stage-two.json" | sed 's/ .*//')"

jq -n \
  --arg kind "$packet_kind" \
  --arg journey "$journey" \
  --arg schedule "$schedule" \
  --arg terminal_phase "$terminal_phase" \
  --arg run_id "${RUN_ID}" \
  --arg bitcoin_run_id "${RUN_ID}-btc" \
  --arg lez_run_id "${RUN_ID}-lez" \
  --arg slot_duration_seconds "$slot_duration_seconds" \
  --arg foreign_stage_two_sha "$foreign_stage_two_sha" \
  --arg lez_stage_two_sha "$lez_stage_two_sha" \
  --arg repository_commit "${M3_RECORDING_TEST_COMMIT}" '
  {
    schema_version: 1,
    kind: $kind,
    journey: $journey,
    schedule: $schedule,
    result: "passed",
    run_id: $run_id,
    repository_commit: $repository_commit,
    execution_provenance: {
      repository_clean_exact_head: true,
      origin_main_equals_head: true,
      executable_hashes_stable_from_start_to_publication: true,
      synthetic_test_fixture: true
    },
    services: {
      bitcoin_core: {
        run_id: $bitcoin_run_id,
        version: "31.1",
        network: "regtest"
      },
      lez: {
        run_id: $lez_run_id,
        version: "v0.2.0",
        network: "private_local",
        slot_duration_seconds: $slot_duration_seconds
      }
    },
    external_resources: {
      public_rpc: false,
      faucet: false,
      public_funds: false,
      certification_success_depends_on_external_network: false
    },
    directions: [
      {direction:"taker_sells_foreign",terminal_revision:4,
       terminal_phase:$terminal_phase,expected_unique_effects:{bitcoin:2,lez:3},
       maker_second_lock_effect_count:1,stage_two_evidence_sha256:$foreign_stage_two_sha},
      {direction:"taker_sells_lez",terminal_revision:4,
       terminal_phase:$terminal_phase,expected_unique_effects:{bitcoin:2,lez:3},
       maker_second_lock_effect_count:1,stage_two_evidence_sha256:$lez_stage_two_sha}
    ],
    concurrency: (
      if $schedule == "overlap" then {
        simultaneous_in_flight: true,
        overlap_revision: 2,
        overlap_phase: "both_legs_locked",
        distinct_funding_outpoints: true,
        distinct_agreements: true,
        distinct_actor_state_dbs: true,
        distinct_signing_journals: true,
        distinct_signer_sessions_per_domain: true,
        distinct_escrows: true,
        distinct_deadlines: true,
        arbitrary_n_or_same_direction_scheduler_proven: false
      } else null end
    ),
    replay_resubmission_count: 0,
    private_material_disclosed: false,
    actor_process_model: "fresh_one_shot_process_per_command",
    actor_owned_effect_semantics: $journey
  }
' >"$evidence_file"
chmod 0600 "$evidence_file"

echo "M3 private demo passed"
