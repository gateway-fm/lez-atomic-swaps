#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly initial_terms_hash="1111111111111111111111111111111111111111111111111111111111111111"
readonly pda_probe_secret_digest="2222222222222222222222222222222222222222222222222222222222222222"
# The outer actor call must outlive the sidecar's bounded 90-second historical
# reconstruction budget without turning one bridge request into a retry.
readonly actor_lez_bridge_request_timeout_millis=120000
readonly asset_mode="${M3_POC_ASSET_MODE:-native}"
readonly m5_btc_application_mode="${M5_BTC_APPLICATION_MODE:-0}"
if [[ "$m5_btc_application_mode" != 0 && "$m5_btc_application_mode" != 1 ]]; then
  echo 'M5_BTC_APPLICATION_MODE must be 0 or 1' >&2
  exit 2
fi
direction_timing_execution_mode=""
direction_timing_dir=""
direction_timing_journal=""
direction_timing_evidence=""
direction_timing_now_ms=0
direction_timing_origin_ms=0
direction_timing_started_at_utc=""
direction_timing_active_phase=""
direction_timing_active_start_ms=0
direction_timing_sequence=0

fail() {
  echo "M3 actor direction failed: $*" >&2
  exit 2
}

emit_contract() {
  jq -n --arg asset_mode "$asset_mode" \
    --arg m5_btc_application_mode "$m5_btc_application_mode" \
    --argjson bridge_timeout "$actor_lez_bridge_request_timeout_millis" '
    {
      schema_version: 1,
      kind: "m3_actor_direction_driver_contract",
      m5_btc_application_mode: ($m5_btc_application_mode == "1"),
      stage_two_swap_id_source:
        (if $m5_btc_application_mode == "1" then
           "authenticated_delivery_reservation"
         else "fresh_local_random" end),
      application_route:
        (if $m5_btc_application_mode == "1" then {
          pair:"bitcoin", direction:"taker_sells_foreign",
          asset_mode:"native", journey:"claim"
        } else null end),
      runtime_backend: "repository_owned_actual_node_implementation",
      stage_two_spec_uses_actual_node_facts: true,
      fresh_actor_process_per_command: true,
      separate_role_state_and_signing_journals: true,
      taker_first_effects: true,
      dual_locks_before_scalar_use: true,
      bitcoin_exact_signed_depth: true,
      bitcoin_planned_funding_anchor_exact: true,
      lez_exact_finalized_ancestry: true,
      actor_owned_maker_lock_effects: true,
      taker_first_lock_external_runner_submission: true,
      maker_lock_submission_actor_output: "awaiting_observation",
      maker_lock_restart_never_resubmits: true,
      runner_only_confirms_actor_submitted_maker_locks: true,
      actor_owned_claim_effects: true,
      actor_owned_survivor_claim_effects: true,
      actor_owned_refund_effects: true,
      actor_owned_first_lock_refund_effects: true,
      first_lock_refund_terminal_revision: 2,
      first_lock_refund_requires_signed_maker_cutoff: true,
      first_lock_refund_requires_two_fresh_absence_and_unspent_reads: true,
      first_lock_refund_lez_absence_window_reaches_current_finalized_tip: true,
      first_lock_refund_bitcoin_cutoff_uses_stable_median_time: true,
      first_lock_refund_owner_restart_never_resubmits: true,
      first_lock_refund_fresh_maker_observer: true,
      first_lock_refund_abandoned_maker_after_activation_until_finality: true,
      first_lock_refund_taker_only_revision_one_and_refund_projection: true,
      survivor_revealer_absent_until_follower_terminal: true,
      survivor_fresh_follower_restarts: true,
      survivor_intermediate_phase: "claim_evidence_available",
      survivor_intermediate_terminal: false,
      journeys: ["claim", "survivor_claim", "refund", "first_lock_refund"],
      maker_lock_cutoff_schedule: {
        claim_and_survivor_seconds_after_preparation: 1800,
        two_lock_refund_seconds_after_preparation: 300,
        first_lock_refund_seconds_after_preparation: 0,
        required_reaction_margin_seconds: 600
      },
      default_journey: "claim",
      timeout_terminal_phase: "refunded",
      asset_mode: $asset_mode,
      actor_config_schema_version:
        (if $m5_btc_application_mode == "1" then 6
         elif $asset_mode == "custom_token" then 5 else 4 end),
      asset_extension_required: ($asset_mode == "custom_token"),
      official_token_ata_derivation_required: ($asset_mode == "custom_token"),
      asset_first_lock_order:
        (if $asset_mode == "custom_token" then ["initialize_witnessed","create_custody_ata","fund"]
         else [] end),
      role_shaped_bitcoin_refund_authority: true,
      secure_sidecar_state_root_required: true,
      single_core_rpc_response_per_call: true,
      anchor_height_uses_allowed_blockchain_info: true,
      prelock_policy_response_retained: true,
      role_allowed_block_and_mempool_observation: true,
      bounded_read_only_observation_retries_never_resubmit: true,
      bounded_pending_observation_retries: true,
      bounded_prepared_bitcoin_claim_reconciliation: true,
      actor_lez_bridge_request_timeout_millis: $bridge_timeout,
      submission_count_query: true,
      owned_process_registry: true,
      pre_lock_presignature_domains: ["bitcoin", "lez"],
      expected_unique_effects:
        {bitcoin: 2, lez:(if $asset_mode == "custom_token" then 4 else 3 end)},
      submission_count_semantics: "unique_effects_plus_durable_one_shot_authority",
      commands: ["preflight","effect-plan","prepare-stage-two-spec","run-actor-flow",
        "run-overlap-actor-flow","submission-counts"]
    }'
}

emit_effect_plan() {
  local direction="$1"
  local journey="${2:-claim}"
  case "$journey:$direction" in
    claim:taker_sells_foreign)
      jq -n --arg direction "$direction" --arg asset_mode "$asset_mode" '
        {schema_version:1,journey:"claim",direction:$direction,
         before_first_effect:
           (if $asset_mode == "custom_token" then
             ["finalize_agreement","finalize_asset_extension","prepare_exact_lez_asset_claim",
              "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"]
            else ["finalize_agreement","prepare_exact_lez_claim",
              "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"] end),
         public_effect_order:
           (if $asset_mode == "custom_token" then
             ["bitcoin_lock_by_taker","lez_initialize_by_maker",
              "lez_create_custody_ata_by_maker","lez_fund_by_maker","dual_lock_gate",
              "lez_claim_by_taker","bitcoin_claim_by_maker"]
            else ["bitcoin_lock_by_taker","lez_initialize_by_maker",
              "lez_fund_by_maker","dual_lock_gate","lez_claim_by_taker",
              "bitcoin_claim_by_maker"] end),
         terminal:{maker_revision:4,taker_revision:4}}
      '
      ;;
    claim:taker_sells_lez)
      jq -n --arg direction "$direction" --arg asset_mode "$asset_mode" '
        {schema_version:1,journey:"claim",direction:$direction,
         before_first_effect:
           (if $asset_mode == "custom_token" then
             ["finalize_agreement","finalize_asset_extension","prepare_exact_lez_asset_claim",
              "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"]
            else ["finalize_agreement","prepare_exact_lez_claim",
              "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"] end),
         public_effect_order:
           (if $asset_mode == "custom_token" then
             ["lez_initialize_by_taker","lez_create_custody_ata_by_taker","lez_fund_by_taker",
              "bitcoin_lock_by_maker","dual_lock_gate","bitcoin_claim_by_taker",
              "lez_claim_by_maker"]
            else ["lez_initialize_by_taker","lez_fund_by_taker",
              "bitcoin_lock_by_maker","dual_lock_gate","bitcoin_claim_by_taker",
              "lez_claim_by_maker"] end),
         terminal:{maker_revision:4,taker_revision:4}}
      '
      ;;
    survivor_claim:taker_sells_foreign)
      jq -n --arg direction "$direction" '
        {schema_version:1,journey:"survivor_claim",direction:$direction,
         before_first_effect:["finalize_agreement","prepare_exact_lez_claim",
           "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"],
         public_effect_order:["bitcoin_lock_by_taker","lez_initialize_by_maker",
           "lez_fund_by_maker","dual_lock_gate","lez_claim_by_taker",
           "fresh_maker_observes_reveal","maker_revision_three_nonterminal",
           "fresh_maker_bitcoin_claim","delayed_taker_observation_only_catchup"],
         survivor:{revealer:"taker",follower:"maker",revealing_chain:"lez",
           followup_chain:"bitcoin",intermediate_phase:"claim_evidence_available",
           intermediate_terminal:false},
         terminal:{maker_revision:4,taker_revision:4}}
      '
      ;;
    survivor_claim:taker_sells_lez)
      jq -n --arg direction "$direction" '
        {schema_version:1,journey:"survivor_claim",direction:$direction,
         before_first_effect:["finalize_agreement","prepare_exact_lez_claim",
           "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"],
         public_effect_order:["lez_initialize_by_taker","lez_fund_by_taker",
           "bitcoin_lock_by_maker","dual_lock_gate","bitcoin_claim_by_taker",
           "fresh_maker_observes_reveal","maker_revision_three_nonterminal",
           "fresh_maker_lez_claim","delayed_taker_observation_only_catchup"],
         survivor:{revealer:"taker",follower:"maker",revealing_chain:"bitcoin",
           followup_chain:"lez",intermediate_phase:"claim_evidence_available",
           intermediate_terminal:false},
         terminal:{maker_revision:4,taker_revision:4}}
      '
      ;;
    refund:taker_sells_foreign)
      jq -n --arg direction "$direction" --arg asset_mode "$asset_mode" '
        {schema_version:1,journey:"refund",direction:$direction,
         before_first_effect:["finalize_agreement","prepare_exact_lez_claim",
           "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"],
         public_effect_order:
           (if $asset_mode == "custom_token" then
             ["bitcoin_lock_by_taker","lez_initialize_by_maker",
              "lez_create_custody_ata_by_maker","lez_fund_by_maker","dual_lock_gate",
              "lez_refund_by_maker","bitcoin_refund_by_taker"]
            else ["bitcoin_lock_by_taker","lez_initialize_by_maker",
              "lez_fund_by_maker","dual_lock_gate","lez_refund_by_maker",
              "bitcoin_refund_by_taker"] end),
         terminal:{maker_revision:4,taker_revision:4,phase:"refunded"}}
      '
      ;;
    refund:taker_sells_lez)
      jq -n --arg direction "$direction" --arg asset_mode "$asset_mode" '
        {schema_version:1,journey:"refund",direction:$direction,
         before_first_effect:["finalize_agreement","prepare_exact_lez_claim",
           "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"],
         public_effect_order:
           (if $asset_mode == "custom_token" then
             ["lez_initialize_by_taker","lez_create_custody_ata_by_taker",
              "lez_fund_by_taker","bitcoin_lock_by_maker","dual_lock_gate",
              "bitcoin_refund_by_maker","lez_refund_by_taker"]
            else ["lez_initialize_by_taker","lez_fund_by_taker",
              "bitcoin_lock_by_maker","dual_lock_gate","bitcoin_refund_by_maker",
              "lez_refund_by_taker"] end),
         terminal:{maker_revision:4,taker_revision:4,phase:"refunded"}}
      '
      ;;
    first_lock_refund:taker_sells_foreign)
      jq -n --arg direction "$direction" '
        {schema_version:1,journey:"first_lock_refund",direction:$direction,
         before_first_effect:["finalize_agreement","prepare_exact_lez_claim",
           "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"],
         public_effect_order:["bitcoin_lock_by_taker","predecessor_one_pending",
           "signed_maker_second_lock_cutoff","fresh_lez_maker_lock_absence_twice",
           "fresh_bitcoin_first_lock_unspent_twice","bitcoin_refund_by_taker",
           "fresh_maker_observer"],
         expected_unique_effects:{bitcoin:2,lez:0},maker_second_lock_effect_count:0,
         actor_availability:{maker_offline_after_activation_until_refund_finality:true,
           taker_only_revision_one_and_refund_projection:true},
         terminal:{maker_revision:2,taker_revision:2,phase:"refunded"}}
      '
      ;;
    first_lock_refund:taker_sells_lez)
      jq -n --arg direction "$direction" '
        {schema_version:1,journey:"first_lock_refund",direction:$direction,
         before_first_effect:["finalize_agreement","prepare_exact_lez_claim",
           "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"],
         public_effect_order:["lez_initialize_by_taker","lez_fund_by_taker",
           "predecessor_one_pending","signed_maker_second_lock_cutoff",
           "fresh_bitcoin_maker_lock_absence_twice","fresh_lez_first_lock_unspent_twice",
           "lez_refund_by_taker","fresh_maker_observer"],
         expected_unique_effects:{bitcoin:0,lez:3},maker_second_lock_effect_count:0,
         actor_availability:{maker_offline_after_activation_until_refund_finality:true,
           taker_only_revision_one_and_refund_projection:true},
         terminal:{maker_revision:2,taker_revision:2,phase:"refunded"}}
      '
      ;;
    *) fail "unsupported effect-plan direction" ;;
  esac
}

preflight() {
  local command_name binary
  for command_name in awk chmod cmp cp curl date docker jq kill mkdir mv openssl perl printf \
    readlink rg sed sha256sum sleep sqlite3 stat timeout tr xxd; do
    command -v "$command_name" >/dev/null || fail "missing required tool: ${command_name}"
  done
  for binary in scripts/run-m3-actor-direction.sh target/debug/btc-local-poc-provision \
    target/debug/btc-reference-actor target/debug/lez-adaptor-role-runner \
    target/debug/examples/m3_witnessed_lez_operator \
    compat/lez-v0_2-sidecar/target/debug/lez-v02-bridge-poc \
    compat/lez-v0_2-sidecar/target/debug/lez-v02-native-escrow-poc; do
    [[ -x "$binary" && -f "$binary" && ! -L "$binary" ]] ||
      fail "required repository-owned runtime is unavailable: ${binary}"
  done
  target/debug/btc-local-poc-provision --help 2>&1 |
    rg -q 'prepare-funding' || fail "provisioner lacks the pre-lock funding command"
  if [[ "$asset_mode" == "custom_token" ]]; then
    [[ -x compat/lez-v0_2-sidecar/target/debug/examples/lez-v02-account-codec &&
       ! -L compat/lez-v0_2-sidecar/target/debug/examples/lez-v02-account-codec ]] ||
      fail "required repository-owned LEZ account codec is unavailable"
    target/debug/btc-local-poc-provision --help 2>&1 |
      rg -q 'finalize-asset-extension' ||
      fail "provisioner lacks the countersigned asset-extension command"
  elif [[ "$asset_mode" != "native" ]]; then
    fail "M3_POC_ASSET_MODE must be native or custom_token"
  fi
}

require_environment() {
  local variable value
  local -a required=(
    M3_POC_RUN_ID M3_POC_DIRECTION M3_POC_JOURNEY M3_POC_DIRECTION_ROOT M3_POC_SECURE_STATE_ROOT
    M3_POC_EVIDENCE_DIR
    M3_POC_PROCESS_REGISTRY M3_POC_ACTOR_BIN M3_POC_PROVISIONER_BIN
    M3_POC_ROLE_RUNNER_BIN M3_POC_LEZ_SIDECAR_BIN M3_POC_LEZ_OPERATOR_BIN
    M3_POC_LEZ_NATIVE_ESCROW_BIN M3_POC_BITCOIN_MANIFEST M3_POC_BITCOIN_RPC_URL
    M3_POC_BITCOIN_MAKER_CURL_CONFIG M3_POC_BITCOIN_TAKER_CURL_CONFIG
    M3_POC_BITCOIN_MAKER_BASIC M3_POC_BITCOIN_TAKER_BASIC
    M3_POC_BITCOIN_FUNDING_CREDENTIALS M3_POC_BITCOIN_FUNDING_SOURCES
    M3_POC_BITCOIN_PLANNED_ANCHOR_HEIGHT M3_POC_BITCOIN_CONTAINER_ID
    M3_POC_LEZ_MANIFEST M3_POC_LEZ_SEQUENCER_RPC_URL M3_POC_LEZ_INDEXER_RPC_URL
    M3_POC_LEZ_CHANNEL_ID M3_POC_LEZ_ESCROW_PROGRAM_ID
    M3_POC_LEZ_AUTH_TRANSFER_PROGRAM_ID M3_POC_LEZ_GENESIS_BLOCK_HASH
    M3_POC_MAKER_LEZ_IDENTITY M3_POC_MAKER_LEZ_PRIVATE_KEY
    M3_POC_TAKER_LEZ_IDENTITY M3_POC_TAKER_LEZ_PRIVATE_KEY
  )
  for variable in "${required[@]}"; do
    value="${!variable:-}"
    [[ -n "$value" ]] || fail "required environment is missing: ${variable}"
  done
  [[ "$M3_POC_DIRECTION" == "taker_sells_foreign" ||
     "$M3_POC_DIRECTION" == "taker_sells_lez" ]] || fail "unsupported direction"
  [[ "$M3_POC_JOURNEY" == "claim" || "$M3_POC_JOURNEY" == "survivor_claim" ||
     "$M3_POC_JOURNEY" == "refund" ||
     "$M3_POC_JOURNEY" == "first_lock_refund" ]] ||
    fail "unsupported actor journey"
  [[ "$asset_mode" == "native" || "$asset_mode" == "custom_token" ]] ||
    fail "M3_POC_ASSET_MODE must be native or custom_token"
  [[ "$asset_mode" != "custom_token" || "$M3_POC_JOURNEY" == "claim" ||
     "$M3_POC_JOURNEY" == "refund" ]] ||
    fail "custom_token currently requires the claim or refund journey"
  if [[ "$m5_btc_application_mode" == 1 ]]; then
    [[ "$asset_mode" == native && "$M3_POC_DIRECTION" == taker_sells_foreign &&
       "$M3_POC_JOURNEY" == claim ]] ||
      fail "M5 BTC application runtime requires native taker_sells_foreign claim"
    value="${M3_POC_SWAP_ID:-}"
    [[ -n "$value" ]] ||
      fail "required M5 BTC environment is missing: M3_POC_SWAP_ID"
    [[ "$value" =~ ^[0-9a-f]{64}$ ]] ||
      fail "M3_POC_SWAP_ID must be a canonical 32-byte lowercase hex value"
    for variable in M3_POC_M5_APPLICATION_ROOT M3_POC_M5_RUNTIME_ROOT M3_POC_MAKER_DAEMON_BIN \
      M3_POC_TAKER_CLI_BIN; do
      value="${!variable:-}"
      [[ -n "$value" ]] || fail "required M5 BTC environment is missing: ${variable}"
    done
  elif [[ -n "${M3_POC_SWAP_ID:-}" ]]; then
    fail "M3_POC_SWAP_ID is reserved for M5 BTC application mode"
  fi
  if [[ "$asset_mode" == "custom_token" ]]; then
    for variable in M3_POC_LEZ_ACCOUNT_CODEC_BIN M3_POC_F7_FIXTURE_ROOT \
      M3_POC_F7_FIXTURE_EVIDENCE M3_POC_F7_FIXTURE_PRIVATE_DIR \
      M3_POC_F7_WALLET_BIN M3_POC_F7_TOKEN_PROGRAM_ID M3_POC_F7_ATA_PROGRAM_ID; do
      value="${!variable:-}"
      [[ -n "$value" ]] || fail "required custom-token environment is missing: ${variable}"
    done
  fi
  for variable in M3_POC_DIRECTION_ROOT M3_POC_SECURE_STATE_ROOT M3_POC_EVIDENCE_DIR \
    M3_POC_PROCESS_REGISTRY \
    M3_POC_ACTOR_BIN M3_POC_PROVISIONER_BIN M3_POC_ROLE_RUNNER_BIN \
    M3_POC_LEZ_SIDECAR_BIN M3_POC_LEZ_OPERATOR_BIN M3_POC_LEZ_NATIVE_ESCROW_BIN \
    M3_POC_BITCOIN_MANIFEST M3_POC_BITCOIN_MAKER_CURL_CONFIG \
    M3_POC_BITCOIN_TAKER_CURL_CONFIG M3_POC_BITCOIN_MAKER_BASIC \
    M3_POC_BITCOIN_TAKER_BASIC M3_POC_BITCOIN_FUNDING_CREDENTIALS \
    M3_POC_BITCOIN_FUNDING_SOURCES \
    M3_POC_LEZ_MANIFEST M3_POC_MAKER_LEZ_IDENTITY M3_POC_MAKER_LEZ_PRIVATE_KEY \
    M3_POC_TAKER_LEZ_IDENTITY M3_POC_TAKER_LEZ_PRIVATE_KEY; do
    value="${!variable}"
    [[ "$value" == /* ]] || fail "path environment must be absolute: ${variable}"
  done
  if [[ "$m5_btc_application_mode" == 1 ]]; then
    for variable in M3_POC_M5_APPLICATION_ROOT M3_POC_M5_RUNTIME_ROOT M3_POC_MAKER_DAEMON_BIN \
      M3_POC_TAKER_CLI_BIN; do
      value="${!variable}"
      [[ "$value" == /* ]] || fail "M5 BTC path environment must be absolute: ${variable}"
    done
    [[ -x "$M3_POC_MAKER_DAEMON_BIN" && ! -L "$M3_POC_MAKER_DAEMON_BIN" &&
       -x "$M3_POC_TAKER_CLI_BIN" && ! -L "$M3_POC_TAKER_CLI_BIN" ]] ||
      fail "M5 BTC application binaries are unavailable or unsafe"
  fi
  if [[ "$asset_mode" == "custom_token" ]]; then
    for variable in M3_POC_LEZ_ACCOUNT_CODEC_BIN M3_POC_F7_FIXTURE_ROOT \
      M3_POC_F7_FIXTURE_EVIDENCE M3_POC_F7_FIXTURE_PRIVATE_DIR M3_POC_F7_WALLET_BIN; do
      value="${!variable}"
      [[ "$value" == /* ]] || fail "custom-token path environment must be absolute: ${variable}"
    done
    [[ -x "$M3_POC_LEZ_ACCOUNT_CODEC_BIN" && ! -L "$M3_POC_LEZ_ACCOUNT_CODEC_BIN" &&
       -x "$M3_POC_F7_WALLET_BIN" && ! -L "$M3_POC_F7_WALLET_BIN" &&
       -f "$M3_POC_F7_FIXTURE_EVIDENCE" && ! -L "$M3_POC_F7_FIXTURE_EVIDENCE" &&
       -d "$M3_POC_F7_FIXTURE_PRIVATE_DIR" && ! -L "$M3_POC_F7_FIXTURE_PRIVATE_DIR" ]] ||
      fail "custom-token fixture or runtime material is unavailable"
    [[ "$M3_POC_F7_TOKEN_PROGRAM_ID" =~ ^[0-9a-f]{64}$ &&
       "$M3_POC_F7_ATA_PROGRAM_ID" =~ ^[0-9a-f]{64}$ &&
       "$M3_POC_F7_TOKEN_PROGRAM_ID" != "$M3_POC_F7_ATA_PROGRAM_ID" ]] ||
      fail "custom-token program IDs are invalid or aliased"
  fi
  [[ "$M3_POC_BITCOIN_PLANNED_ANCHOR_HEIGHT" =~ ^[1-9][0-9]*$ ]] ||
    fail "planned Bitcoin funding anchor must be a positive height"
  [[ "$M3_POC_SECURE_STATE_ROOT" == \
     "/tmp/lez-atomic-swaps-m3-${M3_POC_RUN_ID}-secure-state/directions/${M3_POC_DIRECTION}" ]] ||
    fail "secure state root is not the exact run-owned direction root"
  for endpoint in "$M3_POC_BITCOIN_RPC_URL" "$M3_POC_LEZ_SEQUENCER_RPC_URL" \
    "$M3_POC_LEZ_INDEXER_RPC_URL"; do
    [[ "$endpoint" =~ ^http://127\.0\.0\.1:[1-9][0-9]{0,4}/?$ ]] ||
      fail "node endpoints must be explicit literal-loopback HTTP"
  done
}

parse_direction_proc_uptime_ms() {
  local raw="$1" output_name="$2"
  local seconds fraction fraction_ms seconds_number milliseconds
  [[ "$output_name" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || return 1
  [[ "$raw" =~ ^(0|[1-9][0-9]*)\.([0-9]+)$ ]] || return 1
  seconds="${BASH_REMATCH[1]}"
  fraction="${BASH_REMATCH[2]}"
  if (( ${#seconds} > 13 )); then
    return 1
  fi
  if (( ${#seconds} == 13 && 10#$seconds > 9007199254740 )); then
    return 1
  fi
  fraction_ms="${fraction}000"
  fraction_ms="${fraction_ms:0:3}"
  seconds_number=$((10#$seconds))
  milliseconds=$((10#$fraction_ms))
  if (( seconds_number == 9007199254740 && milliseconds > 991 )); then
    return 1
  fi
  printf -v "$output_name" '%d' "$((seconds_number * 1000 + milliseconds))"
}

read_direction_monotonic_ms() {
  local uptime_value _ignored
  IFS=' ' read -r uptime_value _ignored </proc/uptime || return 1
  parse_direction_proc_uptime_ms "$uptime_value" direction_timing_now_ms
}

expected_direction_phase_timings_json() {
  [[ "$M3_POC_DIRECTION" == "taker_sells_foreign" ||
     "$M3_POC_DIRECTION" == "taker_sells_lez" ]] || return 1
  [[ "$asset_mode" == "native" || "$asset_mode" == "custom_token" ]] || return 1
  [[ "$direction_timing_execution_mode" == "sequential" ||
     "$direction_timing_execution_mode" == "overlap" ]] || return 1
  if [[ "$direction_timing_execution_mode" == "overlap" ]]; then
    [[ "$M3_POC_JOURNEY" == "claim" ]] || return 1
    jq -cn '[
      {phase_id:"final_transcript"},
      {phase_id:"presign_and_activate"},
      {phase_id:"overlap_ready_barrier"},
      {phase_id:"first_lock_to_revision_one"},
      {phase_id:"second_lock_to_revision_two"},
      {phase_id:"dual_lock_gate"},
      {phase_id:"overlap_locked_barrier"},
      {phase_id:"revealing_claim_to_revision_three"},
      {phase_id:"followup_claim_to_revision_four"},
      {phase_id:"terminal_evidence"},
      {phase_id:"overlap_terminal_marker"}
    ]'
    return
  fi
  case "$M3_POC_JOURNEY" in
    claim)
      jq -cn '[
        {phase_id:"final_transcript"},
        {phase_id:"presign_and_activate"},
        {phase_id:"first_lock_to_revision_one"},
        {phase_id:"second_lock_to_revision_two"},
        {phase_id:"dual_lock_gate"},
        {phase_id:"revealing_claim_to_revision_three"},
        {phase_id:"followup_claim_to_revision_four"},
        {phase_id:"terminal_evidence"}
      ]'
      ;;
    survivor_claim)
      jq -cn '[
        {phase_id:"final_transcript"},
        {phase_id:"presign_and_activate"},
        {phase_id:"first_lock_to_revision_one"},
        {phase_id:"second_lock_to_revision_two"},
        {phase_id:"dual_lock_gate"},
        {phase_id:"survivor_settlement_to_revision_four"},
        {phase_id:"terminal_evidence"}
      ]'
      ;;
    refund)
      jq -cn '[
        {phase_id:"final_transcript"},
        {phase_id:"presign_and_activate"},
        {phase_id:"first_lock_to_revision_one"},
        {phase_id:"second_lock_to_revision_two"},
        {phase_id:"dual_lock_gate"},
        {phase_id:"refund_settlement_to_revision_four"},
        {phase_id:"terminal_evidence"}
      ]'
      ;;
    first_lock_refund)
      jq -cn '[
        {phase_id:"final_transcript"},
        {phase_id:"presign_and_activate"},
        {phase_id:"first_lock_refund_to_revision_two"},
        {phase_id:"terminal_evidence"}
      ]'
      ;;
    *) return 1 ;;
  esac
}

initialize_direction_phase_timings() {
  local expected
  direction_timing_dir="${M3_POC_DIRECTION_ROOT}/timings"
  direction_timing_journal="${direction_timing_dir}/actor.ndjson.partial"
  direction_timing_evidence="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actor-phase-timings.json"
  expected="$(expected_direction_phase_timings_json)" || return 1
  jq -e 'length > 0' <<<"$expected" >/dev/null || return 1
  [[ -d "$M3_POC_DIRECTION_ROOT" && ! -L "$M3_POC_DIRECTION_ROOT" &&
     -d "$M3_POC_EVIDENCE_DIR" && ! -L "$M3_POC_EVIDENCE_DIR" ]] || return 1
  [[ ! -e "$direction_timing_dir" && ! -L "$direction_timing_dir" &&
     ! -e "$direction_timing_evidence" && ! -L "$direction_timing_evidence" &&
     ! -e "${direction_timing_evidence}.partial" &&
     ! -L "${direction_timing_evidence}.partial" ]] || return 1
  mkdir -m 0700 "$direction_timing_dir" || return 1
  [[ -d "$direction_timing_dir" && ! -L "$direction_timing_dir" &&
     "$(stat -c '%u:%a' "$direction_timing_dir")" == "$(id -u):700" ]] || return 1
  : >"$direction_timing_journal" || return 1
  chmod 0600 "$direction_timing_journal" || return 1
  [[ -f "$direction_timing_journal" && ! -L "$direction_timing_journal" &&
     "$(stat -c '%u:%a' "$direction_timing_journal")" == "$(id -u):600" ]] || return 1
  read_direction_monotonic_ms || return 1
  direction_timing_origin_ms="$direction_timing_now_ms"
  direction_timing_started_at_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)" || return 1
  direction_timing_active_phase=""
  direction_timing_active_start_ms=0
  direction_timing_sequence=0
}

direction_phase_begin() {
  local phase_id="$1" expected expected_phase
  [[ -z "$direction_timing_active_phase" ]] || return 1
  [[ -f "$direction_timing_journal" && ! -L "$direction_timing_journal" &&
     "$(stat -c '%u:%a' "$direction_timing_journal")" == "$(id -u):600" ]] || return 1
  expected="$(expected_direction_phase_timings_json)" || return 1
  expected_phase="$(jq -er --argjson index "$direction_timing_sequence" \
    '.[$index].phase_id // empty' <<<"$expected")" || return 1
  [[ "$phase_id" == "$expected_phase" ]] || return 1
  read_direction_monotonic_ms || return 1
  (( direction_timing_now_ms >= direction_timing_origin_ms )) || return 1
  direction_timing_active_phase="$phase_id"
  direction_timing_active_start_ms=$((direction_timing_now_ms - direction_timing_origin_ms))
}

direction_phase_end() {
  local phase_id="$1" end_offset duration next_sequence
  [[ -n "$direction_timing_active_phase" &&
     "$phase_id" == "$direction_timing_active_phase" ]] || return 1
  [[ -f "$direction_timing_journal" && ! -L "$direction_timing_journal" &&
     "$(stat -c '%u:%a' "$direction_timing_journal")" == "$(id -u):600" ]] || return 1
  read_direction_monotonic_ms || return 1
  (( direction_timing_now_ms >= direction_timing_origin_ms )) || return 1
  end_offset=$((direction_timing_now_ms - direction_timing_origin_ms))
  (( end_offset >= direction_timing_active_start_ms )) || return 1
  duration=$((end_offset - direction_timing_active_start_ms))
  next_sequence=$((direction_timing_sequence + 1))
  printf '{"schema_version":1,"sequence":%d,"producer":"direction_actor","phase_id":"%s","start_offset_ms":%d,"end_offset_ms":%d,"duration_ms":%d,"outcome":"passed"}\n' \
    "$next_sequence" "$direction_timing_active_phase" \
    "$direction_timing_active_start_ms" "$end_offset" "$duration" \
    >>"$direction_timing_journal" || return 1
  direction_timing_sequence="$next_sequence"
  direction_timing_active_phase=""
  direction_timing_active_start_ms=0
}

finalize_direction_phase_timings() {
  local expected effects effects_sha completed_at total_duration partial
  [[ -z "$direction_timing_active_phase" ]] || return 1
  [[ -f "$direction_timing_journal" && ! -L "$direction_timing_journal" &&
     "$(stat -c '%u:%a' "$direction_timing_journal")" == "$(id -u):600" ]] || return 1
  jq -s -e '
    type == "array" and length > 0
    and ([.[] | (keys | sort) == (["schema_version","sequence","producer",
      "phase_id","start_offset_ms","end_offset_ms","duration_ms","outcome"]
      | sort)] | all)
  ' "$direction_timing_journal" >/dev/null || return 1
  effects="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actual-effects.json"
  [[ -f "$effects" && ! -L "$effects" &&
     "$(stat -c '%u:%a' "$effects")" == "$(id -u):600" ]] || return 1
  jq -e --arg direction "$M3_POC_DIRECTION" '
    .schema_version == 1 and .direction == $direction
    and (.bitcoin_effect_ids | type) == "array"
    and (.lez_effect_ids | type) == "array"
    and (.expected_unique_effects | type) == "object"
  ' "$effects" >/dev/null || return 1
  effects_sha="$(sha256sum "$effects")" || return 1
  effects_sha="${effects_sha%% *}"
  [[ "$effects_sha" =~ ^[0-9a-f]{64}$ ]] || return 1
  expected="$(expected_direction_phase_timings_json)" || return 1
  read_direction_monotonic_ms || return 1
  (( direction_timing_now_ms >= direction_timing_origin_ms )) || return 1
  total_duration=$((direction_timing_now_ms - direction_timing_origin_ms))
  completed_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)" || return 1
  partial="${direction_timing_evidence}.partial"
  [[ ! -e "$direction_timing_evidence" && ! -L "$direction_timing_evidence" &&
     ! -e "$partial" && ! -L "$partial" ]] || return 1
  jq -s --argjson expected "$expected" \
    --arg run_id "$M3_POC_RUN_ID" --arg direction "$M3_POC_DIRECTION" \
    --arg journey "$M3_POC_JOURNEY" --arg asset_mode "$asset_mode" \
    --arg execution_mode "$direction_timing_execution_mode" \
    --arg effects_sha "$effects_sha" \
    --arg started_at "$direction_timing_started_at_utc" \
    --arg completed_at "$completed_at" --argjson total_duration "$total_duration" '
    def exact_keys($keys): (keys | sort) == ($keys | sort);
    . as $records
    | ($expected | length) as $count
    | if
        ($records | length) == $count
        and ([$records[] |
          exact_keys(["schema_version","sequence","producer","phase_id",
            "start_offset_ms","end_offset_ms","duration_ms","outcome"])
          and .schema_version == 1 and .producer == "direction_actor"
          and .outcome == "passed"
          and (.sequence | type) == "number" and .sequence == (.sequence | floor)
          and (.phase_id | type) == "string"
          and (.start_offset_ms | type) == "number"
          and .start_offset_ms == (.start_offset_ms | floor)
          and (.end_offset_ms | type) == "number"
          and .end_offset_ms == (.end_offset_ms | floor)
          and (.duration_ms | type) == "number"
          and .duration_ms == (.duration_ms | floor)
          and .start_offset_ms >= 0 and .end_offset_ms >= .start_offset_ms
          and .duration_ms == (.end_offset_ms - .start_offset_ms)
          and .end_offset_ms <= $total_duration] | all)
        and [$records[].sequence] == [range(1; $count + 1)]
        and [$records[].phase_id] == [$expected[].phase_id]
        and ([range(1; $count) as $index
          | $records[$index].start_offset_ms >=
            $records[$index - 1].end_offset_ms] | all)
      then
        ([$records[].duration_ms] | add // 0) as $measured
        | {
            schema_version:1,
            kind:"m3_actor_direction_phase_timings",
            result:"actor_flow_passed",
            run_id:$run_id,
            direction:$direction,
            journey:$journey,
            asset_mode:$asset_mode,
            execution_mode:$execution_mode,
            actual_effects_sha256:$effects_sha,
            coverage:{
              starts_before_final_transcript:true,
              ends_after_actual_effect_manifest:true,
              excludes_outer_stage_two_replay_and_balances:true
            },
            clock:{
              source:"linux_proc_uptime",
              unit:"milliseconds",
              resolution_ms:10,
              includes_suspend:true,
              wall_clock_used_for_duration:false
            },
            started_at_utc:$started_at,
            completed_at_utc:$completed_at,
            total_duration_ms:$total_duration,
            unattributed_duration_ms:($total_duration - $measured),
            phases:$records,
            private_material_disclosed:false
          }
      else error("invalid direction timing journal") end
  ' "$direction_timing_journal" >"$partial" || {
    rm -f -- "$partial"
    return 1
  }
  chmod 0600 "$partial" || {
    rm -f -- "$partial"
    return 1
  }
  [[ -f "$partial" && ! -L "$partial" &&
     "$(stat -c '%u:%a' "$partial")" == "$(id -u):600" ]] || {
    rm -f -- "$partial"
    return 1
  }
  jq -e --arg direction "$M3_POC_DIRECTION" --arg effects_sha "$effects_sha" '
    (keys | sort) == (["schema_version","kind","result","run_id","direction",
      "journey","asset_mode","execution_mode","actual_effects_sha256","coverage",
      "clock","started_at_utc","completed_at_utc","total_duration_ms",
      "unattributed_duration_ms","phases","private_material_disclosed"] | sort)
    and .schema_version == 1 and .kind == "m3_actor_direction_phase_timings"
    and .result == "actor_flow_passed" and .direction == $direction
    and .actual_effects_sha256 == $effects_sha
    and (.phases | type) == "array"
    and .unattributed_duration_ms >= 0
    and .private_material_disclosed == false
    and (try (.started_at_utc | fromdateiso8601 | type == "number") catch false)
    and (try (.completed_at_utc | fromdateiso8601 | type == "number") catch false)
  ' "$partial" >/dev/null || {
    rm -f -- "$partial"
    return 1
  }
  [[ "$(sha256sum "$effects" | sed 's/ .*//')" == "$effects_sha" ]] || {
    rm -f -- "$partial"
    return 1
  }
  mv -n -- "$partial" "$direction_timing_evidence" || {
    rm -f -- "$partial"
    return 1
  }
  [[ ! -e "$partial" && ! -L "$partial" &&
     -f "$direction_timing_evidence" && ! -L "$direction_timing_evidence" &&
     "$(stat -c '%u:%a' "$direction_timing_evidence")" == "$(id -u):600" ]]
}

file_value() {
  local file="$1"
  local key="$2"
  local -a values=()
  mapfile -t values < <(sed -n "s/^${key}=//p" "$file")
  [[ "${#values[@]}" == 1 && -n "${values[0]}" ]] ||
    fail "${file} does not contain exactly one ${key}"
  printf '%s\n' "${values[0]}"
}

rpc() {
  local endpoint="$1"
  local request="$2"
  curl --fail --silent --show-error --noproxy '*' --connect-timeout 2 --max-time 30 \
    -H 'content-type: application/json' --data "$request" "$endpoint"
}

core_rpc() {
  local role="$1"
  local method="$2"
  local params="$3"
  local config
  local -a config_urls=()
  case "$role" in
    maker) config="$M3_POC_BITCOIN_MAKER_CURL_CONFIG" ;;
    taker) config="$M3_POC_BITCOIN_TAKER_CURL_CONFIG" ;;
    *) fail "invalid Core role: ${role}" ;;
  esac
  mapfile -t config_urls < <(sed -n -E \
    's/^url[[:space:]]*=[[:space:]]*"([^"]+)"[[:space:]]*$/\1/p' "$config")
  [[ "${#config_urls[@]}" == 1 && "${config_urls[0]}" == "$M3_POC_BITCOIN_RPC_URL" ]] ||
    fail "Core role curl config does not own the exact run endpoint"
  curl --fail --silent --show-error --noproxy '*' --config "$config" \
    --connect-timeout 2 --max-time 30 -H 'content-type: application/json' \
    --data "$(jq -cn --arg method "$method" --argjson params "$params" \
      '{jsonrpc:"2.0",id:1,method:$method,params:$params}')"
}

