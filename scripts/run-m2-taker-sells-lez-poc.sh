#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
umask 077

readonly LEZ_SEQUENCER_URL="${LEZ_SEQUENCER_URL:-http://127.0.0.1:32828}"
readonly LEZ_INDEXER_URL="${LEZ_INDEXER_URL:-http://127.0.0.1:32829}"
readonly ZEBRA_RPC_URL="${ZEBRA_RPC_URL:-http://127.0.0.1:32830}"
readonly LEZ_CHAIN_ID="${LEZ_CHAIN_ID:-b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748}"
readonly LEZ_GENESIS_HASH="${LEZ_GENESIS_HASH:-e24c5a4a2d08a747b96cebefa1304cbe80e42dac9ced3a52c2330b22797e10d9}"
readonly ESCROW_PROGRAM_ID="${ESCROW_PROGRAM_ID:-b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433}"
readonly AUTHENTICATED_TRANSFER_PROGRAM_HEX="${AUTHENTICATED_TRANSFER_PROGRAM_HEX:-dcbbfebcd59399961ed9973b8307dc475fd4c5ca5779aacfe7588f7dbc3f4a71}"
readonly AUTHENTICATED_TRANSFER_PROGRAM_BASE58="${AUTHENTICATED_TRANSFER_PROGRAM_BASE58:-FrexXMbyY6iZjwUo8DV3jfB8donj8H4kLRHT7xswCfJg}"
readonly MAKER_ACCOUNT_BASE58="${MAKER_ACCOUNT_BASE58:-B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd}"
readonly TAKER_ACCOUNT_BASE58="${TAKER_ACCOUNT_BASE58:-34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib}"
readonly M5_LEZ_GUEST_SHA256="${M5_LEZ_GUEST_SHA256:-ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee}"
readonly M5_LEZ_DEPLOYMENT_EVIDENCE_FILE="${M5_LEZ_DEPLOYMENT_EVIDENCE_FILE:-}"
readonly M5_LEZ_FINALITY_EVIDENCE_FILE="${M5_LEZ_FINALITY_EVIDENCE_FILE:-}"
readonly M5_LEZ_ONBOARDING_EVIDENCE_FILE="${M5_LEZ_ONBOARDING_EVIDENCE_FILE:-}"
readonly M5_LEZ_MAKER_SIGNER_KEY_FILE="${M5_LEZ_MAKER_SIGNER_KEY_FILE:-}"
readonly M5_LEZ_TAKER_SIGNER_KEY_FILE="${M5_LEZ_TAKER_SIGNER_KEY_FILE:-}"
readonly POC_DIRECTION="${POC_DIRECTION:-taker_sells_lez}"
readonly M5_APPLICATION_MODE="${M5_APPLICATION_MODE:-0}"
readonly M6_TAKER_SERVICE_MODE="${M6_TAKER_SERVICE_MODE:-0}"
readonly M6_ZEC_JOURNEY="${M6_ZEC_JOURNEY:-claim}"
readonly M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION="${M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION:-0}"
readonly M7_ZEC_CRASH_BUILD_CACHE_ROOT="${M7_ZEC_CRASH_BUILD_CACHE_ROOT:-}"
readonly M7_ROUTE_HEALTH_CONFIG="${M7_ROUTE_HEALTH_CONFIG:-}"
readonly M7_ROUTE_HEALTH_POLL_MILLISECONDS="${M7_ROUTE_HEALTH_POLL_MILLISECONDS:-100}"
readonly DISCOVERY_BLOCKS=256
readonly POLL_INTERVAL_SECONDS=0.10
readonly MAX_PRE_EFFECT_SECONDS=25
readonly MAX_ACTOR_CALL_SECONDS=20
readonly MAX_DRIVE_RETRIES=8
readonly DRIVE_RETRY_DELAY_SECONDS=0.15
readonly RAPIDSNARK_LIB_DIR="${RAPIDSNARK_LIB_DIR:-/tmp/lez-atomic-swaps-tools/rapidsnark-v0.0.8/d4133227}"
readonly M6_SERVICE_QUERY_TIMEOUT_MS=15000
readonly M6_SERVICE_ACTION_TIMEOUT_MS=90000
readonly M6_REFUND_SUPERVISOR_ATTEMPT_TIMEOUT_MS=75000
readonly MAX_SUPERVISED_STATUS_RETRIES=8
readonly SUPERVISED_STATUS_RETRY_DELAY_SECONDS=0.05
readonly BINDGEN_EXTRA_CLANG_ARGS="${BINDGEN_EXTRA_CLANG_ARGS:--I/usr/lib/gcc/x86_64-linux-gnu/13/include}"
case "$M6_ZEC_JOURNEY" in
  claim)
    # Retain at least ten seconds against the local 60-second LEZ refund.
    MAX_CORRIDOR_SECONDS=49
    ;;
  refund)
    # The ceiling covers the fixed 60-second LEZ deadline, bounded transient
    # service-to-actor reconciliations, LEZ finality, Zcash CLTV recovery, and
    # terminal no-effect replay. It does not change any protocol deadline.
    MAX_CORRIDOR_SECONDS=300
    ;;
  *) echo 'M6_ZEC_JOURNEY must be claim or refund' >&2; exit 2 ;;
esac
readonly MAX_CORRIDOR_SECONDS
export RAPIDSNARK_LIB_DIR BINDGEN_EXTRA_CLANG_ARGS
export CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"

if [[ -n "${RUN_ID:-}" ]]; then
  run_id="$RUN_ID"
else
  run_id="m2poc-corridor-$(date -u +%Y%m%d%H%M%S)-$$"
fi
if [[ ! "$run_id" =~ ^[a-z0-9][a-z0-9_-]{7,63}$ ]]; then
  echo 'RUN_ID must be 8..=64 lowercase letters, numbers, underscores, or hyphens' >&2
  exit 2
fi
readonly run_id
if [[ "$M5_APPLICATION_MODE" != 0 && "$M5_APPLICATION_MODE" != 1 ]]; then
  echo 'M5_APPLICATION_MODE must be 0 or 1' >&2
  exit 2
fi
if [[ "$M6_TAKER_SERVICE_MODE" != 0 && "$M6_TAKER_SERVICE_MODE" != 1 ]]; then
  echo 'M6_TAKER_SERVICE_MODE must be 0 or 1' >&2
  exit 2
fi
if [[ "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" != 0 \
  && "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" != 1 ]]; then
  echo 'M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION must be 0 or 1' >&2
  exit 2
fi
if [[ "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" == 1 \
  && ( "$M5_APPLICATION_MODE" != 1 || "$M6_TAKER_SERVICE_MODE" != 0 \
    || "$M6_ZEC_JOURNEY" != claim ) ]]; then
  echo 'M7 accepted-ZEC process-kill mode requires M5 application Claim without the M6 service' >&2
  exit 2
fi
if [[ "$M6_TAKER_SERVICE_MODE" == 1 && "$M5_APPLICATION_MODE" != 1 ]]; then
  echo 'M6 Taker service mode requires M5_APPLICATION_MODE=1' >&2
  exit 2
fi
if [[ "$M6_ZEC_JOURNEY" == refund && "$M6_TAKER_SERVICE_MODE" != 1 ]]; then
  echo 'M6 refund journey requires M6_TAKER_SERVICE_MODE=1' >&2
  exit 2
fi
if [[ "$M5_APPLICATION_MODE" == 1 && ! "$run_id" =~ ^[a-z0-9][a-z0-9_-]{7,47}$ ]]; then
  echo 'M5 application RUN_ID must be 8..=48 safe characters' >&2
  exit 2
fi
if [[ "$M5_APPLICATION_MODE" == 1 && "$POC_DIRECTION" != taker_sells_lez ]]; then
  echo 'M5 application composition currently requires POC_DIRECTION=taker_sells_lez' >&2
  exit 2
fi
readonly private_base="${POC_OUTPUT_ROOT:-/tmp/lez-atomic-swaps-${run_id}}"
readonly spec_file="${private_base}/provision-spec.json"
readonly actors_root="${private_base}/actors"
readonly evidence_dir="${private_base}/evidence"
readonly application_root="${private_base}/application"
if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  provision_actors_root="${private_base}/actors-source"
else
  provision_actors_root="$actors_root"
fi
readonly provision_actors_root
readonly m5_handoff_driver="${PWD}/scripts/run-m5-zec-chat-handoff.sh"

