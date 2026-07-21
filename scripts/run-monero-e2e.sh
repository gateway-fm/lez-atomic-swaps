#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077
run_started_epoch="$(date +%s)"

readonly compose_file="tests/e2e/monero/compose.yml"
readonly dockerfile="tests/e2e/monero/Dockerfile"
readonly provenance_file="tests/e2e/monero/provenance.env"
# shellcheck source=tests/e2e/monero/provenance.env
source "$provenance_file"

run_id="${RUN_ID:-local-$(date -u +%Y%m%d%H%M%S)-$$}"
if [[ ! "$run_id" =~ ^[a-z0-9][a-z0-9_-]{7,63}$ ]]; then
  echo "RUN_ID must be 8..64 lowercase letters, numbers, underscores, or hyphens" >&2
  exit 1
fi

keep_running="${MONERO_E2E_KEEP_RUNNING:-0}"
if [[ "$keep_running" != "0" && "$keep_running" != "1" ]]; then
  echo "MONERO_E2E_KEEP_RUNNING must be 0 or 1" >&2
  exit 1
fi
require_clean="${MONERO_E2E_REQUIRE_CLEAN:-0}"
if [[ "$require_clean" != "0" && "$require_clean" != "1" ]]; then
  echo "MONERO_E2E_REQUIRE_CLEAN must be 0 or 1" >&2
  exit 1
fi

readonly project="lez-atomic-swaps-monero-${run_id}"
readonly image="lez-atomic-swaps-monero:${run_id}"
readonly network="${project}-private"
readonly sentinel_network="${project}-foreign-sentinel"
repo_root="$(pwd)"
readonly repo_root
readonly run_dir="${repo_root}/.e2e/${run_id}/monero"
readonly cache_dir="${MONERO_CACHE_DIR:-${repo_root}/.e2e/cache/monero-${MONERO_VERSION}}"
readonly build_context="${run_dir}/build-context"
readonly config_dir="${run_dir}/configs"
readonly credentials_dir="${run_dir}/credentials"
readonly evidence_dir="${run_dir}/evidence"
readonly logs_dir="${run_dir}/logs"
readonly provenance_evidence="${evidence_dir}/provenance.json"
readonly runtime_evidence="${evidence_dir}/runtime.json"
readonly cleanup_evidence="${evidence_dir}/cleanup.json"
readonly critical_evidence_manifest="${evidence_dir}/critical-evidence.sha256"
readonly manifest="${run_dir}/run.env"
readonly funding_amount=10000000000000
readonly confirmation_policy=10

required_commands=(
  awk bash chmod curl date docker git gpg gpgconf jq mkdir mktemp mv
  openssl perl rg rm sha256sum sort stat tar
)
for command_name in "${required_commands[@]}"; do
  command -v "$command_name" >/dev/null || {
    echo "missing Monero E2E tool: ${command_name}" >&2
    exit 1
  }