allocate_port() {
  perl -MIO::Socket::INET -e '
    $socket = IO::Socket::INET->new(LocalAddr => "127.0.0.1", LocalPort => 0,
      Proto => "tcp", Listen => 1) or die "loopback port allocation failed: $!\n";
    print $socket->sockport, "\n";
  '
}

register_process() {
  local role="$1"
  local phase="$2"
  local pid="$3"
  local start executable
  start="$(awk '{print $22}' "/proc/${pid}/stat")"
  executable="$(readlink -f "/proc/${pid}/exe")"
  [[ "$executable" == "$M3_POC_LEZ_SIDECAR_BIN" ]] || fail "sidecar executable drift"
  jq -nc --arg role "$role" --arg phase "$phase" --argjson pid "$pid" \
    --arg start "$start" --arg executable "$executable" \
    '{role:$role,phase:$phase,pid:$pid,start_ticks:$start,executable:$executable}' \
    >>"$M3_POC_PROCESS_REGISTRY"
  printf '%s\t%s\t%s\t%s\n' "$pid" "$start" "$executable" "$role" \
    >>"${M3_POC_DIRECTION_ROOT}/${phase}-sidecars.tsv"
}

m5_application_pid=""
m5_application_start=""
m5_application_executable=""
m5_application_pgid=""
m5_application_sid=""

register_m5_application_process() {
  local phase="$1" pid="$2" expected_executable="$3"
  local fields state ppid pgid sid start executable
  [[ "$phase" == chat && "$pid" =~ ^[1-9][0-9]*$ &&
     "$expected_executable" == /* ]] || return 1
  for _ in {1..200}; do
    [[ -r "/proc/${pid}/stat" ]] || return 1
    fields="$(awk '{print $3, $4, $5, $6, $22}' "/proc/${pid}/stat" 2>/dev/null)" ||
      return 1
    read -r state ppid pgid sid start <<<"$fields"
    executable="$(readlink -f "/proc/${pid}/exe" 2>/dev/null || true)"
    if [[ "$state" != Z && "$ppid" == "$$" && "$pgid" == "$pid" &&
          "$sid" == "$pid" && "$executable" == "$expected_executable" ]]; then
      break
    fi
    sleep 0.01
  done
  [[ "$state" != Z && "$ppid" == "$$" && "$pgid" == "$pid" &&
     "$sid" == "$pid" && -n "$start" && "$executable" == "$expected_executable" ]] ||
    return 1
  jq -nc --arg role maker --arg phase m5-btc-chat --argjson pid "$pid" \
    --arg start "$start" --arg executable "$executable" --argjson ppid "$ppid" \
    --argjson pgid "$pgid" --argjson sid "$sid" \
    '{role:$role,phase:$phase,pid:$pid,start_ticks:$start,executable:$executable,
      ppid:$ppid,pgid:$pgid,sid:$sid,group_owned:true,reap_child:false}' \
    >>"$M3_POC_PROCESS_REGISTRY" || return 1
  m5_application_pid="$pid"
  m5_application_start="$start"
  m5_application_executable="$executable"
  m5_application_pgid="$pgid"
  m5_application_sid="$sid"
}

stop_m5_application_process() {
  local state actual_start actual_executable actual_pgid actual_sid status
  [[ "$m5_application_pid" =~ ^[1-9][0-9]*$ &&
     "$m5_application_pgid" == "$m5_application_pid" &&
     "$m5_application_sid" == "$m5_application_pid" ]] || return 1
  if [[ -r "/proc/${m5_application_pid}/stat" ]]; then
    actual_start="$(awk '{print $22}' "/proc/${m5_application_pid}/stat" 2>/dev/null)"
    actual_pgid="$(awk '{print $5}' "/proc/${m5_application_pid}/stat" 2>/dev/null)"
    actual_sid="$(awk '{print $6}' "/proc/${m5_application_pid}/stat" 2>/dev/null)"
    actual_executable="$(readlink -f "/proc/${m5_application_pid}/exe" 2>/dev/null || true)"
    [[ "$actual_start" == "$m5_application_start" &&
       "$actual_executable" == "$m5_application_executable" &&
       "$actual_pgid" == "$m5_application_pgid" &&
       "$actual_sid" == "$m5_application_sid" ]] || return 1
    kill -TERM -- "-${m5_application_pgid}" 2>/dev/null || return 1
    for _ in {1..100}; do
      state="$(awk '{print $3}' "/proc/${m5_application_pid}/stat" 2>/dev/null || true)"
      [[ -z "$state" || "$state" == Z ]] && break
      sleep 0.05
    done
    state="$(awk '{print $3}' "/proc/${m5_application_pid}/stat" 2>/dev/null || true)"
    if [[ -n "$state" && "$state" != Z ]]; then
      actual_start="$(awk '{print $22}' "/proc/${m5_application_pid}/stat" 2>/dev/null)"
      actual_pgid="$(awk '{print $5}' "/proc/${m5_application_pid}/stat" 2>/dev/null)"
      actual_sid="$(awk '{print $6}' "/proc/${m5_application_pid}/stat" 2>/dev/null)"
      actual_executable="$(readlink -f "/proc/${m5_application_pid}/exe" 2>/dev/null || true)"
      [[ "$actual_start" == "$m5_application_start" &&
         "$actual_executable" == "$m5_application_executable" &&
         "$actual_pgid" == "$m5_application_pgid" &&
         "$actual_sid" == "$m5_application_sid" ]] || return 1
      kill -KILL -- "-${m5_application_pgid}" 2>/dev/null || return 1
    fi
  fi
  if wait "$m5_application_pid" 2>/dev/null; then status=0; else status=$?; fi
  [[ "$status" != 127 ]] || return 1
  m5_application_pid=""
  m5_application_start=""
  m5_application_executable=""
  m5_application_pgid=""
  m5_application_sid=""
}

write_runtime() {
  local role="$1"
  local phase="$2"
  local signer
  signer="$(jq -er '.account_id_hex' "${M3_POC_DIRECTION_ROOT}/actors/${role}/identity.json")"
  jq -n --arg role "$role" --arg chain "$M3_POC_LEZ_CHANNEL_ID" \
    --arg genesis "$M3_POC_LEZ_GENESIS_BLOCK_HASH" \
    --arg program "$M3_POC_LEZ_ESCROW_PROGRAM_ID" --arg signer "$signer" '
    {sidecar_role:$role,compatibility:"lee_v0_2_0",chain_id:$chain,channel_id:$chain,
     genesis_block_hash:$genesis,escrow_program_id:$program,signer_account_id:$signer}' \
    >"${M3_POC_DIRECTION_ROOT}/sidecars/${phase}/${role}/runtime.json"
  chmod 0600 "${M3_POC_DIRECTION_ROOT}/sidecars/${phase}/${role}/runtime.json"
}

start_sidecars() {
  local phase="$1"
  local role port role_root state_root log pid endpoints_file maker_port taker_port
  endpoints_file="${M3_POC_DIRECTION_ROOT}/${phase}-endpoints.env"
  [[ ! -e "$endpoints_file" ]] || fail "refusing to reuse ${phase} sidecar endpoints"
  : >"${M3_POC_DIRECTION_ROOT}/${phase}-sidecars.tsv"
  chmod 0600 "${M3_POC_DIRECTION_ROOT}/${phase}-sidecars.tsv"
  maker_port="$(allocate_port)"
  taker_port="$(allocate_port)"
  while [[ "$taker_port" == "$maker_port" ]]; do taker_port="$(allocate_port)"; done
  for role in maker taker; do
    case "$role" in maker) port="$maker_port" ;; taker) port="$taker_port" ;; esac
    role_root="${M3_POC_DIRECTION_ROOT}/sidecars/${phase}/${role}"
    state_root="${M3_POC_SECURE_STATE_ROOT}/sidecars/${phase}/${role}"
    [[ ! -e "$state_root" && ! -L "$state_root" ]] ||
      fail "refusing to reuse ${phase} ${role} secure sidecar state"
    mkdir -p "$role_root" "$state_root"
    chmod 0700 "$role_root" "$M3_POC_SECURE_STATE_ROOT/sidecars" \
      "$M3_POC_SECURE_STATE_ROOT/sidecars/$phase" "$state_root"
    openssl rand -hex 32 >"$role_root/capability"
    chmod 0600 "$role_root/capability"
    write_runtime "$role" "$phase"
    log="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${phase}-${role}-sidecar.log"
    "$M3_POC_LEZ_SIDECAR_BIN" --listen-address "127.0.0.1:${port}" \
      --node-profile local --sequencer-url "$M3_POC_LEZ_SEQUENCER_RPC_URL" \
      --indexer-url "$M3_POC_LEZ_INDEXER_RPC_URL" --run-id "$M3_POC_RUN_ID" \
      --runtime-file "$role_root/runtime.json" --capability-file "$role_root/capability" \
      --private-key-file "${M3_POC_DIRECTION_ROOT}/actors/${role}/lez-signer.key" \
      --state-directory "$state_root" \
      --authenticated-transfer-program-id "$M3_POC_LEZ_AUTH_TRANSFER_PROGRAM_ID" \
      >"$log" 2>&1 &
    pid=$!
    register_process "$role" "$phase" "$pid"
    for _ in {1..200}; do
      kill -0 "$pid" 2>/dev/null || fail "${phase} ${role} sidecar exited before readiness"
      if jq -e --arg role "$role" --arg run "$M3_POC_RUN_ID" \
        '.event == "ready" and .runtime.sidecar_role == $role and .run_id == $run' \
        "$log" >/dev/null 2>&1; then break; fi
      sleep 0.05
    done
    jq -e '.event == "ready"' "$log" >/dev/null || fail "${phase} ${role} sidecar not ready"
    chmod 0600 "$log"
  done
  {
    printf 'maker=http://127.0.0.1:%s/\n' "$maker_port"
    printf 'taker=http://127.0.0.1:%s/\n' "$taker_port"
  } >"$endpoints_file"
  chmod 0600 "$endpoints_file"
}

stop_sidecars() {
  local phase="$1"
  local pid start executable role actual_start actual_executable
  while IFS=$'\t' read -r pid start executable role; do
    [[ -r "/proc/${pid}/stat" ]] || continue
    actual_start="$(awk '{print $22}' "/proc/${pid}/stat" 2>/dev/null || true)"
    actual_executable="$(readlink -f "/proc/${pid}/exe" 2>/dev/null || true)"
    if [[ "$actual_start" == "$start" && "$actual_executable" == "$executable" ]]; then
      kill -TERM "$pid" 2>/dev/null || true
      for _ in {1..100}; do
        [[ ! -r "/proc/${pid}/stat" ]] && break
        sleep 0.05
      done
    fi
  done <"${M3_POC_DIRECTION_ROOT}/${phase}-sidecars.tsv"
}

operator_call() {
  local phase="$1" role="$2" command="$3" request="$4" output="$5"
  local role_root endpoint
  role_root="${M3_POC_DIRECTION_ROOT}/sidecars/${phase}/${role}"
  endpoint="$(file_value "${M3_POC_DIRECTION_ROOT}/${phase}-endpoints.env" "$role")"
  if ! "$M3_POC_LEZ_OPERATOR_BIN" "$command" --endpoint "$endpoint" \
      --run-id "$M3_POC_RUN_ID" --sidecar-role "$role" \
      --capability-file "$role_root/capability" --runtime-file "$role_root/runtime.json" \
      --request-file "$request" >"$output"; then
    chmod 0600 "$output"
    return 1
  fi
  chmod 0600 "$output"
}

new_request_id() { openssl rand -hex 16; }

prepare_witnessed_pair() {
  local phase="$1" terms="$2" depositor="$3" claimant="$4" prefix="$5"
  local depositor_root claimant_root escrow_request escrow_result claim_request claim_result funding_tx
  depositor_root="${M3_POC_DIRECTION_ROOT}/sidecars/${phase}/${depositor}"
  claimant_root="${M3_POC_DIRECTION_ROOT}/sidecars/${phase}/${claimant}"
  escrow_request="${M3_POC_DIRECTION_ROOT}/${prefix}-prepare-escrow-request.json"
  escrow_result="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${prefix}-prepared-escrow.json"
  claim_request="${M3_POC_DIRECTION_ROOT}/${prefix}-prepare-claim-request.json"
  claim_result="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${prefix}-prepared-claim.json"
  jq -n --arg run "$M3_POC_RUN_ID" --arg request "$(new_request_id)" \
    --arg role "$depositor" --slurpfile runtime "$depositor_root/runtime.json" \
    --slurpfile terms "$terms" '
    {context:{schema_version:1,run_id:$run,request_id:$request,sidecar_role:$role},
     runtime:$runtime[0],terms:$terms[0]}' >"$escrow_request"
  chmod 0600 "$escrow_request"
  operator_call "$phase" "$depositor" prepare-witnessed-escrow "$escrow_request" "$escrow_result"
  funding_tx="$(jq -er '.funding.transaction_id' "$escrow_result")"
  jq -n --arg run "$M3_POC_RUN_ID" --arg request "$(new_request_id)" \
    --arg role "$claimant" --arg funding "$funding_tx" \
    --slurpfile runtime "$claimant_root/runtime.json" --slurpfile terms "$terms" '
    {context:{schema_version:1,run_id:$run,request_id:$request,sidecar_role:$role},
     runtime:$runtime[0],terms:$terms[0],funding_transaction_id:$funding}' >"$claim_request"
  chmod 0600 "$claim_request"
  operator_call "$phase" "$claimant" prepare-witnessed-claim "$claim_request" "$claim_result"
  jq -e '.claim.message_hash | test("^[0-9a-f]{64}$")' "$claim_result" >/dev/null ||
    fail "prepared witnessed claim is invalid"
}

prepare_witnessed_asset_pair() {
  local phase="$1" terms="$2" depositor="$3" claimant="$4" prefix="$5"
  local depositor_root claimant_root escrow_request escrow_result claim_request claim_result
  local funding_tx
  depositor_root="${M3_POC_DIRECTION_ROOT}/sidecars/${phase}/${depositor}"
  claimant_root="${M3_POC_DIRECTION_ROOT}/sidecars/${phase}/${claimant}"
  escrow_request="${M3_POC_DIRECTION_ROOT}/${prefix}-prepare-asset-escrow-request.json"
  escrow_result="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${prefix}-prepared-asset-escrow.json"
  claim_request="${M3_POC_DIRECTION_ROOT}/${prefix}-prepare-asset-claim-request.json"
  claim_result="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${prefix}-prepared-asset-claim.json"
  jq -n --arg run "$M3_POC_RUN_ID" --arg request "$(new_request_id)" \
    --arg role "$depositor" --slurpfile runtime "$depositor_root/runtime.json" \
    --slurpfile terms "$terms" '
    {context:{schema_version:1,run_id:$run,request_id:$request,sidecar_role:$role},
     runtime:$runtime[0],terms:$terms[0]}' >"$escrow_request"
  chmod 0600 "$escrow_request"
  operator_call "$phase" "$depositor" prepare-witnessed-asset-escrow-v2 \
    "$escrow_request" "$escrow_result"
  funding_tx="$(jq -er '
    [.effects[] | select(.step == "fund") | .transaction.transaction_id] |
    if length == 1 then .[0] else error("exactly one custom-token funding effect required") end
  ' "$escrow_result")"
  jq -n --arg run "$M3_POC_RUN_ID" --arg request "$(new_request_id)" \
    --arg role "$claimant" --arg funding "$funding_tx" \
    --slurpfile runtime "$claimant_root/runtime.json" --slurpfile terms "$terms" '
    {context:{schema_version:1,run_id:$run,request_id:$request,sidecar_role:$role},
     runtime:$runtime[0],terms:$terms[0],funding_transaction_id:$funding}' >"$claim_request"
  chmod 0600 "$claim_request"
  operator_call "$phase" "$claimant" prepare-witnessed-asset-claim-v2 \
    "$claim_request" "$claim_result"
  jq -e '
    .terms.asset_terms_version == 2
    and .terms.asset.kind == "custom_token"
    and (.claim.message_hash | test("^[0-9a-f]{64}$"))
    # ExactMessageBytes uses Serde Base64 on the public JSON wire. The sidecar
    # has already decoded and re-encoded these bytes canonically before it
    # returns the result; this boundary check rejects empty or malformed wire
    # values without incorrectly treating the representation as hex.
    and ((.claim.exact_message_bytes | type) == "string")
    and (.claim.exact_message_bytes |
      length > 0
      and test("^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$"))
  ' "$claim_result" >/dev/null || fail "prepared witnessed asset claim is invalid"
}

account_codec() {
  local output
  output="$("$M3_POC_LEZ_ACCOUNT_CODEC_BIN" "$@")" ||
    fail "official LEZ account codec rejected an F7 account"
  jq -e '
    .schema == "lez-v0.2-account-id-codec" and .version == 1
    and (.account_id_base58 | test("^[1-9A-HJ-NP-Za-km-z]{43,44}$"))
    and (.account_id_hex | test("^[0-9a-f]{64}$"))
  ' <<<"$output" >/dev/null || fail "official LEZ account codec returned invalid output"
  printf '%s\n' "$output"
}

write_custom_token_terms() {
  local terms_hash="$1" metadata_hex="$2" swap_id="$3" refund_at_ms="$4"
  local aggregate_account="$5" aggregate_key="$6" output="$7"
  local asset_name depositor claimant definition_base58 depositor_ata_base58 claimant_ata_base58
  local definition_hex depositor_ata_hex claimant_ata_hex metadata_base58 custody_base58 custody_hex
  local depositor_owner claimant_owner wallet_home wallet_output wallet_log
  local -a custody_values=()
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign) asset_name=M3F7A; depositor=maker; claimant=taker ;;
    taker_sells_lez) asset_name=M3F7B; depositor=taker; claimant=maker ;;
  esac
  jq -e --arg run "$M3_POC_RUN_ID" --arg asset "$asset_name" \
    --arg depositor "$depositor" '
    .schema_version == 1 and .kind == "m3_f7_official_token_fixture"
    and .result == "passed" and .run_id == $run
    and .assets[$asset].depositor == $depositor
    and .assets[$asset].initial_balances[$depositor] == 250
    and .external_resources.public_rpc == false and .external_resources.faucet == false
    and .finality.swap_certification_requires_finalized_indexer_evidence == true
  ' "$M3_POC_F7_FIXTURE_EVIDENCE" >/dev/null || fail "F7 fixture evidence is incompatible"
  definition_base58="$(jq -er --arg asset "$asset_name" '.assets[$asset].definition' \
    "$M3_POC_F7_FIXTURE_EVIDENCE")"
  depositor_ata_base58="$(jq -er --arg asset "$asset_name" --arg role "$depositor" \
    '.assets[$asset].atas[$role]' "$M3_POC_F7_FIXTURE_EVIDENCE")"
  claimant_ata_base58="$(jq -er --arg asset "$asset_name" --arg role "$claimant" \
    '.assets[$asset].atas[$role]' "$M3_POC_F7_FIXTURE_EVIDENCE")"
  definition_hex="$(account_codec "$definition_base58" | jq -er '.account_id_hex')"
  depositor_ata_hex="$(account_codec "$depositor_ata_base58" | jq -er '.account_id_hex')"
  claimant_ata_hex="$(account_codec "$claimant_ata_base58" | jq -er '.account_id_hex')"
  metadata_base58="$(account_codec --from-hex "$metadata_hex" | jq -er '.account_id_base58')"
  wallet_home="${M3_POC_F7_FIXTURE_PRIVATE_DIR}/wallets/maker"
  [[ -d "$wallet_home" && ! -L "$wallet_home" ]] || fail "official Maker wallet state is unavailable"
  wallet_output="$(
    printf '%s\n' 'local-poc-password-unused-upstream' |
      timeout --preserve-status 180s env LEE_WALLET_HOME_DIR="$wallet_home" \
        "$M3_POC_F7_WALLET_BIN" ata address --owner "$metadata_base58" \
        --token-definition "$definition_base58"
  )" || fail "official wallet could not derive the metadata custody ATA"
  wallet_log="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-f7-custody-ata.log"
  printf '%s\n' "$wallet_output" >"$wallet_log"
  chmod 0600 "$wallet_log"
  mapfile -t custody_values < <(sed -n '/^[1-9A-HJ-NP-Za-km-z]\{43,44\}$/p' <<<"$wallet_output")
  [[ "${#custody_values[@]}" == 1 ]] || fail "official custody ATA output is ambiguous"
  custody_base58="${custody_values[0]}"
  custody_hex="$(account_codec "$custody_base58" | jq -er '.account_id_hex')"
  depositor_owner="$(jq -er '.account_id_hex' \
    "${M3_POC_DIRECTION_ROOT}/actors/${depositor}/identity.json")"
  claimant_owner="$(jq -er '.account_id_hex' \
    "${M3_POC_DIRECTION_ROOT}/actors/${claimant}/identity.json")"
  jq -n --arg swap "$swap_id" \
    --arg terms "$terms_hash" --arg depositor "$depositor" --arg claimant "$claimant" \
    --arg depositor_owner "$depositor_owner" --arg claimant_owner "$claimant_owner" \
    --arg depositor_ata "$depositor_ata_hex" --arg claimant_ata "$claimant_ata_hex" \
    --arg custody "$custody_hex" --arg token "$M3_POC_F7_TOKEN_PROGRAM_ID" \
    --arg ata "$M3_POC_F7_ATA_PROGRAM_ID" --arg definition "$definition_hex" \
    --arg authority "$aggregate_account" --arg aggregate "$aggregate_key" \
    --argjson refund "$refund_at_ms" '
    {asset_terms_version:2,asset:{kind:"custom_token",terms:{
      swap_id:$swap,terms_hash:$terms,depositor:$depositor,
      depositor_owner_account_id:$depositor_owner,depositor_ata_account_id:$depositor_ata,
      claimant:$claimant,claimant_owner_account_id:$claimant_owner,
      claimant_ata_account_id:$claimant_ata,custody_ata_account_id:$custody,
      token_program_id:$token,ata_program_id:$ata,token_definition_account_id:$definition,
      aggregate_authority_account_id:$authority,aggregate_x_only_public_key:$aggregate,
      amount:"75",refund_at_ms:$refund}}}
  ' >"$output"
  chmod 0600 "$output"
}

prepare_direction_layout() {
  local role source_identity source_key
  [[ ! -e "$M3_POC_SECURE_STATE_ROOT" && ! -L "$M3_POC_SECURE_STATE_ROOT" ]] ||
    fail "refusing to reuse direction secure state"
  mkdir -m 0700 "$M3_POC_SECURE_STATE_ROOT"
  mkdir -p "$M3_POC_DIRECTION_ROOT/actors" "$M3_POC_DIRECTION_ROOT/sidecars"
  chmod 0700 "$M3_POC_DIRECTION_ROOT/actors" "$M3_POC_DIRECTION_ROOT/sidecars"
  for role in maker taker; do
    case "$role" in
      maker) source_identity="$M3_POC_MAKER_LEZ_IDENTITY"; source_key="$M3_POC_MAKER_LEZ_PRIVATE_KEY" ;;
      taker) source_identity="$M3_POC_TAKER_LEZ_IDENTITY"; source_key="$M3_POC_TAKER_LEZ_PRIVATE_KEY" ;;
    esac
    mkdir -m 0700 "$M3_POC_DIRECTION_ROOT/actors/$role"
    cp "$source_identity" "$M3_POC_DIRECTION_ROOT/actors/$role/identity.json"
    cp "$source_key" "$M3_POC_DIRECTION_ROOT/actors/$role/lez-signer.key"
    chmod 0600 "$M3_POC_DIRECTION_ROOT/actors/$role/identity.json" \
      "$M3_POC_DIRECTION_ROOT/actors/$role/lez-signer.key"
  done
}

prepare_stage_two_spec() {
  local output="$1" public_spec stage1_sha authority_mapping aggregate_key aggregate_account
  local depositor claimant amount now maker_cutoff refund_seconds refund_at_ms swap_id terms_file
  local earlier later
  local source_tx source_vout source_value source_script secret_hex secret_file
  local funding_spec funding_summary funding_hex funder mempool genesis height anchor source
  local funding_source_evidence funding_policy_evidence
  local pda_evidence metadata custody claim_hash earlier later asset_terms_file planning_claim_result
  public_spec="${M3_POC_DIRECTION_ROOT}/fixture/public-spec.json"
  stage1_sha="$(sha256sum "$public_spec" | sed 's/ .*//')"
  authority_mapping="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-official-nssa-mapping.json"
  aggregate_key="$(jq -er '.aggregate_internal_key' "$public_spec")"
  aggregate_account="$(jq -er '.account_id' "$authority_mapping")"
  prepare_direction_layout
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign) depositor=maker; claimant=taker ;;
    taker_sells_lez) depositor=taker; claimant=maker ;;
  esac
  if [[ "$asset_mode" == "custom_token" ]]; then amount=75; else amount=1000; fi
  now="$(date -u +%s)"
  case "$M3_POC_JOURNEY" in
    claim | survivor_claim)
      maker_cutoff=$((now + 1800))
      earlier=$((now + 3600))
      later=$((now + 7200))
      ;;
    refund)
      maker_cutoff=$((now + 300))
      earlier=$((now + 900))
      later=$((now + 1500))
      ;;
    first_lock_refund)
      maker_cutoff="$now"
      earlier=$((now + 600))
      later=$((now + 1200))
      ;;
  esac
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign) refund_seconds="$earlier" ;;
    taker_sells_lez) refund_seconds="$later" ;;
  esac
  refund_at_ms=$((refund_seconds * 1000))
  if [[ "$m5_btc_application_mode" == 1 ]]; then
    swap_id="$M3_POC_SWAP_ID"
  else
    swap_id="$(openssl rand -hex 32)"
  fi
  terms_file="${M3_POC_DIRECTION_ROOT}/planning-terms.json"
  jq -n --arg swap "$swap_id" --arg terms "$initial_terms_hash" \
    --arg depositor "$depositor" --arg claimant "$claimant" \
    --arg depositor_account "$(jq -er '.account_id_hex' "${M3_POC_DIRECTION_ROOT}/actors/${depositor}/identity.json")" \
    --arg claimant_account "$(jq -er '.account_id_hex' "${M3_POC_DIRECTION_ROOT}/actors/${claimant}/identity.json")" \
    --arg authority "$aggregate_account" --arg aggregate "$aggregate_key" \
    --arg amount "$amount" --argjson refund "$refund_at_ms" \
    --arg transfer "$M3_POC_LEZ_AUTH_TRANSFER_PROGRAM_ID" '
    {swap_id:$swap,terms_hash:$terms,depositor:$depositor,depositor_account_id:$depositor_account,
     claimant:$claimant,claimant_account_id:$claimant_account,
     aggregate_authority_account_id:$authority,aggregate_x_only_public_key:$aggregate,
     amount:$amount,refund_at_ms:$refund,authenticated_transfer_program_id:$transfer}' >"$terms_file"
  chmod 0600 "$terms_file"

  start_sidecars planning
  if [[ "$asset_mode" == "native" ]]; then
    prepare_witnessed_pair planning "$terms_file" "$depositor" "$claimant" planning
  fi

  pda_evidence="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-pda-observation.json"
  "$M3_POC_LEZ_NATIVE_ESCROW_BIN" observe --sequencer-url "$M3_POC_LEZ_SEQUENCER_RPC_URL" \
    --chain-id "$M3_POC_LEZ_CHANNEL_ID" --escrow-program-id "$M3_POC_LEZ_ESCROW_PROGRAM_ID" \
    --swap-id "$swap_id" --terms-hash "$initial_terms_hash" \
    --secret-digest "$pda_probe_secret_digest" --depositor-role "$depositor" \
    --depositor-account-id "$(jq -er '.depositor_account_id' "$terms_file")" \
    --claimant-role "$claimant" --claimant-account-id "$(jq -er '.claimant_account_id' "$terms_file")" \
    --amount "$amount" --refund-at-ms "$refund_at_ms" >"$pda_evidence"
  chmod 0600 "$pda_evidence"
  jq -e '.action == "observe" and .after.escrow_state == null' "$pda_evidence" >/dev/null ||
    fail "fresh witnessed escrow PDA accounts are not absent"
  metadata="$(jq -er '.after.metadata.account_id' "$pda_evidence")"
  custody="$(jq -er '.after.custody.account_id' "$pda_evidence")"
  if [[ "$asset_mode" == "custom_token" ]]; then
    asset_terms_file="${M3_POC_DIRECTION_ROOT}/planning-asset-terms.json"
    write_custom_token_terms "$initial_terms_hash" "$metadata" "$swap_id" "$refund_at_ms" \
      "$aggregate_account" "$aggregate_key" "$asset_terms_file"
    prepare_witnessed_asset_pair planning "$asset_terms_file" "$depositor" "$claimant" \
      planning
    planning_claim_result="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-planning-prepared-asset-claim.json"
  else
    planning_claim_result="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-planning-prepared-claim.json"
  fi
  claim_hash="$(jq -er '.claim.message_hash' "$planning_claim_result")"

  secret_hex="$(file_value "$M3_POC_BITCOIN_FUNDING_CREDENTIALS" BITCOIN_CORE_FUNDING_SECRET_KEY_HEX)"
  secret_file="${M3_POC_DIRECTION_ROOT}/service-funding.key"
  printf '%s' "$secret_hex" | xxd -r -p >"$secret_file"
  chmod 0600 "$secret_file"
  source="$(jq -ec --arg direction "$M3_POC_DIRECTION" '
    [.sources[] | select(.direction == $direction)] |
    if length == 1 then .[0] else error("direction funding source") end
  ' "$M3_POC_BITCOIN_FUNDING_SOURCES")"
  source_tx="$(jq -er '.source.transaction_id' <<<"$source")"
  source_vout="$(jq -er '.source.output_index | numbers' <<<"$source")"
  source_value="$(jq -er '.source.value_sat | numbers' <<<"$source")"
  source_script="$(jq -er '.source.script_pubkey' <<<"$source")"
  [[ "$(jq -er '.planned_bitcoin_funding_anchor_height | numbers' <<<"$source")" == "$M3_POC_BITCOIN_PLANNED_ANCHOR_HEIGHT" ]] ||
    fail "direction funding source and planned anchor disagree"
  funding_source_evidence="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-funding-source-gettxout.json"
  [[ ! -e "$funding_source_evidence" && ! -L "$funding_source_evidence" ]] ||
    fail "refusing to overwrite Core funding-source evidence"
  core_rpc maker gettxout "[\"${source_tx}\",${source_vout},true]" \
    >"$funding_source_evidence"
  chmod 0600 "$funding_source_evidence"
  jq -se --argjson value "$source_value" --arg script "$source_script" '
    length == 1
    and .[0].error == null
    and .[0].result != null
    and ((.[0].result.value * 100000000 | round) == $value)
    and .[0].result.scriptPubKey.hex == $script
  ' "$funding_source_evidence" >/dev/null ||
    fail "actual Core funding source is not exact and unspent"

  funding_spec="${M3_POC_DIRECTION_ROOT}/funding-spec.json"
  funding_summary="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-funding-prepared.json"
  jq -n --arg stage1 "$stage1_sha" --arg direction "$M3_POC_DIRECTION" \
    --arg tx "$source_tx" --argjson vout "$source_vout" --argjson value "$source_value" \
    --arg script "$source_script" --arg secret "$secret_file" '
    {schema_version:1,stage1_public_sha256:$stage1,direction:$direction,
     service_input:{transaction_id:$tx,output_index:$vout,value_sat:$value,
       script_pubkey:$script,signing_secret_key_file:$secret},
     contract_value_sat:1000000,fee_sat:1000}' >"$funding_spec"
  chmod 0600 "$funding_spec"
  "$M3_POC_PROVISIONER_BIN" prepare-funding --spec-file "$funding_spec" \
    --output-root "${M3_POC_DIRECTION_ROOT}/fixture" >"$funding_summary"
  chmod 0600 "$funding_summary"
  jq -e --arg direction "$M3_POC_DIRECTION" '
    .schema_version == 1 and .direction == $direction
    and .private_material_disclosed == false and .node_state_asserted == false
    and (.contract_merkle_root | test("^[0-9a-f]{64}$"))' "$funding_summary" >/dev/null ||
    fail "offline signed funding summary is invalid"
  funding_hex="$(tr -d '\r\n' <"${M3_POC_DIRECTION_ROOT}/fixture/funding-transaction.hex")"
  case "$M3_POC_DIRECTION" in taker_sells_foreign) funder=taker ;; taker_sells_lez) funder=maker ;; esac
  funding_policy_evidence="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-funding-policy.json"
  [[ ! -e "$funding_policy_evidence" && ! -L "$funding_policy_evidence" ]] ||
    fail "refusing to overwrite Core funding-policy evidence"
  core_rpc "$funder" testmempoolaccept "[[\"${funding_hex}\"]]" \
    >"$funding_policy_evidence"
  chmod 0600 "$funding_policy_evidence"
  jq -se --arg tx "$(jq -er '.transaction_id' "$funding_summary")" '
    length == 1
    and .[0].error == null
    and (.[0].result | type == "array" and length == 1)
    and .[0].result[0].txid == $tx
    and .[0].result[0].allowed == true
  ' "$funding_policy_evidence" >/dev/null ||
    fail "actual Core policy rejected pre-lock funding"
  genesis="$(core_rpc maker getblockhash '[0]' | jq -er '.result')"
  height="$(core_rpc maker getblockchaininfo '[]' |
    jq -ser 'select(length == 1) | .[0].result.blocks | numbers')"
  anchor="$M3_POC_BITCOIN_PLANNED_ANCHOR_HEIGHT"
  (( anchor >= height + 1 && anchor <= height + 2 )) ||
    fail "planned Bitcoin funding anchor ${anchor} is outside the current ${height} execution window $((height + 1))..$((height + 2))"

  jq -n --arg stage1 "$stage1_sha" --arg swap "$swap_id" --arg direction "$M3_POC_DIRECTION" \
    --arg genesis "$genesis" --arg funding_hex "$funding_hex" \
    --arg funding_sha "$(jq -er '.signed_transaction_sha256' "$funding_summary")" \
    --argjson input_value "$(jq -er '.input_value_sat' "$funding_summary")" \
    --arg input_script "$(jq -er '.input_script_pubkey' "$funding_summary")" \
    --arg funding_tx "$(jq -er '.transaction_id' "$funding_summary")" \
    --argjson funding_vout "$(jq -er '.contract_output_index' "$funding_summary")" \
    --argjson funding_value "$(jq -er '.contract_value_sat' "$funding_summary")" \
    --arg chain "$M3_POC_LEZ_CHANNEL_ID" --arg lez_genesis "$M3_POC_LEZ_GENESIS_BLOCK_HASH" \
    --arg escrow "$M3_POC_LEZ_ESCROW_PROGRAM_ID" --arg transfer "$M3_POC_LEZ_AUTH_TRANSFER_PROGRAM_ID" \
    --arg metadata "$metadata" --arg custody "$custody" \
    --arg depositor_account "$(jq -er '.depositor_account_id' "$terms_file")" \
    --arg claimant_account "$(jq -er '.claimant_account_id' "$terms_file")" \
    --argjson amount "$amount" --argjson refund "$refund_at_ms" --arg claim_hash "$claim_hash" \
    --argjson anchor "$anchor" --argjson refund_height "$((anchor + 144))" \
    --argjson earlier "$earlier" --argjson later "$later" \
    --argjson maker_cutoff "$maker_cutoff" --slurpfile authority "$authority_mapping" '
    {schema_version:1,stage1_public_sha256:$stage1,swap_id:$swap,direction:$direction,
     bitcoin:{genesis_block_hash:$genesis,required_confirmations:1,
       funding_signed_transaction:$funding_hex,funding_signed_transaction_sha256:$funding_sha,
       funding_input_value_sat:$input_value,funding_input_script_pubkey:$input_script,
       funding_transaction_id:$funding_tx,funding_output_index:$funding_vout,
       funding_value_sat:$funding_value,claim_value_sat:($funding_value - 1000)},
     lez_runtime:{compatibility:"lee_v0_2_0",chain_id:$chain,channel_id:$chain,
       genesis_block_hash:$lez_genesis,escrow_program_id:$escrow,
       authenticated_transfer_program_id:$transfer},
     lez_terms:{aggregate_authority_mapping:$authority[0],metadata_account:$metadata,
       custody_account:$custody,depositor_account:$depositor_account,
       claimant_account:$claimant_account,amount:$amount,refund_at_ms:$refund,
       prepared_claim_message_hash:$claim_hash},
     recovery:{refund_csv_blocks:144,planned_bitcoin_funding_anchor_height:$anchor,
       bitcoin_refund_height:$refund_height,maker_second_lock_cutoff_unix_seconds:$maker_cutoff,
       earlier_refund_latest_unix_seconds:$earlier,
       later_refund_earliest_unix_seconds:$later,required_margin_seconds:600}}' >"$output"
  chmod 0600 "$output"
  stop_sidecars planning
}

