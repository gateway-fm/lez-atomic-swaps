#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly compose_file="tests/e2e/bitcoin-core/compose.yml"
readonly dockerfile="tests/e2e/bitcoin-core/Dockerfile"
readonly provenance_file="tests/e2e/bitcoin-core/provenance.env"
# shellcheck source=tests/e2e/bitcoin-core/provenance.env
source "$provenance_file"

run_id="${RUN_ID:-local-$(date -u +%Y%m%d%H%M%S)-$$}"
if [[ ! "$run_id" =~ ^[a-z0-9][a-z0-9_-]{7,63}$ ]]; then
  echo "RUN_ID must be 8..64 lowercase letters, numbers, underscores, or hyphens" >&2
  exit 1
fi

keep_running="${BITCOIN_CORE_E2E_KEEP_RUNNING:-0}"
if [[ "$keep_running" != "0" && "$keep_running" != "1" ]]; then
  echo "BITCOIN_CORE_E2E_KEEP_RUNNING must be 0 or 1" >&2
  exit 1
fi
mode="${BITCOIN_CORE_E2E_MODE:-fixture}"
if [[ "$mode" != "fixture" && "$mode" != "service" ]]; then
  echo "BITCOIN_CORE_E2E_MODE must be fixture or service" >&2
  exit 1
fi
require_clean="${BITCOIN_CORE_E2E_REQUIRE_CLEAN:-0}"
if [[ "$require_clean" != "0" && "$require_clean" != "1" ]]; then
  echo "BITCOIN_CORE_E2E_REQUIRE_CLEAN must be 0 or 1" >&2
  exit 1
fi

readonly project="lez-atomic-swaps-bitcoin-core-${run_id}"
readonly image="lez-atomic-swaps-bitcoin-core:${run_id}"
readonly network="${project}_bitcoin_core_private"
readonly volume="${project}_core_data"
readonly sentinel_network="lez-atomic-swaps-core-sentinel-${run_id}"
run_dir="$(pwd)/.e2e/${run_id}/bitcoin-core"
readonly run_dir
readonly cache_dir="${run_dir}/cache"
readonly build_context="${run_dir}/build-context"
readonly credentials_dir="${run_dir}/credentials"
readonly evidence_dir="${run_dir}/evidence"
readonly logs_dir="${run_dir}/logs"
readonly config_file="${run_dir}/bitcoin.conf"
readonly manifest="${run_dir}/run.env"
readonly provenance_evidence="${evidence_dir}/provenance.json"
readonly runtime_evidence="${evidence_dir}/runtime.json"
readonly cleanup_evidence="${evidence_dir}/cleanup.json"
readonly critical_evidence_manifest="${evidence_dir}/critical-evidence.sha256"
readonly attestation_evidence="${evidence_dir}/attestation.json"
readonly actor_matrix="${evidence_dir}/actor-rpc-matrix.ndjson"
readonly contract_evidence="${evidence_dir}/p2tr-contract.json"
readonly funding_transaction_evidence="${evidence_dir}/p2tr-funding-transaction.json"
readonly cooperative_spend_evidence="${evidence_dir}/p2tr-cooperative-spend.json"
readonly cargo_target_dir="${run_dir}/cargo-target"
readonly genesis_hash="0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206"
readonly funding_secret_key="0000000000000000000000000000000000000000000000000000000000000001"
readonly funding_key="79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
readonly funding_descriptor="rawtr(${funding_key})#xsjqcczm"
readonly funding_address="bcrt1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqc8gma6"
readonly mocktime_base=1700000000
readonly actor_allowlist="getblockchaininfo,getnetworkinfo,getblockhash,getblock,getblockheader,getrawtransaction,gettxout,gettxspendingprevout,getindexinfo,getmempoolinfo,getrawmempool,getmempoolentry,testmempoolaccept,sendrawtransaction"

required_commands=(cargo chmod curl date docker git gpg jq mkdir mv python3 rg rm seq sha256sum sleep stat tar tr)
for command_name in "${required_commands[@]}"; do
  command -v "$command_name" >/dev/null || {
    echo "missing Bitcoin Core E2E tool: ${command_name}" >&2
    exit 1
  }
done
docker info >/dev/null
if [[ "$require_clean" == "1" ]] &&
   [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  echo "Bitcoin Core certification requires a clean repository worktree" >&2
  exit 1
fi

if [[ -e "$run_dir" || -L "$run_dir" ]]; then
  echo "refusing to reuse Bitcoin Core E2E run state: ${run_dir}" >&2
  exit 1
fi
if docker image inspect "$image" >/dev/null 2>&1; then
  echo "refusing to reuse Bitcoin Core E2E image: ${image}" >&2
  exit 1
fi
for resource in "$network" "$sentinel_network"; do
  if docker network inspect "$resource" >/dev/null 2>&1; then
    echo "refusing to reuse Bitcoin Core E2E network: ${resource}" >&2
    exit 1
  fi
done
if docker volume inspect "$volume" >/dev/null 2>&1; then
  echo "refusing to reuse Bitcoin Core E2E volume: ${volume}" >&2
  exit 1
fi
if [[ -n "$(docker container ls --all --quiet --filter "label=org.logos-co.atomic-swaps.run=${run_id}")" ]] ||
   [[ -n "$(docker network ls --quiet --filter "label=org.logos-co.atomic-swaps.run=${run_id}")" ]] ||
   [[ -n "$(docker volume ls --quiet --filter "label=org.logos-co.atomic-swaps.run=${run_id}")" ]]; then
  echo "refusing to reuse a Docker resource carrying run label ${run_id}" >&2
  exit 1
fi

mkdir -p "$cache_dir" "$build_context" "$credentials_dir" "$evidence_dir" "$logs_dir"
chmod 0700 "$run_dir" "$cache_dir" "$build_context" "$credentials_dir" "$evidence_dir" "$logs_dir"
: >"$actor_matrix"
chmod 0600 "$actor_matrix"

export RUN_ID="$run_id"
export BITCOIN_CORE_IMAGE="$image"
export BITCOIN_CORE_CONFIG="$config_file"
export BITCOIN_CORE_NETWORK="$network"
readonly -a compose=(docker compose --project-name "$project" --file "$compose_file")

container_id=""
runtime_complete=0
sentinel_created=0

write_cleanup_evidence() {
  local status="$1"
  local sentinel_survived="$2"
  local resources_absent="$3"
  local partial="${cleanup_evidence}.partial"
  jq -n \
    --arg run_id "$run_id" \
    --arg mode "$mode" \
    --arg status "$status" \
    --argjson sentinel_survived "$sentinel_survived" \
    --argjson resources_absent "$resources_absent" \
    '{
      schema_version: 1,
      run_id: $run_id,
      mode: $mode,
      cleanup_status: $status,
      exact_run_resources_absent: $resources_absent,
      foreign_sentinel_survived_exact_cleanup: $sentinel_survived,
      broad_cleanup_used: false
    }' >"$partial"
  chmod 0600 "$partial"
  mv "$partial" "$cleanup_evidence"
}

write_attestation_evidence() {
  if [[ "$runtime_complete" != "1" ]] ||
     [[ ! -f "$runtime_evidence" ]] ||
     [[ ! -f "$cleanup_evidence" ]] ||
     [[ ! -f "$critical_evidence_manifest" ]]; then
    return 0
  fi

  local runtime_sha256
  local cleanup_sha256
  local critical_manifest_sha256
  local partial="${attestation_evidence}.partial"
  runtime_sha256="$(sha256sum "$runtime_evidence")"
  runtime_sha256="${runtime_sha256%% *}"
  cleanup_sha256="$(sha256sum "$cleanup_evidence")"
  cleanup_sha256="${cleanup_sha256%% *}"
  critical_manifest_sha256="$(sha256sum "$critical_evidence_manifest")"
  critical_manifest_sha256="${critical_manifest_sha256%% *}"
  jq -n \
    --arg run_id "$run_id" \
    --arg mode "$mode" \
    --arg runtime_sha256 "$runtime_sha256" \
    --arg cleanup_sha256 "$cleanup_sha256" \
    --arg critical_manifest_sha256 "$critical_manifest_sha256" '
    {
      schema_version: 1,
      result: "passed",
      run_id: $run_id,
      mode: $mode,
      runtime_sha256: $runtime_sha256,
      cleanup_sha256: $cleanup_sha256,
      critical_evidence_manifest_sha256: $critical_manifest_sha256
    }
  ' >"$partial"
  chmod 0600 "$partial"
  mv "$partial" "$attestation_evidence"
}

assert_owned_resources_absent() {
  local failed=0
  local labeled=""
  labeled="$(docker container ls --all --quiet --filter "label=org.logos-co.atomic-swaps.run=${run_id}")"
  [[ -z "$labeled" ]] || failed=1
  docker network inspect "$network" >/dev/null 2>&1 && failed=1
  docker volume inspect "$volume" >/dev/null 2>&1 && failed=1
  docker image inspect "$image" >/dev/null 2>&1 && failed=1
  return "$failed"
}