done
docker info >/dev/null
if [[ "$require_clean" == "1" ]] &&
   [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  echo "Monero certification requires a clean repository worktree" >&2
  exit 1
fi
if [[ -e "$run_dir" || -L "$run_dir" ]]; then
  echo "refusing to reuse Monero E2E run state: ${run_dir}" >&2
  exit 1
fi
if docker image inspect "$image" >/dev/null 2>&1; then
  echo "refusing to reuse Monero E2E image: ${image}" >&2
  exit 1
fi
for resource in "$network" "$sentinel_network"; do
  if docker network inspect "$resource" >/dev/null 2>&1; then
    echo "refusing to reuse Monero E2E network: ${resource}" >&2
    exit 1
  fi
done
if [[ -n "$(docker container ls --all --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${run_id}")" ]] ||
   [[ -n "$(docker network ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${run_id}")" ]] ||
   [[ -n "$(docker volume ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${run_id}")" ]]; then
  echo "refusing to reuse Docker resources carrying run label ${run_id}" >&2
  exit 1
fi

mkdir -p "$config_dir" "$credentials_dir" "$evidence_dir" "$logs_dir"
chmod 0700 "$run_dir" "$config_dir" "$credentials_dir" "$evidence_dir" "$logs_dir"

daemon_secret="$(openssl rand -hex 24)"
funding_rpc_secret="$(openssl rand -hex 24)"
maker_rpc_secret="$(openssl rand -hex 24)"
taker_rpc_secret="$(openssl rand -hex 24)"
funding_wallet_password="$(openssl rand -hex 24)"
maker_wallet_password="$(openssl rand -hex 24)"
taker_wallet_password="$(openssl rand -hex 24)"

write_config() {
  local path="$1"
  shift
  printf '%s\n' "$@" >"$path"
  chmod 0444 "$path"
}

write_secret_value() {
  local path="$1"
  local value="$2"
  printf '%s\n' "$value" >"$path"
  chmod 0600 "$path"
}

write_curl_config() {
  local path="$1"
  local user="$2"
  local password="$3"
  {
    printf '%s\n' 'digest'
    printf 'user = "%s:%s"\n' "$user" "$password"
    printf '%s\n' 'connect-timeout = 2'
    printf '%s\n' 'max-time = 30'
    printf '%s\n' 'silent'
    printf '%s\n' 'show-error'
    printf '%s\n' 'fail-with-body'
    printf '%s\n' 'header = "content-type: application/json"'
  } >"$path"
  chmod 0600 "$path"
}

write_config "${config_dir}/monerod.conf" \
  'regtest=1' \
  'offline=1' \
  'fixed-difficulty=1' \
  'data-dir=/var/lib/monero' \
  'p2p-bind-ip=127.0.0.1' \
  'p2p-bind-port=18080' \
  'rpc-bind-ip=0.0.0.0' \
  'rpc-bind-port=18081' \
  'confirm-external-bind=1' \
  "rpc-login=daemon:${daemon_secret}" \
  'rpc-ssl=disabled' \
  'no-zmq=1' \
  'disable-dns-checkpoints=1' \
  'check-updates=disabled' \
  'no-igd=1' \
  'max-concurrency=1' \
  'log-file=/dev/stdout' \
  'log-level=1'

write_wallet_config() {
  local path="$1"
  local user="$2"
  local secret="$3"
  write_config "$path" \
    'wallet-dir=/var/lib/monero-wallet' \
    'rpc-bind-ip=0.0.0.0' \
    'rpc-bind-port=18083' \
    'confirm-external-bind=1' \
    "rpc-login=${user}:${secret}" \
    'rpc-ssl=disabled' \
    'daemon-address=monerod:18081' \
    "daemon-login=daemon:${daemon_secret}" \
    'daemon-ssl=disabled' \
    'trusted-daemon=1' \
    'allow-mismatched-daemon-version=1' \
    'no-initial-sync=1' \
    'non-interactive=1' \
    'max-concurrency=1' \
    'log-file=/dev/stdout' \
    'log-level=1'
}

write_wallet_config "${config_dir}/funding-wallet.conf" funding "$funding_rpc_secret"
write_wallet_config "${config_dir}/maker-wallet.conf" maker "$maker_rpc_secret"
write_wallet_config "${config_dir}/taker-wallet.conf" taker "$taker_rpc_secret"
write_secret_value "${credentials_dir}/daemon.username" daemon
write_secret_value "${credentials_dir}/daemon.password" "$daemon_secret"
write_curl_config "${credentials_dir}/daemon.curlrc" daemon "$daemon_secret"
write_curl_config "${credentials_dir}/funding.curlrc" funding "$funding_rpc_secret"
write_curl_config "${credentials_dir}/maker.curlrc" maker "$maker_rpc_secret"
write_curl_config "${credentials_dir}/taker.curlrc" taker "$taker_rpc_secret"

export RUN_ID="$run_id"
export MONERO_IMAGE="$image"
export MONERO_NETWORK="$network"
export MONERO_DAEMON_CONFIG="${config_dir}/monerod.conf"
export MONERO_FUNDING_WALLET_CONFIG="${config_dir}/funding-wallet.conf"
export MONERO_MAKER_WALLET_CONFIG="${config_dir}/maker-wallet.conf"
export MONERO_TAKER_WALLET_CONFIG="${config_dir}/taker-wallet.conf"
readonly -a compose=(docker compose --project-name "$project" --file "$compose_file")

compose_started=0
sentinel_created=0
runtime_complete=0

collect_logs() {
  local service
  for service in monerod funding_wallet maker_wallet taker_wallet; do
    if [[ "$compose_started" == "1" ]] &&
       "${compose[@]}" ps --quiet "$service" >/dev/null 2>&1; then
      "${compose[@]}" logs --no-color "$service" \
        >"${logs_dir}/${service}.log" 2>&1 || true
      chmod 0600 "${logs_dir}/${service}.log"
    fi
  done
}

assert_no_secret_leak() {
  local secret
  for secret in \
    "$daemon_secret" "$funding_rpc_secret" "$maker_rpc_secret" "$taker_rpc_secret" \
    "$funding_wallet_password" "$maker_wallet_password" "$taker_wallet_password"; do
    if rg -Fq -- "$secret" "$evidence_dir" "$logs_dir"; then
      echo "Monero runtime evidence or logs contain a generated secret" >&2
      return 1
    fi
  done
}

assert_owned_resources_absent() {
  [[ -z "$(docker container ls --all --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${run_id}")" ]] &&
  [[ -z "$(docker network ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${run_id}")" ]] &&
  [[ -z "$(docker volume ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${run_id}")" ]] &&
  ! docker network inspect "$network" >/dev/null 2>&1 &&
  ! docker image inspect "$image" >/dev/null 2>&1
}

write_cleanup_evidence() {
  local result="$1"
  local sentinel_survived="$2"
  local resources_absent="$3"
  local partial="${cleanup_evidence}.partial"
  jq -n \
    --arg run_id "$run_id" \
    --arg result "$result" \
    --argjson sentinel_survived "$sentinel_survived" \
    --argjson resources_absent "$resources_absent" \
    '{
      schema_version: 1,
      run_id: $run_id,
      result: $result,
      exact_run_resources_absent: $resources_absent,
      foreign_sentinel_survived_exact_cleanup: $sentinel_survived,
      broad_cleanup_used: false
    }' >"$partial"
  chmod 0600 "$partial"
  mv "$partial" "$cleanup_evidence"
}

cleanup() {
  local run_status=$?
  local cleanup_failed=0
  local sentinel_survived=false
  local resources_absent=false
  trap - EXIT
  set +e

  collect_logs
  assert_no_secret_leak || cleanup_failed=1

  if [[ -d "$build_context" && ! -L "$build_context" ]]; then
    rm -rf -- "$build_context" || cleanup_failed=1
  elif [[ -e "$build_context" || -L "$build_context" ]]; then
    cleanup_failed=1
  fi
  if [[ -d "${build_context}.partial" && ! -L "${build_context}.partial" ]]; then
    rm -rf -- "${build_context}.partial" || cleanup_failed=1
  elif [[ -e "${build_context}.partial" || -L "${build_context}.partial" ]]; then
    cleanup_failed=1
  fi
  if [[ -f "${provenance_evidence}.partial" &&
        ! -L "${provenance_evidence}.partial" ]]; then
    rm -- "${provenance_evidence}.partial" || cleanup_failed=1
  elif [[ -e "${provenance_evidence}.partial" ||
          -L "${provenance_evidence}.partial" ]]; then
    cleanup_failed=1
  fi

  if [[ "$keep_running" == "1" && "$run_status" == "0" &&
        "$runtime_complete" == "1" ]]; then
    if [[ "$sentinel_created" == "1" ]]; then
      docker network rm "$sentinel_network" >/dev/null 2>&1 || cleanup_failed=1
    fi
    echo "Monero Regtest remains running for RUN_ID=${run_id}"
    echo "Manifest: ${manifest}"
    echo "Runtime evidence: ${runtime_evidence}"
    echo "Use only the run-scoped credential files under ${credentials_dir}"
    echo "To clean up, source the manifest and run:"
    echo 'docker compose --project-name "$MONERO_COMPOSE_PROJECT" --file "$MONERO_COMPOSE_FILE" down --volumes --remove-orphans'
    echo 'docker network rm "$MONERO_NETWORK"'
    echo 'docker image rm "$MONERO_IMAGE"'
    [[ "$cleanup_failed" == "0" ]] || run_status=1
    exit "$run_status"
  fi

  if [[ "$compose_started" == "1" ]]; then
    "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 ||
      cleanup_failed=1
  fi
  if docker network inspect "$network" >/dev/null 2>&1; then
    docker network rm "$network" >/dev/null 2>&1 || cleanup_failed=1
  fi
  if docker image inspect "$image" >/dev/null 2>&1; then
    docker image rm "$image" >/dev/null 2>&1 || cleanup_failed=1
  fi
  if [[ "$sentinel_created" == "1" ]] &&
     docker network inspect "$sentinel_network" >/dev/null 2>&1; then
    sentinel_survived=true
  fi
  if assert_owned_resources_absent; then
    resources_absent=true
  else
    cleanup_failed=1
  fi
  if [[ "$sentinel_created" == "1" ]]; then
    docker network rm "$sentinel_network" >/dev/null 2>&1 || cleanup_failed=1
  fi

  local cleanup_result="passed"
  [[ "$cleanup_failed" == "0" ]] || cleanup_result="failed"
  write_cleanup_evidence "$cleanup_result" "$sentinel_survived" "$resources_absent"

  if [[ "$runtime_complete" == "1" && -f "$runtime_evidence" ]]; then
    (
      cd "$evidence_dir"
      sha256sum provenance.json runtime.json cleanup.json |
        sort -k2 >"${critical_evidence_manifest}.partial"
      chmod 0600 "${critical_evidence_manifest}.partial"
      mv "${critical_evidence_manifest}.partial" "$critical_evidence_manifest"
    )
  fi

  if [[ "$cleanup_failed" != "0" ]]; then
    run_status=1
  fi
  exit "$run_status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

allocate_ports() {
  perl -MIO::Socket::INET -e '
    @s = map {
      IO::Socket::INET->new(
        LocalAddr => "127.0.0.1",
        LocalPort => 0,
        Proto => "tcp",
        Listen => 1,
      ) or die "loopback port allocation failed: $!\n"
    } 1..4;
    print join(" ", map { $_->sockport } @s), "\n";
  '
}

rpc() {
  local credentials="$1"
  local endpoint="$2"
  local request="$3"
  printf '%s' "$request" |
    curl --config "$credentials" --data-binary @- "${endpoint}/json_rpc"
}

wait_for_rpc() {
  local credentials="$1"
  local endpoint="$2"
  local request="$3"
  local predicate="$4"
  local response=""
  for _ in {1..60}; do
    if response="$(rpc "$credentials" "$endpoint" "$request" 2>/dev/null)" &&
       jq -e "$predicate" <<<"$response" >/dev/null; then
      printf '%s\n' "$response"
      return 0
    fi
    sleep 1
  done
  echo "Monero RPC did not satisfy readiness predicate: ${endpoint}" >&2
  return 1
}

export MONERO_CACHE_DIR="$cache_dir"
export MONERO_BUILD_CONTEXT="$build_context"
export MONERO_PROVENANCE_EVIDENCE="$provenance_evidence"
./scripts/verify-monero-release.sh
provenance_complete_epoch="$(date +%s)"

docker build \
  --file "$dockerfile" \
  --label "org.logos-co.atomic-swaps.run=${run_id}" \
  --label 'org.logos-co.atomic-swaps.scope=monero-regtest-e2e' \
  --label 'org.logos-co.atomic-swaps.component=monero-regtest-image' \
  --tag "$image" \
  "$build_context"

docker network create \
  --driver bridge \
  --opt com.docker.network.bridge.enable_ip_masquerade=false \
  --label "org.logos-co.atomic-swaps.run=${run_id}" \
  --label 'org.logos-co.atomic-swaps.scope=monero-regtest-e2e' \
  --label 'org.logos-co.atomic-swaps.component=monero-regtest-network' \
  "$network" >/dev/null
docker network create \
  --driver bridge \
  --internal \
  --opt com.docker.network.bridge.enable_ip_masquerade=false \
  --label "org.logos-co.atomic-swaps.sentinel-for=${run_id}" \
  "$sentinel_network" >/dev/null
sentinel_created=1

read -r MONERO_DAEMON_HOST_PORT \
  MONERO_FUNDING_WALLET_HOST_PORT \
  MONERO_MAKER_WALLET_HOST_PORT \
  MONERO_TAKER_WALLET_HOST_PORT < <(allocate_ports)
export MONERO_DAEMON_HOST_PORT
export MONERO_FUNDING_WALLET_HOST_PORT
export MONERO_MAKER_WALLET_HOST_PORT
export MONERO_TAKER_WALLET_HOST_PORT
if [[ -z "$MONERO_DAEMON_HOST_PORT" ||
      -z "$MONERO_FUNDING_WALLET_HOST_PORT" ||
      -z "$MONERO_MAKER_WALLET_HOST_PORT" ||
      -z "$MONERO_TAKER_WALLET_HOST_PORT" ]]; then
  echo "failed to allocate four Monero loopback ports" >&2
  exit 1
fi

"${compose[@]}" config --quiet
"${compose[@]}" up --detach
compose_started=1

readonly daemon_endpoint="http://127.0.0.1:${MONERO_DAEMON_HOST_PORT}"
readonly funding_endpoint="http://127.0.0.1:${MONERO_FUNDING_WALLET_HOST_PORT}"
readonly maker_endpoint="http://127.0.0.1:${MONERO_MAKER_WALLET_HOST_PORT}"
readonly taker_endpoint="http://127.0.0.1:${MONERO_TAKER_WALLET_HOST_PORT}"

initial_info="$(wait_for_rpc "${credentials_dir}/daemon.curlrc" "$daemon_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_info"}' \
  '.error == null and .result.status == "OK" and .result.untrusted == false
   and .result.nettype == "fakechain" and .result.mainnet == false
   and .result.testnet == false and .result.stagenet == false
   and .result.offline == true and .result.incoming_connections_count == 0
   and .result.outgoing_connections_count == 0
   and .result.version == "0.18.5.1-release"')"
wait_for_rpc "${credentials_dir}/funding.curlrc" "$funding_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_version"}' \
  '.error == null and .result.release == true and .result.version == 65567' >/dev/null
wait_for_rpc "${credentials_dir}/maker.curlrc" "$maker_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_version"}' \
  '.error == null and .result.release == true and .result.version == 65567' >/dev/null
wait_for_rpc "${credentials_dir}/taker.curlrc" "$taker_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_version"}' \
  '.error == null and .result.release == true and .result.version == 65567' >/dev/null

wrong_actor_http_code="$(
  printf '%s' '{"jsonrpc":"2.0","id":"m4","method":"get_version"}' |
    curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
      --config "${credentials_dir}/maker.curlrc" --data-binary @- \
      "${taker_endpoint}/json_rpc" 2>/dev/null || true
)"
if [[ "$wrong_actor_http_code" != "401" ]]; then
  echo "Maker credentials were not rejected by the Taker wallet RPC" >&2
  exit 1
fi
topology_ready_epoch="$(date +%s)"

create_wallet() {
  local credentials="$1"
  local endpoint="$2"
  local filename="$3"
  local password="$4"
  local request
  request="$(jq -cn \
    --arg filename "$filename" \
    --arg password "$password" \
    '{
      jsonrpc: "2.0",
      id: "m4",
      method: "create_wallet",
      params: {filename: $filename, password: $password, language: "English"}
    }')"
  rpc "$credentials" "$endpoint" "$request" |
    jq -e '.error == null and (.result | type == "object")' >/dev/null
}

create_wallet "${credentials_dir}/funding.curlrc" "$funding_endpoint" \
  funding-regtest "$funding_wallet_password"
create_wallet "${credentials_dir}/maker.curlrc" "$maker_endpoint" \
  maker-regtest "$maker_wallet_password"
create_wallet "${credentials_dir}/taker.curlrc" "$taker_endpoint" \
  taker-regtest "$taker_wallet_password"

funding_address="$(rpc "${credentials_dir}/funding.curlrc" "$funding_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_address"}' |
  jq -er '.result.address')"
maker_address="$(rpc "${credentials_dir}/maker.curlrc" "$maker_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_address"}' |
  jq -er '.result.address')"
