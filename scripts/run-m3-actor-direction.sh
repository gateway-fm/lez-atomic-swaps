#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly initial_terms_hash="1111111111111111111111111111111111111111111111111111111111111111"
readonly pda_probe_secret_digest="2222222222222222222222222222222222222222222222222222222222222222"

fail() {
  echo "M3 actor direction failed: $*" >&2
  exit 2
}

emit_contract() {
  jq -n '
    {
      schema_version: 1,
      kind: "m3_actor_direction_driver_contract",
      runtime_backend: "repository_owned_actual_node_implementation",
      stage_two_spec_uses_actual_node_facts: true,
      fresh_actor_process_per_command: true,
      separate_role_state_and_signing_journals: true,
      taker_first_effects: true,
      dual_locks_before_scalar_use: true,
      bitcoin_exact_signed_depth: true,
      bitcoin_planned_funding_anchor_exact: true,
      lez_exact_finalized_ancestry: true,
      actor_owned_claim_effects: true,
      secure_sidecar_state_root_required: true,
      single_core_rpc_response_per_call: true,
      anchor_height_uses_allowed_blockchain_info: true,
      prelock_policy_response_retained: true,
      role_allowed_block_and_mempool_observation: true,
      bounded_read_only_observation_retries_never_resubmit: true,
      bounded_pending_observation_retries: true,
      submission_count_query: true,
      owned_process_registry: true,
      pre_lock_presignature_domains: ["bitcoin", "lez"],
      expected_unique_effects: {bitcoin: 2, lez: 3},
      submission_count_semantics: "unique_effects_plus_durable_one_shot_authority",
      commands: ["preflight","effect-plan","prepare-stage-two-spec","run-actor-flow","submission-counts"]
    }'
}

emit_effect_plan() {
  local direction="$1"
  case "$direction" in
    taker_sells_foreign)
      jq -n --arg direction "$direction" '
        {schema_version:1,direction:$direction,
         before_first_effect:["finalize_agreement","prepare_exact_lez_claim",
           "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"],
         public_effect_order:["bitcoin_lock_by_taker","lez_initialize_by_maker",
           "lez_fund_by_maker","dual_lock_gate","lez_claim_by_taker",
           "bitcoin_claim_by_maker"],
         terminal:{maker_revision:4,taker_revision:4}}
      '
      ;;
    taker_sells_lez)
      jq -n --arg direction "$direction" '
        {schema_version:1,direction:$direction,
         before_first_effect:["finalize_agreement","prepare_exact_lez_claim",
           "bitcoin_presignature_verified","lez_presignature_verified","activate_both_roles"],
         public_effect_order:["lez_initialize_by_taker","lez_fund_by_taker",
           "bitcoin_lock_by_maker","dual_lock_gate","bitcoin_claim_by_taker",
           "lez_claim_by_maker"],
         terminal:{maker_revision:4,taker_revision:4}}
      '
      ;;
    *) fail "unsupported effect-plan direction" ;;
  esac
}

preflight() {
  local command_name binary
  for command_name in awk chmod cmp cp curl date docker jq kill mkdir mv openssl perl printf \
    readlink rg sed sha256sum sleep stat tr xxd; do
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
}