cleanup() {
  local run_status=$?
  local cleanup_failed=0
  local sentinel_survived=false
  local resources_absent=false
  local final_status

  trap - EXIT
  set +e
  if [[ -n "$container_id" ]]; then
    docker logs "$container_id" >"${logs_dir}/bitcoin-core.log" 2>&1
    chmod 0600 "${logs_dir}/bitcoin-core.log"
  fi
  if [[ -d "$cargo_target_dir" ]]; then
    rm -rf -- "$cargo_target_dir" || cleanup_failed=1
  elif [[ -e "$cargo_target_dir" || -L "$cargo_target_dir" ]]; then
    cleanup_failed=1
  fi

  if [[ "$keep_running" == "1" && "$run_status" == "0" && "$runtime_complete" == "1" ]]; then
    if [[ "$sentinel_created" == "1" ]]; then
      docker network rm "$sentinel_network" >/dev/null 2>&1 || cleanup_failed=1
    fi
    echo "Bitcoin Core Regtest remains running for RUN_ID=${run_id}"
    echo "Mode: ${mode}"
    echo "Evidence: ${runtime_evidence}"
    echo "Maker credential file: ${credentials_dir}/maker.curlrc"
    echo "Taker credential file: ${credentials_dir}/taker.curlrc"
    echo "Cleanup container: docker container rm --force ${container_id}"
    echo "Cleanup volume: docker volume rm ${volume}"
    echo "Cleanup network: docker network rm ${network}"
    echo "Cleanup image: docker image rm ${image}"
    final_status="$run_status"
    [[ "$cleanup_failed" == "0" ]] || final_status=1
    exit "$final_status"
  fi

  if [[ -n "$container_id" ]] && docker container inspect "$container_id" >/dev/null 2>&1; then
    docker container rm --force "$container_id" >/dev/null 2>&1 || cleanup_failed=1
  fi
  if docker volume inspect "$volume" >/dev/null 2>&1; then
    docker volume rm "$volume" >/dev/null 2>&1 || cleanup_failed=1
  fi
  if docker network inspect "$network" >/dev/null 2>&1; then
    docker network rm "$network" >/dev/null 2>&1 || cleanup_failed=1
  fi
  if docker image inspect "$image" >/dev/null 2>&1; then
    docker image rm "$image" >/dev/null 2>&1 || cleanup_failed=1
  fi
  if [[ "$sentinel_created" == "1" ]] && docker network inspect "$sentinel_network" >/dev/null 2>&1; then
    sentinel_survived=true
  else
    cleanup_failed=1
  fi
  if assert_owned_resources_absent; then
    resources_absent=true
  else
    cleanup_failed=1
  fi
  if [[ -d "$evidence_dir" ]]; then
    if [[ "$cleanup_failed" == "0" ]]; then
      write_cleanup_evidence passed "$sentinel_survived" "$resources_absent"
      write_attestation_evidence || cleanup_failed=1
    else
      write_cleanup_evidence failed "$sentinel_survived" "$resources_absent"
    fi
  fi
  if [[ "$sentinel_created" == "1" ]]; then
    docker network rm "$sentinel_network" >/dev/null 2>&1 || cleanup_failed=1
  fi

  final_status="$run_status"
  if [[ "$run_status" == "0" && "$cleanup_failed" != "0" ]]; then
    final_status=1
  fi
  exit "$final_status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

write_core_config() {
  local allowed_cidr="$1"
  local maker_auth="$2"
  local taker_auth="$3"
  local partial="${config_file}.partial"
  {
    printf '%s\n' \
      'regtest=1' \
      'server=1' \
      'daemon=0' \
      'nosettings=1' \
      'assumevalid=0' \
      'txindex=1' \
      'txospenderindex=1' \
      'persistmempool=0' \
      'listen=0' \
      'listenonion=0' \
      'discover=0' \
      'dns=0' \
      'dnsseed=0' \
      'fixedseeds=0' \
      'noconnect=1' \
      'natpmp=0' \
      'networkactive=0' \
      'maxconnections=0' \
      'rest=0' \
      'acceptnonstdtxn=0' \
      'minrelaytxfee=0.00000100' \
      'incrementalrelayfee=0.00000100' \
      'blockmintxfee=0.00000001' \
      'fallbackfee=0' \
      'walletbroadcast=0' \
      'rpccookiefile=/var/lib/bitcoin/.cookie' \
      'rpccookieperms=owner' \
      'rpcwhitelistdefault=0' \
      "rpcauth=${maker_auth}" \
      "rpcauth=${taker_auth}" \
      "rpcwhitelist=maker:${actor_allowlist}" \
      "rpcwhitelist=taker:${actor_allowlist}" \
      'printtoconsole=1' \
      '[regtest]' \
      'rpcport=18443' \
      'rpcbind=0.0.0.0:18443' \
      "rpcallowip=${allowed_cidr}"
  } >"$partial"
  chmod 0444 "$partial"
  mv "$partial" "$config_file"
}

create_curl_config() {
  local path="$1"
  local username="$2"
  local password="$3"
  local rpc_url="$4"
  {
    printf 'silent\nshow-error\nconnect-timeout = 2\nmax-time = 10\n'
    printf 'request = "POST"\nheader = "content-type: application/json"\n'
    printf 'url = "%s"\nuser = "%s:%s"\n' "$rpc_url" "$username" "$password"
  } >"$path"
  chmod 0600 "$path"
}

create_basic_credentials() {
  local path="$1"
  local username="$2"
  local password="$3"
  local partial="${path}.partial"
  printf '%s:%s' "$username" "$password" >"$partial"
  chmod 0600 "$partial"
  mv "$partial" "$path"
}

core_cli() {
  docker exec -i "$container_id" /usr/local/bin/bitcoin-cli \
    -conf=/run-config/bitcoin.conf \
    -datadir=/var/lib/bitcoin "$@"
}

actor_request() {
  local role="$1"
  local curl_config="$2"
  local method="$3"
  local params="$4"
  local label="$5"
  local body="${evidence_dir}/${role}-${label}.body"
  local payload

  payload="$(jq -cn --arg method "$method" --argjson params "$params" \
    '{jsonrpc:"2.0", id:1, method:$method, params:$params}')"
  last_http_code="$(curl --config "$curl_config" \
    --data "$payload" --output "$body" --write-out '%{http_code}')"
  chmod 0600 "$body"
  last_rpc_error_code="null"
  if jq -e . "$body" >/dev/null 2>&1; then
    last_rpc_error_code="$(jq -c '.error.code // null' "$body")"
  fi
}

record_actor_result() {
  local role="$1"
  local method="$2"
  local expected_access="$3"
  jq -cn \
    --arg role "$role" \
    --arg method "$method" \
    --arg expected_access "$expected_access" \
    --arg http_code "$last_http_code" \
    --argjson rpc_error_code "$last_rpc_error_code" \
    '{
      role: $role,
      method: $method,
      expected_access: $expected_access,
      http_code: ($http_code | tonumber),
      rpc_error_code: $rpc_error_code
    }' >>"$actor_matrix"
}

expect_allowed_success() {
  local role="$1"
  local curl_config="$2"
  local method="$3"
  local params="$4"
  local label="$5"
  actor_request "$role" "$curl_config" "$method" "$params" "$label"
  if [[ "$last_http_code" != "200" ]] ||
     ! jq -e '.error == null and .result != null' "${evidence_dir}/${role}-${label}.body" >/dev/null; then
    echo "${role} did not receive allowed successful RPC ${method}" >&2
    exit 1
  fi
  record_actor_result "$role" "$method" allowed
}

expect_allowed_method_error() {
  local role="$1"
  local curl_config="$2"
  local method="$3"
  local params="$4"
  local label="$5"
  actor_request "$role" "$curl_config" "$method" "$params" "$label"
  if [[ "$last_http_code" == "401" || "$last_http_code" == "403" ]] ||
     [[ "$last_rpc_error_code" == "-32601" || "$last_rpc_error_code" == "null" ]]; then
    echo "${role} did not reach allowed RPC method ${method}" >&2
    exit 1
  fi
  record_actor_result "$role" "$method" allowed_method_error
}

expect_allowed_null() {
  local role="$1"
  local curl_config="$2"
  local method="$3"
  local params="$4"
  local label="$5"
  actor_request "$role" "$curl_config" "$method" "$params" "$label"
  if [[ "$last_http_code" != "200" ]] ||
     ! jq -e '.error == null and .result == null' \
       "${evidence_dir}/${role}-${label}.body" >/dev/null; then
    echo "${role} did not receive allowed null result from RPC ${method}" >&2
    exit 1
  fi
  record_actor_result "$role" "$method" allowed_null
}

expect_denied() {
  local role="$1"
  local curl_config="$2"
  local method="$3"
  local label="$4"
  actor_request "$role" "$curl_config" "$method" '[]' "$label"
  if [[ "$last_http_code" != "403" ]]; then
    echo "${role} unexpectedly reached forbidden RPC ${method}: HTTP ${last_http_code}" >&2
    exit 1
  fi
  record_actor_result "$role" "$method" denied
}

expect_auth_rejected() {
  local role="$1"
  local curl_config="$2"
  local label="$3"
  actor_request "$role" "$curl_config" getblockchaininfo '[]' "$label"
  if [[ "$last_http_code" != "401" ]]; then
    echo "${role} mismatched credentials were not rejected" >&2
    exit 1
  fi
  record_actor_result "$role" getblockchaininfo auth_rejected
}