taker_address="$(rpc "${credentials_dir}/taker.curlrc" "$taker_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_address"}' |
  jq -er '.result.address')"
if [[ "$funding_address" == "$maker_address" ||
      "$funding_address" == "$taker_address" ||
      "$maker_address" == "$taker_address" ]]; then
  echo "Monero role wallets did not produce distinct addresses" >&2
  exit 1
fi

mine_request="$(jq -cn --arg address "$funding_address" '{
  jsonrpc: "2.0",
  id: "m4",
  method: "generateblocks",
  params: {
    amount_of_blocks: 100,
    wallet_address: $address,
    starting_nonce: 0
  }
}')"
mine_result="$(rpc "${credentials_dir}/daemon.curlrc" "$daemon_endpoint" "$mine_request")"
jq -e '.error == null and .result.status == "OK"
  and (.result.blocks | length) == 100' <<<"$mine_result" >/dev/null

rpc "${credentials_dir}/funding.curlrc" "$funding_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"refresh"}' |
  jq -e '.error == null' >/dev/null
funding_balance="$(rpc "${credentials_dir}/funding.curlrc" "$funding_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_balance"}')"
jq -e --argjson amount "$funding_amount" \
  '.error == null and .result.unlocked_balance > ($amount * 2)' \
  <<<"$funding_balance" >/dev/null