for endpoint in "$LEZ_SEQUENCER_URL" "$LEZ_INDEXER_URL" "$ZEBRA_RPC_URL"; do
  if [[ ! "$endpoint" =~ ^http://127\.0\.0\.1:[1-9][0-9]{0,4}/?$ ]]; then
    echo "endpoint must be an explicit nonzero literal-loopback HTTP URL: ${endpoint}" >&2
    exit 2
  fi
  endpoint_port="${endpoint##*:}"
  endpoint_port="${endpoint_port%/}"
  if (( 10#$endpoint_port > 65535 )); then
    echo "endpoint port exceeds 65535: ${endpoint}" >&2
    exit 2
  fi
done
for value in \
  "$LEZ_CHAIN_ID" \
  "$LEZ_GENESIS_HASH" \
  "$ESCROW_PROGRAM_ID" \
  "$M5_LEZ_GUEST_SHA256" \
  "$AUTHENTICATED_TRANSFER_PROGRAM_HEX"; do
  if [[ ! "$value" =~ ^[0-9a-f]{64}$ || "$value" =~ ^0+$ ]]; then
    echo 'LEZ chain, genesis, and program identities must be nonzero lowercase hex32 values' >&2
    exit 2
  fi
done
for value in \
  "$AUTHENTICATED_TRANSFER_PROGRAM_BASE58" \
  "$MAKER_ACCOUNT_BASE58" \
  "$TAKER_ACCOUNT_BASE58"; do
  if [[ ! "$value" =~ ^[1-9A-HJ-NP-Za-km-z]{32,64}$ ]]; then
    echo 'LEZ base58 identities must be canonical-alphabet values of bounded length' >&2
    exit 2
  fi
done
if [[ "$MAKER_ACCOUNT_BASE58" == "$TAKER_ACCOUNT_BASE58" ]]; then
  echo 'maker and taker LEZ identities must be distinct' >&2
  exit 2
fi
case "$POC_DIRECTION" in
  taker_sells_lez)
    expected_zcash_funder_role='maker'
    expected_zcash_claimant_role='taker'
    expected_lez_depositor_role='taker'
    expected_lez_depositor_account="$TAKER_ACCOUNT_BASE58"
    ;;
  taker_sells_foreign)
    expected_zcash_funder_role='taker'
    expected_zcash_claimant_role='maker'
    expected_lez_depositor_role='maker'
    expected_lez_depositor_account="$MAKER_ACCOUNT_BASE58"
    ;;
  *)
    echo 'POC_DIRECTION must be taker_sells_lez or taker_sells_foreign' >&2
    exit 2
    ;;
esac
readonly expected_zcash_funder_role expected_zcash_claimant_role
readonly expected_lez_depositor_role expected_lez_depositor_account
if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  for deployment_evidence_file in \
    "$M5_LEZ_DEPLOYMENT_EVIDENCE_FILE" "$M5_LEZ_FINALITY_EVIDENCE_FILE" \
    "$M5_LEZ_ONBOARDING_EVIDENCE_FILE"; do
    if [[ "$deployment_evidence_file" != /* || ! -f "$deployment_evidence_file" \
      || -L "$deployment_evidence_file" \
      || "$(readlink -f -- "$deployment_evidence_file")" != "$deployment_evidence_file" ]]; then
      echo 'M5 requires absolute, canonical, regular LEZ deployment evidence files' >&2
      exit 2
    fi
    deployment_mode="$(stat -c %a -- "$deployment_evidence_file")"
    deployment_owner="$(stat -c %u -- "$deployment_evidence_file")"
    deployment_links="$(stat -c %h -- "$deployment_evidence_file")"
    deployment_size="$(stat -c %s -- "$deployment_evidence_file")"
    if (( (8#$deployment_mode & 077) != 0 || deployment_owner != EUID \
      || deployment_links != 1 || deployment_size == 0 || deployment_size > 65536 )); then
      echo 'M5 LEZ deployment evidence has unsafe owner, mode, links, or size' >&2
      exit 2
    fi
  done
  for signer_file in "$M5_LEZ_MAKER_SIGNER_KEY_FILE" "$M5_LEZ_TAKER_SIGNER_KEY_FILE"; do
    if [[ "$signer_file" != /* || ! -f "$signer_file" || -L "$signer_file" \
      || "$(readlink -f -- "$signer_file")" != "$signer_file" \
      || "$(stat -c %a -- "$signer_file")" != 600 \
      || "$(stat -c %u -- "$signer_file")" != "$EUID" \
      || "$(stat -c %h -- "$signer_file")" != 1 \
      || "$(stat -c %s -- "$signer_file")" != 65 ]]; then
      echo 'M5 LEZ signer file is absent, noncanonical, nonprivate, linked, or malformed' >&2
      exit 2
    fi
  done
  if [[ "$(stat -c %d:%i -- "$M5_LEZ_MAKER_SIGNER_KEY_FILE")" == \
    "$(stat -c %d:%i -- "$M5_LEZ_TAKER_SIGNER_KEY_FILE")" ]]; then
    echo 'M5 Maker and Taker LEZ signer files must be distinct' >&2
    exit 2
  fi
  if [[ "$MAKER_ACCOUNT_BASE58" == 34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib \
    || "$MAKER_ACCOUNT_BASE58" == B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd \
    || "$TAKER_ACCOUNT_BASE58" == B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd \
    || "$TAKER_ACCOUNT_BASE58" == 34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib ]]; then
    echo 'M5 requires fresh LEZ identities rather than deterministic fixture defaults' >&2
    exit 2
  fi
fi
if [[ -e "$private_base" ]]; then
  echo "refusing to reuse PoC output root: ${private_base}" >&2
  exit 2
fi

maker_pid=''
taker_pid=''
maker_start_ticks=''
taker_start_ticks=''
m5_daemon_pid=''
m5_daemon_start_ticks=''
m5_daemon_bin=''
m5_maker_socket=''
m5_application_database=''
m5_chat_socket=''
m5_delivery_directory=''
m5_delivery_offline=''
m5_supervisor_socket=''
m5_maker_actor_config=''
m5_maker_actor_state=''
m5_maker_bundle=''
m5_maker_actor_root=''
m5_maker_state_dir=''
m5_expected_funding_txid=''
m5_maker_phase='not_activated'
m5_transport_cutover_complete=0
m7_zec_process_kill_injected=0
m7_zec_process_kill_recovered=0
m6_service_pid=''
m6_service_start_ticks=''
m6_service_bin=''
m6_registry_init_bin=''
m6_service_socket=''
m6_service_config=''
m6_service_registry=''
m6_service_log=''
m6_initiate_request_id="m6-initiate-${run_id}"
m6_claim_request_id="m6-claim-${run_id}"
m6_claim_admitted=0
m6_claim_generation=''
m6_zcash_claim_txid=''
m6_refund_request_id="m6-refund-${run_id}"
m6_refund_admitted=0
m6_refund_generation=""
m6_lez_refund_txid=""
m6_lez_refund_finalized=0
m6_zcash_refund_txid=""
m6_zcash_refund_block_hash=""
m6_zcash_refund_block_height=""
m6_zcash_refund_mined=0
m6_maker_supervisor_suppressed=0
m6_maker_supervisor_restarted=0
m6_lez_refund_start_tip=""
corridor_deadline_monotonic_ms=''

process_start_ticks() {
  local pid="$1"
  awk '{print $22}' "/proc/${pid}/stat" 2>/dev/null
}

process_start_identity_matches() {
  local pid="$1"
  local start_ticks="$2"
  [[ -n "$pid" && -n "$start_ticks" && -r "/proc/${pid}/stat" ]] || return 1
  [[ "$(process_start_ticks "$pid")" == "$start_ticks" ]]
}

process_is_owned() {
  local pid="$1"
  local start_ticks="$2"
  local expected_exe="$3"
  process_start_identity_matches "$pid" "$start_ticks" || return 1
  [[ "$(readlink -f "/proc/${pid}/exe" 2>/dev/null)" == "$expected_exe" ]]
}

stop_owned_process() {
  local pid="$1"
  local start_ticks="$2"
  local expected_exe="$3"
  if ! process_is_owned "$pid" "$start_ticks" "$expected_exe"; then
    return 0
  fi
  kill -TERM "$pid" || true
  for _ in {1..40}; do
    if ! process_is_owned "$pid" "$start_ticks" "$expected_exe"; then
      wait "$pid" 2>/dev/null || true
      return 0
    fi
    sleep 0.05
  done
  if process_is_owned "$pid" "$start_ticks" "$expected_exe"; then
    kill -KILL "$pid" || true
  fi
  wait "$pid" 2>/dev/null || true
}

stop_owned_m5_daemon() {
  if ! process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin"; then
    return 0
  fi
  kill -INT "$m5_daemon_pid" || true
  for _ in {1..200}; do
    if ! process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin"; then
      wait "$m5_daemon_pid" 2>/dev/null || true
      return 0
    fi
    sleep 0.05
  done
  if process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin"; then
    kill -KILL "$m5_daemon_pid" || true
  fi
  wait "$m5_daemon_pid" 2>/dev/null || true
}

wait_for_m5_daemon_ready() {
  local socket="$1"
  local ready_file="$2"
  local log="$3"
  for _ in {1..200}; do
    if [[ -s "$ready_file" && "$(<"$ready_file")" == "$socket" && -S "$socket" ]]; then
      return 0
    fi
    process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin" || {
      tail -n 40 "$log" >&2 || true
      return 1
    }
    sleep 0.05
  done
  echo "M5 supervised daemon did not become ready on ${socket}" >&2
  tail -n 40 "$log" >&2 || true
  return 1
}

capture_m5_supervised_maker_status() {
  local output_file="$1"
  local stderr_file="$2"
  local label="$3"
  local attempt=1 actor_status=0
  while true; do
    actor_status=0
    if "$actor_bin" --config "$maker_config" status >"$output_file" 2>"$stderr_file"; then
      [[ ! -s "$stderr_file" ]] || {
        echo 'M5 supervised Maker status emitted unexpected diagnostics' >&2
        return 1
      }
      return 0
    else
      actor_status=$?
    fi
    process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin" || {
      echo 'M5 Maker supervisor daemon exited during status observation' >&2
      return 1
    }
    if (( actor_status != 2 || attempt > MAX_SUPERVISED_STATUS_RETRIES )) \
      || [[ -s "$output_file" || ! -f "$stderr_file" || -L "$stderr_file" ]] \
      || [[ "$(stat -c %s -- "$stderr_file")" != 35 ]] \
      || [[ "$(<"$stderr_file")" != 'actor configuration is unavailable' ]]; then
      echo 'M5 supervised Maker status failed outside the exact retriable class' >&2
      return 1
    fi
    jq -nc --arg label "$label" --argjson attempt "$attempt" '
      {
        schema_version: 1,
        event: "supervised_maker_status_retry",
        label: $label,
        attempt: $attempt,
        error_class: "actor_configuration_unavailable"
      }' >>"${evidence_dir}/m5-maker-status-retries.ndjson"
    remaining_budget_milliseconds "${label}-config-retry-${attempt}" >/dev/null || return
    sleep "$SUPERVISED_STATUS_RETRY_DELAY_SECONDS"
    attempt=$((attempt + 1))
  done
}

start_m5_full_supervised_daemon() {
  local instance="${1:-initial}"
  local ready_file log actor_attempt_timeout_ms=20000
  local -a test_pause_arguments=()
  case "$instance" in
    initial)
      ready_file="${application_root}/runtime/ready-supervised"
      log="${evidence_dir}/m5-maker-daemon-supervised.log"
      ;;
    recovery)
      ready_file="${application_root}/runtime/ready-supervised-recovery"
      log="${evidence_dir}/m7-zec-maker-daemon-recovery.log"
      ;;
    *) return 1 ;;
  esac
  if [[ "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" == 1 ]]; then
    # Give the external feature-only crash coordinator enough time to observe
    # the accepted marker under host contention. Production retains 20s.
    actor_attempt_timeout_ms=120000
    test_pause_arguments=(
      --actor-test-pause-swap-id "$m5_swap_id"
      --actor-test-pause-operation zcash_fund
      --actor-test-pause-marker "${application_root}/runtime/m7-zec-funding-submitted.json"
    )
  fi
  [[ ! -e "$ready_file" && ! -e "$m5_maker_socket" && ! -e "$m5_chat_socket" ]] || {
    echo 'M5 full supervised daemon endpoints already exist' >&2
    return 1
  }
  "$m5_daemon_bin" \
    --socket "$m5_maker_socket" \
    --chat-socket "$m5_chat_socket" \
    --database "$m5_application_database" \
    --ready-file "$ready_file" \
    --delivery-directory "$m5_delivery_directory" \
    --delivery-signing-key-file "$provision_actors_root/maker/zcash.key" \
    --maker-claim-key-id "${run_id}-maker-claim" \
    --maker-claim-key-file "$provision_actors_root/maker/claim-recovery.key" \
    --maker-claim-preimage-file "$provision_actors_root/maker/claim-preimage.key" \
    --zec-source-maker-config "$provision_actors_root/maker/actor-config.json" \
    --zec-maker-actor-root "$m5_maker_actor_root" \
    --zec-actor-program "$m5_actor_program" \
    --zec-actor-program-sha256 "$m5_actor_program_sha256" \
    --actor-supervisor \
    --actor-attempt-timeout-milliseconds "$actor_attempt_timeout_ms" \
    --actor-effect-cutoff-boottime-milliseconds "$corridor_deadline_monotonic_ms" \
    --actor-poll-milliseconds 10 \
    --actor-requeue-delay-seconds 1 \
    --actor-failure-backoff-seconds 1 \
    --actor-max-output-bytes 8192 \
    "${test_pause_arguments[@]}" \
    >"$log" 2>&1 &
  m5_daemon_pid=$!
  m5_daemon_start_ticks="$(process_start_ticks "$m5_daemon_pid")"
  [[ -n "$m5_daemon_start_ticks" ]] || {
    echo 'M5 full supervised daemon identity is unavailable' >&2
    return 1
  }
  wait_for_m5_daemon_ready "$m5_maker_socket" "$ready_file" "$log"
  [[ -S "$m5_chat_socket" && -d "$m5_delivery_directory" ]] || {
    echo 'M5 full supervised daemon did not retain Chat and Delivery' >&2
    return 1
  }
}

start_m5_supervisor_only_daemon() {
  local actor_attempt_timeout_ms=20000
  if (( $# == 1 )); then
    actor_attempt_timeout_ms="$1"
  elif (( $# != 0 )); then
    return 1
  fi
  [[ "$actor_attempt_timeout_ms" =~ ^[0-9]+$
    && "$actor_attempt_timeout_ms" -ge 1 && "$actor_attempt_timeout_ms" -le 300000 ]] || return 1
  local ready_file="${application_root}/runtime/ready-supervisor-only"
  local log="${evidence_dir}/m5-maker-daemon-supervisor-only.log"
  [[ ! -e "$ready_file" && ! -e "$m5_supervisor_socket" \
    && ! -e "$m5_maker_socket" && ! -e "$m5_chat_socket" \
    && ! -e "$m5_delivery_directory" && -d "$m5_delivery_offline" ]] || {
    echo 'M5 supervisor-only isolation precondition failed' >&2
    return 1
  }
  "$m5_daemon_bin" \
    --socket "$m5_supervisor_socket" \
    --database "$m5_application_database" \
    --ready-file "$ready_file" \
    --actor-supervisor \
    --actor-attempt-timeout-milliseconds "$actor_attempt_timeout_ms" \
    --actor-effect-cutoff-boottime-milliseconds "$corridor_deadline_monotonic_ms" \
    --actor-poll-milliseconds 10 \
    --actor-requeue-delay-seconds 1 \
    --actor-failure-backoff-seconds 1 \
    --actor-max-output-bytes 8192 \
    >"$log" 2>&1 &
  m5_daemon_pid=$!
  m5_daemon_start_ticks="$(process_start_ticks "$m5_daemon_pid")"
  [[ -n "$m5_daemon_start_ticks" ]] || {
    echo 'M5 supervisor-only daemon identity is unavailable' >&2
    return 1
  }
  wait_for_m5_daemon_ready "$m5_supervisor_socket" "$ready_file" "$log"
  [[ ! -e "$m5_maker_socket" && ! -e "$m5_chat_socket" \
    && ! -e "$m5_delivery_directory" && -d "$m5_delivery_offline" ]] || {
    echo 'M5 supervisor-only daemon restored a negotiation transport' >&2
    return 1
  }
}

prove_m5_terminal_operator_projection() {
  local swap_id actor_state claim_key_id claim_key_file terminal_ready terminal_log
  local history_file status_file terminal_receipt ready=0 expected_phase expected_phase_lower
  swap_id="$(jq -er '.swap_id | strings' "$maker_config")"
  actor_state="$(jq -er '.role_state_db | strings' "$maker_config")"
  claim_key_id="$(jq -er '.claim_recovery.key_id | strings' "$maker_config")"
  claim_key_file="$(jq -er '.claim_recovery.key_file | strings' "$maker_config")"
  jq -e --arg swap "$swap_id" '
    .role == "maker" and .swap_id == $swap
  ' "$maker_config" >/dev/null

  terminal_ready="${application_root}/runtime/ready-terminal"
  terminal_log="${evidence_dir}/m5-terminal-maker-daemon.log"
  history_file="${evidence_dir}/m5-history-after-terminal-restart.json"
  status_file="${evidence_dir}/m5-status-after-terminal-restart.json"
  terminal_receipt="${evidence_dir}/m5-terminal-operator-projection.json"
  if [[ "$M6_ZEC_JOURNEY" == refund ]]; then
    expected_phase=Refunded
    expected_phase_lower=refunded
  else
    expected_phase=Completed
    expected_phase_lower=completed
  fi
  [[ ! -e "$terminal_ready" && ! -e "$m5_maker_socket" ]] || {
    echo 'terminal M5 owner endpoint already exists' >&2
    return 1
  }

  "$m5_daemon_bin" \
    --socket "$m5_maker_socket" \
    --database "$m5_application_database" \
    --ready-file "$terminal_ready" \
    --terminal-zec-maker-state-db "$actor_state" \
    --terminal-zec-swap-id "$swap_id" \
    --terminal-zec-claim-key-id "$claim_key_id" \
    --terminal-zec-claim-key-file "$claim_key_file" \
    >"$terminal_log" 2>&1 &
  m5_daemon_pid=$!
  for _ in {1..40}; do
    m5_daemon_start_ticks="$(process_start_ticks "$m5_daemon_pid")"
    [[ -n "$m5_daemon_start_ticks" ]] && break
    sleep 0.05
  done
  process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin" || {
    tail -n 40 "$terminal_log" >&2 || true
    return 1
  }
  for _ in {1..200}; do
    if [[ -f "$terminal_ready" && "$(<"$terminal_ready")" == "$m5_maker_socket" \
      && -S "$m5_maker_socket" ]]; then
      ready=1
      break
    fi
    process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin" || {
      tail -n 40 "$terminal_log" >&2 || true
      return 1
    }
    sleep 0.05
  done
  (( ready == 1 )) || {
    echo 'terminal M5 owner daemon did not become ready' >&2
    return 1
  }
  [[ ! -e "$m5_chat_socket" && ! -e "$m5_delivery_directory" \
    && -d "$m5_delivery_offline" ]] || {
    echo 'terminal M5 restart restored a removed negotiation transport' >&2
    return 1
  }

  "$maker_cli_bin" --socket "$m5_maker_socket" history >"$history_file"
  "$maker_cli_bin" --socket "$m5_maker_socket" status --id "$swap_id" >"$status_file"
  jq -e --arg swap "$swap_id" --arg phase "$expected_phase" '
    length == 1 and .[0].id == $swap and .[0].phase == $phase
  ' "$history_file" >/dev/null
  jq -e --arg swap "$swap_id" --arg phase "$expected_phase" '
    .id == $swap and .phase == $phase
  ' "$status_file" >/dev/null

  stop_owned_m5_daemon
  m5_daemon_pid=''
  m5_daemon_start_ticks=''
  [[ ! -e "$m5_maker_socket" && ! -e "$terminal_ready" \
    && ! -e "$m5_chat_socket" && ! -e "$m5_delivery_directory" \
    && -d "$m5_delivery_offline" ]] || {
    echo 'terminal M5 restart cleanup or transport isolation failed' >&2
    return 1
  }
  jq -n --arg swap_id "$swap_id" \
    --arg phase "$expected_phase_lower" \
    --arg history_sha256 "$(sha256sum "$history_file" | cut -d ' ' -f1)" \
    --arg status_sha256 "$(sha256sum "$status_file" | cut -d ' ' -f1)" \
    --argjson source_revision "$(jq -er '.revision | numbers' "${evidence_dir}/maker-status-final.json")" '
    {
      schema_version: 1,
      result: "passed",
      swap_id: $swap_id,
      source: {role:"maker",revision:$source_revision,offline_full_history_replay:true},
      operator_history_phase: $phase,
      operator_status_phase: $phase,
      history_sha256: $history_sha256,
      status_sha256: $status_sha256,
      owner_socket_removed_after_query: true,
      chat_remained_absent: true,
      delivery_remained_offline: true,
      chain_rpc_used_during_import: false,
      private_material_disclosed: false
    }' >"$terminal_receipt"
  chmod 0600 "$terminal_receipt"
}

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  if [[ -n "$m6_service_bin" ]]; then
    stop_owned_process "$m6_service_pid" "$m6_service_start_ticks" "$m6_service_bin"
  fi
  if [[ -n "$m5_daemon_bin" ]]; then
    stop_owned_m5_daemon
  fi
  if [[ -n "${sidecar_bin:-}" ]]; then
    stop_owned_process "$maker_pid" "$maker_start_ticks" "$sidecar_bin"
    stop_owned_process "$taker_pid" "$taker_start_ticks" "$sidecar_bin"
  fi
  if (( status != 0 )); then
    if [[ -d "$private_base" ]]; then
      echo "M2 ${POC_DIRECTION} PoC failed; retained private evidence: ${private_base}" >&2
    else
      echo "M2 ${POC_DIRECTION} PoC failed before its evidence root was created" >&2
    fi
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

require_command() {
  command -v "$1" >/dev/null || {
    echo "required command is unavailable: $1" >&2
    exit 2
  }
}
for command in awk base64 cargo curl date flock jq kill od perl readlink sha256sum sleep stat tail timeout tr xxd; do
  require_command "$command"
done
if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  require_command cmp
  require_command install
  require_command strip
fi
if [[ "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" == 1 ]]; then
  require_command rm
fi

# A retained local node tuple may service only one effect-bearing corridor at a
# time. The lock is scoped to the exact endpoints and does not touch unrelated
# Docker resources or processes.
endpoint_lock_key="$(printf '%s\n' \
  "$LEZ_SEQUENCER_URL" "$LEZ_INDEXER_URL" "$ZEBRA_RPC_URL" | sha256sum)"
endpoint_lock_file="/tmp/lez-atomic-swaps-corridor-${endpoint_lock_key%% *}.lock"
readonly endpoint_lock_file
exec 9>"$endpoint_lock_file"
if ! flock -n 9; then
  echo 'another corridor runner owns the configured local node endpoints' >&2
  exit 2
fi

monotonic_milliseconds() {
  [[ -r /proc/uptime ]] || {
    echo 'monotonic /proc/uptime clock is unavailable' >&2
    return 2
  }
  local uptime whole fraction
  read -r uptime _ </proc/uptime
  [[ "$uptime" =~ ^([0-9]+)\.([0-9]+)$ ]] || {
    echo 'monotonic /proc/uptime clock has an invalid format' >&2
    return 2
  }
  whole="${BASH_REMATCH[1]}"
  fraction="${BASH_REMATCH[2]}000"
  fraction="${fraction:0:3}"
  printf '%s\n' "$((10#$whole * 1000 + 10#$fraction))"
}

remaining_budget_milliseconds() {
  local stage="$1"
  [[ -n "$corridor_deadline_monotonic_ms" ]] || {
    echo "corridor deadline is unavailable at ${stage}" >&2
    return 2
  }
  local now remaining
  now="$(monotonic_milliseconds)" || return
  remaining=$((corridor_deadline_monotonic_ms - now))
  (( remaining > 0 )) || {
    echo "corridor budget exhausted at ${stage}" >&2
    return 1
  }
  printf '%s\n' "$remaining"
}

format_milliseconds() {
  local milliseconds="$1"
  printf '%d.%03d\n' "$((milliseconds / 1000))" "$((milliseconds % 1000))"
}

bounded_actor_timeout() {
  local stage="$1"
  local remaining maximum
  remaining="$(remaining_budget_milliseconds "${stage}-before")" || return
  maximum=$((MAX_ACTOR_CALL_SECONDS * 1000))
  (( remaining <= maximum )) || remaining="$maximum"
  format_milliseconds "$remaining"
}

verify_native_library() {
  local filename="$1"
  local expected="$2"
  local path="${RAPIDSNARK_LIB_DIR}/${filename}"
  [[ -f "$path" ]] || {
    echo "missing contracted native library: ${path}" >&2
    exit 2
  }
  local actual
  actual="$(sha256sum -- "$path")"
  actual="${actual%% *}"
  [[ "$actual" == "$expected" ]] || {
    echo "native-library identity drift for ${filename}" >&2
    exit 2
  }
}
[[ "$RAPIDSNARK_LIB_DIR" == /* && -d "$RAPIDSNARK_LIB_DIR" ]] || {
  echo 'RAPIDSNARK_LIB_DIR must be an existing absolute directory' >&2
  exit 2
}
[[ "$BINDGEN_EXTRA_CLANG_ARGS" == '-I/usr/lib/gcc/x86_64-linux-gnu/13/include' ]] || {
  echo 'BINDGEN_EXTRA_CLANG_ARGS does not match the pinned sidecar contract' >&2
  exit 2
}
verify_native_library librapidsnark.a d4133227f845ff5bfa3672eb5b9c018a6a086bfa164b176bdaf76949c7d1f423
verify_native_library libgmp.a 0a910b420c3ad603c83c9dc2818c7ae05394c231ca23135c7b873e8e680ea41b
verify_native_library libfq.a 797b5d24bb8e8b088f811bddfff35f33973af9c797fb3812489cd42ba6a957d0
verify_native_library libfr.a 40f809394904682cb5517845cd3c2f936a5eb4609712534b573f552f2811fb82

echo 'Prebuilding the provisioner, actor, and exact v0.2 bridge before provisioning'
workspace_target_root=''
sidecar_target_root=''
if [[ "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" == 1 ]]; then
  mkdir -m 0700 "$private_base"
  if [[ -n "$M7_ZEC_CRASH_BUILD_CACHE_ROOT" ]]; then
    if [[ "$M7_ZEC_CRASH_BUILD_CACHE_ROOT" != /* \
      || ! -d "$M7_ZEC_CRASH_BUILD_CACHE_ROOT" \
      || -L "$M7_ZEC_CRASH_BUILD_CACHE_ROOT" \
      || "$(readlink -f -- "$M7_ZEC_CRASH_BUILD_CACHE_ROOT")" != "$M7_ZEC_CRASH_BUILD_CACHE_ROOT" \
      || "$(stat -c %a -- "$M7_ZEC_CRASH_BUILD_CACHE_ROOT")" != 700 \
      || "$(stat -c %u -- "$M7_ZEC_CRASH_BUILD_CACHE_ROOT")" != "$EUID" ]]; then
      echo 'M7 crash build cache must be an existing canonical owner-private directory' >&2
      exit 2
    fi
    workspace_target_root="${M7_ZEC_CRASH_BUILD_CACHE_ROOT}/workspace-target"
    sidecar_target_root="${M7_ZEC_CRASH_BUILD_CACHE_ROOT}/sidecar-target"
  else
    workspace_target_root="${private_base}/workspace-target"
    sidecar_target_root="${private_base}/sidecar-target"
  fi
  mkdir -p -m 0700 "$workspace_target_root" "$sidecar_target_root"
  for target_root in "$workspace_target_root" "$sidecar_target_root"; do
    if [[ ! -d "$target_root" || -L "$target_root" \
      || "$(readlink -f -- "$target_root")" != "$target_root" \
      || "$(stat -c %a -- "$target_root")" != 700 \
      || "$(stat -c %u -- "$target_root")" != "$EUID" ]]; then
      echo 'M7 crash build target must be a canonical owner-private directory' >&2
      exit 2
    fi
  done
  CARGO_TARGET_DIR="$workspace_target_root" cargo +1.96.0 build --locked --offline \
    --release -p zec-reference-actor --features test-crash-hooks --bins
elif [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  cargo +1.96.0 build --locked --offline --release -p zec-reference-actor --bins
else
  cargo +1.96.0 build --locked --offline -p zec-reference-actor --bins
fi
if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  if [[ "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" == 1 ]]; then
    CARGO_TARGET_DIR="$workspace_target_root" cargo +1.96.0 build --locked --offline \
      --release -p lez-maker-node --features test-crash-hooks --bins
    CARGO_TARGET_DIR="$workspace_target_root" cargo +1.96.0 build --locked --offline \
      --release -p lez-maker-node --features test-crash-hooks --example maker-actor-inspect
    CARGO_TARGET_DIR="$workspace_target_root" cargo +1.96.0 build --locked --offline \
      --release -p lez-maker-node --features test-crash-hooks \
      --example maker-zec-lock-intent-inspect
    CARGO_TARGET_DIR="$sidecar_target_root" cargo +1.96.0 build \
      --manifest-path compat/lez-v0_2-sidecar/Cargo.toml \
      --locked --offline --release --bin lez-v02-bridge-poc
  else
    cargo +1.96.0 build --locked --offline --release -p lez-maker-node --bins
    cargo +1.96.0 build --locked --offline --release -p lez-maker-node --example maker-actor-inspect
    cargo +1.96.0 build --locked --offline --release -p lez-maker-node --example maker-zec-lock-intent-inspect
    cargo +1.96.0 build \
      --manifest-path compat/lez-v0_2-sidecar/Cargo.toml \
      --locked --offline --release --bin lez-v02-bridge-poc
  fi
else
  cargo +1.96.0 build \
    --manifest-path compat/lez-v0_2-sidecar/Cargo.toml \
    --locked --offline --bin lez-v02-bridge-poc
fi

if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  if [[ "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" == 1 ]]; then
    actor_bin="$(readlink -f "${workspace_target_root}/release/zec-reference-actor")"
    provisioner_bin="$(readlink -f "${workspace_target_root}/release/zec-local-poc-provision")"
    sidecar_bin="$(readlink -f "${sidecar_target_root}/release/lez-v02-bridge-poc")"
  else
    actor_bin="$(readlink -f target/release/zec-reference-actor)"
    provisioner_bin="$(readlink -f target/release/zec-local-poc-provision)"
    sidecar_bin="$(readlink -f compat/lez-v0_2-sidecar/target/release/lez-v02-bridge-poc)"
  fi
else
  actor_bin="$(readlink -f target/debug/zec-reference-actor)"
  provisioner_bin="$(readlink -f target/debug/zec-local-poc-provision)"
  sidecar_bin="$(readlink -f compat/lez-v0_2-sidecar/target/debug/lez-v02-bridge-poc)"
fi
readonly actor_bin provisioner_bin sidecar_bin
required_binaries=("$actor_bin" "$provisioner_bin" "$sidecar_bin")
if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  if [[ "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" == 1 ]]; then
    maker_daemon_bin="$(readlink -f "${workspace_target_root}/release/lez-maker-daemon")"
    maker_cli_bin="$(readlink -f "${workspace_target_root}/release/lez-maker")"
    taker_bin="$(readlink -f "${workspace_target_root}/release/lez-taker")"
    chat_draft_bin="$(readlink -f "${workspace_target_root}/release/zec-local-poc-chat-draft")"
    chat_finalize_bin="$(readlink -f "${workspace_target_root}/release/zec-local-poc-chat-finalize")"
    actor_inspector_bin="$(readlink -f "${workspace_target_root}/release/examples/maker-actor-inspect")"
    m5_pair_inspector_bin="$(readlink -f "${workspace_target_root}/release/zec-actor-pair-inspect")"
    m5_intent_inspector_bin="$(readlink -f "${workspace_target_root}/release/examples/maker-zec-lock-intent-inspect")"
  else
    maker_daemon_bin="$(readlink -f target/release/lez-maker-daemon)"
    maker_cli_bin="$(readlink -f target/release/lez-maker)"
    taker_bin="$(readlink -f target/release/lez-taker)"
    chat_draft_bin="$(readlink -f target/release/zec-local-poc-chat-draft)"
    chat_finalize_bin="$(readlink -f target/release/zec-local-poc-chat-finalize)"
    actor_inspector_bin="$(readlink -f target/release/examples/maker-actor-inspect)"
    m5_pair_inspector_bin="$(readlink -f target/release/zec-actor-pair-inspect)"
    m5_intent_inspector_bin="$(readlink -f target/release/examples/maker-zec-lock-intent-inspect)"
  fi
  readonly maker_daemon_bin maker_cli_bin taker_bin chat_draft_bin chat_finalize_bin
  readonly actor_inspector_bin m5_pair_inspector_bin m5_intent_inspector_bin
  if [[ "$M6_TAKER_SERVICE_MODE" == 1 ]]; then
    m6_service_bin="$(readlink -f target/release/lez-taker-service)"
    m6_registry_init_bin="$(readlink -f target/release/lez-taker-registry-init)"
    readonly m6_service_bin m6_registry_init_bin
  fi
  required_binaries+=("$maker_daemon_bin" "$maker_cli_bin" "$taker_bin")
  required_binaries+=("$chat_draft_bin" "$chat_finalize_bin" "$actor_inspector_bin")
  required_binaries+=("$m5_pair_inspector_bin")
  required_binaries+=("$m5_handoff_driver")
  required_binaries+=("$m5_intent_inspector_bin")
  if [[ "$M6_TAKER_SERVICE_MODE" == 1 ]]; then
    required_binaries+=("$m6_service_bin" "$m6_registry_init_bin")
  fi
fi
for binary in "${required_binaries[@]}"; do
  [[ -x "$binary" ]] || {
    echo "prebuilt binary is unavailable: ${binary}" >&2
    exit 2
  }
done
unset required_binaries

rpc() {
  local endpoint="$1"
  local request="$2"
  local connect_timeout_ms=2000
  local request_timeout_ms=15000
  local connect_timeout request_timeout remaining response
  if [[ -n "$corridor_deadline_monotonic_ms" ]]; then
    remaining="$(remaining_budget_milliseconds 'rpc-before')" || return
    (( request_timeout_ms <= remaining )) || request_timeout_ms="$remaining"
    (( connect_timeout_ms <= remaining )) || connect_timeout_ms="$remaining"
  fi
  connect_timeout="$(format_milliseconds "$connect_timeout_ms")"
  request_timeout="$(format_milliseconds "$request_timeout_ms")"
  response="$(curl --fail --silent --show-error --noproxy '*' \
    --connect-timeout "$connect_timeout" --max-time "$request_timeout" \
    -H 'content-type: application/json' --data "$request" "$endpoint")" || return
  printf '%s\n' "$response"
  if [[ -n "$corridor_deadline_monotonic_ms" ]]; then
    remaining_budget_milliseconds 'rpc-after' >/dev/null || return
  fi
}


m6_service_rpc() {
  local label="$1"
  local request="$2"
  local request_timeout_ms="${3:-$M6_SERVICE_QUERY_TIMEOUT_MS}"
  local remaining request_timeout response
  remaining="$(remaining_budget_milliseconds "${label}-before")" || return
  (( request_timeout_ms <= remaining )) || request_timeout_ms="$remaining"
  request_timeout="$(format_milliseconds "$request_timeout_ms")"
  response="$(curl --fail --silent --show-error --noproxy '*' \
    --connect-timeout 1 --max-time "$request_timeout" \
    --unix-socket "$m6_service_socket" \
    -H 'content-type: application/json' --data "$request" http://localhost/)" || return
  printf '%s\n' "$response"
  remaining_budget_milliseconds "${label}-after" >/dev/null
}

wait_for_m6_service_ready() {
  local health_request health_response
  health_request='{"jsonrpc":"2.0","id":"m6-health","method":"taker_health","params":[{"schema_version":1}]}'
  for _ in {1..100}; do
    process_is_owned "$m6_service_pid" "$m6_service_start_ticks" "$m6_service_bin" || break
    if [[ -S "$m6_service_socket" \
      && "$(stat -c %a -- "$m6_service_socket" 2>/dev/null)" == 600 \
      && "$(stat -c %u -- "$m6_service_socket" 2>/dev/null)" == "$(id -u)" ]]; then
      if health_response="$(m6_service_rpc 'm6-health' "$health_request" 2>/dev/null)" \
        && jq -e '
          .error == null and .result.schema_version == 1 and .result.ready == true
          and .result.registered_methods == {
            health:true,offer_list:true,swap_list:true,initiate:true,
            monitor:true,claim:true,refund:true
          }
        ' <<<"$health_response" >/dev/null; then
        printf '%s\n' "$health_response" >"${evidence_dir}/m6-taker-service-health-initial.json"
        chmod 0600 "${evidence_dir}/m6-taker-service-health-initial.json"
        return 0
      fi
    fi
    sleep 0.05
  done
  echo 'M6 Taker service did not become ready with seven methods' >&2
  sed -n '1,30p' "$m6_service_log" >&2 || true
  return 1
}


start_m6_taker_service() {
  [[ "$M6_TAKER_SERVICE_MODE" == 1 ]] || return 0
  local handoff="${evidence_dir}/m5-chat-handoff.json"
  local discovery="${evidence_dir}/m5-delivery-discovery.json"
  local offer_id reservation_id maker_key signed_envelope
  local signed_sha draft source_config signing_key agreement_output actor_root
  local offer_request offer_response initiate_request initiate_response replay_response

  offer_id="$(jq -er '.offer_id | strings' "$handoff")"
  reservation_id="$(jq -er '.reservation_id | strings' "$handoff")"
  maker_key="$(jq -er '.role_public_keys.maker | strings' "$handoff")"
  signed_envelope="${m5_delivery_directory}/${offer_id}.offer.json"
  draft="${application_root}/unsigned-draft.borsh"
  signing_key="${provision_actors_root}/taker/zcash.key"
  source_config="${provision_actors_root}/taker/actor-config.json"
  agreement_output="${application_root}/final-agreement.borsh"
  actor_root="${application_root}/taker-actors"
  m6_service_registry="${application_root}/taker-service.sqlite3"
  m6_service_config="${application_root}/taker-service.json"
  m6_service_socket="${application_root}/runtime/taker-service.sock"
  m6_service_log="${evidence_dir}/m6-taker-service.log"

  jq -e --arg offer "$offer_id" --arg maker "$maker_key" '
    .schema_version == 1 and (.offers | length) == 1
    and .offers[0].offer.id == $offer and .offers[0].maker_public_key == $maker
    and .offers[0].offer.pair_configuration.route == {pair:"Zcash",direction:"TakerSellsLez"}
    and .offers[0].offer.pair_configuration.minimum_foreign_units == 100000000
    and .offers[0].offer.pair_configuration.maximum_foreign_units == 100000000
    and .offers[0].offer.price.lez_units_per_lot == 1
    and .offers[0].offer.price.foreign_units_per_lot == 2000
    and (.offers[0].signed_envelope_sha256 | test("^[0-9a-f]{64}$"))
  ' "$discovery" >/dev/null || {
    echo 'M6 authenticated offer terms do not yield the exact integral quote' >&2
    return 1
  }
  signed_sha="$(jq -er '.offers[0].signed_envelope_sha256' "$discovery")"
  [[ -f "$signed_envelope" && ! -L "$signed_envelope"
    && "$(sha256sum "$signed_envelope" | cut -d ' ' -f1)" == "$signed_sha"
    && -f "$draft" && ! -L "$draft"
    && -f "$signing_key" && ! -L "$signing_key"
    && -f "$source_config" && ! -L "$source_config"
    && -f "$agreement_output" && ! -L "$agreement_output"
    && -d "$actor_root" && ! -L "$actor_root" ]] || {
    echo 'M6 prepared service authority is unavailable or drifted' >&2
    return 1
  }

  "$m6_registry_init_bin" --database "$m6_service_registry"
  jq -n --arg delivery "$m5_delivery_directory" --arg maker "$maker_key" \
    --arg chat "$m5_chat_socket" --arg registry "$m6_service_registry" \
    --arg swap "$m5_swap_id" --arg offer "$offer_id" \
    --arg reservation "$reservation_id" --arg envelope "$signed_envelope" \
    --arg envelope_sha "$signed_sha" --arg draft "$draft" \
    --arg draft_sha "$(sha256sum "$draft" | cut -d ' ' -f1)" \
    --arg signing_key "$signing_key" --arg source_config "$source_config" \
    --arg source_sha "$(sha256sum "$source_config" | cut -d ' ' -f1)" \
    --arg agreement_output "$agreement_output" --arg actor_root "$actor_root" \
    --arg receipt "$m5_taker_acceptance_receipt" '
    {schema_version:1,delivery_sources:[{source_id:"m6-local-maker",directory:$delivery,
      maker_public_key:$maker}],chat_socket:$chat,maximum_offers:16,initiation:{
      execute_prepared_zec:true,registry_database:$registry,prepared_zec:[{
        source_id:"m6-local-maker",swap_id:$swap,offer_id:$offer,reservation_id:$reservation,
        foreign_units:100000000,lez_units:50000,
        signed_envelope:{path:$envelope,sha256:$envelope_sha},
        unsigned_draft:{path:$draft,sha256:$draft_sha},
        signing_key:{path:$signing_key},
        source_config:{path:$source_config,sha256:$source_sha},
        agreement_output:$agreement_output,actor_root:$actor_root,receipt_output:$receipt
      }]}}
  ' >"$m6_service_config"
  chmod 0600 "$m6_service_config"

  "$m6_service_bin" --config "$m6_service_config" --socket "$m6_service_socket" \
    >"$m6_service_log" 2>&1 &
  m6_service_pid=$!
  m6_service_start_ticks="$(process_start_ticks "$m6_service_pid")"
  [[ -n "$m6_service_start_ticks" ]] || return 1
  wait_for_m6_service_ready

  offer_request='{"jsonrpc":"2.0","id":"m6-offers","method":"taker_offer_list_v1","params":[{"schema_version":1,"route":{"pair":"Zcash","direction":"TakerSellsLez"}}]}'
  offer_response="$(m6_service_rpc 'm6-offers' "$offer_request")"
  printf '%s\n' "$offer_response" >"${evidence_dir}/m6-taker-service-offers.json"
  jq -e --arg offer "$offer_id" --arg maker "$maker_key" '
    .error == null and .result.schema_version == 1 and (.result.offers | length) == 1
    and .result.offers[0].offer.id == $offer and .result.offers[0].maker_identity == $maker
    and (.result.offers[0].signed_envelope_sha256 | arrays | length) == 32
  ' <<<"$offer_response" >/dev/null

  initiate_request="$(jq -nc --arg request_id "$m6_initiate_request_id" \
    --arg offer "$offer_id" --argjson selected "$(jq -c '.result.offers[0]' <<<"$offer_response")" '
    {jsonrpc:"2.0",id:"m6-initiate",method:"taker_swap_initiate_v1",params:[{
      schema_version:1,request_id:$request_id,offer_id:$offer,
      route:{pair:"Zcash",direction:"TakerSellsLez"},maker_identity:$selected.maker_identity,
      signed_envelope_sha256:$selected.signed_envelope_sha256,foreign_units:100000000,
      expected_lez_units:50000}]}
  ')"
  printf '%s\n' "$initiate_request" >"${evidence_dir}/m6-taker-service-init-request.json"
  initiate_response="$(m6_service_rpc 'm6-initiate-first' "$initiate_request")"
  replay_response="$(m6_service_rpc 'm6-initiate-replay' "$initiate_request")"
  printf '%s\n' "$initiate_response" >"${evidence_dir}/m6-taker-service-init.json"
  printf '%s\n' "$replay_response" >"${evidence_dir}/m6-taker-service-init-replay.json"
  chmod 0600 "${evidence_dir}/m6-taker-service-"*.json
  jq -e --arg swap "$m5_swap_id" '
    .error == null and .result.schema_version == 1 and .result.was_replay == false
    and .result.swap.swap_id == $swap and .result.swap.state == "not_activated"
    and .result.swap.progress_generation == 0 and .result.swap.available_action == null
  ' <<<"$initiate_response" >/dev/null
  jq -e --arg swap "$m5_swap_id" '
    .error == null and .result.schema_version == 1 and .result.was_replay == true
    and .result.swap.swap_id == $swap and .result.swap.state == "not_activated"
    and .result.swap.progress_generation == 0 and .result.swap.available_action == null
  ' <<<"$replay_response" >/dev/null
  assert_m5_taker_receipt_unchanged
}

allocate_port() {
  perl -MIO::Socket::INET -e '
    $socket = IO::Socket::INET->new(
      LocalAddr => "127.0.0.1", LocalPort => 0, Proto => "tcp", Listen => 1
    ) or die "loopback port allocation failed: $!\n";
    print $socket->sockport, "\n";
  '
}

sequencer_health="$(rpc "$LEZ_SEQUENCER_URL" '{"jsonrpc":"2.0","id":1,"method":"checkHealth","params":[]}')"
indexer_readiness="$(rpc "$LEZ_INDEXER_URL" '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}')"
channel_response="$(rpc "$LEZ_SEQUENCER_URL" '{"jsonrpc":"2.0","id":1,"method":"getChannelId","params":[]}')"
genesis_response="$(rpc "$LEZ_SEQUENCER_URL" '{"jsonrpc":"2.0","id":1,"method":"getBlock","params":[1]}')"
zebra_tip_response="$(rpc "$ZEBRA_RPC_URL" '{"jsonrpc":"2.0","id":1,"method":"getblockcount","params":[]}')"
jq -e '.error == null' <<<"$sequencer_health" >/dev/null
jq -e '.error == null and (.result | numbers) >= 2' \
  <<<"$indexer_readiness" >/dev/null
jq -e --arg channel_id "$LEZ_CHAIN_ID" \
  '.error == null and .result == $channel_id' <<<"$channel_response" >/dev/null
jq -e '.error == null and (.result | type == "string")' \
  <<<"$genesis_response" >/dev/null
genesis_block_id="$(jq -er '.result' <<<"$genesis_response" \
  | base64 --decode | od -An -tu8 -N8 | tr -d ' ')"
genesis_block_hash="$(jq -er '.result' <<<"$genesis_response" \
  | base64 --decode | xxd -p -s 40 -l 32 -c 32)"
if [[ "$genesis_block_id" != 1 || "$genesis_block_hash" != "$LEZ_GENESIS_HASH" ]]; then
  echo 'LEZ genesis block does not match the configured runtime identity' >&2
  exit 2
fi
zebra_tip="$(jq -er 'select(.error == null) | .result | numbers' \
  <<<"$zebra_tip_response")"
(( zebra_tip >= 104 )) || {
  echo "Zebra tip lacks deterministic mature funds: ${zebra_tip}" >&2
  exit 2
}

if [[ "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" == 1 ]]; then
  mkdir -m 0700 "$evidence_dir"
else
  mkdir -m 0700 "$private_base" "$evidence_dir"
fi
m5_actor_program=''
m5_actor_program_sha256=''
m5_lez_deployment_receipt_sha256=''
m5_lez_deployment_finality_sha256=''
m5_lez_actor_onboarding_sha256=''
m5_lez_deployment_transaction_hash=''
m5_lez_deployment_inclusion_block_id=0
m5_lez_deployment_inclusion_block_hash=''
m5_lez_maker_vault_claim_transaction_hash=''
m5_lez_taker_vault_claim_transaction_hash=''
m5_lez_maker_vault_claim_block_id=0
m5_lez_taker_vault_claim_block_id=0
if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  m5_lez_deployment_receipt="${evidence_dir}/m5-lez-deployment.json"
  install -m 0600 -- "$M5_LEZ_DEPLOYMENT_EVIDENCE_FILE" \
    "$m5_lez_deployment_receipt"
  jq -e --arg program "$ESCROW_PROGRAM_ID" --arg guest "$M5_LEZ_GUEST_SHA256" \
    --arg rpc "$LEZ_SEQUENCER_URL" --arg channel "$LEZ_CHAIN_ID" '
    .schema_version == 1
    and .preflight.image_id == $program
    and .preflight.elf_sha256 == $guest
    and .preflight.rpc_url == $rpc
    and .preflight.channel_id == $channel
    and (.transaction_hash | strings | test("^[0-9a-f]{64}$"))
    and (.inclusion_block_id | numbers) > .preflight.last_block_id
    and (.inclusion_block_hash | strings | test("^[0-9a-f]{64}$"))
  ' "$m5_lez_deployment_receipt" >/dev/null || {
    echo 'M5 LEZ deployment evidence differs from the configured runtime' >&2
    exit 2
  }
  m5_lez_deployment_receipt_sha256="$(sha256sum "$m5_lez_deployment_receipt" | cut -d ' ' -f1)"
  m5_lez_deployment_transaction_hash="$(jq -er '.transaction_hash' "$m5_lez_deployment_receipt")"
  m5_lez_deployment_inclusion_block_id="$(jq -er '.inclusion_block_id | numbers' "$m5_lez_deployment_receipt")"
  m5_lez_deployment_inclusion_block_hash="$(jq -er '.inclusion_block_hash' "$m5_lez_deployment_receipt")"
  m5_lez_deployment_finality="${evidence_dir}/m5-lez-deployment-finality.json"
  install -m 0600 -- "$M5_LEZ_FINALITY_EVIDENCE_FILE" \
    "$m5_lez_deployment_finality"
  jq -e --arg program "$ESCROW_PROGRAM_ID" --arg guest "$M5_LEZ_GUEST_SHA256" \
    --arg sequencer "$LEZ_SEQUENCER_URL" --arg indexer "$LEZ_INDEXER_URL" \
    --arg channel "$LEZ_CHAIN_ID" --arg genesis "$LEZ_GENESIS_HASH" \
    --arg tx "$m5_lez_deployment_transaction_hash" \
    --argjson block "$m5_lez_deployment_inclusion_block_id" \
    --arg block_hash "$m5_lez_deployment_inclusion_block_hash" '
    .schema_version == 1 and .kind == "m4_lez_local_deployment" and .result == "passed"
    and .artifact.image_id == $program and .artifact.elf_sha256 == $guest
    and .artifact.exact_elf_pre_window_occurrences == 0
    and .artifact.exact_elf_post_window_occurrences == 1
    and .artifact.finalized_wire_bytecode_equal == true
    and .stack.sequencer_rpc_url == $sequencer and .stack.indexer_rpc_url == $indexer
    and .stack.channel_id == $channel and .stack.finalized_genesis_hash == $genesis
    and .transaction_id == $tx and .containing_block_id == $block
    and .containing_block_hash == $block_hash and .bedrock_status == "Finalized"
    and .canonical_window_occurrences == 1 and .id_hash_id_lookups_equal == true
    and .sequencer_indexer_inclusion_equal == true and .runtime_external_resources == []
    and .public_rpc_used == false and .faucet_used == false
    and .private_material_disclosed == false
  ' "$m5_lez_deployment_finality" >/dev/null || {
    echo 'M5 LEZ finality evidence differs from the configured runtime or deployment receipt' >&2
    exit 2
  }
  m5_lez_deployment_finality_sha256="$(sha256sum "$m5_lez_deployment_finality" | cut -d ' ' -f1)"
  m5_lez_actor_onboarding="${evidence_dir}/m5-lez-actor-onboarding.json"
  install -m 0600 -- "$M5_LEZ_ONBOARDING_EVIDENCE_FILE" \
    "$m5_lez_actor_onboarding"
  jq -e --arg channel "$LEZ_CHAIN_ID" --arg program "$ESCROW_PROGRAM_ID" \
    --arg deployment_sha "$m5_lez_deployment_finality_sha256" \
    --arg maker "$MAKER_ACCOUNT_BASE58" --arg taker "$TAKER_ACCOUNT_BASE58" \
    --argjson deployment_block "$m5_lez_deployment_inclusion_block_id" '
    .schema_version == 1 and .kind == "m4_lez_actor_onboarding" and .result == "passed"
    and .flow == "flow_0_fresh_vault_claims"
    and .channel_id == $channel and .escrow_program_id == $program
    and .deployment.finalized_evidence_sha256 == $deployment_sha
    and .actors.maker.role == "maker" and .actors.maker.account_id == $maker
    and (.actors.maker.vault_account_id | strings | test("^[1-9A-HJ-NP-Za-km-z]{43,44}$"))
    and (.actors.maker.transaction_id | strings | test("^[0-9a-f]{64}$"))
    and .actors.maker.submission_count == 1
    and .actors.maker.canonical_window_occurrences == 1
    and .actors.maker.finalized_block_id > $deployment_block
    and .actors.maker.owner_after == {balance:100000,nonce:1}
    and .actors.maker.vault_after == {balance:0,nonce:0}
    and .actors.taker.role == "taker" and .actors.taker.account_id == $taker
    and (.actors.taker.vault_account_id | strings | test("^[1-9A-HJ-NP-Za-km-z]{43,44}$"))
    and (.actors.taker.transaction_id | strings | test("^[0-9a-f]{64}$"))
    and .actors.taker.submission_count == 1
    and .actors.taker.canonical_window_occurrences == 1
    and .actors.taker.finalized_block_id > $deployment_block
    and .actors.taker.owner_after == {balance:200000,nonce:1}
    and .actors.taker.vault_after == {balance:0,nonce:0}
    and .total_submission_count == 2 and .automatic_submission_retry == false
    and .monero_or_swap_effects_started == false and .runtime_external_resources == []
    and .public_rpc_used == false and .faucet_used == false
    and .private_material_disclosed == false
    and (.raw_evidence | type == "array" and length > 0
      and all(.[]; (.path | strings | test("^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$"))
        and (.sha256 | strings | test("^[0-9a-f]{64}$"))))
  ' "$m5_lez_actor_onboarding" >/dev/null || {
    echo 'M5 LEZ actor-onboarding evidence differs from the configured runtime or deployment' >&2
    exit 2
  }
  m5_lez_actor_onboarding_sha256="$(sha256sum "$m5_lez_actor_onboarding" | cut -d ' ' -f1)"
  m5_lez_maker_vault_claim_transaction_hash="$(jq -er '.actors.maker.transaction_id' "$m5_lez_actor_onboarding")"
  m5_lez_taker_vault_claim_transaction_hash="$(jq -er '.actors.taker.transaction_id' "$m5_lez_actor_onboarding")"
  m5_lez_maker_vault_claim_block_id="$(jq -er '.actors.maker.finalized_block_id | numbers' "$m5_lez_actor_onboarding")"
  m5_lez_taker_vault_claim_block_id="$(jq -er '.actors.taker.finalized_block_id | numbers' "$m5_lez_actor_onboarding")"
  m5_actor_deployment_root="$private_base/actor-deployment"
  mkdir -m 0700 "$m5_actor_deployment_root"
  m5_actor_program="$m5_actor_deployment_root/zec-reference-actor"
  install -m 0700 "$actor_bin" "$m5_actor_program"
  strip --strip-all "$m5_actor_program"
  chmod 0500 "$m5_actor_program"
  [[ -f "$m5_actor_program" && ! -L "$m5_actor_program" \
    && "$(stat -c %a -- "$m5_actor_program")" == 500 \
    && "$(stat -c %h -- "$m5_actor_program")" == 1 ]] || {
    echo 'M5 private actor deployment is unavailable or unsafe' >&2
    exit 2
  }
  m5_actor_program_sha256="$(sha256sum "$m5_actor_program" | cut -d ' ' -f1)"
fi
readonly m5_actor_program m5_actor_program_sha256 \
  m5_lez_deployment_receipt_sha256 \
  m5_lez_deployment_finality_sha256 \
  m5_lez_actor_onboarding_sha256 \
  m5_lez_maker_vault_claim_transaction_hash \
  m5_lez_taker_vault_claim_transaction_hash \
  m5_lez_maker_vault_claim_block_id \
  m5_lez_taker_vault_claim_block_id \
  m5_lez_deployment_transaction_hash \
  m5_lez_deployment_inclusion_block_id \
  m5_lez_deployment_inclusion_block_hash
lez_tip_response="$(rpc "$LEZ_SEQUENCER_URL" '{"jsonrpc":"2.0","id":1,"method":"getLastBlockId","params":[]}')"
lez_tip="$(jq -er '.result | numbers' <<<"$lez_tip_response")"
discovery_start=$((lez_tip + 1))
discovery_end=$((discovery_start + DISCOVERY_BLOCKS - 1))
readonly lez_tip discovery_start discovery_end zebra_tip

# The sidecar CLI cannot inherit a reserved listener. Allocate at the latest
# pre-provision point; any intervening bind collision fails before activation/effects.
maker_port="$(allocate_port)"
taker_port="$(allocate_port)"
while [[ "$taker_port" == "$maker_port" ]]; do
  taker_port="$(allocate_port)"
done
readonly maker_port taker_port
readonly maker_endpoint="http://127.0.0.1:${maker_port}/"
readonly taker_endpoint="http://127.0.0.1:${taker_port}/"

jq -n \
  --arg run_id "$run_id" \
  --arg swap_id "${run_id}-swap" \
  --arg direction "$POC_DIRECTION" \
  --arg chain_id "$LEZ_CHAIN_ID" \
  --arg genesis "$LEZ_GENESIS_HASH" \
  --arg escrow "$ESCROW_PROGRAM_ID" \
  --arg authenticated_transfer "$AUTHENTICATED_TRANSFER_PROGRAM_BASE58" \
  --arg maker_account "$MAKER_ACCOUNT_BASE58" \
  --arg taker_account "$TAKER_ACCOUNT_BASE58" \
  --arg maker_endpoint "$maker_endpoint" \
  --arg taker_endpoint "$taker_endpoint" \
  --arg zebra_endpoint "$ZEBRA_RPC_URL" \
  --argjson discovery_start "$discovery_start" \
  --argjson discovery_blocks "$DISCOVERY_BLOCKS" '
  {
    schema_version: 1,
    run_id: $run_id,
    swap_id: $swap_id,
    direction: $direction,
    lez_runtime: {
      chain_id: $chain_id,
      channel_id: $chain_id,
      genesis_block_hash: $genesis,
      escrow_program_id: $escrow,
      authenticated_transfer_program_id_base58: $authenticated_transfer,
      maker_signer_account_id_base58: $maker_account,
      taker_signer_account_id_base58: $taker_account
    },
    bridge: {
      maker_endpoint: $maker_endpoint,
      taker_endpoint: $taker_endpoint
    },
    zebra_endpoint: $zebra_endpoint,
    lez_discovery_start_height: $discovery_start,
    lez_discovery_max_blocks: $discovery_blocks
  }' >"$spec_file"
chmod 0600 "$spec_file"

budget_clock_source='proc_uptime_monotonic_milliseconds'
provision_started_monotonic_ms="$(monotonic_milliseconds)"
corridor_deadline_monotonic_ms=$((
  provision_started_monotonic_ms + MAX_CORRIDOR_SECONDS * 1000
))
readonly budget_clock_source provision_started_monotonic_ms corridor_deadline_monotonic_ms
provisioner_signer_args=()
if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  provisioner_signer_args=(
    --maker-lez-signer-key-file "$M5_LEZ_MAKER_SIGNER_KEY_FILE"
    --taker-lez-signer-key-file "$M5_LEZ_TAKER_SIGNER_KEY_FILE"
  )
fi
"$provisioner_bin" --spec-file "$spec_file" --output-root "$provision_actors_root" \
  "${provisioner_signer_args[@]}" >"${evidence_dir}/provision-summary.json"
unset provisioner_signer_args
remaining_budget_milliseconds 'provisioning-after' >/dev/null
if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  cmp -- "$M5_LEZ_MAKER_SIGNER_KEY_FILE" "${provision_actors_root}/maker/lez-signer.key" || {
    echo 'M5 provisioned Maker LEZ signer differs from the fresh identity' >&2
    exit 2
  }
  cmp -- "$M5_LEZ_TAKER_SIGNER_KEY_FILE" "${provision_actors_root}/taker/lez-signer.key" || {
    echo 'M5 provisioned Taker LEZ signer differs from the fresh identity' >&2
    exit 2
  }
fi
jq -e \
  --arg direction "$POC_DIRECTION" \
  --arg zcash_funder "$expected_zcash_funder_role" \
  --arg lez_depositor "$expected_lez_depositor_role" '
  .direction == $direction
  and .zcash_candidate_owner == $zcash_funder
  and .lez_native_amount > 0
  and .lez_native_amount == (.lez_native_amount | floor)
  and .lez_native_amount <= 9007199254740991
  and .lez_depositor_role == $lez_depositor
  and (.authenticated_transfer_program_id_words | arrays | length) == 8
  and all(.authenticated_transfer_program_id_words[];
    . >= 0 and . == floor and . <= 4294967295)
  and .private_material_disclosed == false
  and .actor_pair_validated == true
' "${evidence_dir}/provision-summary.json" >/dev/null
jq -e \
  --arg depositor "$expected_lez_depositor_account" \
  --arg authenticated_transfer "$AUTHENTICATED_TRANSFER_PROGRAM_HEX" '
  .lez_depositor_account_id_base58 == $depositor
  and .authenticated_transfer_program_id == $authenticated_transfer
' "${evidence_dir}/provision-summary.json" >/dev/null

if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  m7_route_health_arguments=()
  if [[ -n "$M7_ROUTE_HEALTH_CONFIG" ]]; then
    m7_route_health_arguments=(
      --route-health-config "$M7_ROUTE_HEALTH_CONFIG"
      --route-health-poll-milliseconds "$M7_ROUTE_HEALTH_POLL_MILLISECONDS"
    )
  fi
  "$m5_handoff_driver" \
    --run-id "$run_id" \
    --source-actors-root "$provision_actors_root" \
    --source-provision-summary "${evidence_dir}/provision-summary.json" \
    --output-actors-root "$actors_root" \
    --application-root "$application_root" \
    --evidence-dir "$evidence_dir" \
    --maker-daemon-bin "$maker_daemon_bin" \
    --maker-cli-bin "$maker_cli_bin" \
    --taker-bin "$taker_bin" \
    --draft-bin "$chat_draft_bin" \
    --finalize-bin "$chat_finalize_bin" \
    --actor-program "$m5_actor_program" \
    --actor-program-sha256 "$m5_actor_program_sha256" \
    --actor-inspector-bin "$actor_inspector_bin" \
    --pair-inspector-bin "$m5_pair_inspector_bin" \
    "${m7_route_health_arguments[@]}" \
    >"${evidence_dir}/m5-handoff-path.txt"
  [[ "$(<"${evidence_dir}/m5-handoff-path.txt")" == \
      "${evidence_dir}/m5-chat-handoff.json" ]] || {
    echo 'M5 handoff returned an unexpected evidence path' >&2
    exit 2
  }
  jq -e --arg program "$m5_actor_program" \
    --arg program_sha256 "$m5_actor_program_sha256" '
    .result == "passed"
    and .scheduled_maker_actor.actor_kind == "zcash"
    and .scheduled_maker_actor.schedule_state == "queued"
    and .scheduled_maker_actor.lease_generation == 0
    and .scheduled_maker_actor.attempt_count == 0
    and .scheduled_maker_actor.child_identity_absent == true
    and .effect_actor_pair_validated == true
    and (.effect_actor_pair_receipt_sha256 | test("^[0-9a-f]{64}$"))
    and .scheduled_maker_actor.actor_program_path == $program
    and .scheduled_maker_actor.actor_program_sha256 == $program_sha256
    and (.scheduled_maker_actor.config_sha256 | test("^[0-9a-f]{64}$"))
  ' "${evidence_dir}/m5-chat-handoff.json" >/dev/null || {
    echo 'M5 handoff did not return the exact queued Maker manifest' >&2
    exit 2
  }
  m5_effect_actor_pair_receipt_sha256="$(jq -er \
    '.effect_actor_pair_receipt_sha256 | strings | select(test("^[0-9a-f]{64}$"))' \
    "${evidence_dir}/m5-chat-handoff.json")"
  [[ -f "${evidence_dir}/m5-effect-actor-pair.json" \
    && ! -L "${evidence_dir}/m5-effect-actor-pair.json" \
    && "$(stat -c %a -- "${evidence_dir}/m5-effect-actor-pair.json")" == 600 \
    && "$(stat -c %u -- "${evidence_dir}/m5-effect-actor-pair.json")" == "$(id -u)" \
    && "$(stat -c %h -- "${evidence_dir}/m5-effect-actor-pair.json")" == 1 \
    && "$(stat -c %s -- "${evidence_dir}/m5-effect-actor-pair.json")" -gt 0 \
    && "$(stat -c %s -- "${evidence_dir}/m5-effect-actor-pair.json")" -le 65536 \
    && "$(sha256sum "${evidence_dir}/m5-effect-actor-pair.json" | cut -d ' ' -f1)" == \
      "$m5_effect_actor_pair_receipt_sha256" ]] || {
    echo 'M5 effect-bearing actor-pair evidence changed after handoff' >&2
    exit 2
  }
  remaining_budget_milliseconds 'm5-handoff-after' >/dev/null
  m5_daemon_pid="$(jq -er '.transport_cutover.maker_daemon_pid | numbers' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_daemon_start_ticks="$(jq -er \
    '.transport_cutover.maker_daemon_start_ticks | strings | select(test("^[0-9]+$"))' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_daemon_bin="$(jq -er '.transport_cutover.maker_daemon_bin | strings' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_maker_socket="$(jq -er '.transport_cutover.maker_socket | strings' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_application_database="$(jq -er '.transport_cutover.application_database | strings' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_chat_socket="$(jq -er '.transport_cutover.chat_socket | strings' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_delivery_directory="$(jq -er '.transport_cutover.delivery_directory | strings' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_delivery_offline="$(jq -er '.transport_cutover.delivery_offline | strings' \
    "${evidence_dir}/m5-chat-handoff.json")"
  process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin" || {
    echo 'M5 maker daemon handoff is not the exact live process' >&2
    exit 2
  }
  m5_taker_acceptance_receipt="$(jq -er '.taker_lifecycle.acceptance_receipt_file | strings' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_taker_acceptance_receipt_sha256="$(jq -er \
    '.taker_lifecycle.acceptance_receipt_sha256 | strings | select(test("^[0-9a-f]{64}$"))' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_taker_actor_config="$(jq -er '.taker_lifecycle.taker_actor_config | strings' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_taker_actor_config_sha256="$(jq -er \
    '.taker_lifecycle.taker_actor_config_sha256 | strings | select(test("^[0-9a-f]{64}$"))' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_taker_actor_state="$(jq -er '.taker_lifecycle.taker_actor_state | strings' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_swap_id="$(jq -er '.swap_id | strings' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_agreement_sha256="$(jq -er \
    '.agreement_sha256 | strings | select(test("^[0-9a-f]{64}$"))' \
    "${evidence_dir}/m5-chat-handoff.json")"
  [[ "$m5_taker_acceptance_receipt" == "$application_root/taker-acceptance-receipt.json" \
    && -f "$m5_taker_acceptance_receipt" && ! -L "$m5_taker_acceptance_receipt" \
    && "$(stat -c %a -- "$m5_taker_acceptance_receipt")" == 600 \
    && "$(stat -c %u -- "$m5_taker_acceptance_receipt")" == "$(id -u)" \
    && "$(stat -c %h -- "$m5_taker_acceptance_receipt")" == 1 \
    && "$(stat -c %s -- "$m5_taker_acceptance_receipt")" -gt 0 \
    && "$(stat -c %s -- "$m5_taker_acceptance_receipt")" -le 65536 \
    && "$(sha256sum "$m5_taker_acceptance_receipt" | cut -d ' ' -f1)" == \
      "$m5_taker_acceptance_receipt_sha256" ]] || {
    echo 'M5 Taker acceptance receipt changed after handoff' >&2
    exit 2
  }
  m5_taker_acceptance_receipt_identity="$(stat -c %d:%i -- \
    "$m5_taker_acceptance_receipt")"
  [[ "$m5_taker_acceptance_receipt_identity" =~ ^[0-9]+:[0-9]+$ ]] || {
    echo 'M5 Taker acceptance receipt identity is invalid' >&2
    exit 2
  }
  [[ "$m5_taker_actor_config" == \
      "$application_root/taker-actors/taker/actor-config.json" \
    && -f "$m5_taker_actor_config" && ! -L "$m5_taker_actor_config" \
    && "$(stat -c %a -- "$m5_taker_actor_config")" == 600 \
    && "$(stat -c %u -- "$m5_taker_actor_config")" == "$(id -u)" \
    && "$(stat -c %h -- "$m5_taker_actor_config")" == 1 \
    && "$(sha256sum "$m5_taker_actor_config" | cut -d ' ' -f1)" == \
      "$m5_taker_actor_config_sha256" \
    && "$m5_taker_actor_state" == \
      "$application_root/taker-actors/taker/state/actor.sqlite3" \
    && ! -e "$application_root/taker-actors/maker" ]] || {
    echo 'M5 receipt-bound Taker config or state changed after handoff' >&2
    exit 2
  }
  jq -e --arg swap "$m5_swap_id" --arg agreement "$m5_agreement_sha256" \
    --arg config "$m5_taker_actor_config" \
    --arg config_sha256 "$m5_taker_actor_config_sha256" \
    --arg state "$m5_taker_actor_state" '
    (keys | length) == 7 and .schema_version == 1 and .role == "taker"
    and .swap_id == $swap and .agreement_sha256 == $agreement
    and .actor_config_file == $config
    and .actor_config_sha256 == $config_sha256
    and .actor_state_database == $state
  ' "$m5_taker_acceptance_receipt" >/dev/null || {
    echo 'M5 Taker acceptance receipt binding changed after handoff' >&2
    exit 2
  }
  jq -e --arg swap "$m5_swap_id" --arg agreement "$m5_agreement_sha256" \
    --arg state "$m5_taker_actor_state" '
    .role == "taker" and .swap_id == $swap
    and .signed_agreement_sha256 == $agreement
    and .role_state_db == $state
  ' "$m5_taker_actor_config" >/dev/null || {
    echo 'M5 receipt-bound Taker config semantics changed after handoff' >&2
    exit 2
  }
  m5_maker_actor_config="$(jq -er '.scheduled_maker_actor.config_path | strings' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_maker_actor_state="$(jq -er '.scheduled_maker_actor.state_db_path | strings' \
    "${evidence_dir}/m5-chat-handoff.json")"
  m5_maker_bundle="${m5_maker_actor_config%/maker/actor-config.json}"
  [[ "$m5_maker_bundle" != "$m5_maker_actor_config" \
    && "$m5_maker_actor_state" == "$m5_maker_bundle/maker/state/actor.sqlite3" ]] || {
    echo 'M5 queued Maker config/state layout is invalid' >&2
    exit 2
  }
  m5_maker_actor_root="${m5_maker_bundle%/*}"
  m5_maker_state_dir="${m5_maker_actor_state%/actor.sqlite3}"
  m5_supervisor_socket="${application_root}/runtime/supervisor.sock"
  [[ -S "$m5_maker_socket" && -S "$m5_chat_socket" \
    && -d "$m5_delivery_directory" && ! -e "$m5_delivery_offline" ]] || {
    echo 'M5 negotiation transports are not armed for post-lock cutover' >&2
    exit 2
  }
fi
application_handoff_sha256=''
if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  application_handoff_sha256="$(sha256sum "${evidence_dir}/m5-chat-handoff.json")"
  application_handoff_sha256="${application_handoff_sha256%% *}"
  [[ "$application_handoff_sha256" =~ ^[0-9a-f]{64}$ ]] || exit 2
fi
readonly application_handoff_sha256

lez_native_amount="$(jq -er '.lez_native_amount | numbers' \
  "${evidence_dir}/provision-summary.json")"
lez_depositor_account="$(jq -er '.lez_depositor_account_id_base58 | strings' \
  "${evidence_dir}/provision-summary.json")"
readonly lez_native_amount lez_depositor_account
lez_depositor_payload="$(jq -nc --arg account "$lez_depositor_account" '
  {
    jsonrpc: "2.0",
    id: 1,
    method: "getAccount",
    params: [$account]
  }')"
lez_balance_snapshot_stable=0
for lez_balance_snapshot_attempt in 1 2 3; do
  lez_balance_tip_before_file="${evidence_dir}/pre-effect-lez-balance-tip-before-attempt-${lez_balance_snapshot_attempt}.json"
  lez_balance_account_file="${evidence_dir}/pre-effect-lez-depositor-account-attempt-${lez_balance_snapshot_attempt}.json"
  lez_balance_tip_after_file="${evidence_dir}/pre-effect-lez-balance-tip-after-attempt-${lez_balance_snapshot_attempt}.json"
  rpc "$LEZ_SEQUENCER_URL" \
    '{"jsonrpc":"2.0","id":1,"method":"getLastBlockId","params":[]}' \
    >"$lez_balance_tip_before_file"
  rpc "$LEZ_SEQUENCER_URL" "$lez_depositor_payload" \
    >"$lez_balance_account_file"
  rpc "$LEZ_SEQUENCER_URL" \
    '{"jsonrpc":"2.0","id":1,"method":"getLastBlockId","params":[]}' \
    >"$lez_balance_tip_after_file"
  lez_balance_tip_before="$(jq -er '.result | numbers' \
    "$lez_balance_tip_before_file")"
  lez_balance_tip_after="$(jq -er '.result | numbers' \
    "$lez_balance_tip_after_file")"
  if [[ "$lez_balance_tip_before" == "$lez_balance_tip_after" ]]; then
    lez_balance_snapshot_stable=1
    break
  fi
  remaining_budget_milliseconds \
    "lez-depositor-preflight-retry-${lez_balance_snapshot_attempt}" >/dev/null
done
readonly lez_balance_snapshot_attempt lez_balance_tip_before lez_balance_tip_after
readonly lez_balance_account_file lez_balance_snapshot_stable
if (( lez_balance_snapshot_stable != 1 )); then
  echo 'LEZ tip moved across all depositor balance preflight attempts' >&2
  exit 2
fi
jq -e \
  --slurpfile provision "${evidence_dir}/provision-summary.json" \
  --argjson amount "$lez_native_amount" '
  .error == null
  and .result.program_owner == $provision[0].authenticated_transfer_program_id_words
  and (.result.balance | numbers) >= 0
  and .result.balance == (.result.balance | floor)
  and .result.balance <= 9007199254740991
  and .result.balance >= $amount
' "$lez_balance_account_file" >/dev/null || {
  echo 'LEZ depositor lacks the agreement-bound balance or expected owner before effects' >&2
  exit 2
}
remaining_budget_milliseconds 'lez-depositor-preflight-after' >/dev/null

if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  maker_config="$m5_maker_actor_config"
  maker_sidecar_state_dir="$m5_maker_state_dir"
  taker_config="$m5_taker_actor_config"
  taker_sidecar_state_dir="${m5_taker_actor_state%/actor.sqlite3}"
else
  maker_config="${actors_root}/maker/actor-config.json"
  maker_sidecar_state_dir="${actors_root}/maker/state"
  taker_config="${actors_root}/taker/actor-config.json"
  taker_sidecar_state_dir="${actors_root}/taker/state"
fi
readonly maker_config maker_sidecar_state_dir
readonly taker_config taker_sidecar_state_dir
readonly maker_log="${evidence_dir}/maker-sidecar.log"
readonly taker_log="${evidence_dir}/taker-sidecar.log"

# Readiness polls remain inside the absolute corridor deadline. The later
# MAX_PRE_EFFECT_SECONDS gate forbids all effects if startup is too slow.
remaining_budget_milliseconds 'sidecar-startup-before' >/dev/null

"$sidecar_bin" \
  --listen-address "127.0.0.1:${maker_port}" \
  --sequencer-url "$LEZ_SEQUENCER_URL" \
  --indexer-url "$LEZ_INDEXER_URL" \
  --run-id "$run_id" \
  --runtime-file "${provision_actors_root}/maker/lez-runtime.json" \
  --capability-file "${provision_actors_root}/maker/sidecar.capability" \
  --private-key-file "${provision_actors_root}/maker/lez-signer.key" \
  --state-directory "$maker_sidecar_state_dir" \
  --authenticated-transfer-program-id "$AUTHENTICATED_TRANSFER_PROGRAM_HEX" \
  >"$maker_log" 2>&1 &
maker_pid=$!
maker_start_ticks="$(process_start_ticks "$maker_pid")"

"$sidecar_bin" \
  --listen-address "127.0.0.1:${taker_port}" \
  --sequencer-url "$LEZ_SEQUENCER_URL" \
  --indexer-url "$LEZ_INDEXER_URL" \
  --run-id "$run_id" \
  --runtime-file "${provision_actors_root}/taker/lez-runtime.json" \
  --capability-file "${provision_actors_root}/taker/sidecar.capability" \
  --private-key-file "${provision_actors_root}/taker/lez-signer.key" \
  --state-directory "$taker_sidecar_state_dir" \
  --authenticated-transfer-program-id "$AUTHENTICATED_TRANSFER_PROGRAM_HEX" \
  >"$taker_log" 2>&1 &
taker_pid=$!
taker_start_ticks="$(process_start_ticks "$taker_pid")"

wait_for_readiness() {
  local role="$1"
  local pid="$2"
  local start_ticks="$3"
  local log="$4"
  for _ in {1..500}; do
    remaining_budget_milliseconds "${role}-sidecar-readiness" >/dev/null || return
    if [[ -s "$log" ]] && jq -e \
      --arg role "$role" --arg run_id "$run_id" '
        .event == "ready"
        and .run_id == $run_id
        and .runtime.sidecar_role == $role
        and .indexer_health == "stable_finalized_tip_bound_to_runtime_genesis"
        and .finality == "exact_genesis_bound_finalized_indexer_clock_available"
      ' "$log" >/dev/null 2>&1; then
      return 0
    fi
    if ! process_is_owned "$pid" "$start_ticks" "$sidecar_bin"; then
      echo "${role} sidecar exited before readiness" >&2
      sed -n '1,40p' "$log" >&2
      return 1
    fi
    sleep 0.05
  done
  echo "${role} sidecar did not become ready" >&2
  if process_is_owned "$maker_pid" "$maker_start_ticks" "$sidecar_bin"; then
    echo 'maker sidecar owned_process_alive=true' >&2
  else
    echo 'maker sidecar owned_process_alive=false' >&2
  fi
  tail -n 40 "$maker_log" >&2
  if process_is_owned "$taker_pid" "$taker_start_ticks" "$sidecar_bin"; then
    echo 'taker sidecar owned_process_alive=true' >&2
  else
    echo 'taker sidecar owned_process_alive=false' >&2
  fi
  tail -n 40 "$taker_log" >&2
  return 1
}
wait_for_readiness maker "$maker_pid" "$maker_start_ticks" "$maker_log"
wait_for_readiness taker "$taker_pid" "$taker_start_ticks" "$taker_log"
remaining_budget_milliseconds 'sidecar-readiness-after' >/dev/null

"$actor_bin" --config "$maker_config" status >"${evidence_dir}/maker-status-before.json"
"$actor_bin" --config "$taker_config" status >"${evidence_dir}/taker-status-before.json"
jq -e '.role == "maker" and .state == "not_activated"' \
  "${evidence_dir}/maker-status-before.json" >/dev/null
jq -e '.role == "taker" and .state == "not_activated"' \
  "${evidence_dir}/taker-status-before.json" >/dev/null

pre_effect_tip_response="$(rpc "$LEZ_SEQUENCER_URL" '{"jsonrpc":"2.0","id":1,"method":"getLastBlockId","params":[]}')"
pre_effect_tip="$(jq -er '.result | numbers' <<<"$pre_effect_tip_response")"
(( pre_effect_tip >= lez_tip && pre_effect_tip <= discovery_end \
  && discovery_end - pre_effect_tip >= 128 )) || {
  echo "LEZ discovery headroom is unsafe before effects: tip=${pre_effect_tip}, end=${discovery_end}" >&2
  exit 2
}
pre_effect_remaining_ms="$(remaining_budget_milliseconds 'pre-effect-gate')"
pre_effect_elapsed_ms=$((MAX_CORRIDOR_SECONDS * 1000 - pre_effect_remaining_ms))
(( pre_effect_elapsed_ms <= MAX_PRE_EFFECT_SECONDS * 1000 )) || {
  echo "JIT provisioning/readiness consumed ${pre_effect_elapsed_ms}ms; refusing effects outside the ${MAX_CORRIDOR_SECONDS}s provision-to-completion budget" >&2
  exit 2
}

jq -n \
  --arg run_id "$run_id" \
  --arg direction "$POC_DIRECTION" \
  --arg zcash_funder_role "$expected_zcash_funder_role" \
  --arg zcash_claimant_role "$expected_zcash_claimant_role" \
  --arg lez_depositor_role "$expected_lez_depositor_role" \
  --arg maker_endpoint "$maker_endpoint" \
  --arg taker_endpoint "$taker_endpoint" \
  --argjson maker_pid "$maker_pid" \
  --argjson taker_pid "$taker_pid" \
  --arg maker_start_ticks "$maker_start_ticks" \
  --arg taker_start_ticks "$taker_start_ticks" \
  --argjson lez_tip "$lez_tip" \
  --argjson discovery_start "$discovery_start" \
  --argjson discovery_end "$discovery_end" \
  --argjson pre_effect_elapsed_ms "$pre_effect_elapsed_ms" \
  --argjson lez_balance_snapshot_attempt "$lez_balance_snapshot_attempt" \
  --argjson lez_balance_tip "$lez_balance_tip_after" \
  --arg budget_clock_source "$budget_clock_source" \
  --argjson provision_started_monotonic_ms "$provision_started_monotonic_ms" \
  --argjson corridor_deadline_monotonic_ms "$corridor_deadline_monotonic_ms" \
  --argjson pre_effect_remaining_ms "$pre_effect_remaining_ms" \
  --argjson m5_application_mode "$M5_APPLICATION_MODE" \
  --arg application_handoff_sha256 "$application_handoff_sha256" \
  --argjson zebra_tip "$zebra_tip" \
  --arg journey "$M6_ZEC_JOURNEY" \
  --argjson max_corridor_seconds "$MAX_CORRIDOR_SECONDS" '
  {
    schema_version: 1,
    run_id: $run_id,
    direction: $direction,
    expected_effect_owners: {
      zcash_funder: $zcash_funder_role,
      zcash_claimant: $zcash_claimant_role,
      lez_depositor: $lez_depositor_role
    },
    sidecars: {
      maker: {endpoint: $maker_endpoint, pid: $maker_pid, process_start_ticks: $maker_start_ticks},
      taker: {endpoint: $taker_endpoint, pid: $taker_pid, process_start_ticks: $taker_start_ticks}
    },
    initial_lez_tip: $lez_tip,
    discovery: {start: $discovery_start, end: $discovery_end},
    pre_effect_elapsed_milliseconds: $pre_effect_elapsed_ms,
    lez_balance_preflight: {
      stable_tip: $lez_balance_tip,
      successful_attempt: $lez_balance_snapshot_attempt
    },
    budget: {
      clock_source: $budget_clock_source,
      provision_started_milliseconds: $provision_started_monotonic_ms,
      absolute_deadline_milliseconds: $corridor_deadline_monotonic_ms,
      pre_effect_remaining_milliseconds: $pre_effect_remaining_ms,
      provision_to_completion_cap_seconds: $max_corridor_seconds,
      lez_delay_margin_seconds:
        (if $journey == "claim" then 10 else 0 end)
    },
    initial_zebra_tip: $zebra_tip,
    application_plane: {
      enabled: ($m5_application_mode == 1),
      handoff_receipt_sha256:
        (if $m5_application_mode == 1 then $application_handoff_sha256 else null end),
      transports_armed_before_activation: ($m5_application_mode == 1)
    },
    public_rpc_or_faucet_used: false,
    actor_outputs_secret_free: true
  }' >"${evidence_dir}/run-identity.json"

if [[ "$M5_APPLICATION_MODE" != 1 ]]; then
  maker_activate_timeout="$(bounded_actor_timeout 'maker-activate')"
  timeout --signal=KILL "${maker_activate_timeout}s" \
    "$actor_bin" --config "$maker_config" activate \
    >"${evidence_dir}/maker-activate.json" 2>"${evidence_dir}/maker-activate.stderr"
  remaining_budget_milliseconds 'maker-activate-after' >/dev/null
  jq -e '.role == "maker" and .outcome == "activated" and .phase == "offered"' \
    "${evidence_dir}/maker-activate.json" >/dev/null
fi

taker_activate_timeout="$(bounded_actor_timeout 'taker-activate')"
timeout --signal=KILL "${taker_activate_timeout}s" \
  "$actor_bin" --config "$taker_config" activate \
  >"${evidence_dir}/taker-activate.json" 2>"${evidence_dir}/taker-activate.stderr"
remaining_budget_milliseconds 'taker-activate-after' >/dev/null
jq -e '.role == "taker" and .outcome == "activated" and .phase == "offered"' \
  "${evidence_dir}/taker-activate.json" >/dev/null

if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  rpc "$ZEBRA_RPC_URL" \
    '{"jsonrpc":"2.0","id":1,"method":"getrawmempool","params":[]}' \
    >"${evidence_dir}/m5-zebra-mempool-before-supervision.json"
  jq -e '.error == null and .result == []' \
    "${evidence_dir}/m5-zebra-mempool-before-supervision.json" >/dev/null || {
    echo 'M5 exact-funding attribution requires an initially empty isolated mempool' >&2
    exit 2
  }
  stop_owned_m5_daemon
  m5_daemon_pid=''
  m5_daemon_start_ticks=''
  [[ ! -e "$m5_maker_socket" && ! -e "$m5_chat_socket" \
    && -d "$m5_delivery_directory" ]] || {
    echo 'M5 unsupervised handoff daemon did not stop cleanly before supervision' >&2
    exit 2
  }
  start_m5_full_supervised_daemon
  maker_supervised_active=0
  for _ in {1..200}; do
    remaining_budget_milliseconds 'm5-maker-supervised-activation' >/dev/null
    activation_status="${evidence_dir}/m5-maker-supervised-activation-status.json"
    activation_stderr="${evidence_dir}/m5-maker-supervised-activation-status.stderr"
    capture_m5_supervised_maker_status "$activation_status" "$activation_stderr" \
      m5-maker-supervised-activation
    if jq -e '.role == "maker" and .state == "active" and (.revision | numbers) >= 0' \
      "$activation_status" >/dev/null; then
      maker_supervised_active=1
      break
    fi
    jq -e '.schema_version == 1 and .role == "maker" and .state == "not_activated"' \
      "$activation_status" >/dev/null || {
      echo 'M5 supervised Maker returned an unexpected startup status' >&2
      exit 1
    }
    process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin" || {
      echo 'M5 full daemon exited during supervised Maker activation' >&2
      exit 1
    }
    sleep 0.05
  done
  (( maker_supervised_active == 1 )) || {
    echo 'M5 supervisor did not activate the daemon-provisioned Maker actor' >&2
    exit 1
  }
  jq -n --slurpfile status "${evidence_dir}/m5-maker-supervised-activation-status.json" '
    {
      schema_version: 1,
      maker_effect_authority: "daemon_supervisor",
      concurrent_direct_maker_effects: false,
      maker_daemon_alive: true,
      actor_status: $status[0]
    }' >"${evidence_dir}/maker-activate.json"
fi

: >"${evidence_dir}/actor-drive.ndjson"
: >"${evidence_dir}/drive-retries.ndjson"
: >"${evidence_dir}/m5-taker-receipt-claim.ndjson"
: >"${evidence_dir}/m5-taker-receipt-monitor.ndjson"
zcash_fund_mined=0
zcash_claim_mined=0
lez_revealing_claim_seen=0
zcash_fund_submitter=''
zcash_claim_submitter=''
lez_revealing_claim_submitter=''
drive_actor() {
  local role="$1"
  local config="$2"
  local round="$3"
  local attempt=1 actor_status actor_timeout output stderr_file
  while true; do
    stderr_file="${evidence_dir}/${role}-drive-${round}-attempt-${attempt}.stderr"
    actor_timeout="$(bounded_actor_timeout "${role}-drive-${round}-attempt-${attempt}")" \
      || return
    actor_status=0
    if output="$(timeout --signal=KILL "${actor_timeout}s" \
      "$actor_bin" --config "$config" drive 2>"$stderr_file")"; then
      break
    else
      actor_status=$?
    fi
    if (( actor_status == 124 || actor_status == 137 || attempt > MAX_DRIVE_RETRIES )) \
      || [[ "$(stat -c %s -- "$stderr_file")" != 27 ]] \
      || [[ "$(<"$stderr_file")" != 'actor drive is unavailable' ]]; then
      echo "${role} drive failed in round ${round}, attempt ${attempt}" >&2
      sed -n '1,20p' "$stderr_file" >&2
      return 1
    fi
    jq -nc \
      --arg role "$role" \
      --argjson round "$round" \
      --argjson retry "$attempt" '
      {
        schema_version: 1,
        event: "same_run_drive_retry",
        role: $role,
        round: $round,
        retry: $retry,
        error_class: "actor_drive_unavailable"
      }' >>"${evidence_dir}/drive-retries.ndjson"
    remaining_budget_milliseconds "${role}-drive-${round}-retry-${attempt}" >/dev/null || return
    sleep "$DRIVE_RETRY_DELAY_SECONDS"
    attempt=$((attempt + 1))
  done
  jq -e --arg role "$role" '
    .schema_version == 1 and .role == $role and .command == "drive"
  ' <<<"$output" >/dev/null
  jq -c --argjson round "$round" '. + {round: $round}' <<<"$output" \
    >>"${evidence_dir}/actor-drive.ndjson"
  remaining_budget_milliseconds "${role}-drive-${round}-after" >/dev/null || return
  printf '%s\n' "$output"
}

assert_m5_taker_receipt_unchanged() {
  [[ "$m5_taker_acceptance_receipt" == \
      "$application_root/taker-acceptance-receipt.json" \
    && -f "$m5_taker_acceptance_receipt" \
    && ! -L "$m5_taker_acceptance_receipt" \
    && "$(stat -c %a -- "$m5_taker_acceptance_receipt")" == 600 \
    && "$(stat -c %u -- "$m5_taker_acceptance_receipt")" == "$(id -u)" \
    && "$(stat -c %h -- "$m5_taker_acceptance_receipt")" == 1 \
    && "$(stat -c %s -- "$m5_taker_acceptance_receipt")" -gt 0 \
    && "$(stat -c %s -- "$m5_taker_acceptance_receipt")" -le 65536 \
    && "$(stat -c %d:%i -- "$m5_taker_acceptance_receipt")" == \
      "$m5_taker_acceptance_receipt_identity" \
    && "$(sha256sum "$m5_taker_acceptance_receipt" | cut -d ' ' -f1)" == \
      "$m5_taker_acceptance_receipt_sha256" ]] || {
    echo 'M5 Taker acceptance receipt identity or bytes changed at point of use' >&2
    return 1
  }
}


m6_taker_lez_submission_set() {
  local journal="${taker_sidecar_state_dir}/bridge-requests.v1.json"
  [[ -f "$journal" && ! -L "$journal" ]] || return 1
  jq -c --arg run_id "$run_id" '
    select(.schema_version == 1 and .run_id == $run_id)
    | [.entries[] | select(.method == "lez_bridge.v1.submit_transaction"
      and .outcome.kind == "success") | .outcome.value.transaction_id
      | strings | select(test("^[0-9a-f]{64}$"))]
    | unique
  ' "$journal"
}

m6_taker_lez_submission_trace() {
  local journal="${taker_sidecar_state_dir}/bridge-requests.v1.json"
  [[ -f "$journal" && ! -L "$journal" ]] || return 1
  jq -c --arg run_id "$run_id" '
    select(.schema_version == 1 and .run_id == $run_id)
    | [.entries[] | select(.method == "lez_bridge.v1.submit_transaction"
        and .outcome.kind == "success")] as $submissions
    | [$submissions[]
      | {
          request_sha256:
            (.request_sha256 | strings | select(test("^[0-9a-f]{64}$"))),
          transaction_id:
            (.outcome.value.transaction_id | strings
              | select(test("^[0-9a-f]{64}$")))
        }]
    | if length == ($submissions | length) then .
      else error("malformed successful LEZ submission trace") end
  ' "$journal"
}

m6_finalized_tip() {
  rpc "$LEZ_INDEXER_URL" '{"jsonrpc":"2.0","id":"m6-refund-tip","method":"getLastFinalizedBlockId","params":[]}' |
    jq -er '.result | numbers'
}

m6_taker_lez_refund_deadline_ms() {
  local journal="${taker_sidecar_state_dir}/bridge-requests.v1.json"
  [[ -f "$journal" && ! -L "$journal" ]] || return 1
  jq -er --arg run_id "$run_id" '
    select(.schema_version == 1 and .run_id == $run_id)
    | [.entries[] | select(.method == "lez_bridge.v1.prepare_native_escrow"
        and .outcome.kind == "success")
      | .replay_request.terms.refund_at_ms | numbers]
    | unique
    | if length == 1 then .[0] else error("ambiguous LEZ refund deadline") end
  ' "$journal"
}

wait_for_m6_lez_refund_window() {
  local deadline tip block timestamp hash
  deadline="$(m6_taker_lez_refund_deadline_ms)" || return
  while true; do
    remaining_budget_milliseconds 'm6-lez-refund-window' >/dev/null || return
    tip="$(m6_finalized_tip)" || return
    block="$(rpc "$LEZ_INDEXER_URL" "$(jq -nc --argjson height "$tip" '
      {jsonrpc:"2.0",id:"m6-refund-window",method:"getBlockById",params:[$height]}
    ')")" || return
    jq -e '
      .error == null and .result.bedrock_status == "Finalized"
      and (.result.header.timestamp | numbers)
    ' <<<"$block" >/dev/null || return 1
    timestamp="$(jq -er '.result.header.timestamp | numbers' <<<"$block")"
    if (( timestamp >= deadline )); then
      hash="$(jq -er '.result.header.hash | strings' <<<"$block")"
      jq -n --argjson deadline "$deadline" --argjson tip "$tip" \
        --argjson timestamp "$timestamp" --arg hash "$hash" '
        {schema_version:1,refund_at_ms:$deadline,finalized_tip:$tip,
          finalized_tip_hash:$hash,finalized_timestamp_ms:$timestamp,
          deadline_reached:true}
      ' >"${evidence_dir}/m6-taker-lez-refund-window.json"
      chmod 0600 "${evidence_dir}/m6-taker-lez-refund-window.json"
      return 0
    fi
    sleep 0.10
  done
}

prove_m6_lez_refund_finality() {
  local transaction_id="$1" start_height="$2"
  local cursor=$((start_height + 1)) tip block height occurrences
  local count=0 containing_height=0 containing_hash='' containing_timestamp=0 transaction
  [[ "$transaction_id" =~ ^[0-9a-f]{64}$ ]] || return 1
  while true; do
    remaining_budget_milliseconds 'm6-lez-refund-finality' >/dev/null || return
    tip="$(m6_finalized_tip)" || return
    (( tip >= start_height && tip - start_height <= 4096 )) || return 1
    while (( cursor <= tip )); do
      height="$cursor"
      block="$(rpc "$LEZ_INDEXER_URL" "$(jq -nc --argjson height "$height"         '{jsonrpc:"2.0",id:"m6-refund-block",method:"getBlockById",params:[$height]}')")" ||
        return
      jq -e '.error == null and .result.bedrock_status == "Finalized"' <<<"$block" >/dev/null ||
        return 1
      occurrences="$(jq -er --arg tx "$transaction_id"         '[.result.body.transactions[]? | select(.Public.hash == $tx)] | length' <<<"$block")"
      if (( occurrences > 0 )); then
        count=$((count + occurrences))
        containing_height="$height"
        containing_hash="$(jq -er '.result.header.hash | strings' <<<"$block")"
        containing_timestamp="$(jq -er '.result.header.timestamp | numbers' <<<"$block")"
      fi
      cursor=$((cursor + 1))
    done
    (( count > 0 )) && break
    sleep 0.10
  done
  (( count == 1 && containing_height > start_height )) || return 1
  transaction="$(rpc "$LEZ_INDEXER_URL" "$(jq -nc --arg tx "$transaction_id"     '{jsonrpc:"2.0",id:"m6-refund-transaction",method:"getTransaction",params:[$tx]}')")"
  jq -e --arg tx "$transaction_id"     '.error == null and .result.Public.hash == $tx' <<<"$transaction" >/dev/null || return 1
  jq -n --arg tx "$transaction_id" --arg hash "$containing_hash"     --argjson start "$((start_height + 1))" --argjson tip "$tip"     --argjson block "$containing_height" --argjson timestamp "$containing_timestamp" '
    {schema_version:1,transaction_id:$tx,
      window:{start_height:$start,finalized_tip:$tip},occurrences:1,
      containing_block_id:$block,containing_block_hash:$hash,
      containing_timestamp_ms:$timestamp,bedrock_status:"Finalized",
      transaction_hash_revalidated:true}
  ' >"${evidence_dir}/m6-taker-lez-refund-finality.json"
  chmod 0600 "${evidence_dir}/m6-taker-lez-refund-finality.json"
}

reconcile_m6_suppressed_maker_lock() {
  (( m6_maker_supervisor_suppressed == 1 && zcash_fund_mined == 2 \
    && m5_transport_cutover_complete == 1 )) || return 1
  [[ -z "$m5_daemon_pid" && -z "$m5_daemon_start_ticks" ]] || {
    echo 'refusing direct Maker-lock observation while daemon authority is live' >&2
    return 1
  }
  [[ ! -e "$m5_maker_socket" && ! -e "$m5_chat_socket" ]] || {
    echo 'refusing direct Maker-lock observation while daemon transports exist' >&2
    return 1
  }
  local before_tip before_mempool after_tip after_mempool actor_output actor_timeout
  local output_file="${evidence_dir}/m6-maker-lock-reconciliation.json"
  before_tip="$(rpc "$ZEBRA_RPC_URL" \
    '{"jsonrpc":"2.0","id":"m6-maker-lock-before-tip","method":"getblockcount","params":[]}')"
  before_mempool="$(rpc "$ZEBRA_RPC_URL" \
    '{"jsonrpc":"2.0","id":"m6-maker-lock-before-mempool","method":"getrawmempool","params":[]}')"
  jq -e '
    .error == null and (.result | numbers) >= 1
  ' <<<"$before_tip" >/dev/null || return
  jq -e '.error == null and .result == []' <<<"$before_mempool" >/dev/null || return

  actor_timeout="$(bounded_actor_timeout 'm6-maker-lock-reconciliation')" || return
  actor_output="$(timeout --signal=KILL "${actor_timeout}s" \
    "$actor_bin" --config "$maker_config" drive)" || return
  remaining_budget_milliseconds 'm6-maker-lock-reconciliation-after' >/dev/null || return
  jq -e '
    .schema_version == 1 and .role == "maker" and .command == "drive"
    and .outcome == "projected" and .operation == "maker_lock"
    and .phase == "both_legs_locked" and (.revision | numbers) >= 2
    and .next_action == "claim_lez"
  ' <<<"$actor_output" >/dev/null || {
    echo 'suppressed Maker lock did not reconcile to both locked legs' >&2
    return 1
  }

  after_tip="$(rpc "$ZEBRA_RPC_URL" \
    '{"jsonrpc":"2.0","id":"m6-maker-lock-after-tip","method":"getblockcount","params":[]}')"
  after_mempool="$(rpc "$ZEBRA_RPC_URL" \
    '{"jsonrpc":"2.0","id":"m6-maker-lock-after-mempool","method":"getrawmempool","params":[]}')"
  jq -e --argjson before "$before_tip" '
    .error == null and .result == $before.result
  ' <<<"$after_tip" >/dev/null || return
  jq -e --argjson before "$before_mempool" '
    .error == null and .result == [] and .result == $before.result
  ' <<<"$after_mempool" >/dev/null || return

  jq -n --argjson actor "$actor_output" \
    --arg actor_timeout "$actor_timeout" \
    --argjson before_tip "$before_tip" --argjson after_tip "$after_tip" \
    --argjson before_mempool "$before_mempool" \
    --argjson after_mempool "$after_mempool" '
    {schema_version:1,result:"passed",authority:"direct_observation_only",
      actor:$actor,before:{tip:$before_tip,mempool:$before_mempool},
      after:{tip:$after_tip,mempool:$after_mempool},
      actor_timeout_seconds:$actor_timeout,
      zebra_tip_unchanged:($before_tip.result == $after_tip.result),
      zebra_mempool_unchanged_empty:
        ($before_mempool.result == [] and $after_mempool.result == []),
      new_chain_effect:false}
  ' >"$output_file"
  chmod 0600 "$output_file"
}

start_m6_refund_maker_supervisor() {
  (( m6_maker_supervisor_suppressed == 1 && m6_lez_refund_finalized == 1 )) || return 1
  local ready_file="${application_root}/runtime/ready-refund-control"
  local log="${evidence_dir}/m6-refund-maker-control.log"
  local monitor_file="${evidence_dir}/m6-refund-maker-monitor.json"
  local first_file="${evidence_dir}/m6-refund-maker-manual-action-first.json"
  local replay_file="${evidence_dir}/m6-refund-maker-manual-action-replay.json"
  local generation refund_eligibility_tip
  refund_eligibility_tip="$(jq -er '.result | numbers' <<<"$(rpc "$ZEBRA_RPC_URL" \
    '{"jsonrpc":"2.0","id":"m6-refund-eligibility-tip","method":"getblockcount","params":[]}')")"
  (( refund_eligibility_tip == zebra_tip + 2 )) || {
    echo 'M6 refund eligibility did not begin at the exact post-funding Zebra tip' >&2
    return 1
  }
  mine_blocks refund-eligibility 3
  [[ ! -e "$ready_file" && ! -e "$m5_supervisor_socket" ]] || return 1
  "$m5_daemon_bin" --socket "$m5_supervisor_socket" --database "$m5_application_database"     --ready-file "$ready_file" >"$log" 2>&1 &
  m5_daemon_pid=$!
  m5_daemon_start_ticks="$(process_start_ticks "$m5_daemon_pid")"
  [[ -n "$m5_daemon_start_ticks" ]] || return 1
  wait_for_m5_daemon_ready "$m5_supervisor_socket" "$ready_file" "$log"
  "$maker_cli_bin" --socket "$m5_supervisor_socket" monitor --id "$m5_swap_id" >"$monitor_file"
  generation="$(jq -er '.lease_generation | numbers' "$monitor_file")"
  "$maker_cli_bin" --socket "$m5_supervisor_socket" refund --id "$m5_swap_id"     --request-id "m6-maker-refund-${run_id}" --expected-generation "$generation" >"$first_file"
  "$maker_cli_bin" --socket "$m5_supervisor_socket" refund --id "$m5_swap_id"     --request-id "m6-maker-refund-${run_id}" --expected-generation "$generation" >"$replay_file"
  jq -e --arg swap "$m5_swap_id" --argjson generation "$generation" '
    .schema_version == 1 and .swap_id == $swap and .action == "refund"
    and .requested_after_generation == $generation and .was_replay == false
  ' "$first_file" >/dev/null
  jq -e --arg swap "$m5_swap_id" --argjson generation "$generation" '
    .schema_version == 1 and .swap_id == $swap and .action == "refund"
    and .requested_after_generation == $generation and .was_replay == true
  ' "$replay_file" >/dev/null
  jq -n --slurpfile first "$first_file" --slurpfile replay "$replay_file"     '{schema_version:1,first:$first[0],replay:$replay[0],
      maker_effect_authority:"daemon_supervisor",queued_before_supervisor_restart:true}'     >"${evidence_dir}/m6-refund-maker-manual-action.json"
  stop_owned_m5_daemon
  m5_daemon_pid=''
  m5_daemon_start_ticks=''
  [[ ! -e "$m5_supervisor_socket" && ! -e "$ready_file" ]] || return 1
  start_m5_supervisor_only_daemon "$M6_REFUND_SUPERVISOR_ATTEMPT_TIMEOUT_MS"
  m6_maker_supervisor_suppressed=0
  m6_maker_supervisor_restarted=1
}

apply_m6_refund_parent_handoff() {
  local output="$1"
  local generation finalized transaction_id start_tip
  jq -e 'type == "object"' <<<"$output" >/dev/null || {
    echo 'M6 Refund child returned malformed service output' >&2
    return 1
  }
  if ! jq -e 'has("m6_refund_parent_handoff")' <<<"$output" >/dev/null; then
    return 0
  fi
  jq -e '
    .m6_refund_parent_handoff == true
    and .m6_refund_admitted == true
    and (.m6_refund_generation | type) == "number"
    and .m6_refund_generation >= 0
    and .m6_refund_generation <= 9007199254740991
    and .m6_refund_generation == (.m6_refund_generation | floor)
    and (.m6_lez_refund_finalized | type) == "boolean"
    and (.m6_lez_refund_start_tip | type) == "number"
    and .m6_lez_refund_start_tip >= 0
    and .m6_lez_refund_start_tip <= 9007199254740991
    and .m6_lez_refund_start_tip == (.m6_lez_refund_start_tip | floor)
    and (
      (.m6_lez_refund_finalized == false and .m6_lez_refund_txid == "")
      or
      (.m6_lez_refund_finalized == true
        and (.m6_lez_refund_txid | strings | test("^[0-9a-f]{64}$")))
    )
  ' <<<"$output" >/dev/null || {
    echo 'M6 Refund child returned an invalid parent handoff' >&2
    return 1
  }
  generation="$(jq -er '.m6_refund_generation | numbers' <<<"$output")"
  finalized="$(jq -r '.m6_lez_refund_finalized' <<<"$output")"
  transaction_id="$(jq -er '.m6_lez_refund_txid | strings' <<<"$output")"
  start_tip="$(jq -er '.m6_lez_refund_start_tip | numbers' <<<"$output")"
  if (( m6_refund_admitted == 1 )); then
    [[ "$m6_refund_generation" == "$generation"
      && "$m6_lez_refund_start_tip" == "$start_tip" ]] || {
      echo 'M6 Refund child attempted to replace admitted parent state' >&2
      return 1
    }
  fi
  if (( m6_lez_refund_finalized == 1 )); then
    [[ "$finalized" == true && "$m6_lez_refund_txid" == "$transaction_id" ]] || {
      echo 'M6 Refund child attempted to replace finalized parent state' >&2
      return 1
    }
  fi
  m6_refund_admitted=1
  m6_refund_generation="$generation"
  m6_lez_refund_txid="$transaction_id"
  m6_lez_refund_start_tip="$start_tip"
  [[ "$finalized" == true ]] && m6_lez_refund_finalized=1
  if (( m6_lez_refund_finalized == 1 && m6_maker_supervisor_suppressed == 1
    && m6_maker_supervisor_restarted == 0 )); then
    start_m6_refund_maker_supervisor
  fi
}

m6_refund_replay_is_transient() {
  local response="$1"
  jq -e '
    . == {
      jsonrpc:"2.0",id:"m6-refund-replay",
      error:{code:-32010,message:"Taker dependency unavailable",
        data:{category:"taker_action_execution_unavailable"}}
    }
  ' <<<"$response" >/dev/null 2>&1
}

m6_refund_waits_for_maker_recovery() {
  (( m6_refund_admitted == 1 && m6_lez_refund_finalized == 1
    && m6_maker_supervisor_restarted == 1 && m6_zcash_refund_mined == 0 ))
}

emit_m6_refund_parent_handoff() {
  local response="$1"
  jq -c --argjson generation "$m6_refund_generation" \
    --arg txid "$m6_lez_refund_txid" \
    --argjson finalized "$m6_lez_refund_finalized" \
    --argjson start_tip "$m6_lez_refund_start_tip" '
    (.result // {}) + {m6_refund_parent_handoff:true,m6_refund_admitted:true,
      m6_refund_generation:$generation,m6_lez_refund_txid:$txid,
      m6_lez_refund_finalized:($finalized == 1),
      m6_lez_refund_start_tip:$start_tip}
  ' <<<"$response"
}

drive_m6_taker_refund() {
  local round="$1" monitor_response="$2" state="$3"
  local status_file="${evidence_dir}/m6-taker-refund-actor-status-${round}.json"
  local refund_request first_response replay_response claim_response
  local before_set after_set new_set output admission_attempt

  case "$state" in
    refunded)
      jq -c '.result' <<<"$monitor_response"
      return 0
      ;;
    awaiting_first_lock|awaiting_second_lock|both_legs_locked|refund_available)
      if (( m6_refund_admitted == 0 )); then
        "$actor_bin" --config "$taker_config" status >"$status_file"
        if ! jq -e '
          .role == "taker" and .state == "active" and .phase == "both_legs_locked"
        ' "$status_file" >/dev/null; then
          output="$(drive_actor taker "$taker_config" "$round")" || return
          if jq -e '
            .operation == "zcash_followup_claim"
            or .operation == "lez_refund" or .operation == "zcash_refund"
          ' <<<"$output" >/dev/null; then
            echo 'direct Taker drive crossed the M6 service terminal-action boundary' >&2
            return 1
          fi
          printf '%s\n' "$output"
          return 0
        fi
      fi
      ;;
    refund_in_progress|attention_required) ;;
    *)
      echo "M6 refund monitor returned inadmissible state: ${state}" >&2
      return 1
      ;;
  esac
  if (( m6_refund_admitted == 0 )) && [[ "$state" != refund_available ]]; then
    jq -c '.result' <<<"$monitor_response"
    return 0
  fi

  (( m5_transport_cutover_complete == 1 \
    && (m6_maker_supervisor_suppressed == 1 || m6_maker_supervisor_restarted == 1) )) ||
    { echo 'M6 refund requires isolated Maker authority after cutover' >&2; return 1; }
  [[ ! -e "$m5_maker_socket" && ! -e "$m5_chat_socket"
    && ! -e "$m5_delivery_directory" && -d "$m5_delivery_offline" ]] || return 1

  if m6_refund_waits_for_maker_recovery; then
    emit_m6_refund_parent_handoff "$monitor_response"
    return
  fi

  if (( m6_refund_admitted == 0 )); then
    [[ "$state" == refund_available
      && "$(jq -er '.result.available_action' <<<"$monitor_response")" == refund ]] || return 1
    m6_refund_generation="$(jq -er '.result.progress_generation | numbers' <<<"$monitor_response")"
    wait_for_m6_lez_refund_window
    m6_lez_refund_start_tip="$(m6_finalized_tip)"
    before_set="$(m6_taker_lez_submission_set)"
    printf '%s\n' "$before_set" >"${evidence_dir}/m6-taker-lez-submissions-before-refund.json"
    refund_request="$(jq -nc --arg request_id "$m6_refund_request_id"       --arg swap "$m5_swap_id" --argjson generation "$m6_refund_generation" '
      {jsonrpc:"2.0",id:"m6-refund",method:"taker_swap_refund_v1",params:[{
        schema_version:1,request_id:$request_id,swap_id:$swap,
        expected_generation:$generation}]}
    ')"
    : >"${evidence_dir}/m6-taker-service-refund-transients.ndjson"
    admission_attempt=1
    while true; do
      first_response="$(m6_service_rpc "m6-refund-admission-${admission_attempt}" \
        "$refund_request" "$M6_SERVICE_ACTION_TIMEOUT_MS")"
      if (( admission_attempt == 1 )); then
        printf '%s\n' "$first_response" >"${evidence_dir}/m6-taker-service-refund-first.json"
      fi
      if jq -e --arg swap "$m5_swap_id" --argjson generation "$m6_refund_generation" '
        .error == null and .result.schema_version == 1 and .result.swap_id == $swap
        and .result.action == "refund"
        and .result.requested_after_generation == $generation
        and (.result.was_replay | type) == "boolean"
      ' <<<"$first_response" >/dev/null; then
        break
      fi
      jq -e '
        .error.code == -32010 and .error.message == "Taker dependency unavailable"
        and .error.data.category == "taker_action_execution_unavailable"
      ' <<<"$first_response" >/dev/null || return 1
      jq -nc --argjson attempt "$admission_attempt" --argjson response "$first_response" '
        {schema_version:1,attempt:$attempt,response:$response}
      ' >>"${evidence_dir}/m6-taker-service-refund-transients.ndjson"
      remaining_budget_milliseconds "m6-refund-admission-retry-${admission_attempt}" >/dev/null ||
        return
      sleep 0.10
      admission_attempt=$((admission_attempt + 1))
    done
    printf '%s\n' "$first_response" >"${evidence_dir}/m6-taker-service-refund-commit.json"
    jq -e --arg swap "$m5_swap_id" --argjson generation "$m6_refund_generation" '
      .error == null and .result.schema_version == 1 and .result.swap_id == $swap
      and .result.action == "refund"
      and .result.requested_after_generation == $generation
      and (.result.was_replay | type) == "boolean"
    ' <<<"$first_response" >/dev/null || return 1

    claim_response="$(m6_service_rpc 'm6-refund-claim-exclusion'       "$(jq -nc --arg swap "$m5_swap_id" --argjson generation "$m6_refund_generation" '
        {jsonrpc:"2.0",id:"m6-refund-claim-exclusion",method:"taker_swap_claim_v1",
          params:[{schema_version:1,request_id:"m6-claim-after-refund",
            swap_id:$swap,expected_generation:$generation}]}
      ')")"
    printf '%s\n' "$claim_response"       >"${evidence_dir}/m6-taker-service-refund-claim-exclusion.json"
    jq -e '
      .error.code == -32017 and .error.message == "Taker action conflict"
      and .error.data.category == "taker_action_conflict"
    ' <<<"$claim_response" >/dev/null || return 1
    m6_refund_admitted=1
  fi

  refund_request="$(jq -nc --arg request_id "$m6_refund_request_id"     --arg swap "$m5_swap_id" --argjson generation "$m6_refund_generation" '
    {jsonrpc:"2.0",id:"m6-refund-replay",method:"taker_swap_refund_v1",params:[{
      schema_version:1,request_id:$request_id,swap_id:$swap,
      expected_generation:$generation}]}
  ')"
  replay_response="$(m6_service_rpc "m6-refund-replay-${round}" "$refund_request" "$M6_SERVICE_ACTION_TIMEOUT_MS")"
  printf '%s\n' "$replay_response" >"${evidence_dir}/m6-taker-service-refund-replay.json"
  if jq -e --arg swap "$m5_swap_id" --argjson generation "$m6_refund_generation" '
    .error == null and .result == {schema_version:1,swap_id:$swap,action:"refund",
      requested_after_generation:$generation,was_replay:true}
  ' <<<"$replay_response" >/dev/null; then
    jq -nc --argjson round "$round" --argjson replay "$replay_response" \
      '{schema_version:1,round:$round,replay:$replay}' \
      >>"${evidence_dir}/m6-taker-service-refund.ndjson"
  elif m6_refund_replay_is_transient "$replay_response"; then
    jq -nc --argjson round "$round" --argjson response "$replay_response" '
      {schema_version:1,phase:"reconcile",round:$round,response:$response}
    ' >>"${evidence_dir}/m6-taker-service-refund-transients.ndjson"
  else
    return 1
  fi

  if (( m6_lez_refund_finalized == 0 )); then
    before_set="$(jq -c . "${evidence_dir}/m6-taker-lez-submissions-before-refund.json")"
    after_set="$(m6_taker_lez_submission_set)"
    new_set="$(jq -nc --argjson before "$before_set" --argjson after "$after_set"       '$after - $before')"
    if [[ "$(jq -er 'length' <<<"$new_set")" == 1 ]]; then
      m6_lez_refund_txid="$(jq -er '.[0] | strings | select(test("^[0-9a-f]{64}$"))' <<<"$new_set")"
      prove_m6_lez_refund_finality "$m6_lez_refund_txid" "$m6_lez_refund_start_tip"
      m6_lez_refund_finalized=1
    fi
  fi
  emit_m6_refund_parent_handoff "$replay_response"
}

prove_m6_terminal_refund_replay() {
  local trace_before="${evidence_dir}/m6-taker-lez-submission-trace-before-terminal-replay.json"
  local trace_after="${evidence_dir}/m6-taker-lez-submission-trace-after-terminal-replay.json"
  local zebra_before="${evidence_dir}/m6-zebra-terminal-replay-before.json"
  local zebra_after="${evidence_dir}/m6-zebra-terminal-replay-after.json"
  local request response tip_response mempool_response
  local lez_height lez_hash lez_block zcash_block zcash_canonical
  local replay_sha trace_sha zebra_before_sha zebra_after_sha

  m6_taker_lez_submission_trace >"$trace_before"
  tip_response="$(rpc "$ZEBRA_RPC_URL" '{"jsonrpc":"2.0","id":"m6-terminal-before-tip","method":"getblockcount","params":[]}')"
  mempool_response="$(rpc "$ZEBRA_RPC_URL" '{"jsonrpc":"2.0","id":"m6-terminal-before-mempool","method":"getrawmempool","params":[]}')"
  jq -n --argjson tip "$tip_response" --argjson mempool "$mempool_response" '
    {schema_version:1,tip_response:$tip,mempool_response:$mempool}
  ' >"$zebra_before"
  jq -e '
    .tip_response.error == null and (.tip_response.result | numbers) >= 1
    and .mempool_response.error == null and .mempool_response.result == []
  ' "$zebra_before" >/dev/null

  request="$(jq -nc --arg request_id "$m6_refund_request_id" --arg swap "$m5_swap_id" --argjson generation "$m6_refund_generation" '
    {jsonrpc:"2.0",id:"m6-refund-terminal-replay",
      method:"taker_swap_refund_v1",params:[{
        schema_version:1,request_id:$request_id,swap_id:$swap,
        expected_generation:$generation}]}
  ')"
  response="$(m6_service_rpc 'm6-refund-terminal-replay' "$request" "$M6_SERVICE_ACTION_TIMEOUT_MS")"
  printf '%s\n' "$response" >"${evidence_dir}/m6-taker-service-refund-terminal-replay.json"
  jq -e --arg swap "$m5_swap_id" --argjson generation "$m6_refund_generation" '
    .error == null and .result == {schema_version:1,swap_id:$swap,action:"refund",
      requested_after_generation:$generation,was_replay:true}
  ' <<<"$response" >/dev/null

  m6_taker_lez_submission_trace >"$trace_after"
  cmp -s "$trace_before" "$trace_after" || {
    echo 'terminal Refund replay changed the ordered successful LEZ submission trace' >&2
    return 1
  }
  tip_response="$(rpc "$ZEBRA_RPC_URL" '{"jsonrpc":"2.0","id":"m6-terminal-after-tip","method":"getblockcount","params":[]}')"
  mempool_response="$(rpc "$ZEBRA_RPC_URL" '{"jsonrpc":"2.0","id":"m6-terminal-after-mempool","method":"getrawmempool","params":[]}')"
  jq -n --argjson tip "$tip_response" --argjson mempool "$mempool_response" '
    {schema_version:1,tip_response:$tip,mempool_response:$mempool}
  ' >"$zebra_after"
  jq -n -e --slurpfile before "$zebra_before" --slurpfile after "$zebra_after" '
    ($before | length) == 1 and ($after | length) == 1
    and $before[0].tip_response.error == null
    and $after[0].tip_response.error == null
    and $before[0].tip_response.result == $after[0].tip_response.result
    and $before[0].mempool_response.error == null
    and $after[0].mempool_response.error == null
    and $before[0].mempool_response.result == []
    and $after[0].mempool_response.result == []
  ' >/dev/null

  lez_height="$(jq -er '.containing_block_id | numbers' "${evidence_dir}/m6-taker-lez-refund-finality.json")"
  lez_hash="$(jq -er '.containing_block_hash | strings' "${evidence_dir}/m6-taker-lez-refund-finality.json")"
  lez_block="$(rpc "$LEZ_INDEXER_URL" "$(jq -nc --argjson height "$lez_height" '{jsonrpc:"2.0",id:"m6-refund-terminal-lez-block",method:"getBlockById",params:[$height]}')")"
  printf '%s\n' "$lez_block" >"${evidence_dir}/m6-taker-lez-refund-terminal-revalidation.json"
  jq -e --arg tx "$m6_lez_refund_txid" --arg hash "$lez_hash" --argjson height "$lez_height" '
    .error == null and .result.header.block_id == $height
    and .result.header.hash == $hash and .result.bedrock_status == "Finalized"
    and ([.result.body.transactions[]? | select(.Public.hash == $tx)] | length) == 1
  ' <<<"$lez_block" >/dev/null

  zcash_block="$(rpc "$ZEBRA_RPC_URL" "$(jq -nc --arg hash "$m6_zcash_refund_block_hash" '{jsonrpc:"2.0",id:"m6-refund-terminal-zcash-block",method:"getblock",params:[$hash,1]}')")"
  zcash_canonical="$(rpc "$ZEBRA_RPC_URL" "$(jq -nc --argjson height "$m6_zcash_refund_block_height" '{jsonrpc:"2.0",id:"m6-refund-terminal-zcash-canonical",method:"getblockhash",params:[$height]}')")"
  jq -n --arg tx "$m6_zcash_refund_txid" --arg hash "$m6_zcash_refund_block_hash" --argjson height "$m6_zcash_refund_block_height" --argjson block "$zcash_block" --argjson canonical "$zcash_canonical" '
    {schema_version:1,transaction_id:$tx,block_hash:$hash,height:$height,
      block_response:$block,canonical_hash_response:$canonical}
  ' >"${evidence_dir}/m6-zebra-zcash-refund-terminal-revalidation.json"
  jq -e --arg tx "$m6_zcash_refund_txid" --arg hash "$m6_zcash_refund_block_hash" --argjson height "$m6_zcash_refund_block_height" '
    .schema_version == 1 and .transaction_id == $tx
    and .block_response.error == null and .block_response.result.hash == $hash
    and .block_response.result.height == $height
    and ([.block_response.result.tx[] | select(. == $tx)] | length) == 1
    and .canonical_hash_response.error == null
    and .canonical_hash_response.result == $hash
  ' "${evidence_dir}/m6-zebra-zcash-refund-terminal-revalidation.json" >/dev/null

  replay_sha="$(sha256sum "${evidence_dir}/m6-taker-service-refund-terminal-replay.json" | cut -d ' ' -f1)"
  trace_sha="$(sha256sum "$trace_after" | cut -d ' ' -f1)"
  zebra_before_sha="$(sha256sum "$zebra_before" | cut -d ' ' -f1)"
  zebra_after_sha="$(sha256sum "$zebra_after" | cut -d ' ' -f1)"
  jq -n --arg replay_sha "$replay_sha" --arg trace_sha "$trace_sha" --arg zebra_before_sha "$zebra_before_sha" --arg zebra_after_sha "$zebra_after_sha" '
    {schema_version:1,terminal_replay_was_exact:true,
      ordered_lez_submission_trace_unchanged:true,zebra_tip_unchanged:true,
      zebra_mempool_empty_before_and_after:true,
      canonical_lez_refund_revalidated:true,
      canonical_zcash_refund_revalidated:true,
      replay_sha256:$replay_sha,lez_trace_sha256:$trace_sha,
      zebra_before_sha256:$zebra_before_sha,zebra_after_sha256:$zebra_after_sha}
  ' >"${evidence_dir}/m6-taker-service-refund-terminal-no-effect.json"
}

drive_m6_taker() {
  local round="$1"
  local monitor_request monitor_response state generation claim_request
  local first_response replay_response mempool_before mempool_after_first mempool_after_replay
  local output
  process_is_owned "$m6_service_pid" "$m6_service_start_ticks" "$m6_service_bin" || {
    echo 'M6 Taker service identity changed during the corridor' >&2
    return 1
  }
  [[ -S "$m6_service_socket" ]] || return 1
  monitor_request="$(jq -nc --arg swap "$m5_swap_id" '
    {jsonrpc:"2.0",id:"m6-monitor",method:"taker_swap_monitor_v1",params:[{
      schema_version:1,swap_id:$swap}]}
  ')"
  monitor_response="$(m6_service_rpc "m6-monitor-${round}" "$monitor_request")"
  jq -e --arg swap "$m5_swap_id" '
    .error == null and .result.schema_version == 1 and .result.swap_id == $swap
    and (.result.progress_generation | numbers) >= 0 and (.result.state | strings)
  ' <<<"$monitor_response" >/dev/null || return 1
  jq -nc --argjson round "$round" --argjson response "$monitor_response" \
    '{schema_version:1,round:$round,response:$response}' \
    >>"${evidence_dir}/m6-taker-service-monitor.ndjson"
  state="$(jq -er '.result.state' <<<"$monitor_response")"
  if [[ "$M6_ZEC_JOURNEY" == refund ]]; then
    drive_m6_taker_refund "$round" "$monitor_response" "$state"
    return
  fi
  if [[ "$state" == completed ]]; then
    jq -c '.result' <<<"$monitor_response"
    return 0
  fi

  if [[ "$state" == claim_available || "$state" == claim_in_progress ]]; then
    (( m5_transport_cutover_complete == 1 )) || {
      echo 'M6 service claim became reachable before negotiation cutover' >&2
      return 1
    }
    [[ ! -e "$m5_maker_socket" && ! -e "$m5_chat_socket"
      && ! -e "$m5_delivery_directory" && -d "$m5_delivery_offline" ]] || {
      echo 'M6 service claim retained a negotiation transport' >&2
      return 1
    }
    if (( m6_claim_admitted == 0 )); then
      [[ "$state" == claim_available
        && "$(jq -er '.result.available_action' <<<"$monitor_response")" == claim ]] || return 1
      m6_claim_generation="$(jq -er '.result.progress_generation | numbers' <<<"$monitor_response")"
    fi
    generation="$m6_claim_generation"
    claim_request="$(jq -nc --arg request_id "$m6_claim_request_id" \
      --arg swap "$m5_swap_id" --argjson generation "$generation" '
      {jsonrpc:"2.0",id:"m6-claim",method:"taker_swap_claim_v1",params:[{
        schema_version:1,request_id:$request_id,swap_id:$swap,expected_generation:$generation}]}
    ')"

    if (( m6_claim_admitted == 0 )); then
      mempool_before="$(rpc "$ZEBRA_RPC_URL" \
        '{"jsonrpc":"2.0","id":"m6-before","method":"getrawmempool","params":[]}')"
      printf '%s\n' "$mempool_before" \
        >"${evidence_dir}/m6-zebra-mempool-before-claim.json"
      chmod 0600 "${evidence_dir}/m6-zebra-mempool-before-claim.json"
      jq -e '.error == null and .result == []' <<<"$mempool_before" >/dev/null || {
        echo 'M6 claim requires an isolated empty Zebra mempool' >&2
        return 1
      }
      first_response="$(m6_service_rpc 'm6-claim-first' "$claim_request" "$M6_SERVICE_ACTION_TIMEOUT_MS")"
      mempool_after_first="$(rpc "$ZEBRA_RPC_URL" \
        '{"jsonrpc":"2.0","id":"m6-after-first","method":"getrawmempool","params":[]}')"
      printf '%s\n' "$first_response" \
        >"${evidence_dir}/m6-taker-service-claim-first.json"
      printf '%s\n' "$mempool_after_first" \
        >"${evidence_dir}/m6-zebra-mempool-after-first-claim.json"
      replay_response="$(m6_service_rpc 'm6-claim-replay' "$claim_request" "$M6_SERVICE_ACTION_TIMEOUT_MS")"
      mempool_after_replay="$(rpc "$ZEBRA_RPC_URL" \
        '{"jsonrpc":"2.0","id":"m6-after-replay","method":"getrawmempool","params":[]}')"
      printf '%s\n' "$replay_response" \
        >"${evidence_dir}/m6-taker-service-claim-replay.json"
      printf '%s\n' "$mempool_after_replay" \
        >"${evidence_dir}/m6-zebra-mempool-after-claim-replay.json"
      chmod 0600 "${evidence_dir}/m6-taker-service-claim-first.json" \
        "${evidence_dir}/m6-taker-service-claim-replay.json" \
        "${evidence_dir}/m6-zebra-mempool-after-first-claim.json" \
        "${evidence_dir}/m6-zebra-mempool-after-claim-replay.json"
      jq -e --arg swap "$m5_swap_id" --argjson generation "$generation" '
        .error == null and .result == {schema_version:1,swap_id:$swap,action:"claim",
          requested_after_generation:$generation,was_replay:false}
      ' <<<"$first_response" >/dev/null || {
        echo 'M6 first claim did not return its admitted action commit' >&2
        return 1
      }
      jq -e --arg swap "$m5_swap_id" --argjson generation "$generation" '
        .error == null and .result == {schema_version:1,swap_id:$swap,action:"claim",
          requested_after_generation:$generation,was_replay:true}
      ' <<<"$replay_response" >/dev/null || {
        echo 'M6 exact claim replay did not return its durable action commit' >&2
        return 1
      }
      m6_zcash_claim_txid="$(jq -er '.result | arrays | select(length == 1) | .[0] | strings' \
        <<<"$mempool_after_first")"
      jq -e --arg txid "$m6_zcash_claim_txid" '
        .error == null and .result == [$txid]
      ' <<<"$mempool_after_replay" >/dev/null || {
        echo 'M6 exact claim replay changed the isolated Zebra mempool' >&2
        return 1
      }
      jq -nc --argjson round "$round" --argjson first "$first_response" \
        --argjson replay "$replay_response" --argjson before "$mempool_before" \
        --argjson after_first "$mempool_after_first" \
        --argjson after_replay "$mempool_after_replay" --arg txid "$m6_zcash_claim_txid" '
        {schema_version:1,round:$round,first:$first,replay:$replay,
          mempool_before:$before,mempool_after_first:$after_first,
          mempool_after_replay:$after_replay,claim_txid:$txid}
      ' >>"${evidence_dir}/m6-taker-service-claim.ndjson"
      m6_claim_admitted=1
      jq -c --argjson generation "$generation" --arg txid "$m6_zcash_claim_txid" '
        .result + {m6_first_claim:true,m6_claim_generation:$generation,
          m6_zcash_claim_txid:$txid}
      ' <<<"$first_response"
      return 0
    fi

    replay_response="$(m6_service_rpc "m6-claim-reconcile-${round}" "$claim_request" "$M6_SERVICE_ACTION_TIMEOUT_MS")"
    jq -e --arg swap "$m5_swap_id" --argjson generation "$generation" '
      .error == null and .result == {schema_version:1,swap_id:$swap,action:"claim",
        requested_after_generation:$generation,was_replay:true}
    ' <<<"$replay_response" >/dev/null
    jq -nc --argjson round "$round" --argjson replay "$replay_response" \
      '{schema_version:1,round:$round,reconcile:true,replay:$replay}' \
      >>"${evidence_dir}/m6-taker-service-claim.ndjson"
    jq -c '.result' <<<"$replay_response"
    return 0
  fi

  case "$state" in
    awaiting_first_lock|awaiting_second_lock|both_legs_locked|refund_available)
      output="$(drive_actor taker "$taker_config" "$round")" || return
      if jq -e '.operation == "zcash_followup_claim"' <<<"$output" >/dev/null; then
        echo 'direct Taker drive crossed the M6 service claim boundary' >&2
        return 1
      fi
      printf '%s\n' "$output"
      ;;
    *)
      echo "M6 Taker monitor returned inadmissible state: ${state}" >&2
      return 1
      ;;
  esac
}

drive_m5_taker() {
  local round="$1"
  if [[ "$M5_APPLICATION_MODE" != 1 ]]; then
    drive_actor taker "$taker_config" "$round"
    return
  fi
  if [[ "$M6_TAKER_SERVICE_MODE" == 1 ]]; then
    drive_m6_taker "$round"
    return
  fi

  local status_file="${evidence_dir}/m5-taker-receipt-status-${round}.json"
  local monitor_stderr="${evidence_dir}/m5-taker-receipt-status-${round}.stderr"
  local claim_stderr="${evidence_dir}/m5-taker-receipt-claim-${round}.stderr"
  local monitor_timeout claim_timeout output
  local raw_taker_drive_admitted=0
  assert_m5_taker_receipt_unchanged || return
  monitor_timeout="$(bounded_actor_timeout "m5-taker-monitor-${round}")" || return
  timeout --signal=KILL "${monitor_timeout}s" \
    "$taker_bin" monitor --receipt "$m5_taker_acceptance_receipt" \
    >"$status_file" 2>"$monitor_stderr" || {
    echo "receipt-bound Taker monitor failed in round ${round}" >&2
    sed -n '1,20p' "$monitor_stderr" >&2
    return 1
  }
  assert_m5_taker_receipt_unchanged || return
  remaining_budget_milliseconds "m5-taker-monitor-${round}-after" >/dev/null || return
  jq -e '
    .schema_version == 1 and .role == "taker" and .state == "active"
    and (.phase | strings) and (.revision | numbers) >= 0
    and (.next_action | strings)
  ' "$status_file" >/dev/null || {
    echo 'receipt-bound Taker monitor returned invalid status' >&2
    return 1
  }
  jq -nc --argjson round "$round" --arg swap "$m5_swap_id" \
    --arg receipt_sha256 "$m5_taker_acceptance_receipt_sha256" \
    --slurpfile status "$status_file" '
    {schema_version:1,round:$round,swap_id:$swap,
      acceptance_receipt_sha256:$receipt_sha256,status:$status[0]}
  ' >>"${evidence_dir}/m5-taker-receipt-monitor.ndjson"

  if ! jq -e '
    .phase == "claim_evidence_available" and .next_action == "claim_zcash"
  ' "$status_file" >/dev/null; then
    if jq -e '
      (.phase == "offered" and .next_action == "create_and_fund_lez")
      or (.phase == "taker_lock_confirmed" and .next_action == "wait")
      or (.phase == "both_legs_locked" and .next_action == "wait")
      or (.phase == "completed" and .next_action == "complete")
    ' "$status_file" >/dev/null; then
      raw_taker_drive_admitted=1
    fi
    (( raw_taker_drive_admitted == 1 )) || {
      echo 'receipt-bound Taker status is not admitted to raw drive or claim' >&2
      return 1
    }
    output="$(drive_actor taker "$taker_config" "$round")" || return
    if jq -e '.operation == "zcash_followup_claim"' <<<"$output" >/dev/null; then
      echo 'direct Taker drive crossed the receipt-bound claim boundary' >&2
      return 1
    fi
    printf '%s\n' "$output"
    return
  fi

  (( m5_transport_cutover_complete == 1 )) || {
    echo 'receipt-bound Taker claim became eligible before post-lock cutover' >&2
    return 1
  }
  [[ ! -e "$m5_maker_socket" && ! -e "$m5_chat_socket" \
    && ! -e "$m5_delivery_directory" && -d "$m5_delivery_offline" ]] || {
    echo 'receipt-bound Taker claim retained an application transport' >&2
    return 1
  }
  assert_m5_taker_receipt_unchanged || return
  claim_timeout="$(bounded_actor_timeout "m5-taker-claim-${round}")" || return
  if ! output="$(timeout --signal=KILL "${claim_timeout}s" \
    "$taker_bin" claim --receipt "$m5_taker_acceptance_receipt" \
    2>"$claim_stderr")"; then
    assert_m5_taker_receipt_unchanged || return
    echo "receipt-bound Taker claim failed in round ${round}" >&2
    sed -n '1,20p' "$claim_stderr" >&2
    return 1
  fi
  assert_m5_taker_receipt_unchanged || return
  remaining_budget_milliseconds "m5-taker-claim-${round}-after" >/dev/null || return
  jq -e '
    .schema_version == 1 and .role == "taker" and .command == "claim"
    and (.phase | strings) and (.revision | numbers) >= 0
    and ((.operation == "zcash_followup_claim"
      and (.outcome == "awaiting_observation" or .outcome == "projected"
        or .outcome == "submitted"))
      or .outcome == "completed")
  ' <<<"$output" >/dev/null || {
    echo 'receipt-bound Taker claim returned invalid output' >&2
    return 1
  }
  jq -nc --argjson round "$round" --arg swap "$m5_swap_id" \
    --arg receipt_sha256 "$m5_taker_acceptance_receipt_sha256" \
    --slurpfile admission "$status_file" --argjson effect "$output" '
    {schema_version:1,round:$round,swap_id:$swap,
      acceptance_receipt_sha256:$receipt_sha256,
      admission:$admission[0],effect:$effect}
  ' >>"${evidence_dir}/m5-taker-receipt-claim.ndjson"
  printf '%s\n' "$output"
}

mine_blocks() {
  local label="$1"
  local count="$2"
  local request response rpc_status=0
  request="$(jq -nc --argjson count "$count" '
    {jsonrpc: "2.0", id: 1, method: "generate", params: [$count]}')"
  response="$(rpc "$ZEBRA_RPC_URL" "$request")" \
    || rpc_status=$?
  jq -e --argjson count "$count" \
    '.error == null and (.result | arrays | length == $count)' \
    <<<"$response" >/dev/null
  jq . <<<"$response" >"${evidence_dir}/zebra-generate-${label}.json"
  (( rpc_status == 0 ))
}

prove_m6_zcash_refund_inclusion() {
  local generated block canonical expected_height occurrences
  generated="${evidence_dir}/zebra-generate-zcash-refund.json"
  jq -e '
    .error == null and (.result | arrays) and (.result | length) == 1
    and (.result[0] | strings | test("^[0-9a-f]{64}$"))
  ' "$generated" >/dev/null
  m6_zcash_refund_block_hash="$(jq -er '.result[0]' "$generated")"
  expected_height=$((zebra_tip + 6))
  block="$(rpc "$ZEBRA_RPC_URL" "$(jq -nc --arg hash "$m6_zcash_refund_block_hash" '
    {jsonrpc:"2.0",id:"m6-zcash-refund-block",method:"getblock",params:[$hash,1]}
  ')")"
  canonical="$(rpc "$ZEBRA_RPC_URL" "$(jq -nc --argjson height "$expected_height" '
    {jsonrpc:"2.0",id:"m6-zcash-refund-canonical",method:"getblockhash",params:[$height]}
  ')")"
  occurrences="$(jq -er --arg tx "$m6_zcash_refund_txid" '
    [.result.tx[] | select(. == $tx)] | length
  ' <<<"$block")"
  jq -n --arg tx "$m6_zcash_refund_txid" \
    --arg hash "$m6_zcash_refund_block_hash" \
    --argjson height "$expected_height" --argjson occurrences "$occurrences" \
    --argjson block "$block" --argjson canonical "$canonical" '
    {schema_version:1,transaction_id:$tx,block_hash:$hash,height:$height,
      occurrences:$occurrences,block_response:$block,
      canonical_hash_response:$canonical}
  ' >"${evidence_dir}/m6-zebra-zcash-refund-inclusion.json"
  jq -e --arg tx "$m6_zcash_refund_txid" \
    --arg hash "$m6_zcash_refund_block_hash" \
    --argjson height "$expected_height" '
    .schema_version == 1 and .transaction_id == $tx and .block_hash == $hash
    and .height == $height and .occurrences == 1
    and .block_response.error == null
    and .block_response.result.hash == $hash
    and .block_response.result.height == $height
    and ([.block_response.result.tx[] | select(. == $tx)] | length) == 1
    and .canonical_hash_response.error == null
    and .canonical_hash_response.result == $hash
  ' "${evidence_dir}/m6-zebra-zcash-refund-inclusion.json" >/dev/null
  m6_zcash_refund_block_height="$expected_height"
}

cut_over_m5_negotiation_transports() {
  stop_owned_m5_daemon
  m5_daemon_pid=''
  m5_daemon_start_ticks=''
  [[ ! -e "$m5_maker_socket" && ! -e "$m5_chat_socket" ]] || {
    echo 'M5 Unix transports survived post-lock full-daemon shutdown' >&2
    return 1
  }
  mv -- "$m5_delivery_directory" "$m5_delivery_offline"
  [[ ! -e "$m5_delivery_directory" && -d "$m5_delivery_offline" ]] || {
    echo 'M5 Delivery transport survived post-lock cutover' >&2
    return 1
  }
  if [[ "$M6_ZEC_JOURNEY" == refund ]]; then
    m6_maker_supervisor_suppressed=1
  else
    start_m5_supervisor_only_daemon
    process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin" || {
      echo 'M5 supervisor-only daemon is not live after transport cutover' >&2
      return 1
    }
  fi
  jq -n \
    --arg first_lock_role maker \
    --arg expected_zebra_txid "$m5_expected_funding_txid" \
    --arg supervisor_socket "$([[ "$M6_ZEC_JOURNEY" == claim ]] && printf '%s' "$m5_supervisor_socket")" \
    --argjson confirmations_mined \
      "$(jq -er '.result | length' "${evidence_dir}/zebra-generate-funding.json")" '
    {
      schema_version: 1,
      result: "passed",
      cutover_after_first_lock: true,
      first_lock: "zcash_funding",
      first_lock_submitter: $first_lock_role,
      expected_zebra_txid: $expected_zebra_txid,
      confirmations_mined: $confirmations_mined,
      maker_effect_authority: "daemon_supervisor",
      maker_daemon_alive: ($supervisor_socket != ""),
      supervisor_suppressed_for_refund: ($supervisor_socket == ""),
      supervisor_socket: (if $supervisor_socket == "" then null else $supervisor_socket end),
      maker_socket_absent: true,
      chat_socket_absent: true,
      delivery_path_absent: true,
      concurrent_direct_maker_effects: false
    }' >"${evidence_dir}/m5-post-lock-cutover.json"
  chmod 0600 "${evidence_dir}/m5-post-lock-cutover.json"
  m5_transport_cutover_complete=1
  if [[ "$M6_TAKER_SERVICE_MODE" == 1 ]]; then
    local m6_health_request m6_health_response cutover_tmp
    process_is_owned "$m6_service_pid" "$m6_service_start_ticks" "$m6_service_bin" || {
      echo 'M6 Taker service did not survive negotiation cutover' >&2
      return 1
    }
    [[ -S "$m6_service_socket" ]] || return 1
    m6_health_request='{"jsonrpc":"2.0","id":"m6-cutover-health","method":"taker_health","params":[{"schema_version":1}]}'
    m6_health_response="$(m6_service_rpc 'm6-cutover-health' "$m6_health_request")"
    jq -e '
      .error == null and .result.ready == true and .result.degraded == true
      and .result.delivery == "unavailable" and .result.chat == "unavailable"
      and .result.registered_methods == {health:true,offer_list:true,swap_list:true,
        initiate:true,monitor:true,claim:true,refund:true}
    ' <<<"$m6_health_response" >/dev/null || return 1
    printf '%s\n' "$m6_health_response" >"${evidence_dir}/m6-taker-service-health-cutover.json"
    cutover_tmp="${evidence_dir}/m5-post-lock-cutover.m6.tmp"
    jq --argjson health "$m6_health_response" '
      . + {m6_taker_service:{survived:true,owner_socket_alive:true,
        negotiation_dependencies_unavailable:true,health:$health}}
    ' "${evidence_dir}/m5-post-lock-cutover.json" >"$cutover_tmp"
    chmod 0600 "$cutover_tmp"
    mv -- "$cutover_tmp" "${evidence_dir}/m5-post-lock-cutover.json"
  fi
}

inject_m7_zec_accepted_process_kill_if_ready() {
  [[ "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" == 1 ]] || return 0
  (( m7_zec_process_kill_injected == 0 )) || return 0
  local marker="${application_root}/runtime/m7-zec-funding-submitted.json"
  [[ -f "$marker" && ! -L "$marker" ]] || return 0
  [[ "$(stat -c %a -- "$marker")" == 600 \
    && "$(stat -c %u -- "$marker")" == "$(id -u)" \
    && "$(stat -c %h -- "$marker")" == 1 ]] || {
    echo 'M7 accepted-ZEC crash marker is unsafe' >&2
    return 1
  }

  local before_scheduler="${evidence_dir}/m7-zec-process-kill-scheduler-before.json"
  local after_scheduler="${evidence_dir}/m7-zec-process-kill-scheduler-after.json"
  local before_mempool="${evidence_dir}/m7-zec-process-kill-mempool-before.json"
  local after_mempool="${evidence_dir}/m7-zec-process-kill-mempool-after.json"
  local before_tip="${evidence_dir}/m7-zec-process-kill-tip-before.json"
  local after_tip="${evidence_dir}/m7-zec-process-kill-tip-after.json"
  local intent_candidate="${evidence_dir}/m5-maker-lock-intent-candidate.json"
  local crashed_actor_pid crashed_actor_start_ticks crashed_generation
  local crashed_daemon_pid="$m5_daemon_pid"
  local crashed_daemon_start_ticks="$m5_daemon_start_ticks"
  local recovered_generation=''

  "$actor_inspector_bin" --database "$m5_application_database" >"$before_scheduler"
  crashed_actor_pid="$(jq -er --arg swap "$m5_swap_id" '
    .[] | select(.swap_id == $swap and .schedule_state == "leased")
    | .child_identity.pid | numbers
  ' "$before_scheduler")"
  crashed_actor_start_ticks="$(jq -er --arg swap "$m5_swap_id" '
    .[] | select(.swap_id == $swap and .schedule_state == "leased")
    | .child_identity.start_ticks | numbers
  ' "$before_scheduler")"
  crashed_generation="$(jq -er --arg swap "$m5_swap_id" '
    .[] | select(.swap_id == $swap and .schedule_state == "leased")
    | .lease_generation | numbers
  ' "$before_scheduler")"
  jq -e --arg swap "$m5_swap_id" --arg program "$m5_actor_program" \
    --arg program_sha256 "$m5_actor_program_sha256" --argjson pid "$crashed_actor_pid" \
    --argjson start_ticks "$crashed_actor_start_ticks" '
    length == 1 and .[0].swap_id == $swap and .[0].actor_kind == "zcash"
    and .[0].schedule_state == "leased"
    and .[0].actor_program_path == $program
    and .[0].actor_program_sha256 == $program_sha256
    and .[0].child_identity == {pid:$pid,start_ticks:$start_ticks}
  ' "$before_scheduler" >/dev/null || {
    echo 'M7 accepted-ZEC scheduler does not bind the exact leased actor identity' >&2
    return 1
  }
  jq -e --arg swap "$m5_swap_id" --argjson pid "$crashed_actor_pid" '
    .schema_version == 1 and .state == "paused_after_submitted_before_stdout"
    and .swap_id == $swap and .role == "maker" and .operation == "zcash_fund"
    and .process_id == $pid
  ' "$marker" >/dev/null || {
    echo 'M7 accepted-ZEC crash marker does not bind the leased Maker actor' >&2
    return 1
  }
  process_start_identity_matches "$crashed_actor_pid" "$crashed_actor_start_ticks" || {
    echo 'M7 accepted-ZEC paused actor identity changed before SIGKILL' >&2
    return 1
  }
  process_is_owned "$crashed_daemon_pid" "$crashed_daemon_start_ticks" "$m5_daemon_bin" || {
    echo 'M7 accepted-ZEC daemon identity changed before SIGKILL' >&2
    return 1
  }

  if [[ -z "$m5_expected_funding_txid" ]]; then
    "$m5_intent_inspector_bin" --config "$maker_config" --taker-config "$taker_config" \
      >"$intent_candidate" 2>"${evidence_dir}/m5-maker-lock-intent-candidate.stderr"
    jq -e --arg swap "$m5_swap_id" '
      .schema_version == 1 and .swap_id == $swap and .role == "maker"
      and .operation == "zcash_fund" and (.staged_revision | numbers) >= 0
      and (.expected_submission_id_internal_hex | test("^[0-9a-f]{64}$"))
      and (.expected_zebra_txid | test("^[0-9a-f]{64}$"))
      and .actor_pair_validated == true and .exact_submission_disclosed == false
    ' "$intent_candidate" >/dev/null || return 1
    mv -- "$intent_candidate" "${evidence_dir}/m5-maker-lock-intent.json"
    m5_expected_funding_txid="$(jq -er '.expected_zebra_txid' \
      "${evidence_dir}/m5-maker-lock-intent.json")"
  fi

  rpc "$ZEBRA_RPC_URL" \
    '{"jsonrpc":"2.0","id":"m7-crash-before-mempool","method":"getrawmempool","params":[]}' \
    >"$before_mempool"
  rpc "$ZEBRA_RPC_URL" \
    '{"jsonrpc":"2.0","id":"m7-crash-before-tip","method":"getblockcount","params":[]}' \
    >"$before_tip"
  jq -e --arg tx "$m5_expected_funding_txid" '
    .error == null and .result == [$tx]
  ' "$before_mempool" >/dev/null || {
    echo 'M7 accepted-ZEC crash boundary lacks the exact singleton funding transaction' >&2
    return 1
  }
  jq -e '.error == null and (.result | numbers) >= 104' "$before_tip" >/dev/null || return 1

  kill -KILL "$crashed_daemon_pid" || return 1
  for _ in {1..200}; do
    process_start_identity_matches "$crashed_daemon_pid" "$crashed_daemon_start_ticks" || break
    sleep 0.05
  done
  process_start_identity_matches "$crashed_daemon_pid" "$crashed_daemon_start_ticks" && {
    echo 'M7 accepted-ZEC daemon survived exact SIGKILL' >&2
    return 1
  }
  wait "$crashed_daemon_pid" 2>/dev/null || true

  kill -KILL -- "-${crashed_actor_pid}" || return 1
  for _ in {1..200}; do
    process_start_identity_matches "$crashed_actor_pid" "$crashed_actor_start_ticks" || break
    sleep 0.05
  done
  process_start_identity_matches "$crashed_actor_pid" "$crashed_actor_start_ticks" && {
    echo 'M7 accepted-ZEC actor survived exact process-group SIGKILL' >&2
    return 1
  }

  m5_daemon_pid=''
  m5_daemon_start_ticks=''
  for stale_path in \
    "${application_root}/runtime/ready-supervised" "$m5_maker_socket" "$m5_chat_socket"; do
    [[ ! -e "$stale_path" && ! -S "$stale_path" ]] || rm -f -- "$stale_path"
  done
  [[ -d "$m5_delivery_directory" && ! -e "$m5_delivery_offline" ]] || {
    echo 'M7 accepted-ZEC crash changed Delivery before the first-lock cutover' >&2
    return 1
  }

  m7_zec_process_kill_injected=1
  start_m5_full_supervised_daemon recovery
  for _ in {1..200}; do
    "$actor_inspector_bin" --database "$m5_application_database" >"$after_scheduler"
    recovered_generation="$(jq -r --arg swap "$m5_swap_id" \
      --argjson crashed "$crashed_generation" '
      first(.[] | select(.swap_id == $swap and .lease_generation > $crashed)
        | .lease_generation) // empty
    ' "$after_scheduler" || true)"
    [[ -n "$recovered_generation" ]] && break
    process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin" || return 1
    sleep 0.05
  done
  [[ -n "$recovered_generation" ]] || {
    echo 'M7 accepted-ZEC restart did not transfer the abandoned actor lease' >&2
    return 1
  }

  rpc "$ZEBRA_RPC_URL" \
    '{"jsonrpc":"2.0","id":"m7-crash-after-mempool","method":"getrawmempool","params":[]}' \
    >"$after_mempool"
  rpc "$ZEBRA_RPC_URL" \
    '{"jsonrpc":"2.0","id":"m7-crash-after-tip","method":"getblockcount","params":[]}' \
    >"$after_tip"
  jq -e --arg tx "$m5_expected_funding_txid" '.error == null and .result == [$tx]' \
    "$after_mempool" >/dev/null || return 1
  jq -e --slurpfile before "$before_tip" '
    .error == null and .result == $before[0].result
  ' "$after_tip" >/dev/null || return 1

  jq -n --slurpfile marker "$marker" --slurpfile before_scheduler "$before_scheduler" \
    --slurpfile after_scheduler "$after_scheduler" --slurpfile before_mempool "$before_mempool" \
    --slurpfile after_mempool "$after_mempool" --slurpfile before_tip "$before_tip" \
    --slurpfile after_tip "$after_tip" --arg swap "$m5_swap_id" \
    --arg tx "$m5_expected_funding_txid" --argjson crashed_daemon_pid "$crashed_daemon_pid" \
    --arg crashed_daemon_start_ticks "$crashed_daemon_start_ticks" \
    --argjson crashed_actor_pid "$crashed_actor_pid" \
    --arg crashed_actor_start_ticks "$crashed_actor_start_ticks" \
    --argjson crashed_generation "$crashed_generation" \
    --argjson recovered_daemon_pid "$m5_daemon_pid" \
    --arg recovered_daemon_start_ticks "$m5_daemon_start_ticks" \
    --argjson recovered_generation "$recovered_generation" '
    {
      schema_version:1,
      kind:"m7_accepted_zec_process_kill_recovery",
      result:"passed",
      swap_id:$swap,
      crash_boundary:"zcash_fund_submitted_before_actor_stdout",
      kill_order:"daemon_then_actor",
      exact_funding_transaction_id:$tx,
      pause_marker:$marker[0],
      crashed:{
        daemon:{pid:$crashed_daemon_pid,start_ticks:$crashed_daemon_start_ticks},
        actor:{pid:$crashed_actor_pid,start_ticks:$crashed_actor_start_ticks},
        lease_generation:$crashed_generation,
        scheduler:$before_scheduler[0][0]
      },
      recovered:{
        daemon:{pid:$recovered_daemon_pid,start_ticks:$recovered_daemon_start_ticks},
        lease_generation:$recovered_generation,
        scheduler:$after_scheduler[0][0]
      },
      chain_before:{tip:$before_tip[0],mempool:$before_mempool[0]},
      chain_after_restart:{tip:$after_tip[0],mempool:$after_mempool[0]},
      confirmations_mined_before_restart:0,
      mempool_identity_preserved:($before_mempool[0].result == [$tx]
        and $after_mempool[0].result == [$tx]),
      tip_unchanged:($before_tip[0].result == $after_tip[0].result),
      abandoned_generation_transferred:($recovered_generation > $crashed_generation),
      old_process_identities_absent:true,
      automatic_resubmission_observed:false,
      production_binary_exposes_crash_hook:false
    }
  ' >"${evidence_dir}/m7-zec-accepted-process-kill.json"
  chmod 0600 "${evidence_dir}/m7-zec-accepted-process-kill.json"
  m7_zec_process_kill_recovered=1
}

observe_m5_supervised_maker() {
  local round="$1"
  local status_file="${evidence_dir}/m5-maker-supervisor-status-current.json"
  local scheduler_file="${evidence_dir}/m5-maker-supervisor-scheduler-current.json"
  local intent_candidate="${evidence_dir}/m5-maker-lock-intent-candidate.json"
  local mempool_file="${evidence_dir}/m5-zebra-mempool-current.json"
  inject_m7_zec_accepted_process_kill_if_ready || return
  process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin" || {
    echo 'M5 Maker supervisor daemon exited before terminal state' >&2
    return 1
  }
  capture_m5_supervised_maker_status "$status_file" \
    "${evidence_dir}/m5-maker-supervisor-status-current.stderr" \
    "m5-maker-supervisor-round-${round}"
  "$actor_inspector_bin" --database "$m5_application_database" >"$scheduler_file"
  jq -e --arg swap "$(jq -er '.swap_id' "$maker_config")" '
    length == 1 and .[0].swap_id == $swap
    and .[0].actor_kind == "zcash"
    and .[0].schedule_state != "failed"
  ' "$scheduler_file" >/dev/null || {
    echo 'M5 Maker supervisor entered an invalid or failed scheduler state' >&2
    return 1
  }
  m5_maker_phase="$(jq -er \
    'select(.role == "maker" and .state == "active") | .phase | strings' \
    "$status_file")"
  jq -nc --argjson round "$round" --slurpfile status "$status_file" \
    --slurpfile scheduler "$scheduler_file" '
    {
      schema_version: 1,
      round: $round,
      maker_effect_authority: "daemon_supervisor",
      maker_daemon_alive: true,
      concurrent_direct_maker_effects: false,
      actor_status: $status[0],
      scheduler: $scheduler[0][0]
    }' >>"${evidence_dir}/m5-maker-supervisor-status.ndjson"

  if [[ -z "$m5_expected_funding_txid" ]] && \
    "$m5_intent_inspector_bin" --config "$maker_config" --taker-config "$taker_config" \
      >"$intent_candidate" 2>"${evidence_dir}/m5-maker-lock-intent-candidate.stderr"; then
    jq -e --arg swap "$(jq -er '.swap_id' "$maker_config")" '
      .schema_version == 1 and .swap_id == $swap and .role == "maker"
      and .operation == "zcash_fund" and (.staged_revision | numbers) >= 0
      and (.expected_submission_id_internal_hex | test("^[0-9a-f]{64}$"))
      and (.expected_zebra_txid | test("^[0-9a-f]{64}$"))
      and .actor_pair_validated == true
      and .exact_submission_disclosed == false
    ' "$intent_candidate" >/dev/null
    mv -- "$intent_candidate" "${evidence_dir}/m5-maker-lock-intent.json"
    m5_expected_funding_txid="$(jq -er '.expected_zebra_txid' \
      "${evidence_dir}/m5-maker-lock-intent.json")"
  fi

  if [[ -n "$m5_expected_funding_txid" && "$zcash_fund_mined" == 0 ]]; then
    # The actor can submit while the status/intent snapshots above are being
    # captured. Recheck the exact crash boundary immediately before mining.
    inject_m7_zec_accepted_process_kill_if_ready || return
    rpc "$ZEBRA_RPC_URL" \
      '{"jsonrpc":"2.0","id":1,"method":"getrawmempool","params":[]}' >"$mempool_file"
    if jq -e '.error == null and (.result | arrays) and (.result | length) == 0' \
      "$mempool_file" >/dev/null; then
      return 0
    fi
    jq -e --arg txid "$m5_expected_funding_txid" '
      .error == null and (.result | arrays)
      and (.result | length) == 1 and .result[0] == $txid
    ' "$mempool_file" >/dev/null || {
      echo 'M5 isolated mempool contains a transaction other than the exact durable funding ID' >&2
      return 1
    }
    install -m 0600 "$mempool_file" \
      "${evidence_dir}/m5-zebra-mempool-exact-funding.json"
    zcash_fund_submitter='maker'
    zcash_fund_mined=2
    if [[ "$M6_ZEC_JOURNEY" == refund ]]; then
      stop_owned_m5_daemon
      m5_daemon_pid=""
      m5_daemon_start_ticks=""
    fi
    mine_blocks funding 2
    cut_over_m5_negotiation_transports
    if [[ "$M6_ZEC_JOURNEY" == refund ]]; then
      reconcile_m6_suppressed_maker_lock
    fi
  fi

  if [[ "$M6_ZEC_JOURNEY" == refund && "$m6_maker_supervisor_restarted" == 1
    && "$zcash_fund_mined" == 2 && "$m6_zcash_refund_mined" == 0 ]]; then
    rpc "$ZEBRA_RPC_URL" \
      '{"jsonrpc":"2.0","id":"m6-zcash-refund","method":"getrawmempool","params":[]}' \
      >"$mempool_file"
    if jq -e '.error == null and .result == []' "$mempool_file" >/dev/null; then
      return 0
    fi
    jq -e --arg funding "$m5_expected_funding_txid" '
      .error == null and (.result | arrays) and (.result | length) == 1
      and (.result[0] | strings | test("^[0-9a-f]{64}$"))
      and .result[0] != $funding
    ' "$mempool_file" >/dev/null || {
      echo 'M6 isolated mempool does not contain the singleton Maker refund' >&2
      return 1
    }
    install -m 0600 "$mempool_file" \
      "${evidence_dir}/m6-zebra-mempool-zcash-refund.json"
    m6_zcash_refund_txid="$(jq -er '.result[0]' "$mempool_file")"
    mine_blocks zcash-refund 1
    prove_m6_zcash_refund_inclusion
    m6_zcash_refund_mined=1
  fi
}

wait_for_m5_supervisor_terminal() {
  local candidate="${evidence_dir}/m5-maker-supervisor-final-candidate.json"
  for _ in {1..500}; do
    remaining_budget_milliseconds 'm5-maker-supervisor-terminal' >/dev/null
    "$actor_inspector_bin" --database "$m5_application_database" >"$candidate"
    if jq -e --arg swap "$(jq -er '.swap_id' "$maker_config")" '
      length == 1 and .[0].swap_id == $swap
      and .[0].schedule_state == "terminal"
      and .[0].lease_generation > 0 and .[0].attempt_count > 0
      and .[0].child_identity_absent == true
    ' "$candidate" >/dev/null; then
      process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin" || {
        echo 'M5 Maker supervisor exited before terminal evidence publication' >&2
        return 1
      }
      mv -- "$candidate" "${evidence_dir}/m5-maker-supervisor-final.json"
      return 0
    fi
    process_is_owned "$m5_daemon_pid" "$m5_daemon_start_ticks" "$m5_daemon_bin" || {
      echo 'M5 Maker supervisor daemon exited before terminal scheduler state' >&2
      return 1
    }
    sleep 0.05
  done
  echo 'M5 Maker supervisor did not reach a fenced terminal scheduler state' >&2
  return 1
}

handle_zcash_submission() {
  local role="$1"
  local output="$2"
  if [[ "$M6_TAKER_SERVICE_MODE" == 1 ]] && jq -e '
    .m6_first_claim == true and .schema_version == 1 and .action == "claim"
    and .was_replay == false
  ' <<<"$output" >/dev/null; then
    (( lez_revealing_claim_seen == 1 )) || {
      echo 'refusing an M6 Zcash claim before the LEZ revealing claim' >&2
      return 1
    }
    [[ "$role" == "$expected_zcash_claimant_role" && "$role" == taker ]] || {
      echo "unexpected M6 Zcash claimant: ${role}" >&2
      return 1
    }
    (( zcash_claim_mined == 0 )) || {
      echo 'M6 Zcash follow-up claim was submitted more than once' >&2
      return 1
    }
    [[ "$m6_zcash_claim_txid" =~ ^[0-9a-f]{64}$ ]] || return 1
    zcash_claim_submitter="$role"
    zcash_claim_mined=1
    mine_blocks followup-claim 1
    return 0
  fi
  if jq -e '.outcome == "submitted" and .operation == "zcash_fund"' \
    <<<"$output" >/dev/null; then
    [[ "$role" == "$expected_zcash_funder_role" ]] || {
      echo "unexpected Zcash funder: expected=${expected_zcash_funder_role}, actual=${role}" >&2
      return 1
    }
    (( zcash_fund_mined == 0 )) || {
      echo 'Zcash funding was submitted more than once' >&2
      return 1
    }
    zcash_fund_submitter="$role"
    zcash_fund_mined=2
    mine_blocks funding 2
    if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
      stop_owned_m5_daemon
      m5_daemon_pid=''
      m5_daemon_start_ticks=''
      [[ ! -e "$m5_maker_socket" && ! -e "$m5_chat_socket" ]] || {
        echo 'M5 Unix transports survived post-lock daemon shutdown' >&2
        return 1
      }
      mv -- "$m5_delivery_directory" "$m5_delivery_offline"
      [[ ! -e "$m5_delivery_directory" && -d "$m5_delivery_offline" ]] || {
        echo 'M5 Delivery transport survived post-lock cutover' >&2
        return 1
      }
      jq -n \
        --arg first_lock_role "$role" \
        --argjson confirmations_mined \
          "$(jq -er '.result | length' "${evidence_dir}/zebra-generate-funding.json")" '
        {
          schema_version: 1,
          result: "passed",
          cutover_after_first_lock: true,
          first_lock: "zcash_funding",
          first_lock_submitter: $first_lock_role,
          confirmations_mined: $confirmations_mined,
          maker_socket_absent: true,
          chat_socket_absent: true,
          delivery_path_absent: true
        }' >"${evidence_dir}/m5-post-lock-cutover.json"
      chmod 0600 "${evidence_dir}/m5-post-lock-cutover.json"
      m5_transport_cutover_complete=1
    fi
  fi
  if jq -e '.outcome == "submitted" and .operation == "zcash_followup_claim"' \
    <<<"$output" >/dev/null; then
    (( lez_revealing_claim_seen == 1 )) || {
      echo 'refusing a Zcash followup claim before the LEZ revealing claim' >&2
      return 1
    }
    [[ "$role" == "$expected_zcash_claimant_role" ]] || {
      echo "unexpected Zcash claimant: expected=${expected_zcash_claimant_role}, actual=${role}" >&2
      return 1
    }
    (( zcash_claim_mined == 0 )) || {
      echo 'Zcash followup claim was submitted more than once' >&2
      return 1
    }
    zcash_claim_submitter="$role"
    zcash_claim_mined=1
    mine_blocks followup-claim 1
  fi
}

handle_lez_revealing_claim() {
  local role="$1"
  local output="$2"
  if [[ "$M5_APPLICATION_MODE" == 1 && "$role" == taker ]] && jq -e \
    '.outcome == "projected" and .operation == "lez_revealing_claim"' \
    <<<"$output" >/dev/null; then
    role='maker'
  elif ! jq -e '.outcome == "submitted" and .operation == "lez_revealing_claim"' \
    <<<"$output" >/dev/null; then
    return 0
  fi
  [[ "$role" == "$expected_zcash_funder_role" ]] || {
    echo "unexpected LEZ claimant: expected=${expected_zcash_funder_role}, actual=${role}" >&2
    return 1
  }
  (( zcash_fund_mined == 2 )) || {
    echo 'refusing a LEZ revealing claim before confirmed Zcash funding' >&2
    return 1
  }
  lez_revealing_claim_seen=1
  lez_revealing_claim_submitter="$role"
}

if [[ "$M6_TAKER_SERVICE_MODE" == 1 ]]; then
  start_m6_taker_service
fi

completed=0
round=0
while true; do
  round=$((round + 1))
  remaining_budget_milliseconds "round-${round}-before" >/dev/null

  taker_output="$(drive_m5_taker "$round")"
  if [[ "$M6_TAKER_SERVICE_MODE" == 1 && "$M6_ZEC_JOURNEY" == refund ]]; then
    apply_m6_refund_parent_handoff "$taker_output"
  fi
  if [[ "$M6_TAKER_SERVICE_MODE" == 1 ]] && \
    jq -e '.m6_first_claim == true' <<<"$taker_output" >/dev/null; then
    m6_claim_admitted=1
    m6_claim_generation="$(jq -er '.m6_claim_generation | numbers' <<<"$taker_output")"
    m6_zcash_claim_txid="$(jq -er '.m6_zcash_claim_txid | strings' <<<"$taker_output")"
  fi
  handle_lez_revealing_claim taker "$taker_output"
  handle_zcash_submission taker "$taker_output"

  if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
    if [[ "$M6_ZEC_JOURNEY" == refund && "$m6_maker_supervisor_suppressed" == 1 ]]; then
      "$actor_bin" --config "$maker_config" status >"${evidence_dir}/m6-maker-suppressed-status.json"
      maker_phase="$(jq -er '.phase | strings' "${evidence_dir}/m6-maker-suppressed-status.json")"
    else
      observe_m5_supervised_maker "$round"
      maker_phase="$m5_maker_phase"
    fi
  else
    maker_output="$(drive_actor maker "$maker_config" "$round")"
    handle_lez_revealing_claim maker "$maker_output"
    handle_zcash_submission maker "$maker_output"
    maker_phase="$(jq -r '.phase' <<<"$maker_output")"
  fi

  if [[ "$M6_TAKER_SERVICE_MODE" == 1 ]]; then
    taker_phase="$(jq -r 'if .state == "completed" then "completed" elif .state == "refunded" then "refunded" else "active" end' <<<"$taker_output")"
  else
    taker_phase="$(jq -r '.phase' <<<"$taker_output")"
  fi
  if [[ ("$M6_ZEC_JOURNEY" == claim && "$maker_phase" == completed && "$taker_phase" == completed)
    || ("$M6_ZEC_JOURNEY" == refund && "$maker_phase" == refunded && "$taker_phase" == refunded) ]]; then
    completed=1
    break
  fi
  remaining_budget_milliseconds "round-${round}-poll" >/dev/null
  sleep "$POLL_INTERVAL_SECONDS"
done

if [[ "$M6_ZEC_JOURNEY" == claim ]]; then
  (( completed == 1 && zcash_fund_mined == 2 && lez_revealing_claim_seen == 1 \
    && zcash_claim_mined == 1 )) || {
    echo "corridor did not complete atomically: completed=${completed}, funding_blocks=${zcash_fund_mined}, lez_reveal=${lez_revealing_claim_seen}, claim_blocks=${zcash_claim_mined}" >&2
    exit 1
  }
else
  (( completed == 1 && zcash_fund_mined == 2 && lez_revealing_claim_seen == 0 \
    && zcash_claim_mined == 0 && m6_refund_admitted == 1 \
    && m6_lez_refund_finalized == 1 && m6_zcash_refund_mined == 1 \
    && m6_maker_supervisor_restarted == 1 )) || {
    echo "refund corridor violated atomic order or authority invariants" >&2
    exit 1
  }
  [[ "$m6_lez_refund_txid" =~ ^[0-9a-f]{64}$ \
    && "$m6_zcash_refund_txid" =~ ^[0-9a-f]{64}$ ]] || exit 1
  jq -e '
    .schema_version == 1 and .result == "passed"
    and .authority == "direct_observation_only"
    and .actor.role == "maker" and .actor.command == "drive"
    and .actor.operation == "maker_lock" and .actor.outcome == "projected"
    and .actor.phase == "both_legs_locked" and .actor.next_action == "claim_lez"
    and (.actor_timeout_seconds | strings | test("^[0-9]+[.][0-9]{3}$"))
    and .before.tip.error == null and (.before.tip.result | numbers) >= 1
    and .after.tip.error == null and (.after.tip.result | numbers) >= 1
    and .before.tip.result == .after.tip.result
    and .before.mempool.error == null and .before.mempool.result == []
    and .after.mempool.error == null and .after.mempool.result == []
    and .before.mempool.result == .after.mempool.result
    and .zebra_tip_unchanged == true
    and .zebra_mempool_unchanged_empty == true
    and .new_chain_effect == false
  ' "${evidence_dir}/m6-maker-lock-reconciliation.json" >/dev/null || {
    echo 'M6 Refund lacks valid no-effect Maker-lock reconciliation evidence' >&2
    exit 1
  }
fi
if [[ "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" == 1 ]]; then
  (( m7_zec_process_kill_injected == 1 && m7_zec_process_kill_recovered == 1 )) || {
    echo 'M7 accepted-ZEC corridor completed without the required process-kill recovery' >&2
    exit 1
  }
  jq -e --arg swap "$m5_swap_id" --arg tx "$m5_expected_funding_txid" '
    .schema_version == 1 and .kind == "m7_accepted_zec_process_kill_recovery"
    and .result == "passed" and .swap_id == $swap
    and .crash_boundary == "zcash_fund_submitted_before_actor_stdout"
    and .kill_order == "daemon_then_actor"
    and .exact_funding_transaction_id == $tx
    and .confirmations_mined_before_restart == 0
    and .mempool_identity_preserved == true and .tip_unchanged == true
    and .abandoned_generation_transferred == true
    and .old_process_identities_absent == true
    and .recovered.lease_generation > .crashed.lease_generation
  ' "${evidence_dir}/m7-zec-accepted-process-kill.json" >/dev/null || {
    echo 'M7 accepted-ZEC process-kill evidence is incomplete' >&2
    exit 1
  }
fi
if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  (( m5_transport_cutover_complete == 1 )) || {
    echo 'M5 corridor completed without the required post-first-lock cutover' >&2
    exit 1
  }
  jq -e --arg expected "$m5_expected_funding_txid" --arg journey "$M6_ZEC_JOURNEY" '
    .result == "passed" and .cutover_after_first_lock == true
    and .first_lock == "zcash_funding" and .confirmations_mined == 2
    and .expected_zebra_txid == $expected
    and .maker_effect_authority == "daemon_supervisor"
    and (if $journey == "claim" then
      .maker_daemon_alive == true and .supervisor_suppressed_for_refund == false
    else
      .maker_daemon_alive == false and .supervisor_suppressed_for_refund == true
      and .supervisor_socket == null
    end)
    and .concurrent_direct_maker_effects == false
    and .maker_socket_absent == true and .chat_socket_absent == true
    and .delivery_path_absent == true' \
    "${evidence_dir}/m5-post-lock-cutover.json" >/dev/null
fi

if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  wait_for_m5_supervisor_terminal
  stop_owned_m5_daemon
  m5_daemon_pid=''
  m5_daemon_start_ticks=''
  [[ ! -e "$m5_supervisor_socket" ]] || {
    echo 'M5 supervisor-only socket survived terminal daemon shutdown' >&2
    exit 1
  }
fi

"$actor_bin" --config "$maker_config" status >"${evidence_dir}/maker-status-final.json"
"$actor_bin" --config "$taker_config" status >"${evidence_dir}/taker-status-final.json"
expected_terminal_phase=completed
[[ "$M6_ZEC_JOURNEY" == refund ]] && expected_terminal_phase=refunded
jq -e --arg phase "$expected_terminal_phase" '
  .role == "maker" and .state == "active" and .phase == $phase
' "${evidence_dir}/maker-status-final.json" >/dev/null
jq -e --arg phase "$expected_terminal_phase" '
  .role == "taker" and .state == "active" and .phase == $phase
' "${evidence_dir}/taker-status-final.json" >/dev/null
if [[ "$M6_TAKER_SERVICE_MODE" == 1 ]]; then
  m6_terminal_request="$(jq -nc --arg swap "$m5_swap_id" '
    {jsonrpc:"2.0",id:"m6-terminal",method:"taker_swap_monitor_v1",params:[{
      schema_version:1,swap_id:$swap}]}
  ')"
  m6_terminal_response="$(m6_service_rpc 'm6-terminal-monitor' "$m6_terminal_request")"
  printf '%s\n' "$m6_terminal_response" >"${evidence_dir}/m6-taker-service-terminal.json"
  m6_terminal_generation="$m6_claim_generation"
  [[ "$M6_ZEC_JOURNEY" == refund ]] && m6_terminal_generation="$m6_refund_generation"
  jq -e --arg swap "$m5_swap_id" --arg journey "$M6_ZEC_JOURNEY" \
    --argjson generation "$m6_terminal_generation" '
    .error == null and .result.schema_version == 1 and .result.swap_id == $swap
    and .result.state == (if $journey == "claim" then "completed" else "refunded" end)
    and .result.progress_generation > $generation
    and .result.available_action == null
    and .result.privacy_guidance ==
      (if $journey == "claim" then "shield_received_transparent_zec_separately" else null end)
  ' <<<"$m6_terminal_response" >/dev/null
  jq -nc --argjson round "$round" --argjson response "$m6_terminal_response" \
    '{schema_version:1,round:$round,terminal:true,response:$response}' \
    >>"${evidence_dir}/m6-taker-service-monitor.ndjson"
  jq -s -e --arg swap "$m5_swap_id" --arg journey "$M6_ZEC_JOURNEY" '
    length >= 2
    and ([.[] | select(.terminal == true)] | length) == 1
    and (.[-1].terminal == true)
    and .[-1].response.error == null
    and .[-1].response.result.swap_id == $swap
    and .[-1].response.result.state ==
      (if $journey == "claim" then "completed" else "refunded" end)
    and ([.[] | select(.terminal != true)] | length) >= 1
  ' "${evidence_dir}/m6-taker-service-monitor.ndjson" >/dev/null
  if [[ "$M6_ZEC_JOURNEY" == claim ]]; then
    (( m6_claim_admitted == 1 ))
    jq -s -e --arg swap "$m5_swap_id" \
    --arg txid "$m6_zcash_claim_txid" --argjson generation "$m6_claim_generation" '
    length >= 1 and .[0].first.error == null and .[0].replay.error == null
    and .[0].first.result == {schema_version:1,swap_id:$swap,action:"claim",
      requested_after_generation:$generation,was_replay:false}
    and .[0].replay.result == {schema_version:1,swap_id:$swap,action:"claim",
      requested_after_generation:$generation,was_replay:true}
    and .[0].mempool_before.result == []
    and .[0].mempool_after_first.result == [$txid]
    and .[0].mempool_after_replay.result == [$txid]
  ' "${evidence_dir}/m6-taker-service-claim.ndjson" >/dev/null
  else
    (( m6_refund_admitted == 1 && m6_lez_refund_finalized == 1 \
      && m6_zcash_refund_mined == 1 && m6_maker_supervisor_restarted == 1 ))
    prove_m6_terminal_refund_replay
    jq -s -e '
      all(.[];
        .schema_version == 1
        and (
          if .phase == "reconcile" then
            (.round | type) == "number" and .round >= 1
            and .round == (.round | floor)
            and .response == {
              jsonrpc:"2.0",id:"m6-refund-replay",
              error:{code:-32010,message:"Taker dependency unavailable",
                data:{category:"taker_action_execution_unavailable"}}
            }
          else
            .phase == null and (.attempt | type) == "number" and .attempt >= 1
            and .attempt == (.attempt | floor)
            and .response == {
              jsonrpc:"2.0",id:"m6-refund",
              error:{code:-32010,message:"Taker dependency unavailable",
                data:{category:"taker_action_execution_unavailable"}}
            }
          end
        )
      )
    ' "${evidence_dir}/m6-taker-service-refund-transients.ndjson" >/dev/null
    jq -e --arg swap "$m5_swap_id" --argjson generation "$m6_refund_generation" '
      if .error == null then
        .result.schema_version == 1 and .result.swap_id == $swap
        and .result.action == "refund"
        and .result.requested_after_generation == $generation
        and (.result.was_replay | type) == "boolean"
      else
        .error.code == -32010 and .error.message == "Taker dependency unavailable"
        and .error.data.category == "taker_action_execution_unavailable"
      end
    ' "${evidence_dir}/m6-taker-service-refund-first.json" >/dev/null
    jq -e --arg swap "$m5_swap_id" --argjson generation "$m6_refund_generation" '
      .error == null and .result.schema_version == 1 and .result.swap_id == $swap
      and .result.action == "refund"
      and .result.requested_after_generation == $generation
      and (.result.was_replay | type) == "boolean"
    ' "${evidence_dir}/m6-taker-service-refund-commit.json" >/dev/null
    jq -e --arg swap "$m5_swap_id" --argjson generation "$m6_refund_generation" '
      .error == null and .result == {schema_version:1,swap_id:$swap,action:"refund",
        requested_after_generation:$generation,was_replay:true}
    ' "${evidence_dir}/m6-taker-service-refund-replay.json" >/dev/null
    jq -e '
      .error.code == -32017 and .error.message == "Taker action conflict"
      and .error.data.category == "taker_action_conflict"
    ' "${evidence_dir}/m6-taker-service-refund-claim-exclusion.json" >/dev/null
    jq -e --arg tx "$m6_lez_refund_txid" '
      .schema_version == 1 and .transaction_id == $tx and .occurrences == 1
      and .bedrock_status == "Finalized" and .transaction_hash_revalidated == true
    ' "${evidence_dir}/m6-taker-lez-refund-finality.json" >/dev/null
    jq -e --arg swap "$m5_swap_id" '
      .schema_version == 1 and .first.swap_id == $swap and .first.action == "refund"
      and .first.was_replay == false and .replay.swap_id == $swap
      and .replay.action == "refund" and .replay.was_replay == true
      and .maker_effect_authority == "daemon_supervisor"
      and .queued_before_supervisor_restart == true
    ' "${evidence_dir}/m6-refund-maker-manual-action.json" >/dev/null
    jq -e --arg tx "$m6_zcash_refund_txid" '
      .error == null and .result == [$tx]
    ' "${evidence_dir}/m6-zebra-mempool-zcash-refund.json" >/dev/null
    jq -e --arg tx "$m6_zcash_refund_txid" --arg hash "$m6_zcash_refund_block_hash" --argjson height "$m6_zcash_refund_block_height" '
      .schema_version == 1 and .transaction_id == $tx
      and .block_hash == $hash and .height == $height and .occurrences == 1
      and .canonical_hash_response.result == $hash
      and ([.block_response.result.tx[] | select(. == $tx)] | length) == 1
    ' "${evidence_dir}/m6-zebra-zcash-refund-inclusion.json" >/dev/null
    jq -e '
      .schema_version == 1 and .terminal_replay_was_exact == true
      and .ordered_lez_submission_trace_unchanged == true
      and .zebra_tip_unchanged == true
      and .zebra_mempool_empty_before_and_after == true
      and .canonical_lez_refund_revalidated == true
      and .canonical_zcash_refund_revalidated == true
    ' "${evidence_dir}/m6-taker-service-refund-terminal-no-effect.json" >/dev/null
  fi
  stop_owned_process "$m6_service_pid" "$m6_service_start_ticks" "$m6_service_bin"
  m6_service_pid=''
  m6_service_start_ticks=''
  [[ ! -e "$m6_service_socket" ]] || {
    echo 'M6 Taker service socket survived exact shutdown' >&2
    exit 1
  }
fi
if [[ "$M5_APPLICATION_MODE" == 1 && "$M6_TAKER_SERVICE_MODE" == 0 ]]; then
  assert_m5_taker_receipt_unchanged
  terminal_taker_timeout="$(bounded_actor_timeout 'm5-taker-terminal-monitor')"
  timeout --signal=KILL "${terminal_taker_timeout}s" \
    "$taker_bin" monitor --receipt "$m5_taker_acceptance_receipt" \
    >"${evidence_dir}/m5-taker-receipt-terminal.json" \
    2>"${evidence_dir}/m5-taker-receipt-terminal.stderr"
  assert_m5_taker_receipt_unchanged
  remaining_budget_milliseconds 'm5-taker-terminal-monitor-after' >/dev/null
  jq -e '
    .schema_version == 1 and .role == "taker" and .state == "active"
    and .phase == "completed" and (.revision | numbers) > 0
    and .next_action == "complete"
  ' "${evidence_dir}/m5-taker-receipt-terminal.json" >/dev/null
  jq -nc --argjson round "$round" --arg swap "$m5_swap_id" \
    --arg receipt_sha256 "$m5_taker_acceptance_receipt_sha256" \
    --slurpfile status "${evidence_dir}/m5-taker-receipt-terminal.json" '
    {schema_version:1,round:$round,terminal:true,swap_id:$swap,
      acceptance_receipt_sha256:$receipt_sha256,status:$status[0]}
  ' >>"${evidence_dir}/m5-taker-receipt-monitor.ndjson"
  jq -s -e --arg swap "$m5_swap_id" \
    --arg receipt_sha256 "$m5_taker_acceptance_receipt_sha256" '
    length > 0
    and all(.[]; .schema_version == 1 and .swap_id == $swap
      and .acceptance_receipt_sha256 == $receipt_sha256
      and .status.schema_version == 1 and .status.role == "taker"
      and .status.state == "active")
    and any(.[]; .terminal == true and .status.phase == "completed"
      and .status.next_action == "complete")
  ' "${evidence_dir}/m5-taker-receipt-monitor.ndjson" >/dev/null
  jq -s -e --arg swap "$m5_swap_id" \
    --arg receipt_sha256 "$m5_taker_acceptance_receipt_sha256" '
    length > 0
    and all(.[]; .schema_version == 1 and .swap_id == $swap
      and .acceptance_receipt_sha256 == $receipt_sha256
      and .admission.role == "taker" and .admission.state == "active"
      and .admission.phase == "claim_evidence_available"
      and .admission.next_action == "claim_zcash"
      and .effect.schema_version == 1 and .effect.role == "taker"
      and .effect.command == "claim")
    and ([.[] | select(.effect.outcome == "submitted"
      and .effect.operation == "zcash_followup_claim")] | length) == 1
  ' "${evidence_dir}/m5-taker-receipt-claim.ndjson" >/dev/null
fi
if [[ "$M5_APPLICATION_MODE" == 1 ]]; then
  prove_m5_terminal_operator_projection
  jq -e --arg phase "$expected_terminal_phase" '
    .result == "passed" and .source.role == "maker"
    and .source.revision > 0 and .source.offline_full_history_replay == true
    and .operator_history_phase == $phase
    and .operator_status_phase == $phase
    and .owner_socket_removed_after_query == true
    and .chat_remained_absent == true and .delivery_remained_offline == true
    and .chain_rpc_used_during_import == false
    and .private_material_disclosed == false
  ' "${evidence_dir}/m5-terminal-operator-projection.json" >/dev/null
fi

final_zebra_tip_response="$(rpc "$ZEBRA_RPC_URL" '{"jsonrpc":"2.0","id":1,"method":"getblockcount","params":[]}')"
final_zebra_tip="$(jq -er '.result | numbers' <<<"$final_zebra_tip_response")"
expected_zebra_advance=3
[[ "$M6_ZEC_JOURNEY" == refund ]] && expected_zebra_advance=6
(( final_zebra_tip == zebra_tip + expected_zebra_advance )) || {
  echo "Zebra advanced by an unexpected count: initial=${zebra_tip}, final=${final_zebra_tip}, expected_advance=${expected_zebra_advance}" >&2
  exit 1
}
if [[ "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" == 1 ]]; then
  m7_process_kill_tmp="${evidence_dir}/m7-zec-accepted-process-kill.tmp"
  jq --argjson final_tip "$final_zebra_tip" \
    --slurpfile terminal_scheduler "${evidence_dir}/m5-maker-supervisor-final.json" '
    . + {
      terminal:{
        maker_phase:"completed",
        taker_phase:"completed",
        scheduler:$terminal_scheduler[0][0],
        exact_funding_transaction_stayed_single:true,
        crash_hook_marker_remained_no_clobber:true
      },
      final_zebra_tip:$final_tip
    }
  ' "${evidence_dir}/m7-zec-accepted-process-kill.json" >"$m7_process_kill_tmp"
  chmod 0600 "$m7_process_kill_tmp"
  mv -- "$m7_process_kill_tmp" "${evidence_dir}/m7-zec-accepted-process-kill.json"
fi

jq -n \
  --arg run_id "$run_id" \
  --arg direction "$POC_DIRECTION" \
  --arg journey "$M6_ZEC_JOURNEY" \
  --arg output_root "$private_base" \
  --arg zcash_fund_submitter "$zcash_fund_submitter" \
  --arg lez_revealing_claim_submitter "$lez_revealing_claim_submitter" \
  --arg zcash_claim_submitter "$zcash_claim_submitter" \
  --argjson initial_zebra_tip "$zebra_tip" \
  --argjson final_zebra_tip "$final_zebra_tip" \
  --argjson drive_rounds "$round" \
  --argjson drive_retry_count "$(jq -s 'length' "${evidence_dir}/drive-retries.ndjson")" \
  --argjson m5_application_mode "$M5_APPLICATION_MODE" \
  --argjson m6_taker_service_mode "$M6_TAKER_SERVICE_MODE" \
  --argjson m7_zec_process_kill_mode "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" \
  --arg application_handoff_sha256 "$application_handoff_sha256" \
  --arg application_cutover_sha256 \
    "$(if [[ "$M5_APPLICATION_MODE" == 1 ]]; then sha256sum "${evidence_dir}/m5-post-lock-cutover.json" | cut -d ' ' -f1; fi)" \
  --arg terminal_projection_sha256 \
    "$(if [[ "$M5_APPLICATION_MODE" == 1 ]]; then sha256sum "${evidence_dir}/m5-terminal-operator-projection.json" | cut -d ' ' -f1; fi)" \
  --arg maker_lock_intent_sha256 \
    "$(if [[ "$M5_APPLICATION_MODE" == 1 ]]; then sha256sum "${evidence_dir}/m5-maker-lock-intent.json" | cut -d ' ' -f1; fi)" \
  --arg exact_funding_mempool_sha256 \
    "$(if [[ "$M5_APPLICATION_MODE" == 1 ]]; then sha256sum "${evidence_dir}/m5-zebra-mempool-exact-funding.json" | cut -d ' ' -f1; fi)" \
  --arg maker_supervisor_trace_sha256 \
    "$(if [[ "$M5_APPLICATION_MODE" == 1 ]]; then sha256sum "${evidence_dir}/m5-maker-supervisor-status.ndjson" | cut -d ' ' -f1; fi)" \
  --arg maker_supervisor_final_sha256 \
    "$(if [[ "$M5_APPLICATION_MODE" == 1 ]]; then sha256sum "${evidence_dir}/m5-maker-supervisor-final.json" | cut -d ' ' -f1; fi)" \
  --arg taker_acceptance_receipt_sha256 \
    "$(if [[ "$M5_APPLICATION_MODE" == 1 ]]; then printf '%s' "$m5_taker_acceptance_receipt_sha256"; fi)" \
  --arg taker_claim_trace_sha256 \
    "$(if [[ "$M5_APPLICATION_MODE" == 1 && "$M6_ZEC_JOURNEY" == claim ]]; then sha256sum "${evidence_dir}/m5-taker-receipt-claim.ndjson" | cut -d ' ' -f1; fi)" \
  --arg taker_monitor_trace_sha256 \
    "$(if [[ "$M6_TAKER_SERVICE_MODE" == 1 ]]; then sha256sum "${evidence_dir}/m6-taker-service-monitor.ndjson" | cut -d ' ' -f1; elif [[ "$M5_APPLICATION_MODE" == 1 ]]; then sha256sum "${evidence_dir}/m5-taker-receipt-monitor.ndjson" | cut -d ' ' -f1; fi)" \
  --arg expected_zebra_funding_txid "$m5_expected_funding_txid" \
  --arg m7_zec_process_kill_sha256 \
    "$(if [[ "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" == 1 ]]; then sha256sum "${evidence_dir}/m7-zec-accepted-process-kill.json" | cut -d ' ' -f1; fi)" \
  --arg lez_escrow_program_id "$ESCROW_PROGRAM_ID" \
  --arg lez_escrow_guest_sha256 "$M5_LEZ_GUEST_SHA256" \
  --arg lez_deployment_receipt_sha256 "$m5_lez_deployment_receipt_sha256" \
  --arg lez_deployment_finality_sha256 "$m5_lez_deployment_finality_sha256" \
  --arg lez_actor_onboarding_sha256 "$m5_lez_actor_onboarding_sha256" \
  --arg lez_maker_vault_claim_transaction_hash "$m5_lez_maker_vault_claim_transaction_hash" \
  --arg lez_taker_vault_claim_transaction_hash "$m5_lez_taker_vault_claim_transaction_hash" \
  --argjson lez_maker_vault_claim_block_id "$m5_lez_maker_vault_claim_block_id" \
  --argjson lez_taker_vault_claim_block_id "$m5_lez_taker_vault_claim_block_id" \
  --arg lez_deployment_transaction_hash "$m5_lez_deployment_transaction_hash" \
  --argjson lez_deployment_inclusion_block_id "$m5_lez_deployment_inclusion_block_id" \
  --arg lez_deployment_inclusion_block_hash "$m5_lez_deployment_inclusion_block_hash" \
  --arg lez_refund_txid "$m6_lez_refund_txid" \
  --arg zcash_refund_txid "$m6_zcash_refund_txid" \
  --arg lez_refund_window_sha256 \
    "$(if [[ "$M6_ZEC_JOURNEY" == refund ]]; then sha256sum "${evidence_dir}/m6-taker-lez-refund-window.json" | cut -d ' ' -f1; fi)" \
  --arg maker_lock_reconciliation_sha256 \
    "$(if [[ "$M6_ZEC_JOURNEY" == refund ]]; then sha256sum "${evidence_dir}/m6-maker-lock-reconciliation.json" | cut -d ' ' -f1; fi)" \
  --arg taker_refund_commit_sha256 \
    "$(if [[ "$M6_ZEC_JOURNEY" == refund ]]; then sha256sum "${evidence_dir}/m6-taker-service-refund-commit.json" | cut -d ' ' -f1; fi)" \
  --arg taker_refund_transients_sha256 \
    "$(if [[ "$M6_ZEC_JOURNEY" == refund ]]; then sha256sum "${evidence_dir}/m6-taker-service-refund-transients.ndjson" | cut -d ' ' -f1; fi)" \
  --arg lez_refund_finality_sha256 \
    "$(if [[ "$M6_ZEC_JOURNEY" == refund ]]; then sha256sum "${evidence_dir}/m6-taker-lez-refund-finality.json" | cut -d ' ' -f1; fi)" \
  --arg maker_refund_action_sha256 \
    "$(if [[ "$M6_ZEC_JOURNEY" == refund ]]; then sha256sum "${evidence_dir}/m6-refund-maker-manual-action.json" | cut -d ' ' -f1; fi)" \
  --arg zcash_refund_mempool_sha256 \
    "$(if [[ "$M6_ZEC_JOURNEY" == refund ]]; then sha256sum "${evidence_dir}/m6-zebra-mempool-zcash-refund.json" | cut -d ' ' -f1; fi)" \
  --arg zcash_refund_inclusion_sha256 \
    "$(if [[ "$M6_ZEC_JOURNEY" == refund ]]; then sha256sum "${evidence_dir}/m6-zebra-zcash-refund-inclusion.json" | cut -d ' ' -f1; fi)" \
  --arg taker_refund_terminal_replay_sha256 \
    "$(if [[ "$M6_ZEC_JOURNEY" == refund ]]; then sha256sum "${evidence_dir}/m6-taker-service-refund-terminal-replay.json" | cut -d ' ' -f1; fi)" \
  --arg taker_refund_terminal_no_effect_sha256 \
    "$(if [[ "$M6_ZEC_JOURNEY" == refund ]]; then sha256sum "${evidence_dir}/m6-taker-service-refund-terminal-no-effect.json" | cut -d ' ' -f1; fi)" \
  --argjson elapsed_ms "$((MAX_CORRIDOR_SECONDS * 1000 - $(remaining_budget_milliseconds 'result-before')))" '
  {
    schema_version: 1,
    run_id: $run_id,
    direction: $direction,
    journey: $journey,
    result: "completed",
    m6_taker_service_mode: ($m6_taker_service_mode == 1),
    maker_status: (if $journey == "claim" then "completed" else "refunded" end),
    taker_status: (if $journey == "claim" then "completed" else "refunded" end),
    zebra_generate_calls:
      (if $journey == "claim" then {
        after_zcash_fund_submitted: 1,
        after_zcash_followup_claim_submitted: 1,
        total: 2
      } else {
        after_zcash_fund_submitted: 1,
        refund_eligibility: 1,
        after_zcash_refund_submitted: 1,
        total: 3
      } end),
    zebra_generate_blocks:
      (if $journey == "claim" then {
        after_zcash_fund_submitted: 2,
        after_zcash_followup_claim_submitted: 1,
        total: 3
      } else {
        after_zcash_fund_submitted: 2,
        refund_eligibility: 3,
        after_zcash_refund_submitted: 1,
        total: 6
      } end),
    zebra_tip: {initial: $initial_zebra_tip, final: $final_zebra_tip},
    effect_owners:
      (if $journey == "claim" then {
        zcash_funder: $zcash_fund_submitter,
        lez_claimant: $lez_revealing_claim_submitter,
        zcash_claimant: $zcash_claim_submitter
      } else {
        zcash_funder: $zcash_fund_submitter,
        lez_refunder: "taker",
        zcash_refunder: "maker"
      } end),
    atomic_order_observed:
      (if $journey == "claim" then [
        "zcash_funded_and_confirmed",
        "lez_revealing_claim_submitted",
        "zcash_followup_claim_submitted_and_confirmed"
      ] else [
        "zcash_funded_and_confirmed",
        "lez_refund_finalized",
        "zcash_refund_submitted_and_confirmed"
      ] end),
    refund_path:
      (if $journey == "refund" then {
        lez_refund_transaction_id: $lez_refund_txid,
        zcash_refund_transaction_id: $zcash_refund_txid,
        lez_refund_window_sha256: $lez_refund_window_sha256,
        maker_lock_reconciliation_sha256: $maker_lock_reconciliation_sha256,
        taker_refund_commit_sha256: $taker_refund_commit_sha256,
        taker_refund_transients_sha256: $taker_refund_transients_sha256,
        lez_refund_finality_sha256: $lez_refund_finality_sha256,
        maker_refund_action_sha256: $maker_refund_action_sha256,
        zcash_refund_mempool_sha256: $zcash_refund_mempool_sha256,
        zcash_refund_inclusion_sha256: $zcash_refund_inclusion_sha256,
        taker_refund_terminal_replay_sha256: $taker_refund_terminal_replay_sha256,
        taker_refund_terminal_no_effect_sha256: $taker_refund_terminal_no_effect_sha256,
        opposite_claim_rejected_after_refund_admission: true,
        exact_terminal_replay_has_no_new_chain_effect: true,
        maker_supervisor_suppressed_until_lez_refund_finality: true,
        maker_supervisor_restarted_after_lez_refund_finality: true,
        deterministic_local_chain_funds: true
      } else null end),
    drive_rounds: $drive_rounds,
    same_run_drive_retries: $drive_retry_count,
    elapsed_milliseconds_from_provisioning: $elapsed_ms,
    application_plane: {
      enabled: ($m5_application_mode == 1),
      handoff_receipt_sha256:
        (if $m5_application_mode == 1 then $application_handoff_sha256 else null end),
      cutover_receipt_sha256:
        (if $m5_application_mode == 1 then $application_cutover_sha256 else null end),
      terminal_projection_receipt_sha256:
        (if $m5_application_mode == 1 then $terminal_projection_sha256 else null end),
      maker_lock_intent_sha256:
        (if $m5_application_mode == 1 then $maker_lock_intent_sha256 else null end),
      exact_funding_mempool_sha256:
        (if $m5_application_mode == 1 then $exact_funding_mempool_sha256 else null end),
      maker_supervisor_trace_sha256:
        (if $m5_application_mode == 1 then $maker_supervisor_trace_sha256 else null end),
      maker_supervisor_final_sha256:
        (if $m5_application_mode == 1 then $maker_supervisor_final_sha256 else null end),
      taker_acceptance_receipt_sha256:
        (if $m5_application_mode == 1 then $taker_acceptance_receipt_sha256 else null end),
      taker_claim_trace_sha256:
        (if $m5_application_mode == 1 and $journey == "claim" then $taker_claim_trace_sha256 else null end),
      taker_monitor_trace_sha256:
        (if $m5_application_mode == 1 then $taker_monitor_trace_sha256 else null end),
      taker_claim_authority:
        (if $journey != "claim" then
           null
         elif $m6_taker_service_mode == 1 then
           "owner_taker_service"
         elif $m5_application_mode == 1 then
           "receipt_bound_cli"
         else null end),
      taker_terminal_action_authority:
        (if $m6_taker_service_mode == 1 then "owner_taker_service" else null end),
      direct_taker_claim_effects:
        (if $m5_application_mode == 1 and $journey == "claim" then false else null end),
      direct_taker_terminal_effects:
        (if $m6_taker_service_mode == 1 then false else null end),
      expected_zebra_funding_txid:
        (if $m5_application_mode == 1 then $expected_zebra_funding_txid else null end),
      maker_effect_authority:
        (if $m5_application_mode == 1 then "daemon_supervisor" else null end),
      maker_daemon_owned_at_terminal_observation: ($m5_application_mode == 1),
      concurrent_direct_maker_effects:
        (if $m5_application_mode == 1 then false else null end),
      fresh_operator_restart_reports_completed:
        ($m5_application_mode == 1 and $journey == "claim"),
      fresh_operator_restart_reports_terminal: ($m5_application_mode == 1),
      transports_removed_after_first_lock: ($m5_application_mode == 1),
      transports_absent_through_terminal_state: ($m5_application_mode == 1),
      accepted_zec_process_kill_recovery:
        (if $m7_zec_process_kill_mode == 1 then {
          enabled:true,
          crash_boundary:"zcash_fund_submitted_before_actor_stdout",
          kill_order:"daemon_then_actor",
          evidence_sha256:$m7_zec_process_kill_sha256,
          same_database:true,
          abandoned_generation_transferred:true,
          terminal_completion:true
        } else null end)
    },
    lez_escrow: {
      program_id:
        (if $m5_application_mode == 1 then $lez_escrow_program_id else null end),
      guest_elf_sha256:
        (if $m5_application_mode == 1 then $lez_escrow_guest_sha256 else null end),
      deployment_receipt_sha256:
        (if $m5_application_mode == 1 then $lez_deployment_receipt_sha256 else null end),
      deployment_finality_sha256:
        (if $m5_application_mode == 1 then $lez_deployment_finality_sha256 else null end),
      actor_onboarding_sha256:
        (if $m5_application_mode == 1 then $lez_actor_onboarding_sha256 else null end),
      maker_vault_claim_transaction_hash:
        (if $m5_application_mode == 1 then $lez_maker_vault_claim_transaction_hash else null end),
      maker_vault_claim_finalized_block_id:
        (if $m5_application_mode == 1 then $lez_maker_vault_claim_block_id else null end),
      taker_vault_claim_transaction_hash:
        (if $m5_application_mode == 1 then $lez_taker_vault_claim_transaction_hash else null end),
      taker_vault_claim_finalized_block_id:
        (if $m5_application_mode == 1 then $lez_taker_vault_claim_block_id else null end),
      deployment_transaction_hash:
        (if $m5_application_mode == 1 then $lez_deployment_transaction_hash else null end),
      deployment_inclusion_block_id:
        (if $m5_application_mode == 1 then $lez_deployment_inclusion_block_id else null end),
      deployment_inclusion_block_hash:
        (if $m5_application_mode == 1 then $lez_deployment_inclusion_block_hash else null end)
    },
    public_rpc_or_faucet_used: false,
    evidence_root: $output_root
  }' >"${evidence_dir}/result.json"

echo "M2 ${POC_DIRECTION} PoC completed: ${evidence_dir}/result.json"