finalized_tip() {
  local response tip
  for _ in {1..120}; do
    if response="$(rpc "$M3_POC_LEZ_INDEXER_RPC_URL" \
      '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' 2>/dev/null)" &&
      tip="$(jq -er '.result | numbers' <<<"$response" 2>/dev/null)"; then
      printf '%s\n' "$tip"
      return 0
    fi
    sleep 0.25
  done
  fail "LEZ finalized tip remained unavailable"
}

rpc_read_file() {
  local endpoint="$1" request="$2" output="$3"
  local partial="${output}.partial"
  for _ in {1..120}; do
    if rpc "$endpoint" "$request" >"$partial" 2>/dev/null &&
      jq -e '.error == null and .result != null' "$partial" >/dev/null 2>&1; then
      chmod 0600 "$partial"
      mv "$partial" "$output"
      return 0
    fi
    sleep 0.25
  done
  fail "bounded read-only RPC remained unavailable: ${output}"
}

lez_proved_tip=0
prove_lez_finalized_transaction() {
  local label="$1" transaction_id="$2" start_height="$3"
  local cursor=$((start_height + 1)) tip height block_file block_hash hash_file
  local transaction_file occurrences containing_file="" count=0 containing_height=0
  [[ "$transaction_id" =~ ^[0-9a-f]{64}$ ]] || fail "${label} transaction ID is invalid"
  for _ in {1..1200}; do
    tip="$(finalized_tip)"
    (( tip >= start_height && tip - start_height <= 4096 )) ||
      fail "${label} finalized window is invalid or exceeded 4096 blocks"
    while (( cursor <= tip )); do
      height="$cursor"
      block_file="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-block-${height}.json"
      rpc_read_file "$M3_POC_LEZ_INDEXER_RPC_URL" \
        "$(jq -cn --argjson height "$height" \
          '{jsonrpc:"2.0",id:1,method:"getBlockById",params:[$height]}')" "$block_file"
      occurrences="$(jq -er --arg tx "$transaction_id" \
        '[.result.body.transactions[]? | select(.Public.hash == $tx)] | length' "$block_file")"
      if (( occurrences > 0 )); then
        count=$((count + occurrences))
        containing_height="$height"
        containing_file="$block_file"
      fi
      cursor=$((cursor + 1))
    done
    (( count > 0 )) && break
    sleep 0.25
  done
  [[ "$count" == 1 && "$containing_height" != 0 && -n "$containing_file" ]] ||
    fail "${label} was not found exactly once in finalized ancestry"
  block_hash="$(jq -er '.result.header.hash | strings' "$containing_file")"
  jq -e --argjson height "$containing_height" --arg hash "$block_hash" '
    .result.header.block_id == $height
    and .result.header.hash == $hash
    and .result.bedrock_status == "Finalized"
  ' "$containing_file" >/dev/null || fail "${label} containing block is not Finalized"
  hash_file="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-block-by-hash.json"
  rpc_read_file "$M3_POC_LEZ_INDEXER_RPC_URL" \
    "$(jq -cn --arg hash "$block_hash" \
      '{jsonrpc:"2.0",id:1,method:"getBlockByHash",params:[$hash]}')" "$hash_file"
  [[ "$(jq -S -c '.result' "$containing_file")" == "$(jq -S -c '.result' "$hash_file")" ]] ||
    fail "${label} block ID/hash lookups disagree"
  transaction_file="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-transaction.json"
  rpc_read_file "$M3_POC_LEZ_INDEXER_RPC_URL" \
    "$(jq -cn --arg tx "$transaction_id" \
      '{jsonrpc:"2.0",id:1,method:"getTransaction",params:[$tx]}')" "$transaction_file"
  jq -e --arg tx "$transaction_id" '.result.Public.hash == $tx' "$transaction_file" >/dev/null ||
    fail "${label} indexed transaction hash disagrees"
  lez_proved_tip="$tip"
  jq -n --arg label "$label" --arg tx "$transaction_id" \
    --argjson start "$((start_height + 1))" --argjson tip "$tip" \
    --argjson block "$containing_height" --arg hash "$block_hash" '
    {schema_version:1,label:$label,transaction_id:$tx,
     window:{start_height:$start,finalized_tip:$tip},occurrences:1,
     containing_block_id:$block,containing_block_hash:$hash,
     bedrock_status:"Finalized",id_hash_lookups_equal:true,
     transaction_hash_revalidated:true}
  ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-finality.json"
  chmod 0600 "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-finality.json"
}

core_admin() {
  local actual_identity
  actual_identity="$(docker inspect --format \
    '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}|{{ index .Config.Labels "org.logos-co.atomic-swaps.scope" }}|{{ index .Config.Labels "org.logos-co.atomic-swaps.component" }}' \
    "$M3_POC_BITCOIN_CONTAINER_ID")"
  [[ "$actual_identity" == \
     "${M3_POC_RUN_ID}-btc|bitcoin-core-regtest-e2e|bitcoin-core" ]] ||
    fail "captured Bitcoin container ownership label drifted"
  docker exec "$M3_POC_BITCOIN_CONTAINER_ID" bitcoin-cli \
    -conf=/run-config/bitcoin.conf -datadir=/var/lib/bitcoin "$@"
}

core_mined_block_hash=""
core_mined_block_height=0
mine_one_core_block() {
  local address mined block
  address="$(file_value "$M3_POC_BITCOIN_FUNDING_CREDENTIALS" BITCOIN_CORE_FUNDING_ADDRESS)"
  mined="$(core_admin generatetoaddress 1 "$address")"
  jq -e 'type == "array" and length == 1 and (.[0] | test("^[0-9a-f]{64}$"))' \
    <<<"$mined" >/dev/null || fail "Core did not mine exactly one run-owned block"
  core_mined_block_hash="$(jq -er '.[0]' <<<"$mined")"
  block="$(core_rpc maker getblock "[\"${core_mined_block_hash}\",1]")"
  core_mined_block_height="$(jq -er '.result.height | numbers' <<<"$block")"
}

wait_core_confirmed() {
  local transaction_id="$1" role="$2" label="$3"
  local response confirmations
  for _ in {1..120}; do
    if response="$(core_rpc "$role" getrawtransaction "[\"${transaction_id}\",true]" 2>/dev/null)" &&
      confirmations="$(jq -er '.result.confirmations | numbers' <<<"$response" 2>/dev/null)" &&
      (( confirmations >= 1 )); then
      jq -e --arg tx "$transaction_id" '.result.txid == $tx' <<<"$response" >/dev/null ||
        fail "${label} Core transaction ID drifted"
      printf '%s\n' "$response" >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-confirmed.json"
      chmod 0600 "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-confirmed.json"
      return 0
    fi
    sleep 0.25
  done
  fail "${label} did not reach the signed one-confirmation local policy"
}

bitcoin_lock_tx=""
bitcoin_claim_tx=""
bitcoin_refund_tx=""
bitcoin_refund_wtxid=""
confirm_bitcoin_lock_after_submission() {
  local funder="$1" peer="$2"
  local expected mempool planned_anchor mined_block
  expected="$(jq -er '.bitcoin_funding_transaction_id' \
    "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-stage-two.json")"
  mempool="$(core_rpc "$peer" getrawmempool '[]')"
  jq -e --arg tx "$expected" '.result == [$tx]' <<<"$mempool" >/dev/null ||
    fail "counterparty did not observe the exact Bitcoin lock in mempool"
  mine_one_core_block
  planned_anchor="$(jq -er '.planned_bitcoin_funding_anchor_height | numbers' \
    "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-stage-two.json")"
  [[ "$core_mined_block_height" == "$planned_anchor" ]] ||
    fail "actual Bitcoin funding height differs from the countersigned planned anchor"
  mined_block="$(core_rpc "$peer" getblock "[\"${core_mined_block_hash}\",1]")"
  jq -e --arg hash "$core_mined_block_hash" --arg tx "$expected" \
    --argjson height "$planned_anchor" '
    .result.hash == $hash and .result.height == $height
    and ([.result.tx[] | select(. == $tx)] | length) == 1
  ' <<<"$mined_block" >/dev/null ||
    fail "Bitcoin funding block does not contain the exact signed lock once"
  jq -n --arg transaction_id "$expected" --arg block_hash "$core_mined_block_hash" \
    --argjson block_height "$core_mined_block_height" \
    --argjson planned_anchor "$planned_anchor" '
    {schema_version:1,transaction_id:$transaction_id,
     containing_block_hash:$block_hash,containing_block_height:$block_height,
     planned_bitcoin_funding_anchor_height:$planned_anchor,
     exact_transaction_occurrences:1,planned_anchor_satisfied:true}
  ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-bitcoin-funding-anchor.json"
  chmod 0600 "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-bitcoin-funding-anchor.json"
  wait_core_confirmed "$expected" "$peer" bitcoin-lock
  bitcoin_lock_tx="$expected"
}

submit_taker_bitcoin_first_lock() {
  local funding_hex expected response
  [[ "$M3_POC_DIRECTION" == "taker_sells_foreign" ]] ||
    fail "external Bitcoin lock submission is reserved for the Taker first lock"
  funding_hex="$(tr -d '\r\n' <"${M3_POC_DIRECTION_ROOT}/fixture/funding-transaction.hex")"
  expected="$(jq -er '.bitcoin_funding_transaction_id' \
    "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-stage-two.json")"
  core_rpc taker testmempoolaccept "[[\"${funding_hex}\"]]" |
    jq -e '.result[0].allowed == true' >/dev/null ||
    fail "Core policy rejected the exact signed Bitcoin Taker first lock"
  response="$(core_rpc taker sendrawtransaction "[\"${funding_hex}\"]")"
  [[ "$(jq -er '.result' <<<"$response")" == "$expected" ]] ||
    fail "Core returned an unexpected Bitcoin Taker first-lock ID"
  confirm_bitcoin_lock_after_submission taker maker
}

submit_actor_maker_bitcoin_second_lock() {
  local funding_hex expected mempool
  [[ "$M3_POC_DIRECTION" == "taker_sells_lez" ]] ||
    fail "actor-owned Bitcoin Maker lock is only valid when the Taker sells LEZ"
  funding_hex="$(tr -d '\r\n' <"${M3_POC_DIRECTION_ROOT}/fixture/funding-transaction.hex")"
  expected="$(jq -er '.bitcoin_funding_transaction_id' \
    "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-stage-two.json")"
  core_rpc maker testmempoolaccept "[[\"${funding_hex}\"]]" |
    jq -e '.result[0].allowed == true' >/dev/null ||
    fail "Core policy rejected the exact signed Bitcoin Maker second lock"

  actor_invoke_bitcoin_lock_awaiting_retry maker drive "$expected" 0 \
    bitcoin-maker-lock-submit
  jq -e '
    .schema_version == 1 and .role == "maker" and .command == "drive"
    and .outcome == "awaiting_observation" and .chain == "bitcoin"
    and .phase == "taker_lock_confirmed" and .revision == 1
  ' "$actor_last_output" >/dev/null ||
    fail "Maker actor did not submit the Bitcoin second lock from revision one"
  mempool="$(core_rpc taker getrawmempool '[]')"
  jq -e --arg tx "$expected" '.result == [$tx]' <<<"$mempool" >/dev/null ||
    fail "Taker did not observe exactly the actor-submitted Bitcoin Maker lock"

  actor_invoke_bitcoin_lock_awaiting_retry maker drive "$expected" 1 \
    bitcoin-maker-lock-accepted-restart
  jq -e '
    .schema_version == 1 and .role == "maker" and .command == "drive"
    and .outcome == "awaiting_observation" and .chain == "bitcoin"
    and .phase == "taker_lock_confirmed" and .revision == 1
  ' "$actor_last_output" >/dev/null ||
    fail "fresh Maker restart changed the accepted Bitcoin lock state"
  core_rpc maker getrawmempool '[]' | jq -e --arg tx "$expected" \
    '.error == null and .result == [$tx]' >/dev/null ||
    fail "fresh Maker restart resubmitted or changed the Bitcoin second lock"

  confirm_bitcoin_lock_after_submission maker taker
}

final_terms=""
final_prepared_escrow=""
final_prepared_claim=""
asset_commitment=""
prepare_final_transcript() {
  local commitment planning_claim_hash final_claim_hash planning_claim_bytes final_claim_bytes
  local planning_claim asset_spec asset_summary
  commitment="$(jq -er '.agreement_commitment' \
    "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-stage-two.json")"
  if [[ "$asset_mode" == "custom_token" ]]; then
    asset_spec="${M3_POC_DIRECTION_ROOT}/asset-extension-spec.json"
    asset_summary="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-asset-extension.json"
    jq -n --slurpfile stage2 \
      "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-stage-two.json" \
      --slurpfile terms "${M3_POC_DIRECTION_ROOT}/planning-asset-terms.json" '
      {schema_version:1,
       expected_agreement_sha256:$stage2[0].agreement_sha256,
       expected_agreement_commitment:$stage2[0].agreement_commitment,
       token_program_id:$terms[0].asset.terms.token_program_id,
       ata_program_id:$terms[0].asset.terms.ata_program_id,
       token_definition_account:$terms[0].asset.terms.token_definition_account_id,
       depositor_ata_account:$terms[0].asset.terms.depositor_ata_account_id,
       claimant_ata_account:$terms[0].asset.terms.claimant_ata_account_id,
       custody_ata_account:$terms[0].asset.terms.custody_ata_account_id}
    ' >"$asset_spec"
    chmod 0600 "$asset_spec"
    "$M3_POC_PROVISIONER_BIN" finalize-asset-extension --spec-file "$asset_spec" \
      --output-root "${M3_POC_DIRECTION_ROOT}/fixture" >"$asset_summary"
    chmod 0600 "$asset_summary"
    jq -e --arg base "$commitment" '
      .schema_version == 1 and .base_agreement_commitment == $base
      and .extension_revalidated == true and .private_material_disclosed == false
      and .amount == 75 and (.asset_commitment | test("^[0-9a-f]{64}$"))
    ' "$asset_summary" >/dev/null || fail "countersigned custom-token extension is invalid"
    asset_commitment="$(jq -er '.asset_commitment' "$asset_summary")"
    final_terms="${M3_POC_DIRECTION_ROOT}/final-asset-terms.json"
    jq --arg terms "$asset_commitment" '.asset.terms.terms_hash = $terms' \
      "${M3_POC_DIRECTION_ROOT}/planning-asset-terms.json" >"$final_terms"
    planning_claim="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-planning-prepared-asset-claim.json"
  else
    final_terms="${M3_POC_DIRECTION_ROOT}/final-terms.json"
    jq --arg terms "$commitment" '.terms_hash = $terms' \
      "${M3_POC_DIRECTION_ROOT}/planning-terms.json" >"$final_terms"
    planning_claim="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-planning-prepared-claim.json"
  fi
  chmod 0600 "$final_terms"
  start_sidecars final
  if [[ "$asset_mode" == "custom_token" ]]; then
    case "$M3_POC_DIRECTION" in
      taker_sells_foreign)
        prepare_witnessed_asset_pair final "$final_terms" maker taker final
        ;;
      taker_sells_lez)
        prepare_witnessed_asset_pair final "$final_terms" taker maker final
        ;;
    esac
    final_prepared_escrow="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-final-prepared-asset-escrow.json"
    final_prepared_claim="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-final-prepared-asset-claim.json"
  else
    case "$M3_POC_DIRECTION" in
      taker_sells_foreign)
        prepare_witnessed_pair final "$final_terms" maker taker final
        ;;
      taker_sells_lez)
        prepare_witnessed_pair final "$final_terms" taker maker final
        ;;
    esac
    final_prepared_escrow="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-final-prepared-escrow.json"
    final_prepared_claim="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-final-prepared-claim.json"
  fi
  planning_claim_hash="$(jq -er '.claim.message_hash' "$planning_claim")"
  final_claim_hash="$(jq -er '.claim.message_hash' "$final_prepared_claim")"
  planning_claim_bytes="$(jq -er '.claim.exact_message_bytes' "$planning_claim")"
  final_claim_bytes="$(jq -er '.claim.exact_message_bytes' "$final_prepared_claim")"
  [[ "$planning_claim_hash" == "$final_claim_hash" &&
     "$planning_claim_bytes" == "$final_claim_bytes" ]] ||
    fail "final agreement binding changed the pre-lock official LEZ claim transcript"
  jq -e --arg terms "${asset_commitment:-$commitment}" '
    if .asset_terms_version == 2 then .asset.terms.terms_hash == $terms
    else .terms_hash == $terms end
  ' "$final_terms" >/dev/null ||
    fail "final witnessed terms do not use the countersigned agreement commitment"
}

btc_session_id=""
lez_session_id=""
btc_session_file=""
lez_session_file=""
provision_signing_material() {
  local public_spec="${M3_POC_DIRECTION_ROOT}/fixture/public-spec.json"
  local role source destination bitcoin_funder nonfunder refund_destination
  mkdir -p "${M3_POC_DIRECTION_ROOT}/public"
  chmod 0700 "${M3_POC_DIRECTION_ROOT}/public"
  for role in maker taker; do
    source="${M3_POC_DIRECTION_ROOT}/fixture/private/${role}-signing.key"
    destination="${M3_POC_DIRECTION_ROOT}/actors/${role}/signing.key"
    [[ "$(stat -c '%s' "$source")" == 32 ]] ||
      fail "${role} stage-one signing key has an unexpected size"
    xxd -p -c 32 "$source" >"$destination"
    chmod 0600 "$destination"
  done
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign) bitcoin_funder=taker; nonfunder=maker ;;
    taker_sells_lez) bitcoin_funder=maker; nonfunder=taker ;;
  esac
  source="${M3_POC_DIRECTION_ROOT}/fixture/private/${bitcoin_funder}-refund.key"
  refund_destination="${M3_POC_DIRECTION_ROOT}/actors/${bitcoin_funder}/bitcoin-refund.key"
  [[ "$(stat -c '%s' "$source")" == 32 ]] ||
    fail "${bitcoin_funder} stage-one refund key has an unexpected size"
  xxd -p -c 32 "$source" >"$refund_destination"
  chmod 0600 "$refund_destination"
  [[ ! -e "${M3_POC_DIRECTION_ROOT}/actors/${nonfunder}/bitcoin-refund.key" ]] ||
    fail "Bitcoin non-funder must not receive refund authority"
  source="${M3_POC_DIRECTION_ROOT}/fixture/private/adaptor-scalar.key"
  [[ "$(stat -c '%s' "$source")" == 32 ]] ||
    fail "stage-one adaptor scalar has an unexpected size"
  xxd -p -c 32 "$source" >"${M3_POC_DIRECTION_ROOT}/actors/taker/adaptor-secret.key"
  chmod 0600 "${M3_POC_DIRECTION_ROOT}/actors/taker/adaptor-secret.key"
  [[ ! -e "${M3_POC_DIRECTION_ROOT}/actors/maker/adaptor-secret.key" ]] ||
    fail "maker must not receive pre-lock adaptor authority"

  btc_session_id="$(openssl rand -hex 32)"
  lez_session_id="$(openssl rand -hex 32)"
  [[ "$btc_session_id" != "$lez_session_id" ]] || fail "signing session IDs collided"
  btc_session_file="${M3_POC_DIRECTION_ROOT}/btc-session.json"
  lez_session_file="${M3_POC_DIRECTION_ROOT}/lez-session.json"
  jq -cn --arg session "$btc_session_id" \
    --arg message "$(jq -er '.bitcoin_claim_bip341_sighash' \
      "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-stage-two.json")" \
    --arg merkle "$(jq -er '.contract_merkle_root' \
      "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-funding-prepared.json")" \
    --arg adaptor "$(jq -er '.adaptor_point' "$public_spec")" \
    --arg maker "$(jq -er '.maker.musig2_public_key' "$public_spec")" \
    --arg taker "$(jq -er '.taker.musig2_public_key' "$public_spec")" '
    {schema_version:1,context:{kind:"btc_taproot",merkle_root:$merkle},
     session_id:$session,exact_message:$message,adaptor_point:$adaptor,
     maker_public_key:$maker,taker_public_key:$taker}
  ' >"$btc_session_file"
  jq -cn --arg session "$lez_session_id" \
    --arg message "$(jq -er '.claim.message_hash' "$final_prepared_claim")" \
    --arg adaptor "$(jq -er '.adaptor_point' "$public_spec")" \
    --arg maker "$(jq -er '.maker.musig2_public_key' "$public_spec")" \
    --arg taker "$(jq -er '.taker.musig2_public_key' "$public_spec")" '
    {schema_version:1,context:{kind:"lez_untweaked"},session_id:$session,
     exact_message:$message,adaptor_point:$adaptor,
     maker_public_key:$maker,taker_public_key:$taker}
  ' >"$lez_session_file"
  chmod 0600 "$btc_session_file" "$lez_session_file"
}