transfer_request="$(jq -cn \
  --arg maker "$maker_address" \
  --arg taker "$taker_address" \
  --argjson amount "$funding_amount" \
  '{
    jsonrpc: "2.0",
    id: "m4",
    method: "transfer",
    params: {
      destinations: [
        {amount: $amount, address: $maker},
        {amount: $amount, address: $taker}
      ],
      account_index: 0,
      priority: 1,
      get_tx_key: false
    }
  }')"
transfer_result="$(rpc "${credentials_dir}/funding.curlrc" "$funding_endpoint" \
  "$transfer_request")"
jq -e --argjson amount "$funding_amount" \
  '.error == null and .result.amount == ($amount * 2)
   and (.result.tx_hash | test("^[0-9a-f]{64}$"))' \
  <<<"$transfer_result" >/dev/null
funding_txid="$(jq -er '.result.tx_hash' <<<"$transfer_result")"
funding_fee="$(jq -er '.result.fee' <<<"$transfer_result")"

confirm_request="$(jq -cn --arg address "$funding_address" \
  --argjson confirmations "$confirmation_policy" '{
    jsonrpc: "2.0",
    id: "m4",
    method: "generateblocks",
    params: {
      amount_of_blocks: $confirmations,
      wallet_address: $address,
      starting_nonce: 100
    }
  }')"