require_environment() {
  local variable value
  local -a required=(
    M3_POC_RUN_ID M3_POC_DIRECTION M3_POC_DIRECTION_ROOT M3_POC_SECURE_STATE_ROOT
    M3_POC_EVIDENCE_DIR
    M3_POC_PROCESS_REGISTRY M3_POC_ACTOR_BIN M3_POC_PROVISIONER_BIN
    M3_POC_ROLE_RUNNER_BIN M3_POC_LEZ_SIDECAR_BIN M3_POC_LEZ_OPERATOR_BIN
    M3_POC_LEZ_NATIVE_ESCROW_BIN M3_POC_BITCOIN_MANIFEST M3_POC_BITCOIN_RPC_URL
    M3_POC_BITCOIN_MAKER_CURL_CONFIG M3_POC_BITCOIN_TAKER_CURL_CONFIG
    M3_POC_BITCOIN_MAKER_BASIC M3_POC_BITCOIN_TAKER_BASIC
    M3_POC_BITCOIN_FUNDING_CREDENTIALS M3_POC_BITCOIN_CONTAINER_ID
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
  for variable in M3_POC_DIRECTION_ROOT M3_POC_SECURE_STATE_ROOT M3_POC_EVIDENCE_DIR \
    M3_POC_PROCESS_REGISTRY \
    M3_POC_ACTOR_BIN M3_POC_PROVISIONER_BIN M3_POC_ROLE_RUNNER_BIN \
    M3_POC_LEZ_SIDECAR_BIN M3_POC_LEZ_OPERATOR_BIN M3_POC_LEZ_NATIVE_ESCROW_BIN \
    M3_POC_BITCOIN_MANIFEST M3_POC_BITCOIN_MAKER_CURL_CONFIG \
    M3_POC_BITCOIN_TAKER_CURL_CONFIG M3_POC_BITCOIN_MAKER_BASIC \
    M3_POC_BITCOIN_TAKER_BASIC M3_POC_BITCOIN_FUNDING_CREDENTIALS \
    M3_POC_LEZ_MANIFEST M3_POC_MAKER_LEZ_IDENTITY M3_POC_MAKER_LEZ_PRIVATE_KEY \
    M3_POC_TAKER_LEZ_IDENTITY M3_POC_TAKER_LEZ_PRIVATE_KEY; do
    value="${!variable}"
    [[ "$value" == /* ]] || fail "path environment must be absolute: ${variable}"
  done
  [[ "$M3_POC_SECURE_STATE_ROOT" == \
     "/tmp/lez-atomic-swaps-m3-${M3_POC_RUN_ID}-secure-state/directions/${M3_POC_DIRECTION}" ]] ||
    fail "secure state root is not the exact run-owned direction root"
  for endpoint in "$M3_POC_BITCOIN_RPC_URL" "$M3_POC_LEZ_SEQUENCER_RPC_URL" \
    "$M3_POC_LEZ_INDEXER_RPC_URL"; do
    [[ "$endpoint" =~ ^http://127\.0\.0\.1:[1-9][0-9]{0,4}/?$ ]] ||
      fail "node endpoints must be explicit literal-loopback HTTP"
  done
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
  "$M3_POC_LEZ_OPERATOR_BIN" "$command" --endpoint "$endpoint" \
    --run-id "$M3_POC_RUN_ID" --sidecar-role "$role" \
    --capability-file "$role_root/capability" --runtime-file "$role_root/runtime.json" \
    --request-file "$request" >"$output"
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
  local depositor claimant amount now refund_seconds refund_at_ms swap_id terms_file
  local source_tx source_vout source_value source_script secret_hex secret_file
  local funding_spec funding_summary funding_hex funder mempool genesis height anchor first_summary
  local funding_source_evidence funding_policy_evidence
  local pda_evidence metadata custody claim_hash earlier later
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
  amount=1000
  now="$(date -u +%s)"
  earlier=$((now + 3600))
  later=$((now + 7200))
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign) refund_seconds="$earlier" ;;
    taker_sells_lez) refund_seconds="$later" ;;
  esac
  refund_at_ms=$((refund_seconds * 1000))
  swap_id="$(openssl rand -hex 32)"
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
  prepare_witnessed_pair planning "$terms_file" "$depositor" "$claimant" planning

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
  claim_hash="$(jq -er '.claim.message_hash' \
    "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-planning-prepared-claim.json")"

  secret_hex="$(file_value "$M3_POC_BITCOIN_FUNDING_CREDENTIALS" BITCOIN_CORE_FUNDING_SECRET_KEY_HEX)"
  secret_file="${M3_POC_DIRECTION_ROOT}/service-funding.key"
  printf '%s' "$secret_hex" | xxd -r -p >"$secret_file"
  chmod 0600 "$secret_file"
  if [[ "$M3_POC_DIRECTION" == "taker_sells_foreign" ]]; then
    source_tx="$(file_value "$M3_POC_BITCOIN_FUNDING_CREDENTIALS" BITCOIN_CORE_FUNDING_TXID)"
    source_vout="$(file_value "$M3_POC_BITCOIN_FUNDING_CREDENTIALS" BITCOIN_CORE_FUNDING_VOUT)"
    source_value="$(file_value "$M3_POC_BITCOIN_FUNDING_CREDENTIALS" BITCOIN_CORE_FUNDING_VALUE_SAT)"
  else
    first_summary="${M3_POC_EVIDENCE_DIR}/taker_sells_foreign-funding-prepared.json"
    source_tx="$(jq -er '.transaction_id' "$first_summary")"
    source_vout="$(jq -er '.change_output_index' "$first_summary")"
    source_value="$(jq -er '.change_value_sat' "$first_summary")"
    source_script="$(jq -er '.input_script_pubkey' "$first_summary")"
  fi
  funding_source_evidence="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-funding-source-gettxout.json"
  [[ ! -e "$funding_source_evidence" && ! -L "$funding_source_evidence" ]] ||
    fail "refusing to overwrite Core funding-source evidence"
  core_rpc maker gettxout "[\"${source_tx}\",${source_vout},true]" \
    >"$funding_source_evidence"
  chmod 0600 "$funding_source_evidence"
  if [[ "$M3_POC_DIRECTION" == "taker_sells_foreign" ]]; then
    source_script="$(jq -ser \
      'select(length == 1) | .[0].result.scriptPubKey.hex' "$funding_source_evidence")"
  fi
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
  anchor=$((height + 1))

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
    --slurpfile authority "$authority_mapping" '
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
       bitcoin_refund_height:$refund_height,earlier_refund_latest_unix_seconds:$earlier,
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
  local actual_run
  actual_run="$(docker inspect --format \
    '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}' \
    "$M3_POC_BITCOIN_CONTAINER_ID")"
  [[ "$actual_run" == "${M3_POC_RUN_ID}-btc" ]] ||
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
submit_bitcoin_lock() {
  local funder peer funding_hex expected response mempool planned_anchor mined_block
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign) funder=taker; peer=maker ;;
    taker_sells_lez) funder=maker; peer=taker ;;
  esac
  funding_hex="$(tr -d '\r\n' <"${M3_POC_DIRECTION_ROOT}/fixture/funding-transaction.hex")"
  expected="$(jq -er '.bitcoin_funding_transaction_id' \
    "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-stage-two.json")"
  core_rpc "$funder" testmempoolaccept "[[\"${funding_hex}\"]]" |
    jq -e '.result[0].allowed == true' >/dev/null ||
    fail "Core policy rejected the exact signed Bitcoin lock"
  response="$(core_rpc "$funder" sendrawtransaction "[\"${funding_hex}\"]")"
  [[ "$(jq -er '.result' <<<"$response")" == "$expected" ]] ||
    fail "Core returned an unexpected Bitcoin lock ID"
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

final_terms=""
final_prepared_escrow=""
final_prepared_claim=""
prepare_final_transcript() {
  local commitment planning_claim_hash final_claim_hash planning_claim_bytes final_claim_bytes
  commitment="$(jq -er '.agreement_commitment' \
    "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-stage-two.json")"
  final_terms="${M3_POC_DIRECTION_ROOT}/final-terms.json"
  jq --arg terms "$commitment" '.terms_hash = $terms' \
    "${M3_POC_DIRECTION_ROOT}/planning-terms.json" >"$final_terms"
  chmod 0600 "$final_terms"
  start_sidecars final
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
  planning_claim_hash="$(jq -er '.claim.message_hash' \
    "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-planning-prepared-claim.json")"
  final_claim_hash="$(jq -er '.claim.message_hash' "$final_prepared_claim")"
  planning_claim_bytes="$(jq -er '.claim.exact_message_bytes' \
    "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-planning-prepared-claim.json")"
  final_claim_bytes="$(jq -er '.claim.exact_message_bytes' "$final_prepared_claim")"
  [[ "$planning_claim_hash" == "$final_claim_hash" &&
     "$planning_claim_bytes" == "$final_claim_bytes" ]] ||
    fail "final agreement binding changed the pre-lock official LEZ claim transcript"
  jq -e --arg terms "$commitment" '.terms_hash == $terms' "$final_terms" >/dev/null ||
    fail "final witnessed terms do not use the countersigned agreement commitment"
}

btc_session_id=""
lez_session_id=""
btc_session_file=""
lez_session_file=""
provision_signing_material() {
  local public_spec="${M3_POC_DIRECTION_ROOT}/fixture/public-spec.json"
  local role source destination
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
write_actor_configs() {
  local start_height="$1" max_blocks="$2"
  local role basic endpoint config partial adaptor
  [[ "$start_height" =~ ^[0-9]+$ && "$max_blocks" =~ ^[0-9]+$ ]] ||
    fail "actor LEZ window is not numeric"
  (( max_blocks >= 1 && max_blocks <= 4096 )) || fail "actor LEZ window is out of bounds"
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
    endpoint="$(file_value "${M3_POC_DIRECTION_ROOT}/final-endpoints.env" "$role")"
    config="${M3_POC_DIRECTION_ROOT}/actors/${role}/actor-config.json"
    partial="${config}.partial"
    jq -n --arg role "$role" \
      --arg agreement "${M3_POC_DIRECTION_ROOT}/fixture/agreement.borsh" \
      --arg state "${M3_POC_DIRECTION_ROOT}/actors/${role}/actor-state.sqlite" \
      --argjson accepted "$accepted_at" --arg core "$M3_POC_BITCOIN_RPC_URL" \
      --arg basic "$basic" --arg bridge "$endpoint" \
      --arg capability "${M3_POC_DIRECTION_ROOT}/sidecars/final/${role}/capability" \
      --arg run "$M3_POC_RUN_ID" --argjson start "$start_height" \
      --argjson blocks "$max_blocks" --arg btc_session "$btc_session_id" \
      --arg btc_journal "${M3_POC_DIRECTION_ROOT}/actors/${role}/btc-journal.sqlite" \
      --arg lez_session "$lez_session_id" \
      --arg lez_journal "${M3_POC_DIRECTION_ROOT}/actors/${role}/lez-journal.sqlite" \
      --arg prepared "$final_prepared_claim" --arg adaptor "$adaptor" \
      --slurpfile runtime "${M3_POC_DIRECTION_ROOT}/sidecars/final/${role}/runtime.json" '
      {
        schema_version:2,role:$role,agreement_file:$agreement,state_db:$state,
        accepted_at_unix_seconds:$accepted,
        bitcoin_core:{endpoint:$core,cookie_file:$basic,connectivity:"isolated_local"},
        lez_bridge:{endpoint:$bridge,capability_file:$capability,run_id:$run,
          runtime:$runtime[0],request_timeout_millis:10000,
          discovery_start_height:$start,discovery_max_blocks:$blocks},
        signing:({
          bitcoin:{session_id:$btc_session,journal_db:$btc_journal},
          lez:{session_id:$lez_session,journal_db:$lez_journal},
          prepared_witnessed_claim_result_file:$prepared
        } + (if $role == "taker" then {adaptor_secret_file:$adaptor} else {} end))
      }
    ' >"$partial"
    chmod 0600 "$partial"
    mv "$partial" "$config"
  done
  jq -n --argjson start "$start_height" --argjson blocks "$max_blocks" '
    {schema_version:1,start_height:$start,max_blocks:$blocks}
  ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actor-lez-window-latest.json"
  chmod 0600 "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actor-lez-window-latest.json"
}

actor_last_output=""
actor_invoke() {
  local role="$1" command="$2" label="$3"
  local config="${M3_POC_DIRECTION_ROOT}/actors/${role}/actor-config.json"
  actor_last_output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-${label}-${role}.json"
  [[ ! -e "$actor_last_output" ]] || fail "refusing to overwrite actor evidence: ${label}/${role}"
  "$M3_POC_ACTOR_BIN" --config "$config" "$command" >"$actor_last_output"
  chmod 0600 "$actor_last_output"
}

actor_invoke_observation_retry() {
  local role="$1" expected="$2" chain="$3" label="$4"
  local config="${M3_POC_DIRECTION_ROOT}/actors/${role}/actor-config.json"
  local attempt attempt_output attempt_error error_text
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
lez_funding_tx=""
lez_claim_tx=""
submit_lez_transaction_once() {
  local role="$1" member="$2" label="$3" start_height="$4"
  local request output expected returned
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

lez_lock_window_start=0
lez_lock_window_blocks=0
submit_lez_lock_pair() {
  local depositor initial_start funding_start
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign) depositor=maker ;;
    taker_sells_lez) depositor=taker ;;
  esac
  initial_start="$(finalized_tip)"
  submit_lez_transaction_once "$depositor" initialization lez-initialization "$initial_start"
  lez_initialization_tx="$(jq -er '.initialization.transaction_id' "$final_prepared_escrow")"
  funding_start="$lez_proved_tip"
  submit_lez_transaction_once "$depositor" funding lez-funding "$funding_start"
  lez_funding_tx="$(jq -er '.funding.transaction_id' "$final_prepared_escrow")"
  lez_lock_window_start=$((initial_start + 1))
  lez_lock_window_blocks=$((lez_proved_tip - initial_start))
  (( lez_lock_window_blocks >= 1 && lez_lock_window_blocks <= 4096 )) ||
    fail "finalized LEZ funding window is out of bounds"
  write_actor_configs "$lez_lock_window_start" "$lez_lock_window_blocks"
}

submit_actor_bitcoin_claim() {
  local owner="$1" expected_revision="$2" label="$3"
  local peer mempool
  case "$owner" in maker) peer=taker ;; taker) peer=maker ;; esac
  core_rpc "$owner" getrawmempool '[]' |
    jq -e '.result == []' >/dev/null || fail "Bitcoin claim began with a nonempty mempool"
  actor_invoke "$owner" drive "${label}-submit"
  jq -e --arg role "$owner" --argjson revision "$((expected_revision - 1))" '
    .schema_version == 1 and .role == $role and .command == "drive"
    and .outcome == "awaiting_observation" and .chain == "bitcoin"
    and .revision == $revision
  ' "$actor_last_output" >/dev/null || fail "${owner} did not submit the actor-owned Bitcoin claim"
  mempool="$(core_rpc "$peer" getrawmempool '[]')"
  bitcoin_claim_tx="$(jq -er '.result | select(length == 1) | .[0]' <<<"$mempool")"
  [[ "$bitcoin_claim_tx" =~ ^[0-9a-f]{64}$ ]] ||
    fail "counterparty did not observe one exact actor-owned Bitcoin claim"
  mine_one_core_block
  wait_core_confirmed "$bitcoin_claim_tx" "$peer" "$label"
  project_both_to_revision "$expected_revision" bitcoin "${label}-project"
}

actor_lez_claim_transaction_id() {
  local owner="$1" journal
  journal="${M3_POC_SECURE_STATE_ROOT}/sidecars/final/${owner}/bridge-requests.v1.json"
  [[ -f "$journal" && ! -L "$journal" ]] || fail "${owner} sidecar request journal is unavailable"
  jq -er --arg initialization "$lez_initialization_tx" --arg funding "$lez_funding_tx" '
    [.entries[] |
      select(.method == "lez_bridge.v1.submit_transaction"
             and .outcome.kind == "success") |
      .outcome.value.transaction_id |
      select(. != $initialization and . != $funding)] |
    unique | select(length == 1) | .[0]
  ' "$journal"
}

submit_actor_lez_claim() {
  local owner="$1" expected_revision="$2" label="$3"
  local claim_start claim_window_blocks
  claim_start="$(finalized_tip)"
  actor_invoke "$owner" drive "${label}-submit"
  jq -e --arg role "$owner" --argjson revision "$((expected_revision - 1))" '
    .schema_version == 1 and .role == $role and .command == "drive"
    and .outcome == "awaiting_observation" and .chain == "lez"
    and .revision == $revision
  ' "$actor_last_output" >/dev/null || fail "${owner} did not submit the actor-owned LEZ claim"
  lez_claim_tx="$(actor_lez_claim_transaction_id "$owner")"
  [[ "$lez_claim_tx" =~ ^[0-9a-f]{64}$ ]] ||
    fail "actor-owned LEZ claim ID is invalid"
  prove_lez_finalized_transaction "$label" "$lez_claim_tx" "$claim_start"
  claim_window_blocks=$((lez_proved_tip - claim_start))
  (( claim_window_blocks >= 1 && claim_window_blocks <= 4096 )) ||
    fail "finalized LEZ claim window is out of bounds"
  write_actor_configs "$((claim_start + 1))" "$claim_window_blocks"
  project_both_to_revision "$expected_revision" lez "${label}-project"
}

write_dual_lock_gate() {
  capture_both_statuses 2 dual-lock-status
  jq -n --arg direction "$M3_POC_DIRECTION" --arg bitcoin "$bitcoin_lock_tx" \
    --arg initialization "$lez_initialization_tx" --arg funding "$lez_funding_tx" \
    --argjson window_start "$lez_lock_window_start" \
    --argjson window_blocks "$lez_lock_window_blocks" \
    --arg opened_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" '
    {schema_version:1,direction:$direction,gate:"open",
     actor_revision:{maker:2,taker:2},
     bitcoin:{transaction_id:$bitcoin,confirmation_policy_satisfied:true},
     lez:{initialization_transaction_id:$initialization,funding_transaction_id:$funding,
          finality:"Finalized",discovery_window:{start_height:$window_start,max_blocks:$window_blocks}},
     adaptor_authority_eligible_only_after_this_evidence:true,opened_at:$opened_at}
  ' >"${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-dual-lock-gate.json"
  chmod 0600 "${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-dual-lock-gate.json"
}

write_actual_effect_manifest() {
  local output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actual-effects.json"
  jq -n --arg direction "$M3_POC_DIRECTION" --arg bitcoin_lock "$bitcoin_lock_tx" \
    --arg bitcoin_claim "$bitcoin_claim_tx" --arg lez_initialization "$lez_initialization_tx" \
    --arg lez_funding "$lez_funding_tx" --arg lez_claim "$lez_claim_tx" '
    {schema_version:1,direction:$direction,
     bitcoin_effect_ids:[$bitcoin_lock,$bitcoin_claim],
     lez_effect_ids:[$lez_initialization,$lez_funding,$lez_claim],
     expected_unique_effects:{bitcoin:2,lez:3},
     actor_owned_claims:{bitcoin:$bitcoin_claim,lez:$lez_claim}}
  ' >"$output"
  chmod 0600 "$output"
}

run_actor_flow() {
  local initial_tip
  prepare_final_transcript
  provision_signing_material
  run_signing_ceremony btc "$btc_session_file"
  run_signing_ceremony lez "$lez_session_file"
  accepted_at="$(date -u +%s)"
  initial_tip="$(finalized_tip)"
  write_actor_configs "$initial_tip" 1
  activate_actors

  case "$M3_POC_DIRECTION" in
    taker_sells_foreign)
      submit_bitcoin_lock
      project_both_to_revision 1 bitcoin bitcoin-first-lock
      submit_lez_lock_pair
      project_both_to_revision 2 lez lez-second-lock
      ;;
    taker_sells_lez)
      submit_lez_lock_pair
      project_both_to_revision 1 lez lez-first-lock
      submit_bitcoin_lock
      project_both_to_revision 2 bitcoin bitcoin-second-lock
      ;;
  esac
  write_dual_lock_gate

  case "$M3_POC_DIRECTION" in
    taker_sells_foreign)
      submit_actor_lez_claim taker 3 lez-revealing-claim
      submit_actor_bitcoin_claim maker 4 bitcoin-followup-claim
      ;;
    taker_sells_lez)
      submit_actor_bitcoin_claim taker 3 bitcoin-revealing-claim
      submit_actor_lez_claim maker 4 lez-followup-claim
      ;;
  esac
  capture_both_statuses 4 terminal-status
  write_actual_effect_manifest
}

submission_counts() {
  local manifest="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actual-effects.json"
  local counts="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-actual-submission-counts.json"
  local transaction response role journal matches bitcoin_count=0 lez_count=0
  local -a bitcoin_ids=() lez_ids=()
  [[ -f "$manifest" && ! -L "$manifest" ]] || fail "actual effect manifest is unavailable"
  mapfile -t bitcoin_ids < <(jq -er '.bitcoin_effect_ids[]' "$manifest")
  mapfile -t lez_ids < <(jq -er '.lez_effect_ids[]' "$manifest")
  [[ "${#bitcoin_ids[@]}" == 2 && "${#lez_ids[@]}" == 3 ]] ||
    fail "actual effect manifest has the wrong cardinality"
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
    [[ "$#" == 2 ]] || fail "effect-plan requires one direction"
    command -v jq >/dev/null || fail "jq is required"
    emit_effect_plan "$2"
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
  submission-counts)
    [[ "$#" == 1 ]] || fail "submission-counts accepts no arguments"
    require_environment
    submission_counts
    ;;
  *) fail "expected contract, preflight, effect-plan, prepare-stage-two-spec, run-actor-flow, or submission-counts" ;;
esac