run_signing_ceremony() {
  local prefix="$1" session="$2"
  local public="${M3_POC_DIRECTION_ROOT}/public"
  local maker_journal="${M3_POC_DIRECTION_ROOT}/actors/maker/${prefix}-journal.sqlite"
  local taker_journal="${M3_POC_DIRECTION_ROOT}/actors/taker/${prefix}-journal.sqlite"
  "$M3_POC_ROLE_RUNNER_BIN" maker --journal "$maker_journal" --session "$session" \
    reserve --secret-key-file "${M3_POC_DIRECTION_ROOT}/actors/maker/signing.key" \
    --output "${public}/${prefix}-maker-commitment.json"
  "$M3_POC_ROLE_RUNNER_BIN" taker --journal "$taker_journal" --session "$session" \
    reserve --secret-key-file "${M3_POC_DIRECTION_ROOT}/actors/taker/signing.key" \
    --output "${public}/${prefix}-taker-commitment.json"
  "$M3_POC_ROLE_RUNNER_BIN" maker --journal "$maker_journal" --session "$session" \
    accept-commitment --input "${public}/${prefix}-taker-commitment.json"
  "$M3_POC_ROLE_RUNNER_BIN" taker --journal "$taker_journal" --session "$session" \
    accept-commitment --input "${public}/${prefix}-maker-commitment.json"
  "$M3_POC_ROLE_RUNNER_BIN" maker --journal "$maker_journal" --session "$session" \
    reveal-nonce --output "${public}/${prefix}-maker-nonce.json"
  "$M3_POC_ROLE_RUNNER_BIN" taker --journal "$taker_journal" --session "$session" \
    reveal-nonce --output "${public}/${prefix}-taker-nonce.json"
  "$M3_POC_ROLE_RUNNER_BIN" maker --journal "$maker_journal" --session "$session" \
    accept-nonce-sign --input "${public}/${prefix}-taker-nonce.json" \
    --secret-key-file "${M3_POC_DIRECTION_ROOT}/actors/maker/signing.key" \
    --output "${public}/${prefix}-maker-partial.json"
  "$M3_POC_ROLE_RUNNER_BIN" taker --journal "$taker_journal" --session "$session" \
    accept-nonce-sign --input "${public}/${prefix}-maker-nonce.json" \
    --secret-key-file "${M3_POC_DIRECTION_ROOT}/actors/taker/signing.key" \
    --output "${public}/${prefix}-taker-partial.json"
  "$M3_POC_ROLE_RUNNER_BIN" maker --journal "$maker_journal" --session "$session" \
    accept-peer-partial --input "${public}/${prefix}-taker-partial.json" \
    --output "${public}/${prefix}-maker-presignature.json"
  "$M3_POC_ROLE_RUNNER_BIN" taker --journal "$taker_journal" --session "$session" \
    accept-peer-partial --input "${public}/${prefix}-maker-partial.json" \
    --output "${public}/${prefix}-taker-presignature.json"
  cmp "${public}/${prefix}-maker-presignature.json" \
    "${public}/${prefix}-taker-presignature.json" ||
    fail "${prefix} role journals did not converge on one verified presignature"
}

accepted_at=0
actor_prelock_lez_tip=0
declare -A m5_btc_actor_configs=()
m5_btc_window_start=0
m5_btc_window_end=0

write_actor_configs() {
  local start_height="$1" max_blocks="$2"
  local role basic endpoint config partial adaptor refund
  local maker_bitcoin_funding maker_lez_request maker_lez_result
  local schema asset_record current_asset_commitment agreement agreement_sha256
  [[ "$start_height" =~ ^[0-9]+$ && "$max_blocks" =~ ^[0-9]+$ ]] ||
    fail "actor LEZ window is not numeric"
  (( max_blocks >= 1 && max_blocks <= 4096 )) || fail "actor LEZ window is out of bounds"
  if [[ "$m5_btc_application_mode" == 1 &&
        "${#m5_btc_actor_configs[@]}" == 2 ]]; then
    local requested_end=$((start_height + max_blocks - 1))
    (( start_height >= m5_btc_window_start && requested_end <= m5_btc_window_end )) ||
      fail "M5 BTC actor observation escaped its provisioned LEZ window"
    jq -n --argjson start "$start_height" --argjson blocks "$max_blocks" '
      {schema_version:1,start_height:$start,max_blocks:$blocks,
       config_publication:"unchanged_provisioned_schema_6"}
    ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actor-lez-window-latest.json"
    chmod 0600 \
      "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actor-lez-window-latest.json"
    return 0
  fi
  agreement="${M3_POC_DIRECTION_ROOT}/fixture/agreement.borsh"
  maker_bitcoin_funding="${M3_POC_DIRECTION_ROOT}/fixture/funding-transaction.hex"
  if [[ "$asset_mode" == "custom_token" ]]; then
    schema=5
    maker_lez_request="${M3_POC_DIRECTION_ROOT}/final-prepare-asset-escrow-request.json"
    asset_record="${M3_POC_DIRECTION_ROOT}/fixture/lez-asset-extension.borsh"
    current_asset_commitment="${asset_commitment:-$(jq -er '.asset_commitment' \
      "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-asset-extension.json")}"
    [[ -f "$asset_record" && ! -L "$asset_record" &&
       "$current_asset_commitment" =~ ^[0-9a-f]{64}$ ]] ||
      fail "exact countersigned asset extension is unavailable"
  else
    if [[ "$m5_btc_application_mode" == "1" ]]; then
      schema=6
    else
      schema=4
    fi
    maker_lez_request="${M3_POC_DIRECTION_ROOT}/final-prepare-escrow-request.json"
    asset_record=""
    current_asset_commitment=""
  fi
  [[ -f "$agreement" && ! -L "$agreement" ]] || fail "exact agreement is unavailable"
  agreement_sha256="$(sha256sum "$agreement" | sed 's/ .*//')"
  [[ "$agreement_sha256" =~ ^[0-9a-f]{64}$ ]] || fail "agreement SHA-256 is invalid"
  maker_lez_result="$final_prepared_escrow"
  [[ -f "$maker_bitcoin_funding" && ! -L "$maker_bitcoin_funding" ]] ||
    fail "exact signed Bitcoin maker-lock material is unavailable"
  [[ -f "$maker_lez_request" && ! -L "$maker_lez_request" &&
     -f "$maker_lez_result" && ! -L "$maker_lez_result" ]] ||
    fail "exact witnessed LEZ maker-lock material is unavailable"
  for role in maker taker; do
    case "$role" in
      maker)
        basic="$M3_POC_BITCOIN_MAKER_BASIC"
        adaptor=""
        ;;
      taker)
        basic="$M3_POC_BITCOIN_TAKER_BASIC"
        adaptor="${M3_POC_DIRECTION_ROOT}/actors/taker/adaptor-secret.key"
        ;;
    esac
    refund=""
    case "$M3_POC_DIRECTION:$role" in
      taker_sells_foreign:taker|taker_sells_lez:maker)
        refund="${M3_POC_DIRECTION_ROOT}/actors/${role}/bitcoin-refund.key"
        ;;
    esac
    endpoint="$(file_value "${M3_POC_DIRECTION_ROOT}/final-endpoints.env" "$role")"
    config="${M3_POC_DIRECTION_ROOT}/actors/${role}/actor-config.json"
    partial="${config}.partial"
    jq -n --arg role "$role" --argjson schema "$schema" \
      --arg agreement "$agreement" \
      --arg agreement_sha256 "$agreement_sha256" \
      --arg state "${M3_POC_DIRECTION_ROOT}/actors/${role}/actor-state.sqlite" \
      --argjson accepted "$accepted_at" --arg core "$M3_POC_BITCOIN_RPC_URL" \
      --arg basic "$basic" --arg bridge "$endpoint" \
      --arg capability "${M3_POC_DIRECTION_ROOT}/sidecars/final/${role}/capability" \
      --arg run "$M3_POC_RUN_ID" --argjson start "$start_height" \
      --argjson blocks "$max_blocks" --arg btc_session "$btc_session_id" \
      --argjson bridge_timeout "$actor_lez_bridge_request_timeout_millis" \
      --arg btc_journal "${M3_POC_DIRECTION_ROOT}/actors/${role}/btc-journal.sqlite" \
      --arg lez_session "$lez_session_id" \
      --arg lez_journal "${M3_POC_DIRECTION_ROOT}/actors/${role}/lez-journal.sqlite" \
      --arg prepared "$final_prepared_claim" --arg adaptor "$adaptor" --arg refund "$refund" \
      --arg direction "$M3_POC_DIRECTION" \
      --arg asset_mode "$asset_mode" --arg asset_record "$asset_record" \
      --arg asset_commitment "$current_asset_commitment" \
      --arg maker_bitcoin_funding "$maker_bitcoin_funding" \
      --arg maker_lez_request "$maker_lez_request" --arg maker_lez_result "$maker_lez_result" \
      --slurpfile runtime "${M3_POC_DIRECTION_ROOT}/sidecars/final/${role}/runtime.json" '
      {
        schema_version:$schema,role:$role,agreement_file:$agreement,state_db:$state,
        accepted_at_unix_seconds:$accepted,
        bitcoin_core:{endpoint:$core,cookie_file:$basic,connectivity:"isolated_local"},
      }
      + (if $schema == 6 then {agreement_sha256:$agreement_sha256} else {} end)
      + {lez_bridge:{endpoint:$bridge,capability_file:$capability,run_id:$run,
          runtime:$runtime[0],request_timeout_millis:$bridge_timeout,
          discovery_start_height:$start,discovery_max_blocks:$blocks},
        signing:({
          bitcoin:{session_id:$btc_session,journal_db:$btc_journal},
          lez:{session_id:$lez_session,journal_db:$lez_journal},
          prepared_witnessed_claim_result_file:$prepared
        } + (if $role == "taker" then {adaptor_secret_file:$adaptor} else {} end)),
        refund:(if $refund == "" then {} else {bitcoin_refund_key_file:$refund} end)
      }
      + (if $asset_mode == "custom_token" then
          {asset_extension:{record_file:$asset_record,
            expected_asset_commitment:$asset_commitment}}
         else {} end)
      + (if $role == "maker" then {maker_lock:
          (if $direction == "taker_sells_lez" then
            {chain:"bitcoin",exact_funding_transaction_file:$maker_bitcoin_funding}
           elif $direction == "taker_sells_foreign" and $asset_mode == "custom_token" then
            {chain:"lez_asset_v2",preparation_request_file:$maker_lez_request,
             preparation_result_file:$maker_lez_result}
           elif $direction == "taker_sells_foreign" then
            {chain:"lez",preparation_request_file:$maker_lez_request,
             preparation_result_file:$maker_lez_result}
           else error("unsupported maker-lock direction") end)} else {} end)
      + (if $role == "maker" and $direction == "taker_sells_lez"
             and $asset_mode == "custom_token" then
          {taker_first_lock:{chain:"lez_asset_v2",
            preparation_request_file:$maker_lez_request,
            preparation_result_file:$maker_lez_result}}
         else {} end)
    ' >"$partial"
    chmod 0600 "$partial"
    mv "$partial" "$config"
  done
  jq -n --argjson start "$start_height" --argjson blocks "$max_blocks" '
    {schema_version:1,start_height:$start,max_blocks:$blocks}
  ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actor-lez-window-latest.json"
  chmod 0600 "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actor-lez-window-latest.json"
  if [[ "$m5_btc_application_mode" == 1 ]]; then
    m5_btc_window_start="$start_height"
    m5_btc_window_end=$((start_height + max_blocks - 1))
  fi
}