confirm_result="$(rpc "${credentials_dir}/daemon.curlrc" "$daemon_endpoint" \
  "$confirm_request")"
jq -e --argjson confirmations "$confirmation_policy" \
  '.error == null and .result.status == "OK"
   and (.result.blocks | length) == $confirmations' \
  <<<"$confirm_result" >/dev/null

for actor in funding maker taker; do
  credentials="${credentials_dir}/${actor}.curlrc"
  case "$actor" in
    funding) endpoint="$funding_endpoint" ;;
    maker) endpoint="$maker_endpoint" ;;
    taker) endpoint="$taker_endpoint" ;;
  esac
  rpc "$credentials" "$endpoint" \
    '{"jsonrpc":"2.0","id":"m4","method":"refresh"}' |
    jq -e '.error == null' >/dev/null
done

maker_balance="$(rpc "${credentials_dir}/maker.curlrc" "$maker_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_balance"}')"
taker_balance="$(rpc "${credentials_dir}/taker.curlrc" "$taker_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_balance"}')"
for balance in "$maker_balance" "$taker_balance"; do
  jq -e --argjson amount "$funding_amount" \
    '.error == null and .result.balance == $amount
     and .result.unlocked_balance == $amount' <<<"$balance" >/dev/null
done

transfer_lookup_request="$(jq -cn --arg txid "$funding_txid" '{
  jsonrpc: "2.0",
  id: "m4",
  method: "get_transfer_by_txid",
  params: {txid: $txid, account_index: 0}
}')"
maker_transfer="$(rpc "${credentials_dir}/maker.curlrc" "$maker_endpoint" \
  "$transfer_lookup_request")"