finish_service_mode() {
  local funding_credentials="${credentials_dir}/funding.env"
  local repository_commit
  local repository_worktree_clean
  local config_sha256
  local provenance_sha256
  local block_policy_sha256
  local critical_manifest_sha256
  local actor_matrix_json
  local completed_at
  local runtime_partial="${runtime_evidence}.partial"

  core_cli getblockchaininfo >"${evidence_dir}/final-chain.json"
  core_cli getnetworkinfo >"${evidence_dir}/final-network.json"
  core_cli getindexinfo >"${evidence_dir}/final-indexes.json"
  core_cli getmempoolinfo >"${evidence_dir}/final-mempool.json"
  core_cli getblockheader "$maturity_block_hash" >"${evidence_dir}/final-header.json"
  chmod 0600 "${evidence_dir}/final-chain.json" "${evidence_dir}/final-network.json" \
    "${evidence_dir}/final-indexes.json" "${evidence_dir}/final-mempool.json" \
    "${evidence_dir}/final-header.json"
  jq -e '.chain == "regtest" and .blocks == 101 and .headers == 101' \
    "${evidence_dir}/final-chain.json" >/dev/null
  jq -e '.networkactive == false and .connections == 0' \
    "${evidence_dir}/final-network.json" >/dev/null
  jq -e '.txindex.synced == true and .txospenderindex.synced == true' \
    "${evidence_dir}/final-indexes.json" >/dev/null
  jq -e '.size == 0 and .unbroadcastcount == 0' \
    "${evidence_dir}/final-mempool.json" >/dev/null
  jq -e --arg hash "$maturity_block_hash" --argjson expected "$((mocktime_base + 100 * 600))" '
    .hash == $hash and .height == 101 and .time == $expected and .confirmations == 1
  ' "${evidence_dir}/final-header.json" >/dev/null
  core_cli setmocktime 0

  {
    printf 'BITCOIN_CORE_NETWORK=regtest\n'
    printf 'BITCOIN_CORE_FUNDING_SECRET_KEY_HEX=%s\n' "$funding_secret_key"
    printf 'BITCOIN_CORE_FUNDING_DESCRIPTOR=%s\n' "$funding_descriptor"
    printf 'BITCOIN_CORE_FUNDING_ADDRESS=%s\n' "$funding_address"
    printf 'BITCOIN_CORE_FUNDING_TXID=%s\n' "$coinbase_txid"
    printf 'BITCOIN_CORE_FUNDING_VOUT=%s\n' "$coinbase_vout"
    printf 'BITCOIN_CORE_FUNDING_VALUE_SAT=5000000000\n'
  } >"$funding_credentials"
  chmod 0600 "$funding_credentials"

  sha256sum \
    scripts/run-bitcoin-core-e2e.sh \
    "$config_file" \
    "$provenance_evidence" \
    "$blocks_file" \
    "$actor_matrix" \
    "${evidence_dir}/container-inspect.json" \
    "${evidence_dir}/network-inspect.json" \
    "${evidence_dir}/volume-inspect.json" \
    "${evidence_dir}/image-inspect.json" \
    "${evidence_dir}/maturity-chain.json" \
    "${evidence_dir}/maturity-network.json" \
    "${evidence_dir}/maturity-indexes.json" \
    "${evidence_dir}/mature-funding.json" \
    "${evidence_dir}/final-chain.json" \
    "${evidence_dir}/final-network.json" \
    "${evidence_dir}/final-indexes.json" \
    "${evidence_dir}/final-mempool.json" \
    "${evidence_dir}/final-header.json" \
    "$funding_credentials" \
    >"$critical_evidence_manifest"
  chmod 0600 "$critical_evidence_manifest"

  repository_commit="$(git rev-parse HEAD)"
  if [[ -z "$(git status --porcelain --untracked-files=normal)" ]]; then
    repository_worktree_clean=true
  else
    repository_worktree_clean=false
  fi
  config_sha256="$(sha256sum "$config_file")"
  config_sha256="${config_sha256%% *}"
  provenance_sha256="$(sha256sum "$provenance_evidence")"
  provenance_sha256="${provenance_sha256%% *}"
  block_policy_sha256="$(sha256sum "$blocks_file")"
  block_policy_sha256="${block_policy_sha256%% *}"
  critical_manifest_sha256="$(sha256sum "$critical_evidence_manifest")"
  critical_manifest_sha256="${critical_manifest_sha256%% *}"
  actor_matrix_json="$(jq -s . "$actor_matrix")"
  completed_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

  jq -n \
    --arg run_id "$run_id" \
    --arg mode "$mode" \
    --arg completed_at "$completed_at" \
    --arg project "$project" \
    --arg container_id "$container_id" \
    --arg network "$network" \
    --arg network_cidr "$network_cidr" \
    --arg volume "$volume" \
    --arg image "$image" \
    --arg image_id "$image_id" \
    --arg rpc_endpoint "$published_endpoint" \
    --arg rpc_url "$rpc_url" \
    --arg maker_credentials "${credentials_dir}/maker.curlrc" \
    --arg taker_credentials "${credentials_dir}/taker.curlrc" \
    --arg maker_basic_credentials "${credentials_dir}/maker.basic" \
    --arg taker_basic_credentials "${credentials_dir}/taker.basic" \
    --arg funding_credentials "$funding_credentials" \
    --arg source_commit "$BITCOIN_CORE_SOURCE_COMMIT" \
    --arg archive_sha256 "$BITCOIN_CORE_ARCHIVE_SHA256" \
    --arg provenance_sha256 "$provenance_sha256" \
    --arg config_sha256 "$config_sha256" \
    --arg genesis "$genesis_hash" \
    --arg descriptor "$funding_descriptor" \
    --arg address "$funding_address" \
    --arg coinbase_txid "$coinbase_txid" \
    --argjson coinbase_vout "$coinbase_vout" \
    --arg block_policy_sha256 "$block_policy_sha256" \
    --argjson actor_matrix "$actor_matrix_json" \
    --arg archive_url "$BITCOIN_CORE_ARCHIVE_URL" \
    --arg source_url "$BITCOIN_CORE_SOURCE_URL" \
    --arg guix_url "$BITCOIN_CORE_GUIX_SIGS_URL" \
    --arg runtime_base "$BITCOIN_CORE_RUNTIME_BASE" \
    --arg repository_commit "$repository_commit" \
    --arg require_clean "$require_clean" \
    --argjson repository_worktree_clean "$repository_worktree_clean" \
    --arg critical_manifest_sha256 "$critical_manifest_sha256" '
    {
      schema_version: 1,
      result: "passed",
      mode: $mode,
      scope: "bitcoin_core_service_provision",
      run_id: $run_id,
      completed_at: $completed_at,
      repository: {
        commit: $repository_commit,
        worktree_clean: $repository_worktree_clean,
        clean_required: ($require_clean == "1"),
        critical_evidence_manifest_sha256: $critical_manifest_sha256
      },
      core: {
        version: "31.1",
        source_commit: $source_commit,
        archive_sha256: $archive_sha256,
        provenance_evidence_sha256: $provenance_sha256,
        image: $image,
        image_id: $image_id,
        fixture_helper_built: false
      },
      isolation: {
        docker_resource_scope: $project,
        lifecycle: "exact_id_native_docker",
        compose_contract_validated: true,
        container_id: $container_id,
        network: $network,
        network_cidr: $network_cidr,
        data_volume: $volume,
        rpc_endpoint: $rpc_endpoint,
        rpc_url: $rpc_url,
        rpc_publication: "dynamic_literal_loopback_only",
        p2p_port_published: false,
        config_sha256: $config_sha256
      },
      chain: {
        network: "regtest",
        genesis: $genesis,
        initial_height: 0,
        final_height: 101,
        peers_before: 0,
        peers_after: 0,
        network_active: false,
        mocktime: {base: 1700000000, spacing_seconds: 600, blocks: 101, reset_after_evidence: true},
        generated_block_policy_sha256: $block_policy_sha256
      },
      provisioned_funding: {
        descriptor: $descriptor,
        address: $address,
        txid: $coinbase_txid,
        vout: $coinbase_vout,
        value_sat: 5000000000,
        confirmations: 101,
        script_type: "witness_v1_taproot",
        deterministic_test_key: true,
        reproducibility: "fixed_test_key_descriptor_and_block_generation_policy",
        credentials_file: $funding_credentials
      },
      service_contract: {
        ready_for_external_actor_processes: true,
        p2tr_fixture_lifecycle_executed: false,
        p2tr_fixture_proof_claimed: false,
        adaptor_signature_proof_claimed: false,
        scalar_extraction_proof_claimed: false,
        lez_composition_proof_claimed: false,
        atomicity_proof_claimed: false
      },
      actor_rpc: {
        users: ["maker", "taker"],
        credentials_distinct: true,
        credentials_mode: "separate_0600_basic_and_curl_files_under_0700_run_root",
        plaintext_credentials_disclosed: false,
        maker_curl_config: $maker_credentials,
        taker_curl_config: $taker_credentials,
        maker_basic_file: $maker_basic_credentials,
        taker_basic_file: $taker_basic_credentials,
        results: $actor_matrix
      },
      external_dependencies: {
        runtime_external_resources: [],
        public_rpc_used: false,
        faucet_used: false,
        public_funds_used: false,
        cold_setup_external_resources: [$archive_url, $source_url, $guix_url, $runtime_base]
      }
    }
  ' >"$runtime_partial"
  jq -e '
    .result == "passed"
    and .mode == "service"
    and .scope == "bitcoin_core_service_provision"
    and .chain.final_height == 101
    and .provisioned_funding.value_sat == 5000000000
    and .provisioned_funding.confirmations == 101
    and .service_contract.ready_for_external_actor_processes == true
    and .service_contract.p2tr_fixture_lifecycle_executed == false
    and .service_contract.p2tr_fixture_proof_claimed == false
    and .service_contract.adaptor_signature_proof_claimed == false
    and .service_contract.scalar_extraction_proof_claimed == false
    and .service_contract.lez_composition_proof_claimed == false
    and .service_contract.atomicity_proof_claimed == false
    and .actor_rpc.credentials_distinct == true
    and .external_dependencies.runtime_external_resources == []
    and .external_dependencies.public_rpc_used == false
  ' "$runtime_partial" >/dev/null
  chmod 0600 "$runtime_partial"
  mv "$runtime_partial" "$runtime_evidence"

  {
    printf 'RUN_ID=%s\n' "$run_id"
    printf 'BITCOIN_CORE_E2E_MODE=service\n'
    printf 'COMPOSE_PROJECT_NAME=%s\n' "$project"
    printf 'BITCOIN_CORE_IMAGE=%s\n' "$image"
    printf 'BITCOIN_CORE_CONFIG=%s\n' "$config_file"
    printf 'BITCOIN_CORE_RPC_URL=%s\n' "$rpc_url"
    printf 'BITCOIN_CORE_MAKER_CURL_CONFIG=%s\n' "${credentials_dir}/maker.curlrc"
    printf 'BITCOIN_CORE_TAKER_CURL_CONFIG=%s\n' "${credentials_dir}/taker.curlrc"
    printf 'BITCOIN_CORE_MAKER_BASIC_CREDENTIALS=%s\n' "${credentials_dir}/maker.basic"
    printf 'BITCOIN_CORE_TAKER_BASIC_CREDENTIALS=%s\n' "${credentials_dir}/taker.basic"
    printf 'BITCOIN_CORE_FUNDING_CREDENTIALS=%s\n' "$funding_credentials"
    printf 'BITCOIN_CORE_RUNTIME_EVIDENCE=%s\n' "$runtime_evidence"
  } >"$manifest"
  chmod 0600 "$manifest"

  unset maker_password taker_password maker_auth taker_auth
  runtime_complete=1
  echo "Bitcoin Core 31.1 isolated service is provisioned for RUN_ID=${run_id}"
  echo "RPC endpoint: ${rpc_url}"
  echo "Maker credential file: ${credentials_dir}/maker.curlrc"
  echo "Taker credential file: ${credentials_dir}/taker.curlrc"
  echo "Maker Basic credential file: ${credentials_dir}/maker.basic"
  echo "Taker Basic credential file: ${credentials_dir}/taker.basic"
  echo "Funding credential file: ${funding_credentials}"
  echo "Evidence: ${runtime_evidence}"
  echo "Run manifest: ${manifest}"
}