actor_runtime_config() {
  local role="$1" config
  case "$role" in
    maker | taker) ;;
    *) fail "unsupported actor runtime role: ${role}" ;;
  esac
  if [[ "$m5_btc_application_mode" == 1 &&
        "${#m5_btc_actor_configs[@]}" == 2 ]]; then
    config="${m5_btc_actor_configs[$role]:-}"
  else
    config="${M3_POC_DIRECTION_ROOT}/actors/${role}/actor-config.json"
  fi
  [[ "$config" == /* && -f "$config" && ! -L "$config" ]] ||
    fail "${role} actor runtime config is unavailable or unsafe"
  printf '%s\n' "$config"
}

actor_last_output=""
survivor_taker_absence_guard=0

write_lez_submission_count_diagnostic() {
  local label="$1" observed="$2" expected="$3" role journal
  local output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-submission-count-diagnostic.json"
  local entries="[]"
  [[ ! -e "$output" && ! -L "$output" ]] || return 1
  for role in maker taker; do
    journal="${M3_POC_SECURE_STATE_ROOT}/sidecars/final/${role}/bridge-requests.v1.json"
    [[ -f "$journal" && ! -L "$journal" ]] || return 1
    entries="$(jq -c --arg role "$role" --argjson existing "$entries" '
      $existing + [.entries[] |
        select(.method == "lez_bridge.v1.submit_transaction") |
        {role:$role,method,outcome_kind:.outcome.kind,
         transaction_id:(.outcome.value.transaction_id // null)}]
    ' "$journal")" || return 1
  done
  jq -n --arg direction "$M3_POC_DIRECTION" --arg label "$label" \
    --argjson observed "$observed" --argjson expected "$expected" \
    --argjson entries "$entries" '
    {schema_version:1,direction:$direction,label:$label,
     observed_successful_submission_count:$observed,
     expected_successful_submission_count:$expected,
     generic_submit_entries:$entries}
  ' >"$output" || return 1
  chmod 0600 "$output"
}

assert_survivor_actor_invocation_allowed() {
  local role="$1" label="$2"
  if [[ "${M3_POC_JOURNEY:-}" == "survivor_claim" &&
        "$survivor_taker_absence_guard" == 1 && "$role" == "taker" ]]; then
    fail "survivor revealer actor invocation attempted during protected absence: ${label}"
  fi
}

actor_invoke() {
  local role="$1" command="$2" label="$3"
  local config
  config="$(actor_runtime_config "$role")"
  assert_survivor_actor_invocation_allowed "$role" "$label"
  actor_last_output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-${role}.json"
  [[ ! -e "$actor_last_output" ]] || fail "refusing to overwrite actor evidence: ${label}/${role}"
  "$M3_POC_ACTOR_BIN" --config "$config" "$command" >"$actor_last_output"
  chmod 0600 "$actor_last_output"
}

actor_invoke_awaiting_retry() {
  local role="$1" command="$2" chain="$3" phase="$4" revision="$5"
  local minimum_count="$6" target_count="$7" label="$8"
  local config
  config="$(actor_runtime_config "$role")"
  local attempt attempt_output attempt_error error_text durable_count
  assert_survivor_actor_invocation_allowed "$role" "$label"
  [[ "$chain" == "lez" ]] ||
    fail "Maker-lock pending retry is only defined for LEZ observations"
  [[ "$minimum_count" =~ ^[0-9]+$ && "$target_count" =~ ^[0-9]+$ &&
     "$minimum_count" -le "$target_count" ]] ||
    fail "Maker-lock pending retry received invalid submission bounds"
  actor_last_output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-${role}.json"
  [[ ! -e "$actor_last_output" && ! -L "$actor_last_output" ]] ||
    fail "refusing to overwrite actor evidence: ${label}/${role}"
  for attempt in {1..120}; do
    if [[ "$asset_mode" == "custom_token" ]]; then
      case "$label" in
        lez-initialization-submit | lez-initialization-accepted-restart | \
          lez-custody-submit | lez-custody-accepted-restart | \
          lez-funding-submit | lez-funding-accepted-restart)
          # The official scanner verifies ancestry from start through the
          # current tip. Refresh absence/restart reads immediately before the
          # fresh actor process so a fast local devnet cannot age the window.
          # Finalized-observe labels deliberately keep their containing range.
          write_actor_configs "$(finalized_tip)" 1
          ;;
      esac
    fi
    durable_count="$(lez_successful_submission_count)"
    (( durable_count >= minimum_count && durable_count <= target_count )) ||
      fail "Maker-lock retry observed an out-of-bound durable submission count"
    attempt_output="${actor_last_output%.json}-attempt-${attempt}.json"
    attempt_error="${actor_last_output%.json}-attempt-${attempt}.stderr"
    if "$M3_POC_ACTOR_BIN" --config "$config" "$command" \
        >"$attempt_output" 2>"$attempt_error"; then
      chmod 0600 "$attempt_output" "$attempt_error"
      [[ ! -s "$attempt_error" ]] ||
        fail "${role} Maker-lock retry succeeded with unexpected stderr"
      jq -e --arg role "$role" --arg chain "$chain" --arg phase "$phase" \
          --argjson revision "$revision" '
        .schema_version == 1 and .role == $role and .command == "drive"
        and .outcome == "awaiting_observation" and .chain == $chain
        and .phase == $phase and .revision == $revision
      ' "$attempt_output" >/dev/null ||
        fail "${role} actor returned an unexpected Maker-lock pending state"
      durable_count="$(lez_successful_submission_count)"
      if [[ "$durable_count" == "$target_count" ]]; then
        mv "$attempt_output" "$actor_last_output"
        return 0
      fi
      if (( durable_count >= minimum_count && durable_count < target_count )); then
        # Awaiting-observation represents a successful typed actor state, not
        # proof that the current step was authorized for submission. Retry the
        # fresh process until the durable journal reaches the exact target;
        # every zero-effect wait keeps its attempt-specific evidence file.
        sleep 0.25
        continue
      fi
      write_lez_submission_count_diagnostic "$label" "$durable_count" "$target_count" || true
      fail "Maker-lock actor success expected ${target_count} durable submissions, observed ${durable_count}"
    fi
    chmod 0600 "$attempt_output" "$attempt_error"
    [[ ! -s "$attempt_output" ]] ||
      fail "${role} Maker-lock retry received ambiguous actor stdout"
    error_text="$(tr -d '\r\n' <"$attempt_error")"
    [[ "$error_text" == "actor chain observation is unavailable" ]] ||
      fail "${role} Maker-lock drive failed with a non-retryable typed error"
    durable_count="$(lez_successful_submission_count)"
    (( durable_count >= minimum_count && durable_count <= target_count )) ||
      fail "Maker-lock typed failure crossed its durable submission bound"
    sleep 0.25
  done
  fail "${role} Maker-lock observation remained unavailable after bounded retries"
}

actor_invoke_bitcoin_lock_awaiting_retry() {
  local role="$1" command="$2" expected="$3" require_present="$4" label="$5"
  local config
  config="$(actor_runtime_config "$role")"
  local attempt attempt_output attempt_error error_text mempool mempool_count lez_count
  local expected_lez_count
  assert_survivor_actor_invocation_allowed "$role" "$label"
  [[ "$M3_POC_DIRECTION" == "taker_sells_lez" && "$role" == "maker" &&
     "$command" == "drive" ]] ||
    fail "Bitcoin Maker-lock retry is restricted to the Maker second lock"
  [[ "$expected" =~ ^[0-9a-f]{64}$ && ! "$expected" =~ ^0+$ ]] ||
    fail "Bitcoin Maker-lock retry received an invalid exact txid"
  [[ "$require_present" == 0 || "$require_present" == 1 ]] ||
    fail "Bitcoin Maker-lock retry presence policy must be zero or one"
  case "$asset_mode" in
    native) expected_lez_count=2 ;;
    custom_token) expected_lez_count=3 ;;
    *) fail "Bitcoin Maker-lock retry received an unsupported asset mode" ;;
  esac
  actor_last_output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-${role}.json"
  [[ ! -e "$actor_last_output" && ! -L "$actor_last_output" ]] ||
    fail "refusing to overwrite actor evidence: ${label}/${role}"
  for attempt in {1..120}; do
    lez_count="$(lez_successful_submission_count)"
    [[ "$lez_count" == "$expected_lez_count" ]] ||
      fail "Bitcoin Maker-lock retry observed LEZ effect-count drift"
    mempool="$(core_rpc taker getrawmempool '[]')"
    if jq -e --arg tx "$expected" '.error == null and .result == [$tx]' \
        <<<"$mempool" >/dev/null; then
      mempool_count=1
    elif jq -e '.error == null and .result == []' <<<"$mempool" >/dev/null; then
      mempool_count=0
    else
      fail "Bitcoin Maker-lock retry observed a foreign or ambiguous mempool"
    fi
    (( require_present == 0 || mempool_count == 1 )) ||
      fail "Bitcoin Maker-lock accepted restart lost its exact mempool effect"

    attempt_output="${actor_last_output%.json}-attempt-${attempt}.json"
    attempt_error="${actor_last_output%.json}-attempt-${attempt}.stderr"
    if "$M3_POC_ACTOR_BIN" --config "$config" "$command" \
        >"$attempt_output" 2>"$attempt_error"; then
      chmod 0600 "$attempt_output" "$attempt_error"
      [[ ! -s "$attempt_error" ]] ||
        fail "${role} Bitcoin Maker-lock success emitted unexpected stderr"
      jq -e --arg role "$role" '
        .schema_version == 1 and .role == $role and .command == "drive"
        and .outcome == "awaiting_observation" and .chain == "bitcoin"
        and .phase == "taker_lock_confirmed" and .revision == 1
      ' "$attempt_output" >/dev/null ||
        fail "${role} actor returned an unexpected Bitcoin Maker-lock pending state"
      [[ "$(lez_successful_submission_count)" == "$expected_lez_count" ]] ||
        fail "Bitcoin Maker-lock success changed the LEZ effect count"
      mempool="$(core_rpc taker getrawmempool '[]')"
      jq -e --arg tx "$expected" '.error == null and .result == [$tx]' \
        <<<"$mempool" >/dev/null ||
        fail "Bitcoin Maker-lock success did not yield exactly the planned mempool tx"
      mv "$attempt_output" "$actor_last_output"
      return 0
    fi

    chmod 0600 "$attempt_output" "$attempt_error"
    [[ ! -s "$attempt_output" ]] ||
      fail "${role} Bitcoin Maker-lock retry received ambiguous actor stdout"
    error_text="$(tr -d '\r\n' <"$attempt_error")"
    [[ "$error_text" == "actor chain observation is unavailable" ]] ||
      fail "${role} Bitcoin Maker-lock drive failed with a non-retryable typed error"
    [[ "$(lez_successful_submission_count)" == "$expected_lez_count" ]] ||
      fail "Bitcoin Maker-lock typed failure changed the LEZ effect count"
    mempool="$(core_rpc taker getrawmempool '[]')"
    if jq -e --arg tx "$expected" '.error == null and .result == [$tx]' \
        <<<"$mempool" >/dev/null; then
      mempool_count=1
    elif jq -e '.error == null and .result == []' <<<"$mempool" >/dev/null; then
      mempool_count=0
    else
      fail "Bitcoin Maker-lock typed failure left a foreign or ambiguous mempool"
    fi
    (( require_present == 0 || mempool_count == 1 )) ||
      fail "Bitcoin Maker-lock accepted restart lost its effect during retry"
    sleep 0.25
  done
  fail "${role} Bitcoin Maker-lock observation remained unavailable after bounded retries"
}

actor_invoke_observation_retry() {
  local role="$1" expected="$2" chain="$3" label="$4"
  local config
  config="$(actor_runtime_config "$role")"
  local attempt attempt_output attempt_error error_text
  assert_survivor_actor_invocation_allowed "$role" "$label"
  actor_last_output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-${role}.json"
  [[ ! -e "$actor_last_output" && ! -L "$actor_last_output" ]] ||
    fail "refusing to overwrite actor evidence: ${label}/${role}"
  for attempt in {1..120}; do
    attempt_output="${actor_last_output%.json}-attempt-${attempt}.json"
    attempt_error="${actor_last_output%.json}-attempt-${attempt}.stderr"
    if "$M3_POC_ACTOR_BIN" --config "$config" drive \
        >"$attempt_output" 2>"$attempt_error"; then
      chmod 0600 "$attempt_output" "$attempt_error"
      if jq -e --arg role "$role" --arg chain "$chain" \
          --argjson revision "$expected" '
        .schema_version == 1 and .role == $role and .command == "drive"
        and (.outcome == "observed_then_projected"
             or .outcome == "converged_on_existing_projection")
        and .chain == $chain and .revision == $revision
      ' "$attempt_output" >/dev/null; then
        mv "$attempt_output" "$actor_last_output"
        return 0
      fi
      jq -e --arg role "$role" --arg chain "$chain" \
        --argjson revision "$((expected - 1))" '
        .schema_version == 1 and .role == $role and .command == "drive"
        and .outcome == "awaiting_observation" and .chain == $chain
        and .revision == $revision
      ' "$attempt_output" >/dev/null ||
        fail "${role} actor returned an unexpected successful observation state"
      sleep 0.25
      continue
    fi
    chmod 0600 "$attempt_output" "$attempt_error"
    error_text="$(tr -d '\r\n' <"$attempt_error")"
    [[ "$error_text" == "actor chain observation is unavailable" ]] ||
      fail "${role} actor observation failed with a non-retryable typed error"
    sleep 0.25
  done
  fail "${role} actor observation remained unavailable after bounded read-only retries"
}

actor_invoke_recovery_retry() {
  local role="$1" expected="$2" chain="$3" label="$4"
  local config
  config="$(actor_runtime_config "$role")"
  local attempt attempt_output attempt_error error_text
  assert_survivor_actor_invocation_allowed "$role" "$label"
  actor_last_output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-${role}.json"
  [[ ! -e "$actor_last_output" && ! -L "$actor_last_output" ]] ||
    fail "refusing to overwrite actor recovery evidence: ${label}/${role}"
  for attempt in {1..120}; do
    attempt_output="${actor_last_output%.json}-attempt-${attempt}.json"
    attempt_error="${actor_last_output%.json}-attempt-${attempt}.stderr"
    if "$M3_POC_ACTOR_BIN" --config "$config" recover \
        >"$attempt_output" 2>"$attempt_error"; then
      chmod 0600 "$attempt_output" "$attempt_error"
      if jq -e --arg role "$role" --arg chain "$chain" \
          --argjson revision "$expected" '
        .schema_version == 1 and .role == $role and .command == "recover"
        and (.outcome == "observed_then_projected"
             or .outcome == "converged_on_existing_projection")
        and .chain == $chain and .revision == $revision
      ' "$attempt_output" >/dev/null; then
        mv "$attempt_output" "$actor_last_output"
        return 0
      fi
      jq -e --arg role "$role" --arg chain "$chain" \
        --argjson revision "$((expected - 1))" '
        .schema_version == 1 and .role == $role and .command == "recover"
        and .outcome == "awaiting_observation" and .chain == $chain
        and .revision == $revision
      ' "$attempt_output" >/dev/null ||
        fail "${role} actor returned an unexpected successful recovery state"
      sleep 0.25
      continue
    fi
    chmod 0600 "$attempt_output" "$attempt_error"
    error_text="$(tr -d '\r\n' <"$attempt_error")"
    [[ "$error_text" == "actor chain observation is unavailable" ]] ||
      fail "${role} actor recovery failed with a non-retryable typed error"
    sleep 0.25
  done
  fail "${role} actor recovery remained unavailable after bounded read-only retries"
}

actor_invoke_recovery_pending_retry() {
  local role="$1" predecessor="$2" chain="$3" label="$4"
  local config
  config="$(actor_runtime_config "$role")"
  local attempt attempt_output attempt_error error_text initial_count current_count
  assert_survivor_actor_invocation_allowed "$role" "$label"
  initial_count="$(lez_successful_submission_count)"
  [[ "$initial_count" =~ ^[0-9]+$ ]] ||
    fail "LEZ pending recovery submission count is invalid"
  actor_last_output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-${role}.json"
  [[ ! -e "$actor_last_output" && ! -L "$actor_last_output" ]] ||
    fail "refusing to overwrite pending recovery evidence: ${label}/${role}"
  for attempt in {1..120}; do
    attempt_output="${actor_last_output%.json}-attempt-${attempt}.json"
    attempt_error="${actor_last_output%.json}-attempt-${attempt}.stderr"
    if "$M3_POC_ACTOR_BIN" --config "$config" recover \
        >"$attempt_output" 2>"$attempt_error"; then
      chmod 0600 "$attempt_output" "$attempt_error"
      jq -e --arg role "$role" --arg chain "$chain" --argjson revision "$predecessor" '
        .schema_version == 1 and .role == $role and .command == "recover"
        and .outcome == "awaiting_observation" and .chain == $chain
        and .revision == $revision
      ' "$attempt_output" >/dev/null ||
        fail "${role} actor returned an unexpected successful pending recovery state"
      mv "$attempt_output" "$actor_last_output"
      return 0
    fi
    chmod 0600 "$attempt_output" "$attempt_error"
    error_text="$(tr -d '\r\n' <"$attempt_error")"
    [[ "$error_text" == "actor chain observation is unavailable" &&
       ! -s "$attempt_output" ]] ||
      fail "${role} actor pending recovery failed with a non-retryable typed error"
    current_count="$(lez_successful_submission_count)"
    [[ "$current_count" == "$initial_count" ]] ||
      fail "${role} actor pending recovery changed the durable LEZ submission count; refusing retry"
    sleep 0.25
  done
  fail "${role} actor pending recovery remained unavailable after bounded read-only retries"
}

assert_recovery_pending_both() {
  local chain="$1" predecessor="$2" label="$3"
  local role config output error error_text status expected_phase expected_action
  case "$predecessor" in
    1) expected_phase=taker_lock_confirmed; expected_action=observe_maker_second_lock_or_recover_taker_leg ;;
    2) expected_phase=both_legs_locked; expected_action=observe_revealing_claim ;;
    3) expected_phase=maker_leg_refunded; expected_action=recover_taker_leg ;;
    *) fail "unsupported recovery predecessor: ${predecessor}" ;;
  esac
  for role in maker taker; do
    config="$(actor_runtime_config "$role")"
    output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-${role}.json"
    error="${output%.json}.stderr"
    status="${output%.json}-after-unavailable.json"
    [[ ! -e "$output" && ! -L "$output" && ! -e "$error" && ! -L "$error" &&
       ! -e "$status" && ! -L "$status" ]] ||
      fail "refusing to overwrite pre-eligibility recovery evidence: ${label}/${role}"
    if "$M3_POC_ACTOR_BIN" --config "$config" recover >"$output" 2>"$error"; then
      chmod 0600 "$output" "$error"
      jq -e --arg role "$role" --arg chain "$chain" --argjson revision "$predecessor" '
        .schema_version == 1 and .role == $role and .command == "recover"
        and .outcome == "awaiting_observation" and .chain == $chain
        and .revision == $revision
      ' "$output" >/dev/null ||
        fail "${role} actor gained recovery authority before ${chain} eligibility"
      continue
    fi
    chmod 0600 "$output" "$error"
    error_text="$(tr -d '\r\n' <"$error")"
    [[ "$error_text" == "actor chain observation is unavailable" && ! -s "$output" ]] ||
      fail "${role} actor pre-eligibility recovery failed with a non-retryable typed error"
    "$M3_POC_ACTOR_BIN" --config "$config" status >"$status"
    chmod 0600 "$status"
    jq -e --arg role "$role" --arg phase "$expected_phase" --arg action "$expected_action" \
      --argjson revision "$predecessor" '
      .schema_version == 1 and .role == $role and .state == "active"
      and .revision == $revision and .phase == $phase and .next_action == $action
    ' "$status" >/dev/null ||
      fail "${role} actor state changed after retryable pre-eligibility observation"
  done
}

actor_reconcile_bitcoin_claim_submission() {
  local role="$1" peer="$2" expected_revision="$3" label="$4"
  local config
  config="$(actor_runtime_config "$role")"
  local attempt attempt_output attempt_error mempool_output error_text mempool_count
  local actor_succeeded
  assert_survivor_actor_invocation_allowed "$role" "$label"
  actor_last_output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-submit-${role}.json"
  [[ ! -e "$actor_last_output" && ! -L "$actor_last_output" ]] ||
    fail "refusing to overwrite actor evidence: ${label}-submit/${role}"
  for attempt in {1..120}; do
    attempt_output="${actor_last_output%.json}-attempt-${attempt}.json"
    attempt_error="${actor_last_output%.json}-attempt-${attempt}.stderr"
    mempool_output="${actor_last_output%.json}-attempt-${attempt}-mempool.json"
    actor_succeeded=0
    if "$M3_POC_ACTOR_BIN" --config "$config" drive \
        >"$attempt_output" 2>"$attempt_error"; then
      actor_succeeded=1
      chmod 0600 "$attempt_output" "$attempt_error"
      jq -e --arg role "$role" --argjson revision "$((expected_revision - 1))" '
        .schema_version == 1 and .role == $role and .command == "drive"
        and .outcome == "awaiting_observation" and .chain == "bitcoin"
        and .revision == $revision
      ' "$attempt_output" >/dev/null ||
        fail "${role} returned an unexpected Bitcoin claim submission state"
    else
      chmod 0600 "$attempt_output" "$attempt_error"
      error_text="$(tr -d '\r\n' <"$attempt_error")"
      [[ "$error_text" == "actor chain observation is unavailable" ]] ||
        fail "${role} Bitcoin claim reconciliation failed with a non-retryable typed error"
    fi
    core_rpc "$peer" getrawmempool '[]' >"$mempool_output" ||
      fail "${peer} could not observe the Bitcoin claim mempool"
    chmod 0600 "$mempool_output"
    jq -e '.error == null and (.result | type == "array")' \
      "$mempool_output" >/dev/null || fail "Bitcoin claim mempool response was malformed"
    mempool_count="$(jq -er '.result | length' "$mempool_output")"
    if [[ "$M3_POC_JOURNEY" == "survivor_claim" && "$role" == "taker" &&
          "$mempool_count" == 1 && "$actor_succeeded" != 1 ]]; then
      fail "survivor Bitcoin reveal became public under an ambiguous actor outcome; refusing a second revealer invocation"
    fi
    if [[ "$mempool_count" == "1" && "$actor_succeeded" == "1" ]]; then
      bitcoin_claim_tx="$(jq -er '.result[0]' "$mempool_output")"
      [[ "$bitcoin_claim_tx" =~ ^[0-9a-f]{64}$ ]] ||
        fail "counterparty observed an invalid actor-owned Bitcoin claim ID"
      mv "$attempt_output" "$actor_last_output"
      return 0
    fi
    [[ "$mempool_count" == "0" || "$mempool_count" == "1" ]] ||
      fail "counterparty observed multiple Bitcoin claim candidates"
    sleep 0.25
  done
  fail "prepared actor-owned Bitcoin claim did not reconcile within the bounded window"
}

activate_actors() {
  local role
  for role in maker taker; do
    actor_invoke "$role" activate activation
    jq -e --arg role "$role" '
      .schema_version == 1 and .role == $role and .command == "activate"
      and .outcome == "activated" and .was_replay == false
      and .revision == 0
    ' "$actor_last_output" >/dev/null || fail "${role} actor activation was invalid"
  done
}

project_role_to_revision() {
  local role="$1" expected="$2" chain="$3" label="$4"
  actor_invoke_observation_retry "$role" "$expected" "$chain" "$label"
  jq -e --arg role "$role" --arg chain "$chain" --argjson revision "$expected" '
    .schema_version == 1 and .role == $role and .command == "drive"
    and (.outcome == "observed_then_projected"
         or .outcome == "converged_on_existing_projection")
    and .chain == $chain and .revision == $revision
  ' "$actor_last_output" >/dev/null ||
    fail "$role did not project $chain revision $expected"
}

project_first_lock_taker_refund_to_revision() {
  local chain="$1" label="$2"
  local revision observed_phase
  actor_invoke taker status "${label}-pre-status"
  revision="$(jq -er '.revision | numbers' "$actor_last_output")"
  observed_phase="$(jq -er '.phase | strings' "$actor_last_output")"
  if [[ "$revision" != 2 || "$observed_phase" != "refunded" ]]; then
    [[ "$revision" == 1 ]] ||
      fail "taker first-lock refund projection began from unexpected revision $revision"
    actor_invoke_recovery_retry taker 2 "$chain" "${label}-project"
  fi
  actor_invoke taker status "${label}-post-status"
  jq -e '
    .schema_version == 1 and .role == "taker" and .state == "active"
    and .phase == "refunded" and .revision == 2 and .next_action == "complete"
  ' "$actor_last_output" >/dev/null ||
    fail "taker did not reach terminal first-lock refund revision two"
}

project_both_to_revision() {
  local expected="$1" chain="$2" label="$3"
  local role
  for role in maker taker; do
    actor_invoke_observation_retry "$role" "$expected" "$chain" "$label"
    jq -e --arg role "$role" --arg chain "$chain" --argjson revision "$expected" '
      .schema_version == 1 and .role == $role and .command == "drive"
      and (.outcome == "observed_then_projected"
           or .outcome == "converged_on_existing_projection")
      and .chain == $chain and .revision == $revision
    ' "$actor_last_output" >/dev/null ||
      fail "${role} did not project ${chain} revision ${expected}"
  done
}

project_both_refunds_to_revision() {
  local expected="$1" chain="$2" phase="$3" label="$4"
  local role revision observed_phase next_action
  if [[ "$expected" == 3 ]]; then next_action=recover_taker_leg; else next_action=complete; fi
  for role in maker taker; do
    actor_invoke "$role" status "${label}-pre-status"
    revision="$(jq -er '.revision | numbers' "$actor_last_output")"
    observed_phase="$(jq -er '.phase | strings' "$actor_last_output")"
    if [[ "$revision" == "$expected" && "$observed_phase" == "$phase" ]]; then
      continue
    fi
    [[ "$revision" == "$((expected - 1))" ]] ||
      fail "${role} refund projection began from unexpected revision ${revision}"
    actor_invoke_recovery_retry "$role" "$expected" "$chain" "${label}-project"
  done
  for role in maker taker; do
    actor_invoke "$role" status "${label}-post-status"
    jq -e --arg role "$role" --arg phase "$phase" --arg next "$next_action" \
      --argjson revision "$expected" '
      .schema_version == 1 and .role == $role and .state == "active"
      and .phase == $phase and .revision == $revision and .next_action == $next
    ' "$actor_last_output" >/dev/null ||
      fail "${role} durable refund status did not reach ${phase} revision ${expected}"
  done
}

capture_both_statuses() {
  local expected="$1" label="$2"
  local role
  for role in maker taker; do
    actor_invoke "$role" status "$label"
    jq -e --arg role "$role" --argjson revision "$expected" '
      .schema_version == 1 and .role == $role and .state == "active"
      and .revision == $revision
    ' "$actor_last_output" >/dev/null ||
      fail "${role} durable status did not reach revision ${expected}"
  done
}

lez_initialization_tx=""
lez_custody_tx=""
lez_funding_tx=""
lez_claim_tx=""
lez_refund_tx=""
submit_lez_transaction_once() {
  local role="$1" member="$2" label="$3" start_height="$4"
  local request output expected returned
  [[ "$M3_POC_DIRECTION" == "taker_sells_lez" && "$role" == "taker" ]] ||
    fail "external LEZ submission is reserved for the Taker first-lock pair"
  [[ "$member" == "initialization" || "$member" == "funding" ]] ||
    fail "external LEZ first-lock member is invalid"
  request="${M3_POC_DIRECTION_ROOT}/${label}-submit-request.json"
  output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-submission.json"
  expected="$(jq -er --arg member "$member" '.[$member].transaction_id' \
    "$final_prepared_escrow")"
  jq -n --arg run "$M3_POC_RUN_ID" --arg request "$(new_request_id)" \
    --arg role "$role" --arg member "$member" \
    --slurpfile runtime "${M3_POC_DIRECTION_ROOT}/sidecars/final/${role}/runtime.json" \
    --slurpfile prepared "$final_prepared_escrow" '
    {context:{schema_version:1,run_id:$run,request_id:$request,sidecar_role:$role},
     runtime:$runtime[0],transaction:$prepared[0][$member]}
  ' >"$request"
  chmod 0600 "$request"
  operator_call final "$role" submit-transaction "$request" "$output"
  returned="$(jq -er '.transaction_id' "$output")"
  [[ "$returned" == "$expected" ]] || fail "${label} submission returned a different ID"
  jq -e --arg role "$role" '
    .context.sidecar_role == $role
    and (.outcome == "accepted" or .outcome == "already_known")
  ' "$output" >/dev/null || fail "${label} was not accepted by the exact sidecar"
  prove_lez_finalized_transaction "$label" "$expected" "$start_height"
}

asset_prepared_transaction_id() {
  local step="$1"
  jq -er --arg step "$step" '
    [.effects[] | select(.step == $step) | .transaction.transaction_id] |
    if length == 1 then .[0] else error("exactly one prepared asset effect required") end
  ' "$final_prepared_escrow"
}

submit_lez_asset_transaction_once() {
  local role="$1" step="$2" label="$3" start_height="$4"
  local request output expected returned
  [[ "$asset_mode" == "custom_token" && "$M3_POC_DIRECTION" == "taker_sells_lez" &&
     "$role" == "taker" ]] ||
    fail "external LEZ asset submission is reserved for the custom-token Taker first lock"
  [[ "$step" == "initialize_witnessed" || "$step" == "create_custody_ata" ||
     "$step" == "fund" ]] || fail "external LEZ asset first-lock step is invalid"
  request="${M3_POC_DIRECTION_ROOT}/${label}-submit-request.json"
  output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-submission.json"
  expected="$(asset_prepared_transaction_id "$step")"
  jq -n --arg run "$M3_POC_RUN_ID" --arg request "$(new_request_id)" \
    --arg role "$role" --arg step "$step" \
    --slurpfile runtime "${M3_POC_DIRECTION_ROOT}/sidecars/final/${role}/runtime.json" \
    --slurpfile prepared "$final_prepared_escrow" '
    {context:{schema_version:1,run_id:$run,request_id:$request,sidecar_role:$role},
     runtime:$runtime[0],
     transaction:([ $prepared[0].effects[] | select(.step == $step) ] |
       if length == 1 then .[0].transaction else error("exactly one asset step") end)}
  ' >"$request"
  chmod 0600 "$request"
  operator_call final "$role" submit-transaction "$request" "$output"
  returned="$(jq -er '.transaction_id' "$output")"
  [[ "$returned" == "$expected" ]] || fail "${label} submission returned a different ID"
  jq -e --arg role "$role" '
    .context.sidecar_role == $role
    and (.outcome == "accepted" or .outcome == "already_known")
  ' "$output" >/dev/null || fail "${label} was not accepted by the exact sidecar"
  prove_lez_finalized_transaction "$label" "$expected" "$start_height"
}

lez_lock_window_start=0
lez_lock_window_blocks=0
submit_taker_lez_first_lock_pair() {
  local initial_start funding_start
  [[ "$M3_POC_DIRECTION" == "taker_sells_lez" ]] ||
    fail "external LEZ lock submission is reserved for the Taker first lock"
  initial_start="$(finalized_tip)"
  submit_lez_transaction_once taker initialization lez-initialization "$initial_start"
  lez_initialization_tx="$(jq -er '.initialization.transaction_id' "$final_prepared_escrow")"
  funding_start="$lez_proved_tip"
  submit_lez_transaction_once taker funding lez-funding "$funding_start"
  lez_funding_tx="$(jq -er '.funding.transaction_id' "$final_prepared_escrow")"
  lez_lock_window_start=$((initial_start + 1))
  lez_lock_window_blocks=$((lez_proved_tip - initial_start))
  (( lez_lock_window_blocks >= 1 && lez_lock_window_blocks <= 4096 )) ||
    fail "finalized LEZ funding window is out of bounds"
  write_actor_configs "$lez_lock_window_start" "$lez_lock_window_blocks"
}

submit_taker_lez_asset_first_lock() {
  local initial_start custody_start funding_start
  [[ "$asset_mode" == "custom_token" && "$M3_POC_DIRECTION" == "taker_sells_lez" ]] ||
    fail "external LEZ asset lock submission is reserved for the Taker first lock"
  initial_start="$(finalized_tip)"
  submit_lez_asset_transaction_once taker initialize_witnessed lez-initialization "$initial_start"
  lez_initialization_tx="$(asset_prepared_transaction_id initialize_witnessed)"
  custody_start="$lez_proved_tip"
  submit_lez_asset_transaction_once taker create_custody_ata lez-custody "$custody_start"
  lez_custody_tx="$(asset_prepared_transaction_id create_custody_ata)"
  funding_start="$lez_proved_tip"
  submit_lez_asset_transaction_once taker fund lez-funding "$funding_start"
  lez_funding_tx="$(asset_prepared_transaction_id fund)"
  lez_lock_window_start=$((initial_start + 1))
  lez_lock_window_blocks=$((lez_proved_tip - initial_start))
  (( lez_lock_window_blocks >= 1 && lez_lock_window_blocks <= 4096 )) ||
    fail "finalized LEZ asset funding window is out of bounds"
  [[ "$(lez_successful_submission_count)" == 3 ]] ||
    fail "custom-token Taker first lock did not submit exactly three durable effects"
  write_actor_configs "$lez_lock_window_start" "$lez_lock_window_blocks"
}

assert_lez_pair_inside_actor_window() {
  local initialization_finality funding_finality initialization_block funding_block
  local window_end evidence
  initialization_finality="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-lez-initialization-finality.json"
  funding_finality="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-lez-funding-finality.json"
  evidence="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-lez-maker-lock-actor-window.json"
  [[ -f "$initialization_finality" && ! -L "$initialization_finality" &&
     -f "$funding_finality" && ! -L "$funding_finality" ]] ||
    fail "LEZ Maker lock finality evidence is unavailable"
  [[ ! -e "$evidence" && ! -L "$evidence" ]] ||
    fail "refusing to overwrite LEZ Maker lock actor-window evidence"
  initialization_block="$(jq -er '.containing_block_id | numbers' "$initialization_finality")"
  funding_block="$(jq -er '.containing_block_id | numbers' "$funding_finality")"
  window_end=$((lez_lock_window_start + lez_lock_window_blocks - 1))
  (( window_end == lez_proved_tip &&
     initialization_block >= lez_lock_window_start &&
     initialization_block <= window_end &&
     funding_block >= lez_lock_window_start &&
     funding_block <= window_end )) ||
    fail "final LEZ actor window does not contain both Maker lock transactions"
  jq -n --arg direction "$M3_POC_DIRECTION" \
    --argjson start "$lez_lock_window_start" --argjson blocks "$lez_lock_window_blocks" \
    --argjson end "$window_end" --argjson tip "$lez_proved_tip" \
    --argjson initialization "$initialization_block" --argjson funding "$funding_block" '
    {schema_version:1,direction:$direction,
     discovery_window:{start_height:$start,max_blocks:$blocks,end_height:$end},
     finalized_tip_height:$tip,
     containing_blocks:{initialization:$initialization,funding:$funding},
     inclusive_window_reaches_tip:($end == $tip),
     initialization_and_funding_inside_window:true}
  ' >"$evidence"
  chmod 0600 "$evidence"
}

submit_actor_maker_lez_second_lock_pair() {
  local initial_start funding_start before_count after_count initialization_window_blocks
  [[ "$M3_POC_DIRECTION" == "taker_sells_foreign" ]] ||
    fail "actor-owned LEZ Maker lock is only valid when the Taker sells foreign"
  initial_start="$(finalized_tip)"
  before_count="$(lez_successful_submission_count)"
  [[ "$before_count" == 0 ]] ||
    fail "LEZ Maker second lock began with an unexpected durable submission count"
  lez_initialization_tx="$(jq -er '.initialization.transaction_id' "$final_prepared_escrow")"
  lez_funding_tx="$(jq -er '.funding.transaction_id' "$final_prepared_escrow")"

  actor_invoke_awaiting_retry maker drive lez taker_lock_confirmed 1 0 1 \
    lez-maker-initialization-submit
  jq -e '
    .schema_version == 1 and .role == "maker" and .command == "drive"
    and .outcome == "awaiting_observation" and .chain == "lez"
    and .phase == "taker_lock_confirmed" and .revision == 1
  ' "$actor_last_output" >/dev/null ||
    fail "Maker actor did not submit LEZ initialization from revision one"
  after_count="$(lez_successful_submission_count)"
  [[ "$after_count" == 1 ]] ||
    fail "Maker actor did not add exactly one LEZ initialization submission"

  actor_invoke_awaiting_retry maker drive lez taker_lock_confirmed 1 1 1 \
    lez-maker-initialization-accepted-restart
  jq -e '
    .schema_version == 1 and .role == "maker" and .command == "drive"
    and .outcome == "awaiting_observation" and .chain == "lez"
    and .phase == "taker_lock_confirmed" and .revision == 1
  ' "$actor_last_output" >/dev/null ||
    fail "fresh Maker restart changed accepted LEZ initialization state"
  [[ "$(lez_successful_submission_count)" == "$after_count" ]] ||
    fail "fresh Maker restart resubmitted LEZ initialization"
  prove_lez_finalized_transaction lez-initialization "$lez_initialization_tx" "$initial_start"

  initialization_window_blocks=$((lez_proved_tip - initial_start))
  (( initialization_window_blocks >= 1 && initialization_window_blocks <= 4096 )) ||
    fail "finalized LEZ initialization window is out of bounds"
  write_actor_configs "$((initial_start + 1))" "$initialization_window_blocks"
  actor_invoke_awaiting_retry maker drive lez taker_lock_confirmed 1 1 1 \
    lez-maker-initialization-finalized-observe
  jq -e '
    .schema_version == 1 and .role == "maker" and .command == "drive"
    and .outcome == "awaiting_observation" and .chain == "lez"
    and .phase == "taker_lock_confirmed" and .revision == 1
  ' "$actor_last_output" >/dev/null ||
    fail "Maker actor did not accept finalized LEZ initialization at revision one"
  [[ "$(lez_successful_submission_count)" == 1 ]] ||
    fail "finalized LEZ initialization observation changed the submission count"

  funding_start="$lez_proved_tip"
  actor_invoke_awaiting_retry maker drive lez taker_lock_confirmed 1 1 2 \
    lez-maker-funding-submit
  jq -e '
    .schema_version == 1 and .role == "maker" and .command == "drive"
    and .outcome == "awaiting_observation" and .chain == "lez"
    and .phase == "taker_lock_confirmed" and .revision == 1
  ' "$actor_last_output" >/dev/null ||
    fail "Maker actor did not submit LEZ funding from revision one"
  after_count="$(lez_successful_submission_count)"
  [[ "$after_count" == 2 ]] ||
    fail "Maker actor did not add exactly one LEZ funding submission"

  actor_invoke_awaiting_retry maker drive lez taker_lock_confirmed 1 2 2 \
    lez-maker-funding-accepted-restart
  jq -e '
    .schema_version == 1 and .role == "maker" and .command == "drive"
    and .outcome == "awaiting_observation" and .chain == "lez"
    and .phase == "taker_lock_confirmed" and .revision == 1
  ' "$actor_last_output" >/dev/null ||
    fail "fresh Maker restart changed accepted LEZ funding state"
  [[ "$(lez_successful_submission_count)" == "$after_count" ]] ||
    fail "fresh Maker restart resubmitted LEZ funding"
  prove_lez_finalized_transaction lez-funding "$lez_funding_tx" "$funding_start"

  lez_lock_window_start=$((initial_start + 1))
  lez_lock_window_blocks=$((lez_proved_tip - initial_start))
  (( lez_lock_window_blocks >= 1 && lez_lock_window_blocks <= 4096 )) ||
    fail "finalized actor-owned LEZ funding window is out of bounds"
  write_actor_configs "$lez_lock_window_start" "$lez_lock_window_blocks"
  assert_lez_pair_inside_actor_window
}

submit_actor_maker_lez_asset_second_lock() {
  local initial_start step_start before_count after_count window_blocks index step label tx
  local -a steps=(initialize_witnessed create_custody_ata fund)
  local -a labels=(lez-initialization lez-custody lez-funding)
  [[ "$asset_mode" == "custom_token" && "$M3_POC_DIRECTION" == "taker_sells_foreign" ]] ||
    fail "actor-owned LEZ asset Maker lock is only valid for the custom-token forward direction"
  initial_start="$(finalized_tip)"
  before_count="$(lez_successful_submission_count)"
  [[ "$before_count" == 0 ]] ||
    fail "LEZ asset Maker second lock began with an unexpected durable submission count"
  for index in 0 1 2; do
    step="${steps[$index]}"
    label="${labels[$index]}"
    step_start="$(finalized_tip)"
    # Accepted predecessors are skipped from the durable journal. Give the
    # next prepared effect a fresh fixed one-block absence window so the
    # official scanner does not have to chase an ever-growing ancestry while
    # the local devnet advances. Finalized acceptance below uses the exact
    # containing range, and the last step restores the complete-plan range.
    write_actor_configs "$step_start" 1
    tx="$(asset_prepared_transaction_id "$step")"
    actor_invoke_awaiting_retry maker drive lez taker_lock_confirmed 1 \
      "$index" "$((index + 1))" "${label}-submit"
    jq -e '
      .schema_version == 1 and .role == "maker" and .command == "drive"
      and .outcome == "awaiting_observation" and .chain == "lez"
      and .phase == "taker_lock_confirmed" and .revision == 1
    ' "$actor_last_output" >/dev/null ||
      fail "Maker actor did not submit ${step} from revision one"
    after_count="$(lez_successful_submission_count)"
    [[ "$after_count" == "$((index + 1))" ]] ||
      fail "Maker actor did not add exactly one ${step} submission"
    actor_invoke_awaiting_retry maker drive lez taker_lock_confirmed 1 \
      "$after_count" "$after_count" "${label}-accepted-restart"
    [[ "$(lez_successful_submission_count)" == "$after_count" ]] ||
      fail "fresh Maker restart resubmitted ${step}"
    prove_lez_finalized_transaction "$label" "$tx" "$step_start"
    case "$step" in
      initialize_witnessed) lez_initialization_tx="$tx" ;;
      create_custody_ata) lez_custody_tx="$tx" ;;
      fund) lez_funding_tx="$tx" ;;
    esac
    window_blocks=$((lez_proved_tip - step_start))
    (( window_blocks >= 1 && window_blocks <= 4096 )) ||
      fail "finalized actor-owned LEZ asset window is out of bounds"
    if (( index < 2 )); then
      write_actor_configs "$((step_start + 1))" "$window_blocks"
      actor_invoke_awaiting_retry maker drive lez taker_lock_confirmed 1 \
        "$after_count" "$after_count" "${label}-finalized-observe"
      [[ "$(lez_successful_submission_count)" == "$after_count" ]] ||
        fail "finalized ${step} observation changed the submission count"
    else
      window_blocks=$((lez_proved_tip - initial_start))
      (( window_blocks >= 1 && window_blocks <= 4096 )) ||
        fail "complete actor-owned LEZ asset window is out of bounds"
      write_actor_configs "$((initial_start + 1))" "$window_blocks"
    fi
  done
  lez_lock_window_start=$((initial_start + 1))
  lez_lock_window_blocks=$((lez_proved_tip - initial_start))
  [[ "$(lez_successful_submission_count)" == 3 ]] ||
    fail "custom-token Maker second lock did not submit exactly three durable effects"
}

submit_actor_bitcoin_claim_effect() {
  local owner="$1" expected_revision="$2" label="$3"
  local peer pre_mempool
  case "$owner" in maker) peer=taker ;; taker) peer=maker ;; esac
  pre_mempool="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-pre-mempool.json"
  core_rpc "$owner" getrawmempool '[]' >"$pre_mempool" ||
    fail "${owner} could not inspect the pre-claim mempool"
  chmod 0600 "$pre_mempool"
  jq -e '.error == null and .result == []' "$pre_mempool" >/dev/null ||
    fail "Bitcoin claim began with a nonempty or malformed mempool"
  actor_reconcile_bitcoin_claim_submission "$owner" "$peer" "$expected_revision" "$label"
  if [[ "$M3_POC_JOURNEY" == "survivor_claim" && "$owner" == "taker" ]]; then
    survivor_taker_absence_guard=1
  fi
  mine_one_core_block
  wait_core_confirmed "$bitcoin_claim_tx" "$peer" "$label"
}

submit_actor_bitcoin_claim() {
  local owner="$1" expected_revision="$2" label="$3"
  submit_actor_bitcoin_claim_effect "$owner" "$expected_revision" "$label"
  project_both_to_revision "$expected_revision" bitcoin "${label}-project"
}

wait_for_host_recovery_bound() {
  local bound_seconds="$1" label="$2"
  local now
  for _ in {1..1200}; do
    now="$(date -u +%s)"
    if (( now >= bound_seconds )); then
      jq -n --arg label "$label" --argjson bound "$bound_seconds" --argjson observed "$now" '
        {schema_version:1,label:$label,bound_unix_seconds:$bound,
         observed_unix_seconds:$observed,bound_satisfied:true}
      ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-wall-clock.json"
      chmod 0600 "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-wall-clock.json"
      return 0
    fi
    sleep 1
  done
  fail "host wall clock did not reach the signed ${label} recovery bound"
}

mine_core_to_refund_eligibility() {
  local label="$1" spec="${M3_POC_DIRECTION_ROOT}/stage-two.json"
  local target current blocks address mined tip_hash tip
  target="$(( $(jq -er '.recovery.bitcoin_refund_height | numbers' "$spec") - 1 ))"
  current="$(core_admin getblockcount)"
  [[ "$current" =~ ^[0-9]+$ && "$target" =~ ^[0-9]+$ && "$current" -le "$target" ]] ||
    fail "Core tip is already beyond the signed refund eligibility boundary"
  blocks=$((target - current))
  address="$(file_value "$M3_POC_BITCOIN_FUNDING_CREDENTIALS" BITCOIN_CORE_FUNDING_ADDRESS)"
  if (( blocks > 0 )); then
    mined="$(core_admin generatetoaddress "$blocks" "$address")"
    [[ "$(jq -er 'length' <<<"$mined")" == "$blocks" ]] ||
      fail "Core did not mine the exact refund maturity distance"
  fi
  tip="$(core_admin getblockcount)"
  [[ "$tip" == "$target" ]] || fail "Core refund eligibility tip drifted"
  tip_hash="$(core_admin getblockhash "$tip")"
  jq -n --arg label "$label" --arg hash "$tip_hash" \
    --argjson previous "$current" --argjson mined "$blocks" --argjson tip "$tip" '
    {schema_version:1,label:$label,previous_tip:$previous,mined_blocks:$mined,
     eligible_tip:$tip,eligible_tip_hash:$hash,
     next_block_is_signed_refund_height:true}
  ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-maturity.json"
  chmod 0600 "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-maturity.json"
}

submit_actor_bitcoin_refund() {
  local owner="$1" expected_revision="$2" label="$3"
  local peer predecessor mempool_output restart_mempool confirmed spender spec public_spec
  local funding_tx funding_vout delay script control destination output_value expected_height phase
  predecessor=$((expected_revision - 1))
  case "$owner" in maker) peer=taker ;; taker) peer=maker ;; *) fail "invalid Bitcoin refund owner" ;; esac
  spec="${M3_POC_DIRECTION_ROOT}/stage-two.json"
  public_spec="${M3_POC_DIRECTION_ROOT}/fixture/public-spec.json"
  mempool_output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-pre-mempool.json"
  core_rpc "$peer" getrawmempool '[]' >"$mempool_output"
  chmod 0600 "$mempool_output"
  jq -e '.error == null and .result == []' "$mempool_output" >/dev/null ||
    fail "Bitcoin refund began with a nonempty mempool"

  actor_invoke "$owner" recover "${label}-submit"
  jq -e --arg role "$owner" --argjson revision "$predecessor" '
    .schema_version == 1 and .role == $role and .command == "recover"
    and .outcome == "awaiting_observation" and .chain == "bitcoin"
    and .revision == $revision
  ' "$actor_last_output" >/dev/null || fail "Bitcoin refund owner did not submit from its predecessor"
  restart_mempool="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-accepted-mempool.json"
  core_rpc "$peer" getrawmempool '[]' >"$restart_mempool"
  chmod 0600 "$restart_mempool"
  jq -e '.error == null and (.result | length) == 1' "$restart_mempool" >/dev/null ||
    fail "counterparty did not observe exactly one Bitcoin refund"
  bitcoin_refund_tx="$(jq -er '.result[0]' "$restart_mempool")"

  actor_invoke "$owner" recover "${label}-accepted-restart-owner"
  jq -e --arg role "$owner" --argjson revision "$predecessor" '
    .schema_version == 1 and .role == $role and .command == "recover"
    and .outcome == "awaiting_observation" and .chain == "bitcoin"
    and .revision == $revision
  ' "$actor_last_output" >/dev/null || fail "accepted Bitcoin refund restart changed owner state"
  if [[ "$expected_revision" != 2 ]]; then
    actor_invoke "$peer" recover "${label}-accepted-restart-peer"
    jq -e --arg role "$peer" --argjson revision "$predecessor" '
      .schema_version == 1 and .role == $role and .command == "recover"
      and .outcome == "awaiting_observation" and .chain == "bitcoin"
      and .revision == $revision
    ' "$actor_last_output" >/dev/null || fail "peer projected an unconfirmed Bitcoin refund"
  fi
  core_rpc "$owner" getrawmempool '[]' | jq -e --arg tx "$bitcoin_refund_tx" \
    '.error == null and .result == [$tx]' >/dev/null ||
    fail "Bitcoin refund restart changed the exact mempool effect"

  mine_one_core_block
  expected_height="$(jq -er '.recovery.bitcoin_refund_height | numbers' "$spec")"
  [[ "$core_mined_block_height" == "$expected_height" ]] ||
    fail "Bitcoin refund was not mined at the countersigned height"
  wait_core_confirmed "$bitcoin_refund_tx" "$peer" "$label"
  confirmed="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-confirmed.json"
  bitcoin_refund_wtxid="$(jq -er '.result.hash' "$confirmed")"
  funding_tx="$(jq -er '.bitcoin.funding_transaction_id' "$spec")"
  funding_vout="$(jq -er '.bitcoin.funding_output_index | numbers' "$spec")"
  output_value="$(jq -er '.bitcoin.claim_value_sat | numbers' "$spec")"
  delay="$(jq -er '.refund_csv_blocks | numbers' "$public_spec")"
  script="$(jq -er --arg direction "$M3_POC_DIRECTION" '.contracts[$direction].refund_script' "$public_spec")"
  control="$(jq -er --arg direction "$M3_POC_DIRECTION" '.contracts[$direction].refund_control_block' "$public_spec")"
  destination="$(jq -er --arg owner "$owner" '.[$owner].bitcoin_claim_destination_script_pubkey' "$public_spec")"
  jq -e --arg tx "$bitcoin_refund_tx" --arg funding "$funding_tx" \
    --arg script "$script" --arg control "$control" --arg destination "$destination" \
    --argjson vout "$funding_vout" --argjson delay "$delay" --argjson value "$output_value" '
    .error == null and .result.txid == $tx and (.result.hash | test("^[0-9a-f]{64}$"))
    and (.result.confirmations | numbers) >= 1
    and (.result.vin | length) == 1 and .result.vin[0].txid == $funding
    and .result.vin[0].vout == $vout and .result.vin[0].sequence == $delay
    and (.result.vin[0].txinwitness | length) == 3
    and (.result.vin[0].txinwitness[0] | test("^[0-9a-f]{128}$"))
    and .result.vin[0].txinwitness[1] == $script
    and .result.vin[0].txinwitness[2] == $control
    and (.result.vout | length) == 1
    and ((.result.vout[0].value * 100000000 | round) == $value)
    and .result.vout[0].scriptPubKey.hex == $destination
  ' "$confirmed" >/dev/null || fail "confirmed Bitcoin refund violates the countersigned transaction"
  spender="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-spender.json"
  core_rpc "$peer" gettxspendingprevout \
    "[[{\"txid\":\"${funding_tx}\",\"vout\":${funding_vout}}],{\"mempool_only\":false,\"return_spending_tx\":true}]" \
    >"$spender"
  chmod 0600 "$spender"
  jq -e --arg tx "$bitcoin_refund_tx" --arg block "$core_mined_block_hash" '
    .error == null and (.result | length) == 1
    and .result[0].spendingtxid == $tx and .result[0].blockhash == $block
    and (.result[0].spendingtx | test("^[0-9a-f]+$"))
  ' "$spender" >/dev/null || fail "Core did not return the exact finalized refund spender"
  if [[ "$expected_revision" == 3 ]]; then phase=maker_leg_refunded; else phase=refunded; fi
  jq -n --arg direction "$M3_POC_DIRECTION" --arg owner "$owner" \
    --arg tx "$bitcoin_refund_tx" --arg wtxid "$bitcoin_refund_wtxid" \
    --arg block "$core_mined_block_hash" --argjson height "$core_mined_block_height" \
    --argjson signed_height "$expected_height" --argjson sequence "$delay" \
    --argjson revision "$expected_revision" '
    {schema_version:1,direction:$direction,owner:$owner,transaction_id:$tx,
     witness_transaction_id:$wtxid,containing_block_hash:$block,
     containing_block_height:$height,signed_refund_height:$signed_height,
     signed_csv_sequence:$sequence,projected_revision:$revision,
     exact_countersigned_transaction:true,canonical_three_item_witness:true}
  ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-refund.json"
  chmod 0600 "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-refund.json"
  if [[ "$expected_revision" == 2 ]]; then
    project_first_lock_taker_refund_to_revision bitcoin "$label"
  else
    project_both_refunds_to_revision "$expected_revision" bitcoin "$phase" "$label"
  fi
}

actor_lez_claim_transaction_id() {
  local owner="$1" journal
  journal="${M3_POC_SECURE_STATE_ROOT}/sidecars/final/${owner}/bridge-requests.v1.json"
  [[ -f "$journal" && ! -L "$journal" ]] || fail "${owner} sidecar request journal is unavailable"
  jq -er --arg initialization "$lez_initialization_tx" --arg custody "$lez_custody_tx" \
    --arg funding "$lez_funding_tx" '
    [.entries[] |
      select(.method == "lez_bridge.v1.submit_transaction"
             and .outcome.kind == "success") |
      .outcome.value.transaction_id |
      select(. != $initialization and . != $custody and . != $funding)] |
    unique | select(length == 1) | .[0]
  ' "$journal"
}

lez_successful_submission_count() {
  local role journal total=0
  for role in maker taker; do
    journal="${M3_POC_SECURE_STATE_ROOT}/sidecars/final/${role}/bridge-requests.v1.json"
    [[ -f "$journal" && ! -L "$journal" ]] || fail "${role} sidecar request journal is unavailable"
    total=$((total + $(jq -er '
      [.entries[] |
        select(.method == "lez_bridge.v1.submit_transaction"
               and .outcome.kind == "success")] | length
    ' "$journal")))
  done
  printf '%s\n' "$total"
}

actor_lez_refund_transaction_id() {
  local owner="$1" journal
  journal="${M3_POC_SECURE_STATE_ROOT}/sidecars/final/${owner}/bridge-requests.v1.json"
  [[ -f "$journal" && ! -L "$journal" ]] || fail "${owner} sidecar request journal is unavailable"
  jq -er --arg initialization "$lez_initialization_tx" --arg custody "$lez_custody_tx" \
    --arg funding "$lez_funding_tx" '
    [.entries[] |
      select(.method == "lez_bridge.v1.submit_transaction"
             and .outcome.kind == "success") |
      .outcome.value.transaction_id |
      select(. != $initialization and . != $custody and . != $funding)] |
    unique | select(length == 1) | .[0]
  ' "$journal"
}

wait_lez_finalized_deadline() {
  local deadline_ms="$1" label="$2"
  local tip response timestamp block_hash
  for _ in {1..1200}; do
    tip="$(finalized_tip)"
    if response="$(rpc "$M3_POC_LEZ_INDEXER_RPC_URL" \
        "$(jq -cn --argjson height "$tip" \
          '{jsonrpc:"2.0",id:1,method:"getBlockById",params:[$height]}')" 2>/dev/null)" &&
      timestamp="$(jq -er '.result.header.timestamp | numbers' <<<"$response" 2>/dev/null)" &&
      jq -e '.error == null and .result.bedrock_status == "Finalized"' <<<"$response" >/dev/null 2>&1 &&
      (( timestamp >= deadline_ms )); then
      block_hash="$(jq -er '.result.header.hash | strings' <<<"$response")"
      jq -n --arg label "$label" --arg hash "$block_hash" \
        --argjson deadline "$deadline_ms" --argjson timestamp "$timestamp" --argjson tip "$tip" '
        {schema_version:1,label:$label,deadline_ms:$deadline,
         finalized_tip:{height:$tip,block_hash:$hash,timestamp_ms:$timestamp},
         deadline_satisfied:true}
      ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-deadline.json"
      chmod 0600 "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-deadline.json"
      return 0
    fi
    sleep 1
  done
  fail "LEZ finalized clock did not reach the countersigned refund deadline"
}

assert_lez_first_lock_refund_preprojection_taker() {
  local label="$1"
  [[ "$(lez_successful_submission_count)" == 3 ]] ||
    fail "LEZ first-lock refund preprojection requires exactly three durable effects"
  actor_invoke taker status "${label}-post-submit-pre-finality"
  jq -e '
    .schema_version == 1 and .role == "taker" and .state == "active"
    and .revision == 1 and .phase == "taker_lock_confirmed"
    and .next_action == "observe_maker_second_lock_or_recover_taker_leg"
  ' "$actor_last_output" >/dev/null ||
    fail "taker changed lifecycle state before LEZ first-lock refund finality"
  [[ "$(lez_successful_submission_count)" == 3 ]] ||
    fail "LEZ taker preprojection status changed the durable effect count"
}

assert_lez_refund_preprojection_status_both() {
  local predecessor="$1" label="$2"
  local role expected_phase expected_action expected_count=3
  if [[ "$asset_mode" == "custom_token" ]]; then expected_count=4; fi
  [[ "$(lez_successful_submission_count)" == "$expected_count" ]] ||
    fail "LEZ preprojection proof has an unexpected durable submission count"
  case "$predecessor" in
    1) expected_phase=taker_lock_confirmed; expected_action=observe_maker_second_lock_or_recover_taker_leg ;;
    2) expected_phase=both_legs_locked; expected_action=observe_revealing_claim ;;
    3) expected_phase=maker_leg_refunded; expected_action=recover_taker_leg ;;
    *) fail "unsupported LEZ refund preprojection predecessor: ${predecessor}" ;;
  esac
  for role in maker taker; do
    actor_invoke "$role" status "${label}-post-submit-pre-finality"
    jq -e --arg role "$role" --arg phase "$expected_phase" --arg action "$expected_action" \
      --argjson revision "$predecessor" '
      .schema_version == 1 and .role == $role and .state == "active"
      and .revision == $revision and .phase == $phase and .next_action == $action
    ' "$actor_last_output" >/dev/null ||
      fail "${role} actor changed lifecycle state before LEZ refund finality"
  done
  [[ "$(lez_successful_submission_count)" == "$expected_count" ]] ||
    fail "LEZ preprojection status proof changed the durable submission count"
}

submit_actor_lez_refund() {
  local owner="$1" expected_revision="$2" label="$3"
  local peer predecessor refund_start before_count after_count finality block_height block_file
  local block_timestamp deadline window_blocks phase expected_before=2 expected_after=3
  predecessor=$((expected_revision - 1))
  case "$owner" in maker) peer=taker ;; taker) peer=maker ;; *) fail "invalid LEZ refund owner" ;; esac
  refund_start="$(finalized_tip)"
  # The lock/claim discovery window may end before the signed refund deadline.
  # Pin the pre-submit reconciliation to the fresh finalized baseline: the
  # exact refund cannot have been accepted before this post-deadline block,
  # and the actor's durable one-attempt journal still guards submission.
  write_actor_configs "$refund_start" 1
  before_count="$(lez_successful_submission_count)"
  if [[ "$asset_mode" == "custom_token" ]]; then
    expected_before=3
    expected_after=4
  fi
  [[ "$before_count" == "$expected_before" ]] ||
    fail "LEZ refund began with an unexpected submission count"

  actor_invoke_recovery_pending_retry "$owner" "$predecessor" lez "${label}-submit"
  jq -e --arg role "$owner" --argjson revision "$predecessor" '
    .schema_version == 1 and .role == $role and .command == "recover"
    and .outcome == "awaiting_observation" and .chain == "lez"
    and .revision == $revision
  ' "$actor_last_output" >/dev/null || fail "LEZ refund owner did not submit from its predecessor"
  lez_refund_tx="$(actor_lez_refund_transaction_id "$owner")"
  [[ "$lez_refund_tx" =~ ^[0-9a-f]{64}$ ]] || fail "actor-owned LEZ refund ID is invalid"
  after_count="$(lez_successful_submission_count)"
  [[ "$after_count" == "$expected_after" ]] ||
    fail "LEZ refund did not add exactly one durable submission"

  if [[ "$expected_revision" == 2 ]]; then
    assert_lez_first_lock_refund_preprojection_taker "$label"
  else
    assert_lez_refund_preprojection_status_both "$predecessor" "$label"
  fi

  prove_lez_finalized_transaction "$label" "$lez_refund_tx" "$refund_start"
  finality="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-finality.json"
  block_height="$(jq -er '.containing_block_id | numbers' "$finality")"
  block_file="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-block-${block_height}.json"
  block_timestamp="$(jq -er '.result.header.timestamp | numbers' "$block_file")"
  deadline="$(jq -er '.lez_terms.refund_at_ms | numbers' "${M3_POC_DIRECTION_ROOT}/stage-two.json")"
  (( block_timestamp >= deadline )) || fail "LEZ refund containing block predates the signed deadline"
  if [[ "$expected_revision" == 2 ]]; then
    window_blocks=$((lez_proved_tip - actor_prelock_lez_tip))
    (( window_blocks >= 1 && window_blocks <= 4096 )) ||
      fail "fresh maker LEZ replay window is out of bounds"
    write_actor_configs "$((actor_prelock_lez_tip + 1))" "$window_blocks"
  else
    window_blocks=$((lez_proved_tip - refund_start))
    (( window_blocks >= 1 && window_blocks <= 4096 )) ||
      fail "finalized LEZ refund window is out of bounds"
    write_actor_configs "$((refund_start + 1))" "$window_blocks"
  fi
  if [[ "$expected_revision" == 3 ]]; then phase=maker_leg_refunded; else phase=refunded; fi
  if [[ "$expected_revision" == 2 ]]; then
    project_first_lock_taker_refund_to_revision lez "$label"
  else
    project_both_refunds_to_revision "$expected_revision" lez "$phase" "$label"
  fi
  [[ "$(lez_successful_submission_count)" == 3 ]] ||
    fail "LEZ finalized refund projection changed the durable submission count"
  jq -n --arg direction "$M3_POC_DIRECTION" --arg owner "$owner" --arg tx "$lez_refund_tx" \
    --argjson block "$block_height" --argjson timestamp "$block_timestamp" \
    --argjson deadline "$deadline" --argjson submissions "$(lez_successful_submission_count)" '
    {schema_version:1,direction:$direction,owner:$owner,transaction_id:$tx,
     containing_block_id:$block,containing_timestamp_ms:$timestamp,
     signed_refund_at_ms:$deadline,deadline_satisfied:($timestamp >= $deadline),
     durable_total_lez_submissions:$submissions,exact_finalized_projection:true,
     actor_validated_refunded_metadata_zero_custody_and_immutable_depositor:true}
  ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-refund.json"
  chmod 0600 "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-refund.json"
}

submit_actor_lez_claim_effect() {
  local owner="$1" expected_revision="$2" label="$3"
  local claim_start claim_window_blocks before_count after_count expected_before
  claim_start="$(finalized_tip)"
  if [[ "$asset_mode" == "custom_token" ]]; then expected_before=3; else expected_before=2; fi
  before_count="$(lez_successful_submission_count)"
  [[ "$before_count" == "$expected_before" ]] ||
    fail "actor-owned LEZ claim began with an unexpected durable submission count"
  actor_invoke "$owner" drive "${label}-submit"
  jq -e --arg role "$owner" --argjson revision "$((expected_revision - 1))" '
    .schema_version == 1 and .role == $role and .command == "drive"
    and .outcome == "awaiting_observation" and .chain == "lez"
    and .revision == $revision
  ' "$actor_last_output" >/dev/null || fail "${owner} did not submit the actor-owned LEZ claim"
  after_count="$(lez_successful_submission_count)"
  [[ "$after_count" == "$((expected_before + 1))" ]] ||
    fail "actor-owned LEZ claim did not add exactly one durable submission"
  lez_claim_tx="$(actor_lez_claim_transaction_id "$owner")"
  [[ "$lez_claim_tx" =~ ^[0-9a-f]{64}$ ]] ||
    fail "actor-owned LEZ claim ID is invalid"
  actor_invoke "$owner" drive "${label}-accepted-restart"
  jq -e --arg role "$owner" --argjson revision "$((expected_revision - 1))" '
    .schema_version == 1 and .role == $role and .command == "drive"
    and .outcome == "awaiting_observation" and .chain == "lez"
    and .revision == $revision
  ' "$actor_last_output" >/dev/null ||
    fail "fresh ${owner} restart changed the accepted LEZ claim state"
  [[ "$(lez_successful_submission_count)" == "$after_count" ]] ||
    fail "fresh ${owner} restart resubmitted the LEZ claim"
  if [[ "$M3_POC_JOURNEY" == "survivor_claim" && "$owner" == "taker" ]]; then
    survivor_taker_absence_guard=1
  fi
  prove_lez_finalized_transaction "$label" "$lez_claim_tx" "$claim_start"
  claim_window_blocks=$((lez_proved_tip - claim_start))
  (( claim_window_blocks >= 1 && claim_window_blocks <= 4096 )) ||
    fail "finalized LEZ claim window is out of bounds"
  write_actor_configs "$((claim_start + 1))" "$claim_window_blocks"
}

submit_actor_lez_claim() {
  local owner="$1" expected_revision="$2" label="$3"
  submit_actor_lez_claim_effect "$owner" "$expected_revision" "$label"
  project_both_to_revision "$expected_revision" lez "${label}-project"
}

write_dual_lock_gate() {
  local gate_filter="scripts/jq/m3-dual-lock-gate.jq"
  local gate_file="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-dual-lock-gate.json"
  local gate_partial="${gate_file}.partial"
  capture_both_statuses 2 dual-lock-status
  [[ -f "$gate_filter" && ! -L "$gate_filter" ]] ||
    fail "dual-lock evidence filter is missing or unsafe"
  [[ ! -e "$gate_file" && ! -L "$gate_file" && ! -e "$gate_partial" && ! -L "$gate_partial" ]] ||
    fail "dual-lock evidence output already exists"
  jq -n --arg direction "$M3_POC_DIRECTION" --arg bitcoin "$bitcoin_lock_tx" \
    --arg initialization "$lez_initialization_tx" --arg custody "$lez_custody_tx" \
    --arg funding "$lez_funding_tx" --arg asset_mode "$asset_mode" \
    --arg asset_commitment "$asset_commitment" \
    --argjson window_start "$lez_lock_window_start" \
    --argjson window_blocks "$lez_lock_window_blocks" \
    --arg opened_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    -f "$gate_filter" >"$gate_partial"
  jq -e '.schema_version == 1 and .gate == "open"' "$gate_partial" >/dev/null ||
    fail "dual-lock evidence filter emitted an invalid object"
  chmod 0600 "$gate_partial"
  mv -- "$gate_partial" "$gate_file"
}


native_observation_file=""
write_native_escrow_observation() {
  local label="$1"
  local terms="$final_terms"
  native_observation_file="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}.json"
  [[ -f "$terms" && ! -L "$terms" && ! -e "$native_observation_file" ]] ||
    fail "native escrow admission inputs are unavailable or would overwrite evidence"
  "$M3_POC_LEZ_NATIVE_ESCROW_BIN" observe \
    --sequencer-url "$M3_POC_LEZ_SEQUENCER_RPC_URL" \
    --chain-id "$M3_POC_LEZ_CHANNEL_ID" \
    --escrow-program-id "$M3_POC_LEZ_ESCROW_PROGRAM_ID" \
    --swap-id "$(jq -er '.swap_id' "$terms")" \
    --terms-hash "$(jq -er '.terms_hash' "$terms")" \
    --secret-digest "$pda_probe_secret_digest" \
    --depositor-role "$(jq -er '.depositor' "$terms")" \
    --depositor-account-id "$(jq -er '.depositor_account_id' "$terms")" \
    --claimant-role "$(jq -er '.claimant' "$terms")" \
    --claimant-account-id "$(jq -er '.claimant_account_id' "$terms")" \
    --amount "$(jq -er '.amount | strings | select(test("^[1-9][0-9]*$"))' "$terms")" \
    --refund-at-ms "$(jq -er '.refund_at_ms | numbers' "$terms")" \
    >"$native_observation_file"
  chmod 0600 "$native_observation_file"
  jq -e '
    .action == "observe"
    and .transactions == []
    and (.after.sequencer_tip | numbers) >= 0
    and (.after.tip_block_hash | test("^[0-9a-f]{64}$"))
  ' "$native_observation_file" >/dev/null ||
    fail "native escrow admission observation is malformed"
}

finalized_witnessed_funding_observation_file=""
write_finalized_witnessed_funding_observation() {
  local label="$1" depositor request output amount terms_hash start blocks funding
  local attempt attempt_request attempt_output attempt_error submissions_before submissions_after
  local successful_attempt=0 moving_tip_retries=0 retry_evidence
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign) depositor=maker ;;
    taker_sells_lez) depositor=taker ;;
    *) fail "unsupported witnessed-funding observation direction" ;;
  esac
  start="$lez_lock_window_start"
  blocks="$lez_lock_window_blocks"
  funding="$lez_funding_tx"
  [[ "$start" =~ ^[0-9]+$ && "$blocks" =~ ^[0-9]+$ &&
     "$funding" =~ ^[0-9a-f]{64}$ ]] ||
    fail "finalized witnessed-funding observation inputs are unavailable"
  amount="$(jq -er '.amount | strings | select(test("^[1-9][0-9]*$"))' "$final_terms")"
  terms_hash="$(jq -er '.terms_hash | strings | select(test("^[0-9a-f]{64}$"))' "$final_terms")"
  request="${M3_POC_DIRECTION_ROOT}/${label}-request.json"
  output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}.json"
  retry_evidence="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-retry.json"
  [[ ! -e "$request" && ! -L "$request" && ! -e "$output" && ! -L "$output" &&
     ! -e "$retry_evidence" && ! -L "$retry_evidence" ]] ||
    fail "refusing to overwrite finalized witnessed-funding evidence"
  submissions_before="$(lez_successful_submission_count)"
  for attempt in {1..120}; do
    attempt_request="${M3_POC_DIRECTION_ROOT}/${label}-attempt-${attempt}-request.json"
    attempt_output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-attempt-${attempt}.json"
    attempt_error="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-attempt-${attempt}.stderr"
    [[ ! -e "$attempt_request" && ! -L "$attempt_request" &&
       ! -e "$attempt_output" && ! -L "$attempt_output" &&
       ! -e "$attempt_error" && ! -L "$attempt_error" ]] ||
      fail "refusing to overwrite a finalized witnessed-funding attempt"
    jq -n --arg run "$M3_POC_RUN_ID" --arg request "$(new_request_id)" \
      --arg role "$depositor" --arg funding "$funding" \
      --argjson start "$start" --argjson blocks "$blocks" \
      --slurpfile runtime "${M3_POC_DIRECTION_ROOT}/sidecars/final/${depositor}/runtime.json" \
      --slurpfile terms "$final_terms" '
      {context:{schema_version:1,run_id:$run,request_id:$request,sidecar_role:$role},
       runtime:$runtime[0],terms:$terms[0],
       target:{mode:"exact",funding_transaction_id:$funding},
       window:{start_height:$start,max_blocks:$blocks}}
    ' >"$attempt_request"
    chmod 0600 "$attempt_request"
    if operator_call final "$depositor" observe-finalized-witnessed-funding \
        "$attempt_request" "$attempt_output" 2>"$attempt_error"; then
      chmod 0600 "$attempt_error"
      [[ -s "$attempt_output" ]] ||
        fail "successful finalized witnessed-funding observation returned empty evidence"
      mv "$attempt_request" "$request"
      mv "$attempt_output" "$output"
      successful_attempt="$attempt"
      break
    fi
    chmod 0600 "$attempt_error"
    [[ ! -s "$attempt_output" ]] ||
      fail "failed finalized witnessed-funding observation returned ambiguous stdout"
    if ! rg -Fq 'bridge observation unavailable: moving_tip' "$attempt_error"; then
      fail "finalized witnessed-funding observation failed outside typed moving-tip"
    fi
    moving_tip_retries=$((moving_tip_retries + 1))
    sleep 0.25
  done
  (( successful_attempt > 0 )) ||
    fail "finalized witnessed-funding observation exhausted its moving-tip retry bound"
  submissions_after="$(lez_successful_submission_count)"
  [[ "$submissions_after" == "$submissions_before" ]] ||
    fail "read-only finalized witnessed-funding retry changed the durable submission count"
  jq -n --argjson attempts "$successful_attempt" \
    --argjson retries "$moving_tip_retries" --argjson before "$submissions_before" \
    --argjson after "$submissions_after" '
    {schema_version:1,operation:"observe_finalized_witnessed_funding",
     attempts:$attempts,moving_tip_retries:$retries,max_attempts:120,
     fresh_request_id_per_attempt:true,failed_attempt_stdout_empty:true,
     only_typed_moving_tip_retried:true,observation_only:true,
     durable_lez_submissions_before:$before,durable_lez_submissions_after:$after,
     public_effect_count_unchanged:($before == $after)}
  ' >"$retry_evidence"
  chmod 0600 "$retry_evidence"
  jq -e --arg run "$M3_POC_RUN_ID" --arg role "$depositor" \
    --arg funding "$funding" --arg amount "$amount" --arg terms_hash "$terms_hash" \
    --argjson start "$start" --argjson end "$((start + blocks - 1))" '
    .context.run_id == $run and .context.sidecar_role == $role
    and .funding.transaction.transaction_id == $funding
    and .funding.metadata.terms_hash == $terms_hash
    and .funding.metadata.status == "funded"
    and .funding.metadata.amount == $amount
    and .funding.custody.balance == $amount
    and (.funding.containing_block.block_id | numbers) >= $start
    and (.funding.containing_block.block_id | numbers) <= $end
    and (.finalized_tip.height | numbers) >= $end
  ' "$output" >/dev/null ||
    fail "finalized witnessed funding does not exactly match the taker first lock"
  finalized_witnessed_funding_observation_file="$output"
}

refresh_first_lock_lez_absence_window() {
  local label="$1"
  local baseline="$actor_prelock_lez_tip"
  local spec="${M3_POC_DIRECTION_ROOT}/stage-two.json"
  local tip=0 blocks=0 cutoff tip_file tip_timestamp=0 waited=0
  [[ "$label" =~ ^[a-z0-9-]{1,48}$ ]] ||
    fail "first-lock LEZ absence-window evidence label is invalid"
  [[ "$baseline" =~ ^[0-9]+$ ]] || fail "pre-lock LEZ baseline is unavailable"
  cutoff="$(jq -er '.recovery.maker_second_lock_cutoff_unix_seconds | numbers' "$spec")"
  tip_file="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-first-lock-lez-${label}-cutoff-tip.json"
  for _ in {1..1200}; do
    waited=$((waited + 1))
    tip="$(finalized_tip)"
    blocks=$((tip - baseline))
    if (( blocks >= 1 && blocks <= 4096 )); then
      rpc_read_file "$M3_POC_LEZ_INDEXER_RPC_URL" \
        "$(jq -cn --argjson height "$tip" \
          '{jsonrpc:"2.0",id:1,method:"getBlockById",params:[$height]}')" "$tip_file"
      jq -e --argjson tip "$tip" '
        .result.header.block_id == $tip and .result.bedrock_status == "Finalized"
      ' "$tip_file" >/dev/null || fail "LEZ cutoff tip is not the exact finalized block"
      tip_timestamp="$(jq -er '.result.header.timestamp | numbers' "$tip_file")"
      (( tip_timestamp >= cutoff * 1000 )) && break
    fi
    sleep 0.25
  done
  (( blocks >= 1 && blocks <= 4096 )) ||
    fail "first-lock LEZ absence window does not reach a bounded current finalized tip"
  (( tip_timestamp >= cutoff * 1000 )) ||
    fail "finalized LEZ chain time did not cross the signed maker cutoff within the bound"
  write_actor_configs "$((baseline + 1))" "$blocks"
  jq -n --argjson baseline "$baseline" --argjson start "$((baseline + 1))" \
    --argjson tip "$tip" --argjson blocks "$blocks" \
    --argjson cutoff "$cutoff" --argjson tip_timestamp "$tip_timestamp" \
    --argjson wait_iterations "$waited" '
    {schema_version:1,pre_lock_finalized_baseline:$baseline,
     discovery_start_height:$start,current_finalized_tip:$tip,
     discovery_max_blocks:$blocks,
     signed_maker_second_lock_cutoff_unix_seconds:$cutoff,
     current_finalized_tip_timestamp_ms:$tip_timestamp,
     finalized_cutoff_wait_iterations:$wait_iterations,
     finalized_clock_cutoff_satisfied:($tip_timestamp >= $cutoff * 1000),
     every_finalized_block_after_baseline_included:
       ($start + $blocks - 1 == $tip)}
  ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-first-lock-lez-${label}-absence-window.json"
  chmod 0600 \
    "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-first-lock-lez-${label}-absence-window.json"
}

advance_core_median_time_to_first_lock_cutoff() {
  local spec="${M3_POC_DIRECTION_ROOT}/stage-two.json"
  local cutoff target before after median mined=0
  cutoff="$(jq -er '.recovery.maker_second_lock_cutoff_unix_seconds | numbers' "$spec")"
  target=$((cutoff + 1))
  before="$(core_rpc maker getblockchaininfo '[]')"
  median="$(jq -er '.result.mediantime | numbers' <<<"$before")"
  while (( median < cutoff && mined < 12 )); do
    core_admin setmocktime "$target" >/dev/null
    mine_one_core_block
    mined=$((mined + 1))
    after="$(core_rpc maker getblockchaininfo '[]')"
    median="$(jq -er '.result.mediantime | numbers' <<<"$after")"
  done
  core_admin setmocktime 0 >/dev/null
  (( median >= cutoff )) ||
    fail "stable Bitcoin median time did not cross the signed maker cutoff"
  core_rpc maker getrawmempool '[]' |
    jq -e '.error == null and .result == []' >/dev/null ||
    fail "advancing Bitcoin median time introduced a non-coinbase effect"
  jq -n --argjson cutoff "$cutoff" --argjson target "$target" \
    --argjson before "$(jq -er '.result.mediantime | numbers' <<<"$before")" \
    --argjson after "$median" --argjson blocks "$mined" '
    {schema_version:1,signed_maker_second_lock_cutoff_unix_seconds:$cutoff,
     mock_block_time:$target,median_time_before:$before,median_time_after:$after,
     run_owned_coinbase_blocks_mined:$blocks,
     stable_median_time_cutoff_satisfied:($after >= $cutoff),
     planned_maker_funding_submitted:false,public_swap_effect_count:0}
  ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-first-lock-bitcoin-cutoff.json"
  chmod 0600 \
    "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-first-lock-bitcoin-cutoff.json"
}

write_first_lock_recovery_admission_sample() {
  local sample="$1"
  local spec="${M3_POC_DIRECTION_ROOT}/stage-two.json"
  local funding="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-funding-prepared.json"
  local prefix="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-first-lock-admission-${sample}"
  local cutoff observed core_before core_after mempool txout spender source_tx source_vout planned_tx
  local expected_lez output lez_observation_file
  cutoff="$(jq -er '.recovery.maker_second_lock_cutoff_unix_seconds | numbers' "$spec")"
  observed="$(date -u +%s)"
  (( observed >= cutoff )) || fail "first-lock recovery admission sampled before the signed maker cutoff"

  core_before="${prefix}-core-before.json"
  core_after="${prefix}-core-after.json"
  mempool="${prefix}-core-mempool.json"
  core_rpc maker getblockchaininfo '[]' >"$core_before"
  core_rpc maker getrawmempool '[]' >"$mempool"
  chmod 0600 "$core_before" "$mempool"
  jq -e '.error == null and .result == []' "$mempool" >/dev/null ||
    fail "first-lock recovery admission found an unexpected Bitcoin mempool effect"

  case "$M3_POC_DIRECTION" in
    taker_sells_foreign)
      write_native_escrow_observation "first-lock-admission-${sample}-native"
      lez_observation_file="$native_observation_file"
      expected_lez=0
      [[ "$(lez_successful_submission_count)" == "$expected_lez" ]] ||
        fail "maker LEZ second-lock submission exists in the first-lock-only branch"
      jq -e '.after.escrow_state == null' "$native_observation_file" >/dev/null ||
        fail "maker LEZ second lock is not affirmatively absent"
      source_tx="$(jq -er '.bitcoin.funding_transaction_id' "$spec")"
      source_vout="$(jq -er '.bitcoin.funding_output_index | numbers' "$spec")"
      txout="${prefix}-taker-first-lock-gettxout.json"
      spender="${prefix}-taker-first-lock-spender.json"
      core_rpc maker gettxout "[\"${source_tx}\",${source_vout},true]" >"$txout"
      core_rpc maker gettxspendingprevout \
        "[[{\"txid\":\"${source_tx}\",\"vout\":${source_vout}}],{\"mempool_only\":false}]" \
        >"$spender"
      chmod 0600 "$txout" "$spender"
      jq -e --argjson value "$(jq -er '.bitcoin.funding_value_sat | numbers' "$spec")" '
        .error == null and .result != null
        and ((.result.value * 100000000 | round) == $value)
        and (.result.confirmations | numbers) >= 1
      ' "$txout" >/dev/null || fail "Bitcoin taker first lock is not canonical and unspent"
      jq -e '
        .error == null and (.result | length) == 1
        and (.result[0].spendingtxid == null)
      ' "$spender" >/dev/null || fail "Bitcoin taker first lock has a canonical spender"
      ;;
    taker_sells_lez)
      write_finalized_witnessed_funding_observation \
        "first-lock-admission-${sample}-finalized-witnessed-funding"
      lez_observation_file="$finalized_witnessed_funding_observation_file"
      expected_lez=2
      [[ "$(lez_successful_submission_count)" == "$expected_lez" ]] ||
        fail "LEZ first lock does not have exactly its initialization and funding effects"
      jq -e --arg amount "$(jq -er '.amount | strings | select(test("^[1-9][0-9]*$"))' \
          "$final_terms")" '
        .funding.metadata.status == "funded"
        and .funding.metadata.amount == $amount
        and .funding.custody.balance == $amount
      ' "$lez_observation_file" >/dev/null ||
        fail "LEZ taker first lock is not canonical, funded, and unspent"
      source_tx="$(jq -er '.input_transaction_id' "$funding")"
      source_vout="$(jq -er '.input_output_index | numbers' "$funding")"
      planned_tx="$(jq -er '.bitcoin.funding_transaction_id' "$spec")"
      txout="${prefix}-maker-second-lock-source-gettxout.json"
      spender="${prefix}-maker-second-lock-getrawtransaction.json"
      core_rpc maker gettxout "[\"${source_tx}\",${source_vout},true]" >"$txout"
      core_rpc maker getrawtransaction "[\"${planned_tx}\",true]" >"$spender"
      chmod 0600 "$txout" "$spender"
      jq -e '.error == null and .result != null' "$txout" >/dev/null ||
        fail "maker Bitcoin second-lock source is not affirmatively unspent"
      jq -e '.result == null and .error != null' "$spender" >/dev/null ||
        fail "maker Bitcoin second lock exists despite the first-lock-only branch"
      ;;
  esac

  core_rpc maker getblockchaininfo '[]' >"$core_after"
  chmod 0600 "$core_after"
  jq -e --slurpfile before "$core_before" '
    .error == null
    and .result.blocks == $before[0].result.blocks
    and .result.bestblockhash == $before[0].result.bestblockhash
  ' "$core_after" >/dev/null ||
    fail "Bitcoin canonical tip moved across a first-lock recovery admission sample"

  output="${prefix}.json"
  jq -n --arg direction "$M3_POC_DIRECTION" --argjson sample "$sample" \
    --argjson cutoff "$cutoff" --argjson observed "$observed" \
    --arg lez_file "$(basename "$lez_observation_file")" \
    --argjson lez_submissions "$expected_lez" \
    --slurpfile before "$core_before" --slurpfile after "$core_after" '
    {schema_version:1,direction:$direction,sample:$sample,
     signed_maker_second_lock_cutoff_unix_seconds:$cutoff,
     observed_unix_seconds:$observed,cutoff_satisfied:($observed >= $cutoff),
     maker_second_lock_absent:true,taker_first_lock_unspent:true,
     bitcoin_stable_tip:{
       height:$before[0].result.blocks,
       block_hash:$before[0].result.bestblockhash,
       revalidated_after_read:
         ($before[0].result.blocks == $after[0].result.blocks
          and $before[0].result.bestblockhash == $after[0].result.bestblockhash)},
     lez_stable_escrow_observation_file:$lez_file,
     durable_lez_submission_count:$lez_submissions,
     runner_checks_are_corroborating_not_send_authority:true,
     actor_internal_two_read_admission_required:true}
  ' >"$output"
  chmod 0600 "$output"
}

write_first_lock_recovery_admission_evidence() {
  local spec="${M3_POC_DIRECTION_ROOT}/stage-two.json"
  local cutoff output first second
  cutoff="$(jq -er '.recovery.maker_second_lock_cutoff_unix_seconds | numbers' "$spec")"
  wait_for_host_recovery_bound "$cutoff" first-lock-maker-second-lock-cutoff
  write_first_lock_recovery_admission_sample 1
  write_first_lock_recovery_admission_sample 2
  first="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-first-lock-admission-1.json"
  second="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-first-lock-admission-2.json"
  output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-first-lock-recovery-admission.json"
  jq -s --arg direction "$M3_POC_DIRECTION" --argjson cutoff "$cutoff" '
    {schema_version:1,direction:$direction,
     signed_maker_second_lock_cutoff_unix_seconds:$cutoff,
     samples:.,
     two_fresh_matching_reads:
       (length == 2 and all(.[];
         .cutoff_satisfied == true
         and .maker_second_lock_absent == true
         and .taker_first_lock_unspent == true
         and .bitcoin_stable_tip.revalidated_after_read == true)),
     actor_internal_admission_is_authoritative:true,
     cutoff_refund_race_policy:
       "canonical_maker_second_lock_wins_otherwise_two_fresh_absence_and_unspent_reads_admit_refund"}
  ' "$first" "$second" >"$output"
  chmod 0600 "$output"
  jq -e '.two_fresh_matching_reads == true and .actor_internal_admission_is_authoritative == true' \
    "$output" >/dev/null || fail "first-lock recovery admission evidence is incomplete"
}

assert_first_lock_recovery_pending() {
  local chain="$1"
  local before after expected
  before="$(lez_successful_submission_count)"
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign) expected=0 ;;
    taker_sells_lez) expected=2 ;;
  esac
  [[ "$before" == "$expected" ]] ||
    fail "first-lock predecessor began with an unexpected LEZ effect count"
  actor_invoke_recovery_pending_retry taker 1 "$chain" first-lock-refund-pending
  jq -e --arg chain "$chain" '
    .schema_version == 1 and .role == "taker" and .command == "recover"
    and .outcome == "awaiting_observation" and .chain == $chain and .revision == 1
  ' "$actor_last_output" >/dev/null ||
    fail "taker did not remain pending at the first-lock recovery predecessor"
  actor_invoke taker status first-lock-refund-pending-status
  jq -e '
    .schema_version == 1 and .role == "taker" and .state == "active"
    and .revision == 1 and .phase == "taker_lock_confirmed"
    and .next_action == "observe_maker_second_lock_or_recover_taker_leg"
  ' "$actor_last_output" >/dev/null ||
    fail "taker predecessor-one status changed during pending recovery"
  after="$(lez_successful_submission_count)"
  [[ "$after" == "$before" ]] ||
    fail "pre-eligibility first-lock recovery submitted a LEZ effect"
  core_rpc taker getrawmempool '[]' |
    jq -e '.error == null and .result == []' >/dev/null ||
    fail "pre-eligibility first-lock recovery submitted a Bitcoin effect"
  jq -n --arg direction "$M3_POC_DIRECTION" --arg chain "$chain" \
    --argjson lez "$after" '
    {schema_version:1,direction:$direction,predecessor_revision:1,
     recovery_chain:$chain,taker_owner_pending:true,
     abandoned_maker_actor_invocation_count:0,
     maker_second_lock_effect_count:0,durable_lez_submission_count:$lez,
     bitcoin_mempool_effect_count:0}
  ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-first-lock-refund-pending.json"
  chmod 0600 \
    "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-first-lock-refund-pending.json"
}

assert_first_lock_owner_terminal_restart() {
  local before after
  before="$(lez_successful_submission_count)"
  actor_invoke taker recover first-lock-owner-terminal-restart
  jq -e '
    .schema_version == 1 and .role == "taker" and .command == "recover"
    and .outcome == "not_yet_composed" and .durable_revision == 2
    and .phase == "refunded" and .revision == 2
  ' "$actor_last_output" >/dev/null ||
    fail "first-lock refund owner restart did not retain terminal revision two"
  after="$(lez_successful_submission_count)"
  [[ "$after" == "$before" ]] ||
    fail "first-lock refund owner terminal restart resubmitted a LEZ effect"
  core_rpc taker getrawmempool '[]' |
    jq -e '.error == null and .result == []' >/dev/null ||
    fail "first-lock refund owner terminal restart resubmitted a Bitcoin effect"
}

assert_fresh_maker_first_lock_terminal() {
  local before after chain
  before="$(lez_successful_submission_count)"
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign) chain=bitcoin ;;
    taker_sells_lez) chain=lez ;;
  esac
  project_role_to_revision maker 1 "$chain" first-lock-fresh-maker-observe-first-lock
  actor_invoke_recovery_retry maker 2 "$chain" first-lock-fresh-maker-observe-refund
  actor_invoke maker status first-lock-fresh-maker-terminal-status
  jq -e '
    .schema_version == 1 and .role == "maker" and .state == "active"
    and .phase == "refunded" and .revision == 2 and .next_action == "complete"
  ' "$actor_last_output" >/dev/null ||
    fail "fresh maker did not observe first lock then finalized refund to revision two"
  after="$(lez_successful_submission_count)"
  [[ "$after" == "$before" ]] ||
    fail "fresh maker terminal observer resubmitted a LEZ effect"
  core_rpc maker getrawmempool '[]' |
    jq -e '.error == null and .result == []' >/dev/null ||
    fail "fresh maker terminal observer resubmitted a Bitcoin effect"
}

run_actor_first_lock_refund_flow() {
  local spec="${M3_POC_DIRECTION_ROOT}/stage-two.json"
  local deadline
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign)
      submit_taker_bitcoin_first_lock
      project_role_to_revision taker 1 bitcoin bitcoin-first-lock
      refresh_first_lock_lez_absence_window pre-maturity
      assert_first_lock_recovery_pending bitcoin
      mine_core_to_refund_eligibility bitcoin-taker-first-lock-refund
      refresh_first_lock_lez_absence_window pre-admission
      write_first_lock_recovery_admission_evidence
      submit_actor_bitcoin_refund taker 2 bitcoin-taker-first-lock-refund
      ;;
    taker_sells_lez)
      submit_taker_lez_first_lock_pair
      project_role_to_revision taker 1 lez lez-first-lock
      assert_first_lock_recovery_pending lez
      deadline="$(jq -er '.lez_terms.refund_at_ms | numbers' "$spec")"
      wait_lez_finalized_deadline "$deadline" lez-taker-first-lock-refund
      advance_core_median_time_to_first_lock_cutoff
      write_first_lock_recovery_admission_evidence
      submit_actor_lez_refund taker 2 lez-taker-first-lock-refund
      ;;
  esac
  assert_first_lock_owner_terminal_restart
  assert_fresh_maker_first_lock_terminal
  capture_both_statuses 2 first-lock-terminal-status
}

write_actual_effect_manifest() {
  local output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actual-effects.json"
  case "$M3_POC_JOURNEY" in
    claim | survivor_claim)
      jq -n --arg direction "$M3_POC_DIRECTION" --arg bitcoin_lock "$bitcoin_lock_tx" \
        --arg bitcoin_claim "$bitcoin_claim_tx" --arg lez_initialization "$lez_initialization_tx" \
        --arg lez_custody "$lez_custody_tx" --arg lez_funding "$lez_funding_tx" \
        --arg lez_claim "$lez_claim_tx" --arg journey "$M3_POC_JOURNEY" \
        --arg asset_mode "$asset_mode" --arg asset_commitment "$asset_commitment" '
        {schema_version:1,journey:$journey,direction:$direction,
         bitcoin_effect_ids:[$bitcoin_lock,$bitcoin_claim],
         lez_effect_ids:
           (if $asset_mode == "custom_token" then
             [$lez_initialization,$lez_custody,$lez_funding,$lez_claim]
            else [$lez_initialization,$lez_funding,$lez_claim] end),
         expected_unique_effects:
           {bitcoin:2,lez:(if $asset_mode == "custom_token" then 4 else 3 end)},
         actor_owned_claims:{bitcoin:$bitcoin_claim,lez:$lez_claim}}
        + (if $asset_mode == "custom_token" then
          {asset:{kind:"custom_token",asset_commitment:$asset_commitment,
            first_lock_order:["initialize_witnessed","create_custody_ata","fund"]}}
          else {} end)
        + (if $journey == "survivor_claim" then {
          survivor_evidence_file:($direction + "-survivor-claim.json"),
          revealer:"taker",follower:"maker",
          intermediate_phase:"claim_evidence_available",
          intermediate_terminal:false
        } else {} end)
      ' >"$output"
      ;;
    refund)
      jq -n --arg direction "$M3_POC_DIRECTION" --arg bitcoin_lock "$bitcoin_lock_tx" \
        --arg bitcoin_refund "$bitcoin_refund_tx" --arg lez_initialization "$lez_initialization_tx" \
        --arg lez_custody "$lez_custody_tx" --arg lez_funding "$lez_funding_tx" \
        --arg lez_refund "$lez_refund_tx" --arg asset_mode "$asset_mode" \
        --arg asset_commitment "$asset_commitment" '
        {schema_version:1,journey:"refund",direction:$direction,
         bitcoin_effect_ids:[$bitcoin_lock,$bitcoin_refund],
         lez_effect_ids:
           (if $asset_mode == "custom_token" then
             [$lez_initialization,$lez_custody,$lez_funding,$lez_refund]
            else [$lez_initialization,$lez_funding,$lez_refund] end),
         expected_unique_effects:
           {bitcoin:2,lez:(if $asset_mode == "custom_token" then 4 else 3 end)},
         actor_owned_refunds:{bitcoin:$bitcoin_refund,lez:$lez_refund},
         cooperative_claim_effects_present:false}
        + (if $asset_mode == "custom_token" then
          {asset:{kind:"custom_token",asset_commitment:$asset_commitment,
            first_lock_order:["initialize_witnessed","create_custody_ata","fund"]}}
          else {} end)
      ' >"$output"
      ;;
    first_lock_refund)
      case "$M3_POC_DIRECTION" in
        taker_sells_foreign)
          jq -n --arg direction "$M3_POC_DIRECTION" --arg bitcoin_lock "$bitcoin_lock_tx" \
            --arg bitcoin_refund "$bitcoin_refund_tx" '
            {schema_version:1,journey:"first_lock_refund",direction:$direction,
             bitcoin_effect_ids:[$bitcoin_lock,$bitcoin_refund],lez_effect_ids:[],
             expected_unique_effects:{bitcoin:2,lez:0},
             actor_owned_refunds:{bitcoin:$bitcoin_refund},
             maker_second_lock:{chain:"lez",effect_count:0},
             cooperative_claim_effects_present:false,dual_lock_gate_opened:false}
          ' >"$output"
          ;;
        taker_sells_lez)
          jq -n --arg direction "$M3_POC_DIRECTION" \
            --arg lez_initialization "$lez_initialization_tx" \
            --arg lez_funding "$lez_funding_tx" --arg lez_refund "$lez_refund_tx" '
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
  esac
  chmod 0600 "$output"
}

run_actor_claim_flow() {
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign)
      direction_phase_begin revealing_claim_to_revision_three ||
        fail "could not begin revealing-claim timing"
      submit_actor_lez_claim taker 3 lez-revealing-claim
      direction_phase_end revealing_claim_to_revision_three ||
        fail "could not end revealing-claim timing"
      direction_phase_begin followup_claim_to_revision_four ||
        fail "could not begin follow-up-claim timing"
      submit_actor_bitcoin_claim maker 4 bitcoin-followup-claim
      direction_phase_end followup_claim_to_revision_four ||
        fail "could not end follow-up-claim timing"
      ;;
    taker_sells_lez)
      direction_phase_begin revealing_claim_to_revision_three ||
        fail "could not begin revealing-claim timing"
      submit_actor_bitcoin_claim taker 3 bitcoin-revealing-claim
      direction_phase_end revealing_claim_to_revision_three ||
        fail "could not end revealing-claim timing"
      direction_phase_begin followup_claim_to_revision_four ||
        fail "could not begin follow-up-claim timing"
      submit_actor_lez_claim maker 4 lez-followup-claim
      direction_phase_end followup_claim_to_revision_four ||
        fail "could not end follow-up-claim timing"
      ;;
  esac
}

survivor_recovering_evidence=""
survivor_maker_reveal_observation_output=""
write_survivor_recovering_evidence() {
  local spec="${M3_POC_DIRECTION_ROOT}/stage-two.json"
  local status output lez_count mempool
  local funding_tx funding_vout funding_value txout spender tip refund_height
  local observation tip_file tip_timestamp refund_at_ms status_sha observation_sha
  actor_invoke maker status survivor-maker-revision-three-status
  status="$actor_last_output"
  jq -e '
    .schema_version == 1 and .role == "maker" and .state == "active"
    and .revision == 3 and .phase == "claim_evidence_available"
    and .next_action == "observe_followup_claim"
  ' "$status" >/dev/null || fail "fresh maker did not retain nonterminal survivor revision three"
  [[ -f "$survivor_maker_reveal_observation_output" &&
     ! -L "$survivor_maker_reveal_observation_output" ]] ||
    fail "fresh maker reveal-observation evidence is unavailable"
  jq -e '
    .schema_version == 1 and .role == "maker" and .revision == 3
    and .phase == "claim_evidence_available"
    and .outcome == "observed_then_projected"
  ' "$survivor_maker_reveal_observation_output" >/dev/null ||
    fail "fresh maker did not project canonical reveal evidence"
  status_sha="$(sha256sum "$status" | sed 's/ .*//')"
  observation_sha="$(sha256sum "$survivor_maker_reveal_observation_output" | sed 's/ .*//')"
  output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-survivor-recovering.json"
  mempool="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-survivor-recovering-mempool.json"
  [[ ! -e "$output" && ! -L "$output" && ! -e "$mempool" && ! -L "$mempool" ]] ||
    fail "refusing to overwrite survivor recovery evidence"
  core_rpc maker getrawmempool '[]' >"$mempool"
  chmod 0600 "$mempool"
  jq -e '.error == null and .result == []' "$mempool" >/dev/null ||
    fail "survivor recovery window contains an unexpected Bitcoin mempool effect"
  lez_count="$(lez_successful_submission_count)"
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign)
      [[ -z "$bitcoin_claim_tx" && "$lez_count" == 3 ]] ||
        fail "foreign-first survivor follow-up effect exists before maker continuation"
      funding_tx="$(jq -er '.bitcoin.funding_transaction_id' "$spec")"
      funding_vout="$(jq -er '.bitcoin.funding_output_index | numbers' "$spec")"
      funding_value="$(jq -er '.bitcoin.funding_value_sat | numbers' "$spec")"
      refund_height="$(jq -er '.recovery.bitcoin_refund_height | numbers' "$spec")"
      txout="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-survivor-remaining-bitcoin-gettxout.json"
      spender="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-survivor-remaining-bitcoin-spender.json"
      core_rpc maker gettxout "[\"${funding_tx}\",${funding_vout},true]" >"$txout"
      core_rpc maker gettxspendingprevout \
        "[[{\"txid\":\"${funding_tx}\",\"vout\":${funding_vout}}],{\"mempool_only\":false}]" \
        >"$spender"
      chmod 0600 "$txout" "$spender"
      jq -e --argjson value "$funding_value" '
        .error == null and .result != null
        and ((.result.value * 100000000 | round) == $value)
        and (.result.confirmations | numbers) >= 1
      ' "$txout" >/dev/null || fail "survivor Bitcoin leg is not canonical and unspent"
      jq -e '
        .error == null and (.result | length) == 1
        and .result[0].spendingtxid == null
      ' "$spender" >/dev/null || fail "survivor Bitcoin leg already has a canonical spender"
      tip="$(core_admin getblockcount)"
      (( tip + 1 < refund_height )) ||
        fail "survivor Bitcoin continuation reached the signed refund boundary"
      jq -n --arg direction "$M3_POC_DIRECTION" --arg reveal "$lez_claim_tx" \
        --arg funding "$funding_tx" --argjson vout "$funding_vout" \
        --argjson value "$funding_value" --argjson tip "$tip" \
        --argjson refund_height "$refund_height" --argjson lez "$lez_count" \
        --arg status_sha "$status_sha" --arg observation_sha "$observation_sha" \
        --slurpfile status "$status" '
        {schema_version:1,journey:"survivor_claim",direction:$direction,
         reveal:{role:"taker",chain:"lez",transaction_id:$reveal,canonical:true},
         continuation:{follower_role:"maker",canonical_reveal_observed_by_fresh_process:true,
           caller_supplied_secret:false,related_presignature_and_adaptor_point_validated:true,
           projected_revision:3},
         intermediate:{protocol_phase:$status[0].phase,lifecycle_disposition:"recovering",
           terminal:false,remaining_leg:{chain:"bitcoin",transaction_id:$funding,
             output_index:$vout,value_sat:$value,canonical:true,unspent_or_funded:true,
             observed_tip_height:$tip,signed_refund_height:$refund_height,
             before_signed_later_refund_boundary:($tip + 1 < $refund_height)},
           followup_effect_present:false,bitcoin_effect_count:1,lez_effect_count:$lez},
         availability:{taker_invocations_after_reveal_before_maker_terminal:0,
           taker_absence_guard_enforced:true,follower_process_exited_at_revision_three:true},
         process_evidence:{maker_reveal_observation_sha256:$observation_sha,
           maker_revision_three_status_sha256:$status_sha},
         secret_recorded:false,delivery_or_chat_used:false}
      ' >"$output"
      ;;
    taker_sells_lez)
      [[ -n "$bitcoin_claim_tx" && -z "$lez_claim_tx" && "$lez_count" == 2 ]] ||
        fail "LEZ-first survivor follow-up effect exists before maker continuation"
      write_finalized_witnessed_funding_observation survivor-remaining-lez-funding
      observation="$finalized_witnessed_funding_observation_file"
      tip="$(finalized_tip)"
      tip_file="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-survivor-remaining-lez-tip.json"
      rpc_read_file "$M3_POC_LEZ_INDEXER_RPC_URL" \
        "$(jq -cn --argjson height "$tip" \
          '{jsonrpc:"2.0",id:1,method:"getBlockById",params:[$height]}')" "$tip_file"
      jq -e --argjson tip "$tip" '
        .error == null and .result.header.block_id == $tip
        and .result.bedrock_status == "Finalized"
        and (.result.header.timestamp | numbers) >= 0
      ' "$tip_file" >/dev/null ||
        fail "survivor LEZ tip is not the exact finalized block"
      tip_timestamp="$(jq -er '.result.header.timestamp | numbers' "$tip_file")"
      refund_at_ms="$(jq -er '.lez_terms.refund_at_ms | numbers' "$spec")"
      (( tip_timestamp < refund_at_ms )) ||
        fail "survivor LEZ continuation reached the signed refund boundary"
      jq -n --arg direction "$M3_POC_DIRECTION" --arg reveal "$bitcoin_claim_tx" \
        --arg funding "$lez_funding_tx" --arg amount \
          "$(jq -er '.amount | strings' "$final_terms")" \
        --argjson tip "$tip" --argjson timestamp "$tip_timestamp" \
        --argjson refund_at "$refund_at_ms" --argjson lez "$lez_count" \
        --arg status_sha "$status_sha" --arg observation_sha "$observation_sha" \
        --slurpfile status "$status" --slurpfile observation "$observation" '
        {schema_version:1,journey:"survivor_claim",direction:$direction,
         reveal:{role:"taker",chain:"bitcoin",transaction_id:$reveal,canonical:true},
         continuation:{follower_role:"maker",canonical_reveal_observed_by_fresh_process:true,
           caller_supplied_secret:false,related_presignature_and_adaptor_point_validated:true,
           projected_revision:3},
         intermediate:{protocol_phase:$status[0].phase,lifecycle_disposition:"recovering",
           terminal:false,remaining_leg:{chain:"lez",transaction_id:$funding,amount:$amount,
             canonical:true,unspent_or_funded:true,finalized_tip_height:$tip,
             finalized_tip_timestamp_ms:$timestamp,signed_refund_at_ms:$refund_at,
             before_signed_later_refund_boundary:($timestamp < $refund_at),
             metadata_status:$observation[0].funding.metadata.status,
             custody_balance:$observation[0].funding.custody.balance},
           followup_effect_present:false,bitcoin_effect_count:2,lez_effect_count:$lez},
         availability:{taker_invocations_after_reveal_before_maker_terminal:0,
           taker_absence_guard_enforced:true,follower_process_exited_at_revision_three:true},
         process_evidence:{maker_reveal_observation_sha256:$observation_sha,
           maker_revision_three_status_sha256:$status_sha},
         secret_recorded:false,delivery_or_chat_used:false}
      ' >"$output"
      ;;
  esac
  chmod 0600 "$output"
  jq -e '
    .intermediate.protocol_phase == "claim_evidence_available"
    and .intermediate.lifecycle_disposition == "recovering"
    and .intermediate.terminal == false
    and .intermediate.remaining_leg.canonical == true
    and .intermediate.remaining_leg.unspent_or_funded == true
    and .intermediate.remaining_leg.before_signed_later_refund_boundary == true
    and .intermediate.followup_effect_present == false
    and .availability.taker_invocations_after_reveal_before_maker_terminal == 0
    and .continuation.caller_supplied_secret == false
    and .secret_recorded == false
  ' "$output" >/dev/null || fail "survivor nonterminal recovery evidence is incomplete"
  survivor_recovering_evidence="$output"
}