taker_transfer="$(rpc "${credentials_dir}/taker.curlrc" "$taker_endpoint" \
  "$transfer_lookup_request")"
for transfer in "$maker_transfer" "$taker_transfer"; do
  jq -e \
    --arg txid "$funding_txid" \
    --argjson amount "$funding_amount" \
    --argjson confirmations "$confirmation_policy" \
    '.error == null
     and .result.transfer.txid == $txid
     and .result.transfer.amount == $amount
     and .result.transfer.confirmations >= $confirmations
     and .result.transfer.locked == false
     and .result.transfer.double_spend_seen == false' \
    <<<"$transfer" >/dev/null
done

final_info="$(rpc "${credentials_dir}/daemon.curlrc" "$daemon_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_info"}')"
daemon_height="$(jq -er '.result.height' <<<"$final_info")"
funding_height="$(rpc "${credentials_dir}/funding.curlrc" "$funding_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_height"}' |
  jq -er '.result.height')"
maker_height="$(rpc "${credentials_dir}/maker.curlrc" "$maker_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_height"}' |
  jq -er '.result.height')"
taker_height="$(rpc "${credentials_dir}/taker.curlrc" "$taker_endpoint" \
  '{"jsonrpc":"2.0","id":"m4","method":"get_height"}' |
  jq -er '.result.height')"
if ! [[ "$daemon_height" == "$funding_height" &&
        "$daemon_height" == "$maker_height" &&
        "$daemon_height" == "$taker_height" ]]; then
  echo "Monero daemon and wallets did not agree on the final height" >&2
  exit 1
fi

monerod_id="$("${compose[@]}" ps --quiet monerod)"
funding_wallet_id="$("${compose[@]}" ps --quiet funding_wallet)"
maker_wallet_id="$("${compose[@]}" ps --quiet maker_wallet)"
taker_wallet_id="$("${compose[@]}" ps --quiet taker_wallet)"
network_id="$(docker network inspect "$network" --format '{{.Id}}')"
image_id="$(docker image inspect "$image" --format '{{.Id}}')"
network_cidr="$(docker network inspect "$network" \
  --format '{{(index .IPAM.Config 0).Subnet}}')"
if [[ -z "$monerod_id" || -z "$funding_wallet_id" ||
      -z "$maker_wallet_id" || -z "$taker_wallet_id" ]]; then
  echo "Monero Compose topology did not expose all four container IDs" >&2
  exit 1
fi
if ! docker network inspect "$network" | jq -e \
  --arg run_id "$run_id" '
    length == 1
    and .[0].Internal == false
    and .[0].Options["com.docker.network.bridge.enable_ip_masquerade"] == "false"
    and .[0].Labels["org.logos-co.atomic-swaps.run"] == $run_id
  ' >/dev/null; then
  echo "Monero private network isolation contract failed" >&2
  exit 1