if [[ "$mode" == "fixture" ]]; then
  cargo fetch --locked
  CARGO_TARGET_DIR="$cargo_target_dir" cargo run --quiet --locked --offline \
    -p lez-btc-swap-sdk --example btc-core-p2tr-fixture -- contract \
    >"$contract_evidence"
  chmod 0600 "$contract_evidence"
  helper_binary="${cargo_target_dir}/debug/examples/btc-core-p2tr-fixture"
  if [[ ! -f "$helper_binary" || ! -x "$helper_binary" || -L "$helper_binary" ]]; then
    echo "Bitcoin P2TR fixture helper is missing or not a regular executable" >&2
    exit 1
  fi
  helper_sha256="$(sha256sum "$helper_binary")"
  helper_sha256="${helper_sha256%% *}"
  jq -e '
  .schema_version == 1
  and .kind == "p2tr_contract"
  and .fixture_only == true
  and .fixture_authority == "two_party_musig2_adaptor_public_regtest_vector"
  and .signing_protocol == "BIP327_MUSIG2_SCHNORR_ADAPTOR"
  and .musig2_version == "0.4.1"
  and .signer_order == ["maker", "taker"]
  and .maker_public_key == "036930f46dd0b16d866d59d1054aa63298b357499cd1862ef16f3f55f1cafceb82"
  and .taker_public_key == "0324653eac434488002cc06bbfb7f10fe18991e35f9fe4302dbea6d2353dc0ab1c"
  and .adaptor_point == "031428f3a3532ff4f1cac70f7292bfad06d1037f800ee8839b56ebba917a22e900"
  and .network == "regtest"
  and .internal_key == "8f85903b3c8dbc1bae36d0b7974c24a75a8581dd4054013f11214e2431b151cf"
  and .refund_key == "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"
  and .csv_blocks == 144
  and .refund_script == "029000b27520c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5ac"
  and .leaf_version == 192
  and .tapleaf_hash == "49b170c98e175f70e07e8d33734f8a2d8f3763529ce2a67aeca952038925299c"
  and .tapleaf_hash == .merkle_root
  and .tap_tweak_hash == "201e5d59953059209dbfe733c0c88b2788bf831eebdaf00b3678b481368afe5e"
  and .output_key == "e077de917e5cff6c4055f07ef4676f3d0df57dc2ff66036824d917e1937c8a3a"
  and .output_key_parity == "even"
  and .control_block == "c08f85903b3c8dbc1bae36d0b7974c24a75a8581dd4054013f11214e2431b151cf"
  and .script_pubkey == ("5120" + .output_key)
  and .address == "bcrt1pupmaayt7tnlkcsz47pl0gem085xl2lwzlanqx6pymyt7rymu3gaq6psr5y"
' "$contract_evidence" >/dev/null
  contract_address="$(jq -er '.address' "$contract_evidence")"
  contract_script_pubkey="$(jq -er '.script_pubkey' "$contract_evidence")"
  contract_adaptor_point="$(jq -er '.adaptor_point' "$contract_evidence")"
fi

export BITCOIN_CORE_CACHE_DIR="$cache_dir"
export BITCOIN_CORE_PROVENANCE_EVIDENCE="$provenance_evidence"
./scripts/verify-bitcoin-core-release.sh

archive="${cache_dir}/${BITCOIN_CORE_ARCHIVE_NAME}"
tar -xzf "$archive" --strip-components=1 -C "$build_context" \
  "bitcoin-${BITCOIN_CORE_VERSION}/bin/bitcoind" \
  "bitcoin-${BITCOIN_CORE_VERSION}/bin/bitcoin-cli" \
  "bitcoin-${BITCOIN_CORE_VERSION}/share/rpcauth/rpcauth.py"
chmod 0555 "${build_context}/bin/bitcoind" "${build_context}/bin/bitcoin-cli"
chmod 0500 "${build_context}/share/rpcauth/rpcauth.py"

maker_json="$(python3 "${build_context}/share/rpcauth/rpcauth.py" -j maker)"
taker_json="$(python3 "${build_context}/share/rpcauth/rpcauth.py" -j taker)"
maker_password="$(jq -er '.password' <<<"$maker_json")"
taker_password="$(jq -er '.password' <<<"$taker_json")"
maker_auth="$(jq -er '.rpcauth' <<<"$maker_json")"
taker_auth="$(jq -er '.rpcauth' <<<"$taker_json")"
unset maker_json taker_json
if [[ "$maker_password" == "$taker_password" || "$maker_auth" == "$taker_auth" ]]; then
  echo "maker and taker RPC credentials must be distinct" >&2
  exit 1
fi

docker network create \
  --driver bridge \
  --opt com.docker.network.bridge.enable_ip_masquerade=false \
  --label "org.logos-co.atomic-swaps.run=${run_id}" \
  --label 'org.logos-co.atomic-swaps.scope=bitcoin-core-regtest-e2e' \
  --label 'org.logos-co.atomic-swaps.component=bitcoin-core-network' \
  "$network" >/dev/null

network_cidr="$(docker network inspect "$network" --format '{{(index .IPAM.Config 0).Subnet}}')"
if [[ ! "$network_cidr" =~ ^[0-9a-fA-F:.]+/[0-9]+$ ]]; then
  echo "Bitcoin Core run network did not expose one literal CIDR" >&2
  exit 1
fi
write_core_config "$network_cidr" "$maker_auth" "$taker_auth"

docker build \
  --file "$dockerfile" \
  --label "org.logos-co.atomic-swaps.run=${run_id}" \
  --label 'org.logos-co.atomic-swaps.scope=bitcoin-core-regtest-e2e' \
  --label 'org.logos-co.atomic-swaps.component=bitcoin-core-image' \
  --tag "$image" \
  "$build_context"

docker network create \
  --driver bridge \
  --internal \
  --opt com.docker.network.bridge.enable_ip_masquerade=false \
  --label "org.logos-co.atomic-swaps.sentinel-for=${run_id}" \
  "$sentinel_network" >/dev/null
sentinel_created=1

docker volume create \
  --driver local \
  --opt type=tmpfs \
  --opt device=tmpfs \
  --opt o=uid=65532,gid=65532,mode=0700,noexec,nosuid,nodev,size=1073741824 \
  --label "org.logos-co.atomic-swaps.run=${run_id}" \
  --label 'org.logos-co.atomic-swaps.scope=bitcoin-core-regtest-e2e' \
  --label 'org.logos-co.atomic-swaps.component=bitcoin-core-data' \
  "$volume" >/dev/null

"${compose[@]}" config --quiet
# local: raw `docker create --publish` uses docker-proxy mode, which modern
# Docker Desktop engines do not expose to --network host processes; compose
# carries the mode:host port spec from compose.yml, which is. Same name,
# network, volume, and config mount as the raw create it replaces.
"${compose[@]}" create >/dev/null
container_id="$(docker ps -aq --filter "name=${project}-bitcoin_core" | head -1)"
if [[ -z "$container_id" ]]; then
  echo "Docker did not create the run-owned Bitcoin Core container" >&2
  exit 1
fi
docker start "$container_id" >/dev/null
ready=0
for _ in {1..60}; do
  if core_cli getblockchaininfo >"${evidence_dir}/initial-chain.json" 2>/dev/null; then
    ready=1
    break
  fi
  sleep 1
done
if [[ "$ready" != "1" ]]; then
  echo "Bitcoin Core RPC did not become ready within 60 seconds" >&2
  exit 1
fi

published_endpoint="$(docker inspect "$container_id" | jq -er '
  .[0].NetworkSettings.Ports["18443/tcp"] as $bindings
  | if ($bindings | length) == 1
    then ($bindings[0].HostIp + ":" + $bindings[0].HostPort)
    else error("binding count") end
')"
if [[ ! "$published_endpoint" =~ ^127\.0\.0\.1:([0-9]+)$ ]]; then
  echo "Bitcoin Core RPC is not published on one dynamic literal-loopback port" >&2
  exit 1
fi
rpc_url="http://${published_endpoint}"