survivor_maker_terminal_status=""
assert_survivor_maker_terminal() {
  actor_invoke maker status survivor-maker-terminal-status
  survivor_maker_terminal_status="$actor_last_output"
  jq -e '
    .schema_version == 1 and .role == "maker" and .state == "active"
    and .revision == 4 and .phase == "completed" and .next_action == "complete"
  ' "$survivor_maker_terminal_status" >/dev/null ||
    fail "fresh survivor maker did not reach terminal completion"
}

write_survivor_completion_evidence() {
  local reveal_chain="$1" followup_chain="$2"
  local before_lez="$3" after_lez="$4" reveal_output="$5" followup_output="$6"
  local bitcoin_before="$7" bitcoin_after="$8" output reveal_tx followup_tx
  local followup_submit_output="$9" terminal_projection_output="${10}"
  local maker_sha recovering_sha reveal_output_sha followup_output_sha
  local followup_submit_sha terminal_projection_sha
  local bitcoin_before_sha bitcoin_after_sha completion_boundary
  local spec="${M3_POC_DIRECTION_ROOT}/stage-two.json"
  local tip refund_height refund_at_ms finality containing_block block_file
  local containing_timestamp finality_sha block_sha
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign) reveal_tx="$lez_claim_tx"; followup_tx="$bitcoin_claim_tx" ;;
    taker_sells_lez) reveal_tx="$bitcoin_claim_tx"; followup_tx="$lez_claim_tx" ;;
  esac
  maker_sha="$(sha256sum "$survivor_maker_terminal_status" | sed 's/ .*//')"
  recovering_sha="$(sha256sum "$survivor_recovering_evidence" | sed 's/ .*//')"
  reveal_output_sha="$(sha256sum "$reveal_output" | sed 's/ .*//')"
  followup_output_sha="$(sha256sum "$followup_output" | sed 's/ .*//')"
  bitcoin_before_sha="$(sha256sum "$bitcoin_before" | sed 's/ .*//')"
  bitcoin_after_sha="$(sha256sum "$bitcoin_after" | sed 's/ .*//')"
  followup_submit_sha="$(sha256sum "$followup_submit_output" | sed 's/ .*//')"
  terminal_projection_sha="$(sha256sum "$terminal_projection_output" | sed 's/ .*//')"
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign)
      tip="$(core_admin getblockcount)"
      refund_height="$(jq -er '.recovery.bitcoin_refund_height | numbers' "$spec")"
      (( tip < refund_height )) ||
        fail "survivor Bitcoin completion reached the signed refund boundary"
      completion_boundary="$(jq -cn --argjson tip "$tip" \
        --argjson refund_height "$refund_height" '
        {chain:"bitcoin",confirmed_tip_height:$tip,
         signed_refund_height:$refund_height,
         completed_before_signed_refund_boundary:($tip < $refund_height)}')"
      ;;
    taker_sells_lez)
      finality="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-survivor-lez-followup-finality.json"
      [[ -f "$finality" && ! -L "$finality" ]] ||
        fail "survivor LEZ completion finality evidence is unavailable"
      containing_block="$(jq -er '.containing_block_id | numbers' "$finality")"
      block_file="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-survivor-lez-followup-block-${containing_block}.json"
      [[ -f "$block_file" && ! -L "$block_file" ]] ||
        fail "survivor LEZ containing-block evidence is unavailable"
      refund_at_ms="$(jq -er '.lez_terms.refund_at_ms | numbers' "$spec")"
      jq -e --arg tx "$lez_claim_tx" --argjson block "$containing_block" '
        .transaction_id == $tx and .occurrences == 1
        and .containing_block_id == $block and .bedrock_status == "Finalized"
        and .id_hash_lookups_equal == true
        and .transaction_hash_revalidated == true
      ' "$finality" >/dev/null || fail "survivor LEZ completion finality proof is inconsistent"
      jq -e --argjson block "$containing_block" --argjson refund_at "$refund_at_ms" '
        .error == null and .result.header.block_id == $block
        and .result.bedrock_status == "Finalized"
        and (.result.header.timestamp | numbers) < $refund_at
      ' "$block_file" >/dev/null ||
        fail "survivor LEZ claim did not finalize before its signed refund boundary"
      containing_timestamp="$(jq -er '.result.header.timestamp | numbers' "$block_file")"
      finality_sha="$(sha256sum "$finality" | sed 's/ .*//')"
      block_sha="$(sha256sum "$block_file" | sed 's/ .*//')"
      completion_boundary="$(jq -cn --argjson block "$containing_block" \
        --argjson timestamp "$containing_timestamp" --argjson refund_at "$refund_at_ms" \
        --arg finality_sha "$finality_sha" --arg block_sha "$block_sha" '
        {chain:"lez",finalized_containing_block_id:$block,
         finalized_containing_block_timestamp_ms:$timestamp,
         signed_refund_at_ms:$refund_at,
         finality_evidence_sha256:$finality_sha,
         containing_block_evidence_sha256:$block_sha,
         completed_before_signed_refund_boundary:($timestamp < $refund_at)}')"
      ;;
  esac
  output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-survivor-claim.json"
  [[ ! -e "$output" && ! -L "$output" ]] ||
    fail "refusing to overwrite survivor completion evidence"
  jq -n --arg direction "$M3_POC_DIRECTION" --arg reveal_chain "$reveal_chain" \
    --arg followup_chain "$followup_chain" --arg reveal_tx "$reveal_tx" \
    --arg followup_tx "$followup_tx" --arg maker_sha "$maker_sha" \
    --arg recovering_sha "$recovering_sha" --argjson before_lez "$before_lez" \
    --argjson after_lez "$after_lez" --arg reveal_output_sha "$reveal_output_sha" \
    --arg followup_output_sha "$followup_output_sha" \
    --arg bitcoin_before_sha "$bitcoin_before_sha" \
    --arg bitcoin_after_sha "$bitcoin_after_sha" \
    --arg followup_submit_sha "$followup_submit_sha" \
    --arg terminal_projection_sha "$terminal_projection_sha" \
    --argjson completion_boundary "$completion_boundary" '
    {schema_version:1,journey:"survivor_claim",direction:$direction,
     reveal:{role:"taker",chain:$reveal_chain,transaction_id:$reveal_tx,canonical:true},
     continuation:{follower_role:"maker",canonical_reveal_observed_by_fresh_process:true,
       caller_supplied_secret:false,related_presignature_and_adaptor_point_validated:true,
       projected_revision:3},
     intermediate:{protocol_phase:"claim_evidence_available",
       lifecycle_disposition:"recovering",terminal:false,
       recovering_evidence_sha256:$recovering_sha},
     availability:{taker_invocations_after_reveal_before_maker_terminal:0,
       taker_absence_guard_enforced:true,follower_process_exited_at_revision_three:true,
       fresh_follower_process_submitted_followup:true,
       distinct_fresh_follower_process_projected_terminal:true,
       followup_submission_output_sha256:$followup_submit_sha,
       terminal_projection_output_sha256:$terminal_projection_sha},
     completion:{followup_role:"maker",chain:$followup_chain,
       transaction_id:$followup_tx,canonical:true,maker_revision:4,phase:"completed",
       maker_terminal_status_sha256:$maker_sha,boundary:$completion_boundary},
     delayed_revealer_catchup:{began_after_maker_terminal:true,revisions:[3,4],
       observation_only:true,
       actor_observations:{
         reveal:{chain:$reveal_chain,revision:3,outcome:"observed_then_projected",
           sha256:$reveal_output_sha},
         followup:{chain:$followup_chain,revision:4,outcome:"observed_then_projected",
           sha256:$followup_output_sha}},
       per_chain:{
         bitcoin:{actor_observation_sha256:
             (if $reveal_chain == "bitcoin" then $reveal_output_sha else $followup_output_sha end),
           mempool_before_count:0,mempool_after_count:0,
           bitcoin_mempool_before_sha256:$bitcoin_before_sha,
           bitcoin_mempool_after_sha256:$bitcoin_after_sha,
           successful_resubmission_count:0},
         lez:{actor_observation_sha256:
             (if $reveal_chain == "lez" then $reveal_output_sha else $followup_output_sha end),
           durable_submission_count_before:$before_lez,
           durable_submission_count_after:$after_lez,
           successful_resubmission_count:($after_lez - $before_lez)}},
       successful_resubmission_count:($after_lez - $before_lez)},
     secret_recorded:false,delivery_or_chat_used:false}
  ' >"$output"
  chmod 0600 "$output"
  jq -e '
    .availability.taker_invocations_after_reveal_before_maker_terminal == 0
    and .completion.maker_revision == 4 and .completion.phase == "completed"
    and .delayed_revealer_catchup.began_after_maker_terminal == true
    and .delayed_revealer_catchup.observation_only == true
    and .delayed_revealer_catchup.per_chain.bitcoin.successful_resubmission_count == 0
    and .delayed_revealer_catchup.per_chain.lez.successful_resubmission_count == 0
    and .delayed_revealer_catchup.successful_resubmission_count == 0
    and .completion.boundary.completed_before_signed_refund_boundary == true
    and .secret_recorded == false
  ' "$output" >/dev/null || fail "survivor completion evidence is incomplete"
}

