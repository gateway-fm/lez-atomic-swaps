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
readonly actor_matrix="${evidence_dir}/actor-rpc-matrix.ndjson"
readonly genesis_hash="0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206"
readonly funding_key="79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
readonly funding_descriptor="rawtr(${funding_key})#xsjqcczm"
readonly funding_address="bcrt1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqc8gma6"
readonly mocktime_base=1700000000
readonly actor_allowlist="getblockchaininfo,getnetworkinfo,getblockhash,getblockheader,getrawtransaction,gettxout,gettxspendingprevout,getmempoolinfo,getmempoolentry,testmempoolaccept,sendrawtransaction"

required_commands=(chmod curl date docker git gpg jq mkdir mv python3 rg seq sha256sum sleep stat tar tr)
for command_name in "${required_commands[@]}"; do
  command -v "$command_name" >/dev/null || {
    echo "missing Bitcoin Core E2E tool: ${command_name}" >&2
    exit 1
  }
done
docker info >/dev/null

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
    --arg status "$status" \
    --argjson sentinel_survived "$sentinel_survived" \
    --argjson resources_absent "$resources_absent" \
    '{
      schema_version: 1,
      run_id: $run_id,
      cleanup_status: $status,
      exact_run_resources_absent: $resources_absent,
      foreign_sentinel_survived_exact_cleanup: $sentinel_survived,
      broad_cleanup_used: false
    }' >"$partial"
  chmod 0600 "$partial"
  mv "$partial" "$cleanup_evidence"
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

  if [[ "$keep_running" == "1" && "$run_status" == "0" && "$runtime_complete" == "1" ]]; then
    if [[ "$sentinel_created" == "1" ]]; then
      docker network rm "$sentinel_network" >/dev/null 2>&1 || cleanup_failed=1
    fi
    echo "Bitcoin Core Regtest remains running for RUN_ID=${run_id}"
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
container_id="$(docker create \
  --name "${project}-bitcoin-core" \
  --label "org.logos-co.atomic-swaps.run=${run_id}" \
  --label 'org.logos-co.atomic-swaps.scope=bitcoin-core-regtest-e2e' \
  --label 'org.logos-co.atomic-swaps.component=bitcoin-core' \
  --network "$network" \
  --publish '127.0.0.1::18443' \
  --user '65532:65532' \
  --read-only \
  --cap-drop ALL \
  --security-opt no-new-privileges=true \
  --pids-limit 256 \
  --cpus 2 \
  --memory 2g \
  --stop-timeout 30 \
  --env HOME=/tmp \
  --mount "type=bind,source=${config_file},target=/run-config/bitcoin.conf,readonly" \
  --mount "type=volume,source=${volume},target=/var/lib/bitcoin" \
  --tmpfs /tmp:rw,noexec,nosuid,size=134217728,mode=1777 \
  "$image" \
  -conf=/run-config/bitcoin.conf \
  -datadir=/var/lib/bitcoin)"
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

core_cli getblockchaininfo >"${evidence_dir}/final-chain.json"
core_cli getnetworkinfo >"${evidence_dir}/final-network.json"
core_cli getindexinfo >"${evidence_dir}/final-indexes.json"
first_block_hash="$(core_cli getblockhash 1)"
last_block_hash="$(core_cli getblockhash 101)"
core_cli getblockheader "$first_block_hash" >"${evidence_dir}/first-header.json"
core_cli getblockheader "$last_block_hash" >"${evidence_dir}/last-header.json"
core_cli getblock "$first_block_hash" 2 >"${evidence_dir}/first-block.json"
chmod 0600 "${evidence_dir}/final-chain.json" "${evidence_dir}/final-network.json" \
  "${evidence_dir}/final-indexes.json" "${evidence_dir}/first-header.json" \
  "${evidence_dir}/last-header.json" "${evidence_dir}/first-block.json"

jq -e '.chain == "regtest" and .blocks == 101 and .headers == 101' \
  "${evidence_dir}/final-chain.json" >/dev/null
jq -e '.networkactive == false and .connections == 0' \
  "${evidence_dir}/final-network.json" >/dev/null
jq -e '.txindex.synced == true and .txospenderindex.synced == true' \
  "${evidence_dir}/final-indexes.json" >/dev/null