fi

for tuple in \
  "$monerod_id:18081:${MONERO_DAEMON_HOST_PORT}" \
  "$funding_wallet_id:18083:${MONERO_FUNDING_WALLET_HOST_PORT}" \
  "$maker_wallet_id:18083:${MONERO_MAKER_WALLET_HOST_PORT}" \
  "$taker_wallet_id:18083:${MONERO_TAKER_WALLET_HOST_PORT}"; do
  IFS=: read -r container_id container_port host_port <<<"$tuple"
  docker inspect "$container_id" | jq -e \
    --arg network "$network" \
    --arg image_id "$image_id" \
    --arg port "${container_port}/tcp" \
    --arg host_port "$host_port" '
      length == 1
      and .[0].Image == $image_id
      and .[0].HostConfig.ReadonlyRootfs == true
      and (.[0].HostConfig.CapDrop | index("ALL")) != null
      and (.[0].HostConfig.SecurityOpt | index("no-new-privileges:true")) != null
      and .[0].Config.User == "65532:65532"
      and ((.[0].NetworkSettings.Networks | keys) == [$network])
      and ((.[0].NetworkSettings.Ports | keys) == [$port])
      and (.[0].NetworkSettings.Ports[$port] | length) == 1
      and .[0].NetworkSettings.Ports[$port][0].HostIp == "127.0.0.1"
      and .[0].NetworkSettings.Ports[$port][0].HostPort == $host_port
    ' >/dev/null
done
bootstrap_complete_epoch="$(date +%s)"

{
  printf 'export RUN_ID=%q\n' "$run_id"
  printf 'export MONERO_COMPOSE_PROJECT=%q\n' "$project"
  printf 'export MONERO_COMPOSE_FILE=%q\n' "$(pwd)/${compose_file}"
  printf 'export MONERO_IMAGE=%q\n' "$image"
  printf 'export MONERO_NETWORK=%q\n' "$network"
  printf 'export MONERO_DAEMON_HOST_PORT=%q\n' "$MONERO_DAEMON_HOST_PORT"
  printf 'export MONERO_FUNDING_WALLET_HOST_PORT=%q\n' "$MONERO_FUNDING_WALLET_HOST_PORT"
  printf 'export MONERO_MAKER_WALLET_HOST_PORT=%q\n' "$MONERO_MAKER_WALLET_HOST_PORT"
  printf 'export MONERO_TAKER_WALLET_HOST_PORT=%q\n' "$MONERO_TAKER_WALLET_HOST_PORT"
  printf 'export MONERO_DAEMON_CONFIG=%q\n' "$MONERO_DAEMON_CONFIG"
  printf 'export MONERO_FUNDING_WALLET_CONFIG=%q\n' "$MONERO_FUNDING_WALLET_CONFIG"
  printf 'export MONERO_MAKER_WALLET_CONFIG=%q\n' "$MONERO_MAKER_WALLET_CONFIG"
  printf 'export MONERO_TAKER_WALLET_CONFIG=%q\n' "$MONERO_TAKER_WALLET_CONFIG"
  printf 'export MONERO_DAEMON_ENDPOINT=%q\n' "$daemon_endpoint"
  printf 'export MONERO_FUNDING_WALLET_ENDPOINT=%q\n' "$funding_endpoint"
  printf 'export MONERO_MAKER_WALLET_ENDPOINT=%q\n' "$maker_endpoint"
  printf 'export MONERO_TAKER_WALLET_ENDPOINT=%q\n' "$taker_endpoint"
  printf 'export MONERO_DAEMON_CREDENTIAL_FILE=%q\n' "${credentials_dir}/daemon.curlrc"
  printf 'export MONERO_DAEMON_USERNAME_FILE=%q\n' "${credentials_dir}/daemon.username"
  printf 'export MONERO_DAEMON_PASSWORD_FILE=%q\n' "${credentials_dir}/daemon.password"
  printf 'export MONERO_FUNDING_CREDENTIAL_FILE=%q\n' "${credentials_dir}/funding.curlrc"
  printf 'export MONERO_MAKER_CREDENTIAL_FILE=%q\n' "${credentials_dir}/maker.curlrc"
  printf 'export MONERO_TAKER_CREDENTIAL_FILE=%q\n' "${credentials_dir}/taker.curlrc"
} >"$manifest"
chmod 0600 "$manifest"