run_actor_survivor_claim_flow() {
  local reveal_chain followup_chain before_lez after_lez
  local reveal_output followup_output bitcoin_before bitcoin_after
  local followup_submit_output terminal_projection_output
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign)
      reveal_chain=lez
      followup_chain=bitcoin
      submit_actor_lez_claim_effect taker 3 survivor-lez-reveal
      project_role_to_revision maker 3 lez survivor-maker-observe-reveal
      survivor_maker_reveal_observation_output="$actor_last_output"
      write_survivor_recovering_evidence
      submit_actor_bitcoin_claim_effect maker 4 survivor-bitcoin-followup
      followup_submit_output="$actor_last_output"
      project_role_to_revision maker 4 bitcoin survivor-maker-project-followup
      terminal_projection_output="$actor_last_output"
      ;;
    taker_sells_lez)
      reveal_chain=bitcoin
      followup_chain=lez
      submit_actor_bitcoin_claim_effect taker 3 survivor-bitcoin-reveal
      project_role_to_revision maker 3 bitcoin survivor-maker-observe-reveal
      survivor_maker_reveal_observation_output="$actor_last_output"
      write_survivor_recovering_evidence
      submit_actor_lez_claim_effect maker 4 survivor-lez-followup
      followup_submit_output="$actor_last_output"
      project_role_to_revision maker 4 lez survivor-maker-project-followup
      terminal_projection_output="$actor_last_output"
      ;;
  esac
  assert_survivor_maker_terminal
  survivor_taker_absence_guard=0
  before_lez="$(lez_successful_submission_count)"
  bitcoin_before="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-survivor-catchup-bitcoin-mempool-before.json"
  bitcoin_after="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-survivor-catchup-bitcoin-mempool-after.json"
  [[ ! -e "$bitcoin_before" && ! -L "$bitcoin_before" &&
     ! -e "$bitcoin_after" && ! -L "$bitcoin_after" ]] ||
    fail "refusing to overwrite survivor catch-up Bitcoin evidence"
  core_rpc taker getrawmempool '[]' >"$bitcoin_before"
  chmod 0600 "$bitcoin_before"
  jq -e '.error == null and .result == []' "$bitcoin_before" >/dev/null ||
    fail "survivor catch-up began with an unexpected Bitcoin mempool effect"
  project_role_to_revision taker 3 "$reveal_chain" survivor-delayed-taker-reveal
  reveal_output="$actor_last_output"
  project_role_to_revision taker 4 "$followup_chain" survivor-delayed-taker-followup
  followup_output="$actor_last_output"
  after_lez="$(lez_successful_submission_count)"
  [[ "$after_lez" == "$before_lez" ]] ||
    fail "delayed survivor revealer catch-up resubmitted a LEZ effect"
  core_rpc taker getrawmempool '[]' >"$bitcoin_after"
  chmod 0600 "$bitcoin_after"
  jq -e '.error == null and .result == []' "$bitcoin_after" >/dev/null ||
    fail "delayed survivor revealer catch-up resubmitted a Bitcoin effect"
  jq -e '
    .schema_version == 1 and .role == "taker" and .revision == 3
    and .phase == "claim_evidence_available"
    and .outcome == "observed_then_projected"
  ' "$reveal_output" >/dev/null ||
    fail "delayed survivor reveal catch-up was not observation-only"
  jq -e '
    .schema_version == 1 and .role == "taker" and .revision == 4
    and .phase == "completed" and .outcome == "observed_then_projected"
  ' "$followup_output" >/dev/null ||
    fail "delayed survivor follow-up catch-up was not observation-only"
  write_survivor_completion_evidence "$reveal_chain" "$followup_chain" \
    "$before_lez" "$after_lez" "$reveal_output" "$followup_output" \
    "$bitcoin_before" "$bitcoin_after" "$followup_submit_output" \
    "$terminal_projection_output"
}