jq -e --argjson expected "$mocktime_base" '.height == 1 and .time == $expected' \
  "${evidence_dir}/first-header.json" >/dev/null
jq -e --argjson expected "$((mocktime_base + 100 * 600))" \
  '.height == 101 and .time == $expected' "${evidence_dir}/last-header.json" >/dev/null

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
' "${evidence_dir}/mature-funding.json" >/dev/null

for role in maker taker; do
  role_config="${credentials_dir}/${role}.curlrc"
  expect_allowed_success "$role" "$role_config" getblockchaininfo '[]' \
    allowed-getblockchaininfo
  expect_allowed_success "$role" "$role_config" getnetworkinfo '[]' \
    allowed-getnetworkinfo
  expect_allowed_success "$role" "$role_config" getblockhash '[0]' \
    allowed-getblockhash
  expect_allowed_success "$role" "$role_config" getblockheader \
    "[\"${genesis_hash}\"]" allowed-getblockheader
  expect_allowed_success "$role" "$role_config" getrawtransaction \
    "[\"${coinbase_txid}\",true,\"${first_block_hash}\"]" allowed-getrawtransaction
  expect_allowed_success "$role" "$role_config" gettxout \
    "[\"${coinbase_txid}\",${coinbase_vout}]" allowed-gettxout
  expect_allowed_success "$role" "$role_config" gettxspendingprevout \
    "[[{\"txid\":\"${coinbase_txid}\",\"vout\":${coinbase_vout}}]]" \
    allowed-gettxspendingprevout
  expect_allowed_success "$role" "$role_config" getmempoolinfo '[]' \
    allowed-getmempoolinfo
  expect_allowed_method_error "$role" "$role_config" getmempoolentry \
    '["0000000000000000000000000000000000000000000000000000000000000000"]' \
    allowed-getmempoolentry-error
  expect_allowed_method_error "$role" "$role_config" testmempoolaccept \
    '[["00"]]' allowed-testmempoolaccept-error
  expect_allowed_method_error "$role" "$role_config" sendrawtransaction \
    '["00"]' allowed-sendrawtransaction-error
done

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
  --arg runtime_base "$BITCOIN_CORE_RUNTIME_BASE" '
  {
    schema_version: 1,
    result: "passed",
    run_id: $run_id,
    completed_at: $completed_at,
    core: {
      version: "31.1",
      source_commit: $source_commit,
      archive_sha256: $archive_sha256,
      provenance_evidence_sha256: $provenance_sha256,
      image: $image,
      image_id: $image_id
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
      final_height: 101,
      peers_before: 0,
      peers_after: 0,
      network_active: false,
      mocktime: {base: 1700000000, spacing_seconds: 600, blocks: 101},
      generated_block_policy_sha256: $block_policy_sha256
    },
    deterministic_funding: {
      descriptor: $descriptor,
      address: $address,
      txid: $coinbase_txid,
      vout: $coinbase_vout,
      value_btc: 50,
      confirmations: 101,
      script_type: "witness_v1_taproot",
      reproducibility: "semantic_policy_not_cross_run_block_or_tx_identity"
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
      cold_setup_external_resources: [$archive_url, $source_url, $guix_url, $runtime_base]
    }
  }
' >"$runtime_partial"
jq -e '
  .result == "passed"
  and .chain.final_height == 101
  and .actor_rpc.credentials_distinct == true
  and .external_dependencies.runtime_external_resources == []
  and .external_dependencies.public_rpc_used == false
' "$runtime_partial" >/dev/null
chmod 0600 "$runtime_partial"
mv "$runtime_partial" "$runtime_evidence"

{
  printf 'RUN_ID=%s\n' "$run_id"
  printf 'COMPOSE_PROJECT_NAME=%s\n' "$project"
  printf 'BITCOIN_CORE_IMAGE=%s\n' "$image"
  printf 'BITCOIN_CORE_CONFIG=%s\n' "$config_file"
  printf 'BITCOIN_CORE_RPC_URL=%s\n' "$rpc_url"
  printf 'BITCOIN_CORE_RUNTIME_EVIDENCE=%s\n' "$runtime_evidence"
} >"$manifest"
chmod 0600 "$manifest"

unset maker_password taker_password maker_auth taker_auth
runtime_complete=1
echo "Bitcoin Core 31.1 isolated actor smoke passed: ${runtime_evidence}"