docker inspect "$container_id" >"${evidence_dir}/container-inspect.json"
docker network inspect "$network" >"${evidence_dir}/network-inspect.json"
docker volume inspect "$volume" >"${evidence_dir}/volume-inspect.json"
docker image inspect "$image" >"${evidence_dir}/image-inspect.json"
chmod 0600 "${evidence_dir}"/*-inspect.json

jq -e --arg network "$network" --arg image "$image" '
  length == 1
  and .[0].Config.Image == $image
  and ((.[0].NetworkSettings.Ports | keys) == ["18443/tcp"])
  and ((.[0].NetworkSettings.Ports["18443/tcp"] | length) == 1)
  and (.[0].NetworkSettings.Ports["18443/tcp"][0].HostIp == "127.0.0.1")
  and (.[0].NetworkSettings.Ports["18443/tcp"][0].HostPort != "")
  and ((.[0].NetworkSettings.Networks | keys) == [$network])
' "${evidence_dir}/container-inspect.json" >/dev/null
jq -e --arg run_id "$run_id" '
  length == 1
  and .[0].Internal == false
  and .[0].Options["com.docker.network.bridge.enable_ip_masquerade"] == "false"
  and .[0].Labels["org.logos-co.atomic-swaps.run"] == $run_id
' "${evidence_dir}/network-inspect.json" >/dev/null
jq -e --arg run_id "$run_id" '
  length == 1
  and .[0].Labels["org.logos-co.atomic-swaps.run"] == $run_id
  and .[0].Options.type == "tmpfs"
' "${evidence_dir}/volume-inspect.json" >/dev/null

image_id="$(docker image inspect "$image" --format '{{.Id}}')"
running_image_id="$(docker inspect "$container_id" --format '{{.Image}}')"
if [[ "$image_id" != "$running_image_id" ]]; then
  echo "running Bitcoin Core container does not use the built immutable image" >&2
  exit 1
fi

create_curl_config "${credentials_dir}/maker.curlrc" maker "$maker_password" "$rpc_url"
create_curl_config "${credentials_dir}/taker.curlrc" taker "$taker_password" "$rpc_url"
create_curl_config "${credentials_dir}/maker-with-taker-secret.curlrc" maker "$taker_password" "$rpc_url"
create_curl_config "${credentials_dir}/taker-with-maker-secret.curlrc" taker "$maker_password" "$rpc_url"
create_basic_credentials "${credentials_dir}/maker.basic" maker "$maker_password"
create_basic_credentials "${credentials_dir}/taker.basic" taker "$taker_password"

core_cli getnetworkinfo >"${evidence_dir}/initial-network.json"
core_cli getblockhash 0 >"${evidence_dir}/genesis.txt"
chmod 0600 "${evidence_dir}/initial-chain.json" "${evidence_dir}/initial-network.json" \
  "${evidence_dir}/genesis.txt"
jq -e '
  .chain == "regtest"
  and .blocks == 0
  and .headers == 0
' "${evidence_dir}/initial-chain.json" >/dev/null
jq -e '
  .version == 310100
  and .subversion == "/Satoshi:31.1.0/"
  and .networkactive == false
  and .connections == 0
' "${evidence_dir}/initial-network.json" >/dev/null
if [[ "$(tr -d '\r\n' <"${evidence_dir}/genesis.txt")" != "$genesis_hash" ]]; then
  echo "Bitcoin Core Regtest genesis mismatch" >&2
  exit 1
fi

expect_allowed_success maker "${credentials_dir}/maker.curlrc" \
  getblockchaininfo '[]' allowed-before-mining
expect_allowed_success taker "${credentials_dir}/taker.curlrc" \
  getblockchaininfo '[]' allowed-before-mining
expect_auth_rejected maker-cross "${credentials_dir}/maker-with-taker-secret.curlrc" \
  cross-secret
expect_auth_rejected taker-cross "${credentials_dir}/taker-with-maker-secret.curlrc" \
  cross-secret

denied_methods=(
  stop
  setmocktime
  generatetoaddress
  generatetodescriptor
  createwallet
  getwalletinfo
  setnetworkactive
  logging
)
for role in maker taker; do
  role_config="${credentials_dir}/${role}.curlrc"
  for method in "${denied_methods[@]}"; do
    expect_denied "$role" "$role_config" "$method" "denied-${method}"
  done
done
if [[ "$(core_cli getblockcount)" != "0" ]]; then
  echo "actor authorization checks unexpectedly changed the chain height" >&2
  exit 1
fi

descriptor_info="$(core_cli getdescriptorinfo "rawtr(${funding_key})")"
if [[ "$(jq -er '.descriptor' <<<"$descriptor_info")" != "$funding_descriptor" ]]; then
  echo "Bitcoin Core canonical rawtr descriptor mismatch" >&2
  exit 1
fi
derived_address="$(core_cli deriveaddresses "$funding_descriptor" | jq -er 'if length == 1 then .[0] else error("address count") end')"
if [[ "$derived_address" != "$funding_address" ]]; then
  echo "Bitcoin Core canonical rawtr Regtest address mismatch" >&2
  exit 1
fi
if [[ "$mode" == "fixture" ]]; then
  contract_descriptor_info="$(core_cli getdescriptorinfo "addr(${contract_address})")"
  contract_descriptor="$(jq -er '.descriptor' <<<"$contract_descriptor_info")"
  derived_contract_address="$(core_cli deriveaddresses "$contract_descriptor" | jq -er 'if length == 1 then .[0] else error("address count") end')"
  if [[ "$derived_contract_address" != "$contract_address" ]]; then
    echo "Bitcoin Core did not derive the exact SDK P2TR contract address" >&2
    exit 1
  fi
fi

blocks_file="${evidence_dir}/generated-blocks.ndjson"
: >"$blocks_file"
for height in $(seq 1 101); do
  mocktime=$((mocktime_base + (height - 1) * 600))
  core_cli setmocktime "$mocktime"
  generated="$(core_cli generatetodescriptor 1 "$funding_descriptor")"
  block_hash="$(jq -er 'if length == 1 then .[0] else error("block count") end' <<<"$generated")"
  jq -cn \
    --argjson height "$height" \
    --argjson mocktime "$mocktime" \
    --arg block_hash "$block_hash" \
    '{height:$height, mocktime:$mocktime, block_hash:$block_hash}' >>"$blocks_file"
done
chmod 0600 "$blocks_file"

core_cli getblockchaininfo >"${evidence_dir}/maturity-chain.json"
core_cli getnetworkinfo >"${evidence_dir}/maturity-network.json"
core_cli getindexinfo >"${evidence_dir}/maturity-indexes.json"
first_block_hash="$(core_cli getblockhash 1)"
maturity_block_hash="$(core_cli getblockhash 101)"
core_cli getblockheader "$first_block_hash" >"${evidence_dir}/first-header.json"
core_cli getblockheader "$maturity_block_hash" >"${evidence_dir}/maturity-header.json"
core_cli getblock "$first_block_hash" 2 >"${evidence_dir}/first-block.json"
chmod 0600 "${evidence_dir}/maturity-chain.json" "${evidence_dir}/maturity-network.json" \
  "${evidence_dir}/maturity-indexes.json" "${evidence_dir}/first-header.json" \
  "${evidence_dir}/maturity-header.json" "${evidence_dir}/first-block.json"

jq -e '.chain == "regtest" and .blocks == 101 and .headers == 101' \
  "${evidence_dir}/maturity-chain.json" >/dev/null
jq -e '.networkactive == false and .connections == 0' \
  "${evidence_dir}/maturity-network.json" >/dev/null
jq -e '.txindex.synced == true and .txospenderindex.synced == true' \
  "${evidence_dir}/maturity-indexes.json" >/dev/null
jq -e --argjson expected "$mocktime_base" '.height == 1 and .time == $expected' \
  "${evidence_dir}/first-header.json" >/dev/null
jq -e --argjson expected "$((mocktime_base + 100 * 600))" \
  '.height == 101 and .time == $expected' "${evidence_dir}/maturity-header.json" >/dev/null

coinbase_txid="$(jq -er '.tx[0].txid' "${evidence_dir}/first-block.json")"
coinbase_vout="$(jq -er --arg address "$funding_address" '
  [.tx[0].vout[] | select(.scriptPubKey.address == $address) | .n]
  | if length == 1 then .[0] else error("funding output") end
' "${evidence_dir}/first-block.json")"
core_cli gettxout "$coinbase_txid" "$coinbase_vout" >"${evidence_dir}/mature-funding.json"
chmod 0600 "${evidence_dir}/mature-funding.json"
jq -e --arg address "$funding_address" '
  .value == 50
  and .confirmations == 101
  and .coinbase == true
  and .scriptPubKey.type == "witness_v1_taproot"
  and .scriptPubKey.address == $address
  and .scriptPubKey.hex == "512079be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
' "${evidence_dir}/mature-funding.json" >/dev/null

for role in maker taker; do
  role_config="${credentials_dir}/${role}.curlrc"
  expect_allowed_success "$role" "$role_config" getblockchaininfo '[]' \
    allowed-getblockchaininfo
  expect_allowed_success "$role" "$role_config" getnetworkinfo '[]' \
    allowed-getnetworkinfo
  expect_allowed_success "$role" "$role_config" getblockhash '[0]' \
    allowed-getblockhash
  expect_allowed_success "$role" "$role_config" getblock \
    "[\"${first_block_hash}\",1]" allowed-getblock
  expect_allowed_success "$role" "$role_config" getblockheader \
    "[\"${genesis_hash}\"]" allowed-getblockheader
  expect_allowed_success "$role" "$role_config" getrawtransaction \
    "[\"${coinbase_txid}\",true,\"${first_block_hash}\"]" allowed-getrawtransaction
  expect_allowed_success "$role" "$role_config" gettxout \
    "[\"${coinbase_txid}\",${coinbase_vout}]" allowed-gettxout
  expect_allowed_success "$role" "$role_config" gettxspendingprevout \
    "[[{\"txid\":\"${coinbase_txid}\",\"vout\":${coinbase_vout}}],{\"mempool_only\":false,\"return_spending_tx\":true}]" \
    allowed-gettxspendingprevout
  jq -e --arg txid "$coinbase_txid" --argjson vout "$coinbase_vout" '
    .result == [{txid:$txid,vout:$vout}]
  ' "${evidence_dir}/${role}-allowed-gettxspendingprevout.body" >/dev/null
  expect_allowed_success "$role" "$role_config" getindexinfo '[]' \
    allowed-getindexinfo
  expect_allowed_success "$role" "$role_config" getmempoolinfo '[]' \
    allowed-getmempoolinfo
  expect_allowed_success "$role" "$role_config" getrawmempool '[]' \
    allowed-getrawmempool
  expect_allowed_method_error "$role" "$role_config" getmempoolentry \
    '["0000000000000000000000000000000000000000000000000000000000000000"]' \
    allowed-getmempoolentry-error
  expect_allowed_method_error "$role" "$role_config" testmempoolaccept \
    '[["00"]]' allowed-testmempoolaccept-error
  expect_allowed_method_error "$role" "$role_config" sendrawtransaction \
    '["00"]' allowed-sendrawtransaction-error
done

if [[ "$mode" == "service" ]]; then
  finish_service_mode
  exit 0
fi

core_cli getmempoolinfo >"${evidence_dir}/pre-p2tr-mempool.json"
chmod 0600 "${evidence_dir}/pre-p2tr-mempool.json"
jq -e '.size == 0 and .unbroadcastcount == 0' \
  "${evidence_dir}/pre-p2tr-mempool.json" >/dev/null

"$helper_binary" fund "$coinbase_txid" "$coinbase_vout" 5000000000 \
  >"$funding_transaction_evidence"
chmod 0600 "$funding_transaction_evidence"
jq -e \
  --arg input_txid "$coinbase_txid" \
  --argjson input_vout "$coinbase_vout" '
  .schema_version == 1
  and .kind == "p2tr_funding_transaction"
  and .fixture_only == true
  and .network == "regtest"
  and .input_txid == $input_txid
  and .input_vout == $input_vout
  and .input_value_sat == 5000000000
  and .contract_vout == 0
  and .contract_value_sat == 100000000
  and .change_vout == 1
  and .change_value_sat == 4899999000
  and .fee_sat == 1000
  and .witness_items == 1
  and .witness_bytes == 64
  and (.sighash | test("^[0-9a-f]{64}$"))
  and (.raw_transaction | test("^[0-9a-f]+$"))
  and (.txid | test("^[0-9a-f]{64}$"))
  and (.wtxid | test("^[0-9a-f]{64}$"))
' "$funding_transaction_evidence" >/dev/null
funding_raw="$(jq -er '.raw_transaction' "$funding_transaction_evidence")"
funding_txid="$(jq -er '.txid' "$funding_transaction_evidence")"
funding_wtxid="$(jq -er '.wtxid' "$funding_transaction_evidence")"
funding_evidence_sha256="$(sha256sum "$funding_transaction_evidence")"
funding_evidence_sha256="${funding_evidence_sha256%% *}"

expect_allowed_success taker "${credentials_dir}/taker.curlrc" testmempoolaccept \
  "[[\"${funding_raw}\"]]" p2tr-funding-policy
jq -e --arg txid "$funding_txid" --arg wtxid "$funding_wtxid" '
  .result | length == 1
  and .[0].allowed == true
  and .[0].txid == $txid
  and .[0].wtxid == $wtxid
  and .[0].fees.base == 0.00001
' "${evidence_dir}/taker-p2tr-funding-policy.body" >/dev/null
expect_allowed_success taker "${credentials_dir}/taker.curlrc" sendrawtransaction \
  "[\"${funding_raw}\"]" p2tr-funding-broadcast
jq -e --arg txid "$funding_txid" '.result == $txid' \
  "${evidence_dir}/taker-p2tr-funding-broadcast.body" >/dev/null
expect_allowed_success maker "${credentials_dir}/maker.curlrc" getmempoolentry \
  "[\"${funding_txid}\"]" p2tr-funding-mempool-observation
jq -e '.result.fees.base == 0.00001 and .result.unbroadcast == true' \
  "${evidence_dir}/maker-p2tr-funding-mempool-observation.body" >/dev/null
expect_allowed_success maker "${credentials_dir}/maker.curlrc" gettxspendingprevout \
  "[[{\"txid\":\"${coinbase_txid}\",\"vout\":${coinbase_vout}}],{\"mempool_only\":false,\"return_spending_tx\":true}]" \
  p2tr-funding-prevout-observation
jq -e --arg txid "$funding_txid" --arg raw "$funding_raw" '
  .result | length == 1
  and .[0].spendingtxid == $txid
  and .[0].spendingtx == $raw
  and (.[0] | has("blockhash") | not)
' "${evidence_dir}/maker-p2tr-funding-prevout-observation.body" >/dev/null
expect_allowed_success maker "${credentials_dir}/maker.curlrc" getmempoolinfo \
  '[]' p2tr-funding-mempool-size
jq -e '.result.size == 1 and .result.unbroadcastcount == 1' \
  "${evidence_dir}/maker-p2tr-funding-mempool-size.body" >/dev/null

funding_block_mocktime=$((mocktime_base + 101 * 600))
core_cli setmocktime "$funding_block_mocktime"
generated="$(core_cli generatetodescriptor 1 "$funding_descriptor")"
funding_block_hash="$(jq -er 'if length == 1 then .[0] else error("block count") end' <<<"$generated")"
jq -cn \
  --argjson height 102 \
  --argjson mocktime "$funding_block_mocktime" \
  --arg block_hash "$funding_block_hash" \
  --arg purpose p2tr_funding_confirmation \
  --arg txid "$funding_txid" \
  '{height:$height, mocktime:$mocktime, block_hash:$block_hash, purpose:$purpose, txid:$txid}' \
  >>"$blocks_file"
core_cli getblock "$funding_block_hash" 2 >"${evidence_dir}/p2tr-funding-block.json"
chmod 0600 "$blocks_file" "${evidence_dir}/p2tr-funding-block.json"
jq -e --arg txid "$funding_txid" --arg raw "$funding_raw" '
  .height == 102
  and (.tx | length) == 2
  and .tx[1].txid == $txid
  and .tx[1].hex == $raw
' "${evidence_dir}/p2tr-funding-block.json" >/dev/null

for role in maker taker; do
  role_config="${credentials_dir}/${role}.curlrc"
  expect_allowed_success "$role" "$role_config" getrawtransaction \
    "[\"${funding_txid}\",true,\"${funding_block_hash}\"]" \
    p2tr-funding-confirmed
  jq -e \
    --arg txid "$funding_txid" \
    --arg wtxid "$funding_wtxid" \
    --arg raw "$funding_raw" \
    --arg blockhash "$funding_block_hash" \
    --arg coinbase_txid "$coinbase_txid" \
    --argjson coinbase_vout "$coinbase_vout" \
    --arg contract_script "$contract_script_pubkey" \
    --arg contract_address "$contract_address" \
    --arg funding_script "5120${funding_key}" \
    --arg funding_address "$funding_address" '
    .result.txid == $txid
    and .result.hash == $wtxid
    and .result.hex == $raw
    and .result.blockhash == $blockhash
    and .result.version == 2
    and .result.locktime == 0
    and .result.confirmations == 1
    and .result.in_active_chain == true
    and (.result.vin | length) == 1
    and .result.vin[0].txid == $coinbase_txid
    and .result.vin[0].vout == $coinbase_vout
    and .result.vin[0].sequence == 4294967293
    and .result.vin[0].scriptSig.hex == ""
    and (.result.vin[0].txinwitness | length) == 1
    and (.result.vin[0].txinwitness[0] | test("^[0-9a-f]{128}$"))
    and (.result.vout | length) == 2
    and .result.vout[0].n == 0
    and .result.vout[0].value == 1
    and .result.vout[0].scriptPubKey.hex == $contract_script
    and .result.vout[0].scriptPubKey.type == "witness_v1_taproot"
    and .result.vout[0].scriptPubKey.address == $contract_address
    and .result.vout[1].n == 1
    and .result.vout[1].value == 48.99999
    and .result.vout[1].scriptPubKey.hex == $funding_script
    and .result.vout[1].scriptPubKey.address == $funding_address
  ' "${evidence_dir}/${role}-p2tr-funding-confirmed.body" >/dev/null
done

expect_allowed_null maker "${credentials_dir}/maker.curlrc" gettxout \
  "[\"${coinbase_txid}\",${coinbase_vout}]" p2tr-mining-source-spent
expect_allowed_success maker "${credentials_dir}/maker.curlrc" gettxspendingprevout \
  "[[{\"txid\":\"${coinbase_txid}\",\"vout\":${coinbase_vout}}],{\"mempool_only\":false,\"return_spending_tx\":true}]" \
  p2tr-funding-confirmed-spender
jq -e --arg txid "$funding_txid" --arg raw "$funding_raw" \
  --arg block "$funding_block_hash" '
  .result | length == 1
  and .[0].spendingtxid == $txid
  and .[0].spendingtx == $raw
  and .[0].blockhash == $block
' "${evidence_dir}/maker-p2tr-funding-confirmed-spender.body" >/dev/null
expect_allowed_success maker "${credentials_dir}/maker.curlrc" getmempoolinfo \
  '[]' p2tr-after-funding-block-mempool
jq -e '.result.size == 0 and .result.unbroadcastcount == 0' \
  "${evidence_dir}/maker-p2tr-after-funding-block-mempool.body" >/dev/null
expect_allowed_success maker "${credentials_dir}/maker.curlrc" gettxout \
  "[\"${funding_txid}\",0]" p2tr-contract-unspent
jq -e --arg address "$contract_address" --arg script "$contract_script_pubkey" '
  .result.value == 1
  and .result.confirmations == 1
  and .result.coinbase == false
  and .result.scriptPubKey.type == "witness_v1_taproot"
  and .result.scriptPubKey.address == $address
  and .result.scriptPubKey.hex == $script
' "${evidence_dir}/maker-p2tr-contract-unspent.body" >/dev/null

"$helper_binary" spend "$funding_txid" 0 100000000 "$funding_address" \
  >"$cooperative_spend_evidence"
chmod 0600 "$cooperative_spend_evidence"
jq -e \
  --arg funding_txid "$funding_txid" \
  --arg destination "$funding_address" \
  --arg adaptor_point "$contract_adaptor_point" '
  .schema_version == 1
  and .kind == "p2tr_cooperative_spend"
  and .fixture_only == true
  and .fixture_authority == "two_party_musig2_adaptor_public_regtest_vector"
  and .signing_protocol == "BIP327_MUSIG2_SCHNORR_ADAPTOR"
  and .musig2_version == "0.4.1"
  and .signer_order == ["maker", "taker"]
  and .nonce_commitment_scheme == "SHA256_domain_role_session_message_pubnonce"
  and (.maker_nonce_commitment | test("^[0-9a-f]{64}$"))
  and (.taker_nonce_commitment | test("^[0-9a-f]{64}$"))
  and .maker_nonce_commitment != .taker_nonce_commitment
  and .adaptor_point == $adaptor_point
  and (.adaptor_presignature | test("^[0-9a-f]{130}$"))
  and .adaptor_presignature_bytes == 65
  and .adaptor_presignature_verified == true
  and (.final_signature | test("^[0-9a-f]{128}$"))
  and .final_signature_verified_under_q == true
  and .extracted_scalar == "5353535353535353535353535353535353535353535353535353535353535353"
  and .extracted_scalar_public_fixture == true
  and .extracted_point_matches == true
  and .network == "regtest"
  and .funding_txid == $funding_txid
  and .funding_vout == 0
  and .funding_value_sat == 100000000
  and .destination == $destination
  and .output_value_sat == 99999000
  and .fee_sat == 1000
  and .witness_items == 1
  and .witness_bytes == 64
  and .sighash_type == "DEFAULT"
  and .annex == false
  and (.sighash | test("^[0-9a-f]{64}$"))
  and (.unsigned_transaction | test("^[0-9a-f]+$"))
  and (.raw_transaction | test("^[0-9a-f]+$"))
  and (.txid | test("^[0-9a-f]{64}$"))
  and (.wtxid | test("^[0-9a-f]{64}$"))
' "$cooperative_spend_evidence" >/dev/null
cooperative_raw="$(jq -er '.raw_transaction' "$cooperative_spend_evidence")"
cooperative_txid="$(jq -er '.txid' "$cooperative_spend_evidence")"
cooperative_wtxid="$(jq -er '.wtxid' "$cooperative_spend_evidence")"
cooperative_evidence_sha256="$(sha256sum "$cooperative_spend_evidence")"
cooperative_evidence_sha256="${cooperative_evidence_sha256%% *}"

expect_allowed_success maker "${credentials_dir}/maker.curlrc" testmempoolaccept \
  "[[\"${cooperative_raw}\"]]" p2tr-cooperative-policy
jq -e --arg txid "$cooperative_txid" --arg wtxid "$cooperative_wtxid" '
  .result | length == 1
  and .[0].allowed == true
  and .[0].txid == $txid
  and .[0].wtxid == $wtxid
  and .[0].fees.base == 0.00001
' "${evidence_dir}/maker-p2tr-cooperative-policy.body" >/dev/null
expect_allowed_success maker "${credentials_dir}/maker.curlrc" sendrawtransaction \
  "[\"${cooperative_raw}\"]" p2tr-cooperative-broadcast
jq -e --arg txid "$cooperative_txid" '.result == $txid' \
  "${evidence_dir}/maker-p2tr-cooperative-broadcast.body" >/dev/null
expect_allowed_success taker "${credentials_dir}/taker.curlrc" getmempoolentry \
  "[\"${cooperative_txid}\"]" p2tr-cooperative-mempool-observation
jq -e '.result.fees.base == 0.00001 and .result.unbroadcast == true' \
  "${evidence_dir}/taker-p2tr-cooperative-mempool-observation.body" >/dev/null
expect_allowed_success taker "${credentials_dir}/taker.curlrc" gettxspendingprevout \
  "[[{\"txid\":\"${funding_txid}\",\"vout\":0}],{\"mempool_only\":false,\"return_spending_tx\":true}]" \
  p2tr-cooperative-prevout-observation
jq -e --arg txid "$cooperative_txid" --arg raw "$cooperative_raw" '
  .result | length == 1
  and .[0].spendingtxid == $txid
  and .[0].spendingtx == $raw
  and (.[0] | has("blockhash") | not)
' "${evidence_dir}/taker-p2tr-cooperative-prevout-observation.body" >/dev/null
expect_allowed_success taker "${credentials_dir}/taker.curlrc" getmempoolinfo \
  '[]' p2tr-cooperative-mempool-size
jq -e '.result.size == 1 and .result.unbroadcastcount == 1' \
  "${evidence_dir}/taker-p2tr-cooperative-mempool-size.body" >/dev/null

cooperative_block_mocktime=$((mocktime_base + 102 * 600))
core_cli setmocktime "$cooperative_block_mocktime"
generated="$(core_cli generatetodescriptor 1 "$funding_descriptor")"
cooperative_block_hash="$(jq -er 'if length == 1 then .[0] else error("block count") end' <<<"$generated")"
jq -cn \
  --argjson height 103 \
  --argjson mocktime "$cooperative_block_mocktime" \
  --arg block_hash "$cooperative_block_hash" \
  --arg purpose p2tr_cooperative_confirmation \
  --arg txid "$cooperative_txid" \
  '{height:$height, mocktime:$mocktime, block_hash:$block_hash, purpose:$purpose, txid:$txid}' \
  >>"$blocks_file"
core_cli getblock "$cooperative_block_hash" 2 >"${evidence_dir}/p2tr-cooperative-block.json"
chmod 0600 "$blocks_file" "${evidence_dir}/p2tr-cooperative-block.json"
jq -e --arg txid "$cooperative_txid" --arg raw "$cooperative_raw" '
  .height == 103
  and (.tx | length) == 2
  and .tx[1].txid == $txid
  and .tx[1].hex == $raw
' "${evidence_dir}/p2tr-cooperative-block.json" >/dev/null

for role in maker taker; do
  role_config="${credentials_dir}/${role}.curlrc"
  expect_allowed_success "$role" "$role_config" getrawtransaction \
    "[\"${cooperative_txid}\",true,\"${cooperative_block_hash}\"]" \
    p2tr-cooperative-confirmed
  jq -e \
    --arg txid "$cooperative_txid" \
    --arg wtxid "$cooperative_wtxid" \
    --arg raw "$cooperative_raw" \
    --arg blockhash "$cooperative_block_hash" \
    --arg funding_txid "$funding_txid" \
    --arg destination "$funding_address" \
    --arg destination_script "5120${funding_key}" '
    .result.txid == $txid
    and .result.hash == $wtxid
    and .result.hex == $raw
    and .result.blockhash == $blockhash
    and .result.version == 2
    and .result.locktime == 0
    and .result.confirmations == 1
    and .result.in_active_chain == true
    and (.result.vin | length) == 1
    and .result.vin[0].txid == $funding_txid
    and .result.vin[0].vout == 0
    and .result.vin[0].sequence == 4294967293
    and .result.vin[0].scriptSig.hex == ""
    and (.result.vin[0].txinwitness | length) == 1
    and (.result.vin[0].txinwitness[0] | test("^[0-9a-f]{128}$"))
    and (.result.vout | length) == 1
    and .result.vout[0].n == 0
    and .result.vout[0].value == 0.99999
    and .result.vout[0].scriptPubKey.type == "witness_v1_taproot"
    and .result.vout[0].scriptPubKey.address == $destination
    and .result.vout[0].scriptPubKey.hex == $destination_script
  ' "${evidence_dir}/${role}-p2tr-cooperative-confirmed.body" >/dev/null
done

expect_allowed_null taker "${credentials_dir}/taker.curlrc" gettxout \
  "[\"${funding_txid}\",0]" p2tr-contract-spent
expect_allowed_success taker "${credentials_dir}/taker.curlrc" gettxspendingprevout \
  "[[{\"txid\":\"${funding_txid}\",\"vout\":0}],{\"mempool_only\":false,\"return_spending_tx\":true}]" \
  p2tr-cooperative-confirmed-spender
jq -e --arg txid "$cooperative_txid" --arg raw "$cooperative_raw" \
  --arg block "$cooperative_block_hash" '
  .result | length == 1
  and .[0].spendingtxid == $txid
  and .[0].spendingtx == $raw
  and .[0].blockhash == $block
' "${evidence_dir}/taker-p2tr-cooperative-confirmed-spender.body" >/dev/null

core_cli getrawtransaction "$funding_txid" true "$funding_block_hash" \
  >"${evidence_dir}/final-funding-transaction.json"
core_cli getblockchaininfo >"${evidence_dir}/final-chain.json"
core_cli getnetworkinfo >"${evidence_dir}/final-network.json"
core_cli getindexinfo >"${evidence_dir}/final-indexes.json"
core_cli getmempoolinfo >"${evidence_dir}/final-mempool.json"
final_tip_hash="$(core_cli getblockhash 103)"
core_cli getblockheader "$final_tip_hash" >"${evidence_dir}/final-header.json"
chmod 0600 "${evidence_dir}/final-funding-transaction.json" \
  "${evidence_dir}/final-chain.json" "${evidence_dir}/final-network.json" \
  "${evidence_dir}/final-indexes.json" "${evidence_dir}/final-mempool.json" \
  "${evidence_dir}/final-header.json"
jq -e --arg txid "$funding_txid" --arg blockhash "$funding_block_hash" '
  .txid == $txid and .blockhash == $blockhash and .confirmations == 2 and .in_active_chain == true
' "${evidence_dir}/final-funding-transaction.json" >/dev/null
jq -e '.chain == "regtest" and .blocks == 103 and .headers == 103' \
  "${evidence_dir}/final-chain.json" >/dev/null
jq -e '.networkactive == false and .connections == 0' \
  "${evidence_dir}/final-network.json" >/dev/null
jq -e '.txindex.synced == true and .txospenderindex.synced == true' \
  "${evidence_dir}/final-indexes.json" >/dev/null
jq -e '.size == 0 and .unbroadcastcount == 0' \
  "${evidence_dir}/final-mempool.json" >/dev/null
jq -e --arg blockhash "$cooperative_block_hash" --argjson expected "$cooperative_block_mocktime" '
  .hash == $blockhash and .height == 103 and .time == $expected and .confirmations == 1
' "${evidence_dir}/final-header.json" >/dev/null
core_cli setmocktime 0

sha256sum \
  scripts/run-bitcoin-core-e2e.sh \
  Cargo.lock \
  deny.toml \
  crates/btc-swap-sdk/Cargo.toml \
  crates/btc-swap-sdk/src/lib.rs \
  crates/btc-swap-sdk/src/p2tr.rs \
  crates/btc-swap-sdk/examples/btc-core-p2tr-fixture.rs \
  crates/btc-swap-sdk/examples/musig2-adaptor-poc.rs \
  "$config_file" \
  "$provenance_evidence" \
  "$contract_evidence" \
  "$funding_transaction_evidence" \
  "$cooperative_spend_evidence" \
  "$blocks_file" \
  "$actor_matrix" \
  "${evidence_dir}/container-inspect.json" \
  "${evidence_dir}/network-inspect.json" \
  "${evidence_dir}/volume-inspect.json" \
  "${evidence_dir}/image-inspect.json" \
  "${evidence_dir}/taker-p2tr-funding-policy.body" \
  "${evidence_dir}/taker-p2tr-funding-broadcast.body" \
  "${evidence_dir}/maker-p2tr-funding-mempool-observation.body" \
  "${evidence_dir}/p2tr-funding-block.json" \
  "${evidence_dir}/maker-p2tr-funding-confirmed.body" \
  "${evidence_dir}/taker-p2tr-funding-confirmed.body" \
  "${evidence_dir}/maker-p2tr-mining-source-spent.body" \
  "${evidence_dir}/maker-p2tr-funding-confirmed-spender.body" \
  "${evidence_dir}/maker-p2tr-contract-unspent.body" \
  "${evidence_dir}/maker-p2tr-cooperative-policy.body" \
  "${evidence_dir}/maker-p2tr-cooperative-broadcast.body" \
  "${evidence_dir}/taker-p2tr-cooperative-mempool-observation.body" \
  "${evidence_dir}/p2tr-cooperative-block.json" \
  "${evidence_dir}/maker-p2tr-cooperative-confirmed.body" \
  "${evidence_dir}/taker-p2tr-cooperative-confirmed.body" \
  "${evidence_dir}/taker-p2tr-contract-spent.body" \
  "${evidence_dir}/taker-p2tr-cooperative-confirmed-spender.body" \
  "${evidence_dir}/final-funding-transaction.json" \
  "${evidence_dir}/final-chain.json" \
  "${evidence_dir}/final-network.json" \
  "${evidence_dir}/final-indexes.json" \
  "${evidence_dir}/final-mempool.json" \
  "${evidence_dir}/final-header.json" \
  >"$critical_evidence_manifest"
chmod 0600 "$critical_evidence_manifest"
critical_manifest_sha256="$(sha256sum "$critical_evidence_manifest")"
critical_manifest_sha256="${critical_manifest_sha256%% *}"

contract_evidence_sha256="$(sha256sum "$contract_evidence")"
contract_evidence_sha256="${contract_evidence_sha256%% *}"
contract_summary="$(jq -c . "$contract_evidence")"
funding_summary="$(jq -c 'del(.raw_transaction)' "$funding_transaction_evidence")"
cooperative_summary="$(jq -c 'del(.raw_transaction, .unsigned_transaction)' "$cooperative_spend_evidence")"
repository_commit="$(git rev-parse HEAD)"
if [[ -z "$(git status --porcelain --untracked-files=normal)" ]]; then
  repository_worktree_clean=true
else
  repository_worktree_clean=false
fi

config_sha256="$(sha256sum "$config_file")"
config_sha256="${config_sha256%% *}"
provenance_sha256="$(sha256sum "$provenance_evidence")"
provenance_sha256="${provenance_sha256%% *}"
block_policy_sha256="$(sha256sum "$blocks_file")"
block_policy_sha256="${block_policy_sha256%% *}"
actor_matrix_json="$(jq -s . "$actor_matrix")"
completed_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
runtime_partial="${runtime_evidence}.partial"

jq -n \
  --arg run_id "$run_id" \
  --arg mode "$mode" \
  --arg completed_at "$completed_at" \
  --arg project "$project" \
  --arg container_id "$container_id" \
  --arg network "$network" \
  --arg network_cidr "$network_cidr" \
  --arg volume "$volume" \
  --arg image "$image" \
  --arg image_id "$image_id" \
  --arg rpc_endpoint "$published_endpoint" \
  --arg source_commit "$BITCOIN_CORE_SOURCE_COMMIT" \
  --arg archive_sha256 "$BITCOIN_CORE_ARCHIVE_SHA256" \
  --arg provenance_sha256 "$provenance_sha256" \
  --arg config_sha256 "$config_sha256" \
  --arg genesis "$genesis_hash" \
  --arg descriptor "$funding_descriptor" \
  --arg address "$funding_address" \
  --arg coinbase_txid "$coinbase_txid" \
  --argjson coinbase_vout "$coinbase_vout" \
  --arg block_policy_sha256 "$block_policy_sha256" \
  --argjson actor_matrix "$actor_matrix_json" \
  --arg archive_url "$BITCOIN_CORE_ARCHIVE_URL" \
  --arg source_url "$BITCOIN_CORE_SOURCE_URL" \
  --arg guix_url "$BITCOIN_CORE_GUIX_SIGS_URL" \
  --arg runtime_base "$BITCOIN_CORE_RUNTIME_BASE" \
  --arg helper_sha256 "$helper_sha256" \
  --arg repository_commit "$repository_commit" \
  --arg require_clean "$require_clean" \
  --argjson repository_worktree_clean "$repository_worktree_clean" \
  --arg critical_manifest_sha256 "$critical_manifest_sha256" \
  --arg contract_evidence_sha256 "$contract_evidence_sha256" \
  --arg funding_evidence_sha256 "$funding_evidence_sha256" \
  --arg cooperative_evidence_sha256 "$cooperative_evidence_sha256" \
  --arg funding_block_hash "$funding_block_hash" \
  --arg cooperative_block_hash "$cooperative_block_hash" \
  --argjson contract "$contract_summary" \
  --argjson funding "$funding_summary" \
  --argjson cooperative "$cooperative_summary" \
  --arg crates_index_url "https://index.crates.io/" \
  --arg crates_static_url "https://static.crates.io/" '
  {
    schema_version: 1,
    result: "passed",
    mode: $mode,
    run_id: $run_id,
    completed_at: $completed_at,
    repository: {
      commit: $repository_commit,
      worktree_clean: $repository_worktree_clean,
      clean_required: ($require_clean == "1"),
      critical_evidence_manifest_sha256: $critical_manifest_sha256
    },
    core: {
      version: "31.1",
      source_commit: $source_commit,
      archive_sha256: $archive_sha256,
      provenance_evidence_sha256: $provenance_sha256,
      image: $image,
      image_id: $image_id,
      fixture_helper_sha256: $helper_sha256
    },
    isolation: {
      docker_resource_scope: $project,
      lifecycle: "exact_id_native_docker",
      compose_contract_validated: true,
      container_id: $container_id,
      network: $network,
      network_cidr: $network_cidr,
      data_volume: $volume,
      rpc_endpoint: $rpc_endpoint,
      rpc_publication: "dynamic_literal_loopback_only",
      p2p_port_published: false,
      config_sha256: $config_sha256,
      assertion_basis: "configuration_plus_Docker_and_RPC_inspection"
    },
    chain: {
      network: "regtest",
      genesis: $genesis,
      initial_height: 0,
      final_height: 103,
      peers_before: 0,
      peers_after: 0,
      network_active: false,
      mocktime: {base: 1700000000, spacing_seconds: 600, blocks: 103, reset_after_evidence: true},
      generated_block_policy_sha256: $block_policy_sha256
    },
    mining_source: {
      descriptor: $descriptor,
      address: $address,
      txid: $coinbase_txid,
      vout: $coinbase_vout,
      value_btc: 50,
      confirmations_before_spend: 101,
      spent_by: $funding.txid,
      script_type: "witness_v1_taproot",
      reproducibility: "semantic_policy_not_cross_run_block_or_tx_identity"
    },
    p2tr_contract: ($contract + {
      evidence_sha256: $contract_evidence_sha256,
      authority_scope: "two_party_musig2_adaptor_public_fixture"
    }),
    p2tr_funding: ($funding + {
      evidence_sha256: $funding_evidence_sha256,
      submitted_by: "taker",
      observed_by: "maker",
      block_hash: $funding_block_hash,
      policy_accepted: true,
      consensus_accepted: true,
      confirmations_final: 2
    }),
    cooperative_key_path_claim: ($cooperative + {
      evidence_sha256: $cooperative_evidence_sha256,
      submitted_by: "maker",
      observed_by: "taker",
      block_hash: $cooperative_block_hash,
      policy_accepted: true,
      consensus_accepted: true,
      exact_contract_outpoint_spent_once: true
    }),
    security_claims: {
      direction: "TakerSellsForeign",
      fixture_rpc_role_ordering_proven: true,
      taproot_tweak_and_consensus_spend_proven: true,
      known_private_key_fixture: true,
      production_signing_authority_proven: false,
      independent_actor_processes_proven: false,
      durable_actor_stores_proven: false,
      musig2_taproot_fixture_proven: true,
      adaptor_signature_fixture_proven: true,
      scalar_extraction_fixture_proven: true,
      nonce_commitment_exchange_proven: false,
      crash_safe_nonce_journal_proven: false,
      lez_composition_proven: false,
      atomicity_proven: false
    },
    actor_rpc: {
      users: ["maker", "taker"],
      credentials_distinct: true,
      credentials_mode: "0600_under_0700_run_root",
      plaintext_credentials_disclosed: false,
      results: $actor_matrix
    },
    external_dependencies: {
      runtime_external_resources: [],
      public_rpc_used: false,
      faucet_used: false,
      public_funds_used: false,
      cold_setup_external_resources: [
        $archive_url,
        $source_url,
        $guix_url,
        $runtime_base,
        $crates_index_url,
        $crates_static_url
      ]
    }
  }
' >"$runtime_partial"
jq -e '
  .result == "passed"
  and .mode == "fixture"
  and .chain.final_height == 103
  and (.repository.critical_evidence_manifest_sha256 | test("^[0-9a-f]{64}$"))
  and (.repository.clean_required == false or .repository.worktree_clean == true)
  and .p2tr_contract.output_key == "e077de917e5cff6c4055f07ef4676f3d0df57dc2ff66036824d917e1937c8a3a"
  and .p2tr_funding.submitted_by == "taker"
  and .p2tr_funding.consensus_accepted == true
  and .cooperative_key_path_claim.submitted_by == "maker"
  and .cooperative_key_path_claim.consensus_accepted == true
  and .cooperative_key_path_claim.exact_contract_outpoint_spent_once == true
  and .security_claims.taproot_tweak_and_consensus_spend_proven == true
  and .security_claims.fixture_rpc_role_ordering_proven == true
  and .security_claims.known_private_key_fixture == true
  and .security_claims.production_signing_authority_proven == false
  and .security_claims.independent_actor_processes_proven == false
  and .security_claims.durable_actor_stores_proven == false
  and .security_claims.musig2_taproot_fixture_proven == true
  and .security_claims.adaptor_signature_fixture_proven == true
  and .security_claims.scalar_extraction_fixture_proven == true
  and .security_claims.nonce_commitment_exchange_proven == false
  and .security_claims.crash_safe_nonce_journal_proven == false
  and .security_claims.lez_composition_proven == false
  and .security_claims.atomicity_proven == false
  and .actor_rpc.credentials_distinct == true
  and .external_dependencies.runtime_external_resources == []
  and .external_dependencies.public_rpc_used == false
' "$runtime_partial" >/dev/null
chmod 0600 "$runtime_partial"
mv "$runtime_partial" "$runtime_evidence"

{
  printf 'RUN_ID=%s\n' "$run_id"
  printf 'BITCOIN_CORE_E2E_MODE=%s\n' "$mode"
  printf 'COMPOSE_PROJECT_NAME=%s\n' "$project"
  printf 'BITCOIN_CORE_IMAGE=%s\n' "$image"
  printf 'BITCOIN_CORE_CONFIG=%s\n' "$config_file"
  printf 'BITCOIN_CORE_RPC_URL=%s\n' "$rpc_url"
  printf 'BITCOIN_CORE_RUNTIME_EVIDENCE=%s\n' "$runtime_evidence"
} >"$manifest"
chmod 0600 "$manifest"

unset maker_password taker_password maker_auth taker_auth
runtime_complete=1
echo "Bitcoin Core 31.1 isolated MuSig2 adaptor P2TR flow passed: ${runtime_evidence}"