run_actor_refund_flow() {
  local spec="${M3_POC_DIRECTION_ROOT}/stage-two.json"
  local deadline later_bound earlier_bound before_count expected_pre_refund=2 now
  if [[ "$asset_mode" == "custom_token" ]]; then
    expected_pre_refund=3
  fi
  deadline="$(jq -er '.lez_terms.refund_at_ms | numbers' "$spec")"
  later_bound="$(jq -er '.recovery.later_refund_earliest_unix_seconds | numbers' "$spec")"
  earlier_bound="$(jq -er '.recovery.earlier_refund_latest_unix_seconds | numbers' "$spec")"
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign)
      before_count="$(lez_successful_submission_count)"
      [[ "$before_count" == "$expected_pre_refund" ]] ||
        fail "LEZ pre-refund submission count drifted"
      assert_recovery_pending_both lez 2 lez-maker-refund-predeadline
      [[ "$(lez_successful_submission_count)" == "$before_count" ]] ||
        fail "pre-deadline LEZ recovery submitted an effect"
      wait_lez_finalized_deadline "$deadline" lez-maker-refund
      submit_actor_lez_refund maker 3 lez-maker-refund

      assert_recovery_pending_both bitcoin 3 bitcoin-taker-refund-immature
      core_rpc maker getrawmempool '[]' | jq -e '.error == null and .result == []' >/dev/null ||
        fail "immature Bitcoin recovery changed the mempool"
      wait_for_host_recovery_bound "$later_bound" bitcoin-taker-refund-later-bound
      mine_core_to_refund_eligibility bitcoin-taker-refund
      submit_actor_bitcoin_refund taker 4 bitcoin-taker-refund
      ;;
    taker_sells_lez)
      assert_recovery_pending_both bitcoin 2 bitcoin-maker-refund-immature
      core_rpc taker getrawmempool '[]' | jq -e '.error == null and .result == []' >/dev/null ||
        fail "immature Bitcoin recovery changed the mempool"
      now="$(date -u +%s)"
      (( now <= earlier_bound )) || fail "Bitcoin first refund missed the signed earlier bound"
      mine_core_to_refund_eligibility bitcoin-maker-refund
      submit_actor_bitcoin_refund maker 3 bitcoin-maker-refund
      now="$(date -u +%s)"
      (( now <= earlier_bound )) || fail "Bitcoin first refund completed after the signed earlier bound"

      before_count="$(lez_successful_submission_count)"
      [[ "$before_count" == "$expected_pre_refund" ]] ||
        fail "LEZ pre-refund submission count drifted"
      assert_recovery_pending_both lez 3 lez-taker-refund-predeadline
      [[ "$(lez_successful_submission_count)" == "$before_count" ]] ||
        fail "pre-deadline LEZ recovery submitted an effect"
      wait_lez_finalized_deadline "$deadline" lez-taker-refund
      submit_actor_lez_refund taker 4 lez-taker-refund
      ;;
  esac
}

complete_m5_btc_application_handoff() {
  local application_root="$M3_POC_M5_APPLICATION_ROOT"
  local fixture_root="${M3_POC_DIRECTION_ROOT}/fixture"
  local owner_root="${application_root}/owner"
  local runtime_root="$M3_POC_M5_RUNTIME_ROOT"
  local socket="${runtime_root}/m.sock"
  local chat_socket="${runtime_root}/c.sock"
  local ready_file="${runtime_root}/m.ready"
  local database="${application_root}/maker.sqlite3"
  local delivery="${application_root}/delivery"
  local delivery_offline="${application_root}/delivery.offline"
  local delivery_key="${fixture_root}/private/maker-signing.key"
  local maker_signing_key="$delivery_key"
  local taker_signing_key="${fixture_root}/private/taker-signing.key"
  local actor_program_root="${M3_POC_SECURE_STATE_ROOT}/m5-btc-actor-program"
  local actor_program="${actor_program_root}/btc-reference-actor"
  local maker_source_config="${M3_POC_DIRECTION_ROOT}/actors/maker/actor-config.json"
  local taker_source_config="${M3_POC_DIRECTION_ROOT}/actors/taker/actor-config.json"
  local maker_actor_root="${owner_root}/maker-actors"
  local taker_actor_root="${owner_root}/taker-actor"
  local draft_file="${owner_root}/unsigned-draft-v1.borsh"
  local agreement_file="${owner_root}/agreement-v1.borsh"
  local receipt_file="${owner_root}/acceptance-receipt.json"
  local acceptance_file="${owner_root}/acceptance.json"
  local monitor_file="${owner_root}/offline-monitor.json"
  local daemon_log="${owner_root}/chat-daemon.log"
  local plan_file="${application_root}/btc-plan.json"
  local draft_evidence="${owner_root}/draft-export.json"
  local offer_id reservation_id maker_public_key now actor_sha final_sha
  local maker_config taker_config source_maker_sha source_taker_sha
  local source_maker_inode source_taker_inode daemon_pid role_config
  local -a maker_configs=()

  [[ "$m5_btc_application_mode" == 1 &&
     "$M3_POC_DIRECTION" == taker_sells_foreign && "$asset_mode" == native ]] ||
    fail "M5 BTC application handoff is restricted to the native forward route"
  [[ "$runtime_root" == "$(dirname "$(dirname "$M3_POC_SECURE_STATE_ROOT")")/c" ]] ||
    fail "M5 BTC Chat runtime root escaped the exact run-owned secure root"
  [[ "$application_root" == "${M3_POC_DIRECTION_ROOT}/application" &&
     -d "$application_root" && ! -L "$application_root" &&
     "$(stat -c '%u:%a' "$application_root")" == "$(id -u):700" ]] ||
    fail "M5 BTC application root is unavailable or unsafe"
  for input in "$plan_file" "$delivery_key" "$taker_signing_key" \
    "$maker_source_config" "$taker_source_config" \
    "${fixture_root}/agreement.borsh"; do
    [[ -f "$input" && ! -L "$input" ]] ||
      fail "M5 BTC handoff input is unavailable or unsafe: ${input##*/}"
  done
  [[ ! -e "$owner_root" && ! -L "$owner_root" &&
     ! -e "$delivery_offline" && ! -L "$delivery_offline" ]] ||
    fail "M5 BTC owner output already exists"
  [[ ! -e "$runtime_root" && ! -L "$runtime_root" ]] ||
    fail "M5 BTC Chat runtime root already exists"
  mkdir -m 0700 "$runtime_root"
  mkdir -m 0700 "$owner_root"
  mkdir -m 0700 "$maker_actor_root"

  offer_id="$(jq -er '.offer_id | strings' "$plan_file")"
  reservation_id="$(jq -er '.reservation_id | strings' "$plan_file")"
  [[ "$(jq -er '.swap_id' "$plan_file")" == "$M3_POC_SWAP_ID" ]] ||
    fail "M5 BTC planned swap identity drifted before Chat"
  maker_public_key="$(jq -er '.maker.musig2_public_key' \
    "${fixture_root}/public-spec.json")"
  [[ "$maker_public_key" =~ ^0[23][0-9a-f]{64}$ ]] ||
    fail "M5 BTC Delivery public key is invalid"

  "$M3_POC_PROVISIONER_BIN" export-draft \
    --agreement-file "${fixture_root}/agreement.borsh" \
    --output-file "$draft_file" >"$draft_evidence"
  chmod 0600 "$draft_evidence"
  [[ -f "$draft_file" && ! -L "$draft_file" &&
     "$(stat -c '%u:%a:%h' "$draft_file")" == "$(id -u):600:1" ]] ||
    fail "M5 BTC canonical draft publication is unsafe"

  source_maker_sha="$(sha256sum "$maker_source_config" | sed 's/ .*//')"
  source_taker_sha="$(sha256sum "$taker_source_config" | sed 's/ .*//')"
  source_maker_inode="$(stat -c '%d:%i' "$maker_source_config")"
  source_taker_inode="$(stat -c '%d:%i' "$taker_source_config")"
  actor_sha="$(sha256sum "$M3_POC_ACTOR_BIN" | sed 's/ .*//')"
  [[ "$actor_sha" =~ ^[0-9a-f]{64}$ ]] || fail "M5 BTC actor digest is invalid"
  [[ ! -e "$actor_program_root" && ! -L "$actor_program_root" ]] ||
    fail "M5 BTC staged actor runtime already exists"
  mkdir -m 0700 "$actor_program_root"
  cp --reflink=auto -- "$M3_POC_ACTOR_BIN" "$actor_program"
  chmod 0700 "$actor_program"
  [[ "$(stat -c '%u:%a:%h' "$actor_program")" == "$(id -u):700:1" &&
     "$(sha256sum "$actor_program" | sed 's/ .*//')" == "$actor_sha" ]] ||
    fail "M5 BTC staged actor runtime is unsafe or changed"

  setsid "$M3_POC_MAKER_DAEMON_BIN" --socket "$socket" \
    --chat-socket "$chat_socket" --database "$database" --ready-file "$ready_file" \
    --delivery-directory "$delivery" --delivery-signing-key-file "$delivery_key" \
    --btc-maker-signing-key-file "$maker_signing_key" \
    --btc-source-maker-config "$maker_source_config" \
    --btc-maker-actor-root "$maker_actor_root" \
    --btc-actor-program "$actor_program" \
    --btc-actor-program-sha256 "$actor_sha" >"$daemon_log" 2>&1 &
  daemon_pid=$!
  if ! register_m5_application_process chat "$daemon_pid" \
      "$M3_POC_MAKER_DAEMON_BIN"; then
    kill -TERM -- "-${daemon_pid}" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
    fail "M5 BTC Chat daemon registration failed"
  fi
  # Debug actor validation reads and hashes the complete staged executable before
  # readiness. The measured cold local startup is about 20.4 seconds; this
  # 60-second ceiling preserves fail-closed validation without delaying success.
  for _ in {1..1200}; do
    if [[ -f "$ready_file" && "$(cat "$ready_file")" == "$socket" &&
         -S "$socket" && -S "$chat_socket" ]]; then
      break
    fi
    kill -0 "$daemon_pid" 2>/dev/null || fail "M5 BTC Chat daemon exited before readiness"
    sleep 0.05
  done
  [[ -S "$socket" && -S "$chat_socket" && -f "$ready_file" &&
     "$(cat "$ready_file")" == "$socket" ]] ||
    fail "M5 BTC Chat daemon readiness timed out"

  now="$(date -u +%s)"
  "$M3_POC_TAKER_CLI_BIN" --delivery-directory "$delivery" \
    --maker-public-key "$maker_public_key" --now-unix-seconds "$now" \
    --pair bitcoin --direction taker-sells-foreign \
    --accept-btc-offer "$offer_id" --chat-socket "$chat_socket" \
    --reservation-id "$reservation_id" --foreign-units 1000000 \
    --unsigned-draft-file "$draft_file" \
    --taker-signing-key-file "$taker_signing_key" \
    --agreement-output-file "$agreement_file" \
    --btc-source-taker-config "$taker_source_config" \
    --btc-taker-actor-root "$taker_actor_root" \
    --btc-acceptance-receipt "$receipt_file" >"$acceptance_file"
  chmod 0600 "$acceptance_file" "$daemon_log"
  jq -e --arg offer "$offer_id" --arg reservation "$reservation_id" \
    --arg swap "$M3_POC_SWAP_ID" --arg agreement "$agreement_file" '
    .schema_version == 1 and .offer_id == $offer and .offer_revision == 3
    and .reservation_id == $reservation and .swap_id == $swap
    and .agreement_file == $agreement
    and (.agreement_sha256 | test("^[0-9a-f]{64}$"))
    and .replay == {proposal:false,completion:false,agreement_file:false}
    and .private_material_disclosed == false
    and .actor.role == "taker" and .actor.provisioning_replay == false
    and .actor.receipt_replay == false
  ' "$acceptance_file" >/dev/null || fail "M5 BTC Taker acceptance output is invalid"
  final_sha="$(jq -er '.agreement_sha256' "$acceptance_file")"
  [[ "$(sha256sum "$agreement_file" | sed 's/ .*//')" == "$final_sha" ]] ||
    fail "M5 BTC final agreement digest drifted"

  mapfile -t maker_configs < <(sqlite3 -batch -noheader -readonly "$database" \
    "SELECT manifest_path FROM maker_actor_processes WHERE swap_id = '${M3_POC_SWAP_ID}' AND actor_kind = 'bitcoin';")
  [[ "${#maker_configs[@]}" == 1 ]] || fail "M5 BTC Maker actor manifest is ambiguous"
  maker_config="${maker_configs[0]}"
  taker_config="$(jq -er '.actor_config_file | strings' "$receipt_file")"
  for role_config in "$maker_config" "$taker_config"; do
    [[ "$role_config" == /* && -f "$role_config" && ! -L "$role_config" ]] ||
      fail "M5 BTC provisioned actor config is unavailable or unsafe"
    jq -e --arg agreement_sha "$final_sha" \
      '.schema_version == 6 and .agreement_sha256 == $agreement_sha' \
      "$role_config" >/dev/null || fail "M5 BTC provisioned actor config is unbound"
    cmp "$(jq -er '.agreement_file' "$role_config")" "$agreement_file" ||
      fail "M5 BTC provisioned actors do not share the exact final agreement"
  done
  jq -e --arg swap "$M3_POC_SWAP_ID" --arg agreement_sha "$final_sha" '
    .schema_version == 1 and .pair == "bitcoin" and .role == "taker"
    and .swap_id == $swap and .agreement_sha256 == $agreement_sha
  ' "$receipt_file" >/dev/null || fail "M5 BTC acceptance receipt is invalid"
  jq -e '.role == "maker"' "$maker_config" >/dev/null || fail "Maker role config drifted"
  jq -e '.role == "taker"' "$taker_config" >/dev/null || fail "Taker role config drifted"
  [[ "$(sha256sum "$maker_source_config" | sed 's/ .*//')" == "$source_maker_sha" &&
     "$(sha256sum "$taker_source_config" | sed 's/ .*//')" == "$source_taker_sha" &&
     "$(stat -c '%d:%i' "$maker_source_config")" == "$source_maker_inode" &&
     "$(stat -c '%d:%i' "$taker_source_config")" == "$source_taker_inode" ]] ||
    fail "M5 BTC Chat mutated source actor authority"

  m5_btc_actor_configs[maker]="$maker_config"
  m5_btc_actor_configs[taker]="$taker_config"
  stop_m5_application_process || fail "M5 BTC Chat daemon shutdown failed"
  [[ ! -e "$socket" && ! -e "$chat_socket" && ! -e "$ready_file" ]] ||
    fail "M5 BTC Chat daemon left live endpoints"
  mv "$delivery" "$delivery_offline"
  "$M3_POC_TAKER_CLI_BIN" monitor --receipt "$receipt_file" >"$monitor_file"
  chmod 0600 "$monitor_file"
  jq -e '
    (keys | sort) == (["pair","role","schema_version","state"] | sort)
    and .schema_version == 1 and .pair == "bitcoin" and .role == "taker"
    and .state == "not_activated"
  ' "$monitor_file" >/dev/null ||
    fail "M5 BTC offline Taker monitor is invalid"
}

prepare_actor_flow_runtime() {
  local initial_tip
  direction_phase_begin final_transcript ||
    fail "could not begin final-transcript timing"
  prepare_final_transcript
  direction_phase_end final_transcript ||
    fail "could not end final-transcript timing"
  direction_phase_begin presign_and_activate ||
    fail "could not begin presign-and-activate timing"
  provision_signing_material
  run_signing_ceremony btc "$btc_session_file"
  run_signing_ceremony lez "$lez_session_file"
  accepted_at="$(date -u +%s)"
  initial_tip="$(finalized_tip)"
  actor_prelock_lez_tip="$initial_tip"
  if [[ "$m5_btc_application_mode" == "1" ]]; then
    write_actor_configs "$initial_tip" 4096
    complete_m5_btc_application_handoff
  else
    write_actor_configs "$initial_tip" 1
  fi
  activate_actors
  direction_phase_end presign_and_activate ||
    fail "could not end presign-and-activate timing"
}

run_actor_two_lock_phase() {
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign)
      direction_phase_begin first_lock_to_revision_one ||
        fail "could not begin first-lock timing"
      submit_taker_bitcoin_first_lock
      project_both_to_revision 1 bitcoin bitcoin-first-lock
      direction_phase_end first_lock_to_revision_one ||
        fail "could not end first-lock timing"
      direction_phase_begin second_lock_to_revision_two ||
        fail "could not begin second-lock timing"
      if [[ "$asset_mode" == "custom_token" ]]; then
        submit_actor_maker_lez_asset_second_lock
      else
        submit_actor_maker_lez_second_lock_pair
      fi
      project_both_to_revision 2 lez lez-second-lock
      direction_phase_end second_lock_to_revision_two ||
        fail "could not end second-lock timing"
      ;;
    taker_sells_lez)
      direction_phase_begin first_lock_to_revision_one ||
        fail "could not begin first-lock timing"
      if [[ "$asset_mode" == "custom_token" ]]; then
        submit_taker_lez_asset_first_lock
      else
        submit_taker_lez_first_lock_pair
      fi
      project_both_to_revision 1 lez lez-first-lock
      direction_phase_end first_lock_to_revision_one ||
        fail "could not end first-lock timing"
      direction_phase_begin second_lock_to_revision_two ||
        fail "could not begin second-lock timing"
      submit_actor_maker_bitcoin_second_lock
      project_both_to_revision 2 bitcoin bitcoin-second-lock
      direction_phase_end second_lock_to_revision_two ||
        fail "could not end second-lock timing"
      ;;
  esac
  direction_phase_begin dual_lock_gate ||
    fail "could not begin dual-lock-gate timing"
  write_dual_lock_gate
  direction_phase_end dual_lock_gate ||
    fail "could not end dual-lock-gate timing"
}

run_actor_settlement_phase() {
  case "$M3_POC_JOURNEY" in
    claim) run_actor_claim_flow ;;
    survivor_claim)
      direction_phase_begin survivor_settlement_to_revision_four ||
        fail "could not begin survivor-settlement timing"
      run_actor_survivor_claim_flow
      direction_phase_end survivor_settlement_to_revision_four ||
        fail "could not end survivor-settlement timing"
      ;;
    refund)
      direction_phase_begin refund_settlement_to_revision_four ||
        fail "could not begin refund-settlement timing"
      run_actor_refund_flow
      direction_phase_end refund_settlement_to_revision_four ||
        fail "could not end refund-settlement timing"
      ;;
    *) fail "two-lock settlement is unavailable for ${M3_POC_JOURNEY}" ;;
  esac
  direction_phase_begin terminal_evidence ||
    fail "could not begin terminal-evidence timing"
  capture_both_statuses 4 terminal-status
  write_actual_effect_manifest
  direction_phase_end terminal_evidence ||
    fail "could not end terminal-evidence timing"
}

run_actor_flow() {
  direction_timing_execution_mode="sequential"
  initialize_direction_phase_timings ||
    fail "could not initialize direction phase timings"
  prepare_actor_flow_runtime
  if [[ "$M3_POC_JOURNEY" == "first_lock_refund" ]]; then
    direction_phase_begin first_lock_refund_to_revision_two ||
      fail "could not begin first-lock-refund timing"
    run_actor_first_lock_refund_flow
    direction_phase_end first_lock_refund_to_revision_two ||
      fail "could not end first-lock-refund timing"
    direction_phase_begin terminal_evidence ||
      fail "could not begin terminal-evidence timing"
    write_actual_effect_manifest
    direction_phase_end terminal_evidence ||
      fail "could not end terminal-evidence timing"
    finalize_direction_phase_timings ||
      fail "could not publish direction phase timings"
    return
  fi
  run_actor_two_lock_phase
  run_actor_settlement_phase
  finalize_direction_phase_timings ||
    fail "could not publish direction phase timings"
}

overlap_gate_path() {
  printf '%s/overlap-%s-permit.json\n' "$M3_POC_DIRECTION_ROOT" "$1"
}

overlap_marker_path() {
  printf '%s/overlap-%s-arrived.json\n' "$M3_POC_DIRECTION_ROOT" "$1"
}

write_overlap_marker() {
  local phase="$1" revision="$2" marker partial
  marker="$(overlap_marker_path "$phase")"
  partial="${marker}.partial"
  [[ ! -e "$marker" && ! -L "$marker" && ! -e "$partial" && ! -L "$partial" ]] ||
    fail "refusing to overwrite overlap ${phase} marker"
  jq -n --arg run "$M3_POC_RUN_ID" --arg direction "$M3_POC_DIRECTION" \
    --arg phase "$phase" --argjson revision "$revision" \
    --arg recorded_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" '
    {schema_version:1,run_id:$run,direction:$direction,phase:$phase,
     revision:$revision,recorded_at:$recorded_at}
  ' >"$partial"
  chmod 0600 "$partial"
  mv "$partial" "$marker"
}

await_overlap_permit() {
  local phase="$1" expected_revision="$2" permit
  permit="$(overlap_gate_path "$phase")"
  for _ in {1..14400}; do
    if [[ -f "$permit" && ! -L "$permit" ]]; then
      [[ "$(stat -c '%a' "$permit")" == 600 ]] ||
        fail "overlap ${phase} permit is not owner private"
      jq -e --arg run "$M3_POC_RUN_ID" --arg direction "$M3_POC_DIRECTION" \
        --arg phase "$phase" --argjson revision "$expected_revision" '
        .schema_version == 1 and .run_id == $run and .direction == $direction
        and .phase == $phase and .expected_revision == $revision
      ' "$permit" >/dev/null || fail "overlap ${phase} permit is inconsistent"
      return
    fi
    sleep 0.25
  done
  fail "overlap ${phase} permit timed out"
}

run_overlap_actor_flow() {
  [[ "$M3_POC_JOURNEY" == "claim" ]] ||
    fail "overlap actor flow currently supports only the claim journey"
  direction_timing_execution_mode="overlap"
  initialize_direction_phase_timings ||
    fail "could not initialize overlap direction phase timings"
  prepare_actor_flow_runtime
  direction_phase_begin overlap_ready_barrier ||
    fail "could not begin overlap-ready-barrier timing"
  capture_both_statuses 0 overlap-ready-status
  write_overlap_marker ready 0
  await_overlap_permit lock 0
  direction_phase_end overlap_ready_barrier ||
    fail "could not end overlap-ready-barrier timing"
  run_actor_two_lock_phase
  direction_phase_begin overlap_locked_barrier ||
    fail "could not begin overlap-locked-barrier timing"
  capture_both_statuses 2 overlap-locked-status
  write_overlap_marker locked 2
  await_overlap_permit settle 2
  direction_phase_end overlap_locked_barrier ||
    fail "could not end overlap-locked-barrier timing"
  run_actor_settlement_phase
  direction_phase_begin overlap_terminal_marker ||
    fail "could not begin overlap-terminal-marker timing"
  write_overlap_marker terminal 4
  direction_phase_end overlap_terminal_marker ||
    fail "could not end overlap-terminal-marker timing"
  finalize_direction_phase_timings ||
    fail "could not publish overlap direction phase timings"
}

submission_counts() {
  local manifest="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actual-effects.json"
  local counts="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actual-submission-counts.json"
  local transaction response role journal matches bitcoin_count=0 lez_count=0
  local expected_bitcoin expected_lez
  local -a bitcoin_ids=() lez_ids=()
  [[ -f "$manifest" && ! -L "$manifest" ]] || fail "actual effect manifest is unavailable"
  mapfile -t bitcoin_ids < <(jq -er '.bitcoin_effect_ids[]' "$manifest")
  mapfile -t lez_ids < <(jq -er '.lez_effect_ids[]' "$manifest")
  expected_bitcoin="$(jq -er '.expected_unique_effects.bitcoin | numbers' "$manifest")"
  expected_lez="$(jq -er '.expected_unique_effects.lez | numbers' "$manifest")"
  [[ "${#bitcoin_ids[@]}" == "$expected_bitcoin" &&
     "${#lez_ids[@]}" == "$expected_lez" ]] ||
    fail "actual effect manifest has the wrong direction-specific cardinality"
  for transaction in "${bitcoin_ids[@]}"; do
    response="$(core_rpc maker getrawtransaction "[\"${transaction}\",true]")"
    jq -e --arg tx "$transaction" '
      .result.txid == $tx and (.result.confirmations | numbers) >= 1
    ' <<<"$response" >/dev/null || fail "Bitcoin effect is not uniquely confirmed"
    bitcoin_count=$((bitcoin_count + 1))
  done
  for transaction in "${lez_ids[@]}"; do
    matches=0
    for role in maker taker; do
      journal="${M3_POC_SECURE_STATE_ROOT}/sidecars/final/${role}/bridge-requests.v1.json"
      [[ -f "$journal" && ! -L "$journal" ]] || fail "LEZ submit journal is unavailable"
      matches=$((matches + $(jq -er --arg tx "$transaction" '
        [.entries[] |
          select(.method == "lez_bridge.v1.submit_transaction"
                 and .outcome.kind == "success"
                 and .outcome.value.transaction_id == $tx)] | length
      ' "$journal")))
    done
    [[ "$matches" == 1 ]] || fail "LEZ effect does not have exactly one durable submission"
    lez_count=$((lez_count + 1))
  done
  jq -n --arg direction "$M3_POC_DIRECTION" \
    --argjson bitcoin "$bitcoin_count" --argjson lez "$lez_count" \
    --slurpfile effects "$manifest" '
    {schema_version:1,direction:$direction,bitcoin:$bitcoin,lez:$lez,
     measurement:"confirmed_unique_bitcoin_effects_and_exact_durable_lez_submissions",
     effect_ids:{bitcoin:$effects[0].bitcoin_effect_ids,lez:$effects[0].lez_effect_ids}}
  ' >"$counts"
  chmod 0600 "$counts"
  jq -c . "$counts"
}

command_name="${1:-}"
case "$command_name" in
  contract)
    [[ "$#" == 1 ]] || fail "contract accepts no arguments"
    command -v jq >/dev/null || fail "jq is required"
    emit_contract
    ;;
  preflight)
    [[ "$#" == 1 ]] || fail "preflight accepts no arguments"
    preflight
    ;;
  effect-plan)
    [[ "$#" == 2 || "$#" == 3 ]] || fail "effect-plan requires a direction and optional journey"
    command -v jq >/dev/null || fail "jq is required"
    emit_effect_plan "$2" "${3:-claim}"
    ;;
  prepare-stage-two-spec)
    [[ "$#" == 2 ]] || fail "prepare-stage-two-spec requires one new output path"
    require_environment
    [[ "$2" == /* && ! -e "$2" && ! -L "$2" ]] ||
      fail "stage-two output must be one new absolute path"
    prepare_stage_two_spec "$2"
    ;;
  run-actor-flow)
    [[ "$#" == 1 ]] || fail "run-actor-flow accepts no arguments"
    require_environment
    run_actor_flow
    ;;
  run-overlap-actor-flow)
    [[ "$#" == 1 ]] || fail "run-overlap-actor-flow accepts no arguments"
    require_environment
    run_overlap_actor_flow
    ;;
  submission-counts)
    [[ "$#" == 1 ]] || fail "submission-counts accepts no arguments"
    require_environment
    submission_counts
    ;;
  *) fail "expected contract, preflight, effect-plan, prepare-stage-two-spec, run-actor-flow, run-overlap-actor-flow, or submission-counts" ;;
esac