runtime_partial="${runtime_evidence}.partial"
jq -n \
  --arg run_id "$run_id" \
  --arg source_version "$MONERO_VERSION" \
  --arg image_id "$image_id" \
  --arg network_id "$network_id" \
  --arg network_cidr "$network_cidr" \
  --arg monerod_id "$monerod_id" \
  --arg funding_wallet_id "$funding_wallet_id" \
  --arg maker_wallet_id "$maker_wallet_id" \
  --arg taker_wallet_id "$taker_wallet_id" \
  --arg daemon_endpoint "$daemon_endpoint" \
  --arg funding_endpoint "$funding_endpoint" \
  --arg maker_endpoint "$maker_endpoint" \
  --arg taker_endpoint "$taker_endpoint" \
  --arg funding_address "$funding_address" \
  --arg maker_address "$maker_address" \
  --arg taker_address "$taker_address" \
  --arg funding_txid "$funding_txid" \
  --argjson funding_fee "$funding_fee" \
  --argjson amount "$funding_amount" \
  --argjson confirmations "$confirmation_policy" \
  --argjson daemon_height "$daemon_height" \
  --argjson initial_height "$(jq -er '.result.height' <<<"$initial_info")" \
  --arg genesis_top_hash "$(jq -er '.result.top_block_hash' <<<"$initial_info")" \
  --argjson provenance_seconds "$((provenance_complete_epoch - run_started_epoch))" \
  --argjson topology_seconds "$((topology_ready_epoch - provenance_complete_epoch))" \
  --argjson bootstrap_seconds "$((bootstrap_complete_epoch - topology_ready_epoch))" \
  --argjson total_seconds "$((bootstrap_complete_epoch - run_started_epoch))" \
  --argjson crossed_actor_http_code "$wrong_actor_http_code" '
  {
    schema_version: 1,
    result: "passed",
    run_id: $run_id,
    milestone: "M4",
    scope: "Monero local-functional Regtest topology bootstrap; not an atomic swap",
    release: {
      version: $source_version,
      image_id: $image_id
    },
    chain: {
      nettype: "fakechain",
      offline: true,
      peers: 0,
      initial_height: $initial_height,
      initial_top_hash: $genesis_top_hash,
      final_height: $daemon_height,
      daemon_wallet_height_agreement: true
    },
    isolation: {
      network_id: $network_id,
      network_cidr: $network_cidr,
      ip_masquerade: false,
      public_p2p_ports: [],
      public_zmq_ports: [],
      rpc_bindings_literal_loopback_only: true,
      distinct_wallet_rpc_credentials: true,
      crossed_maker_to_taker_http_code: $crossed_actor_http_code,
      read_only_roots: true,
      capabilities_dropped: true,
      no_new_privileges: true,
      tmpfs_role_stores: true
    },
    components: [
      {role: "provisioner", kind: "monerod", container_id: $monerod_id, endpoint: $daemon_endpoint},
      {role: "provisioner", kind: "funding-wallet-rpc", container_id: $funding_wallet_id, endpoint: $funding_endpoint},
      {role: "Maker", kind: "wallet-rpc", container_id: $maker_wallet_id, endpoint: $maker_endpoint},
      {role: "Taker", kind: "wallet-rpc", container_id: $taker_wallet_id, endpoint: $taker_endpoint}
    ],
    wallets: {
      funding_address: $funding_address,
      maker_address: $maker_address,
      taker_address: $taker_address,
      distinct_addresses: true
    },
    local_funding: {
      transaction_id: $funding_txid,
      amount_per_role_piconero: $amount,
      fee_piconero: $funding_fee,
      confirmations: $confirmations,
      maker_unlocked: true,
      taker_unlocked: true
    },
    timings_seconds: {
      release_verification: $provenance_seconds,
      image_build_and_topology_readiness: $topology_seconds,
      wallet_bootstrap_and_assertions: $bootstrap_seconds,
      total_before_cleanup: $total_seconds
    },
    runtime_external_resources: [],
    public_rpc_used: false,
    faucet_used: false,
    public_funds_used: false,
    semantic_reproducibility: true,
    cross_run_transaction_hash_reproducibility: false
  }
' >"$runtime_partial"
chmod 0600 "$runtime_partial"
mv "$runtime_partial" "$runtime_evidence"
runtime_complete=1

collect_logs
assert_no_secret_leak

printf 'Monero %s local Regtest topology passed for RUN_ID=%s\n' \
  "$MONERO_VERSION" "$run_id"
printf 'Runtime evidence: %s\n' "$runtime_evidence"
if [[ "$keep_running" == "1" ]]; then
  printf 'Endpoints and credential paths: %s\n' "$manifest"
fi
