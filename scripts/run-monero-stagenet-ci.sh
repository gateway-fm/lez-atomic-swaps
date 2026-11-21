#!/usr/bin/env bash
# A self-hosted Monero Stagenet monerod and wallet-rpc, the pinned 0.18.5.1
# release verified as the Regtest lane verifies it, and the adapter's and the
# SDK's Stagenet tests against them (crates/xmr-monero-adapter/tests/
# stagenet_daemon.rs). The daemon syncs from public Stagenet peers for a
# bounded time and must reach MONERO_STAGENET_SYNC_HEIGHT blocks; a full sync
# (more than two million blocks) does not fit a CI job and is not attempted.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly dockerfile="tests/e2e/monero/Dockerfile"
readonly provenance_file="tests/e2e/monero/provenance.env"
# shellcheck source=tests/e2e/monero/provenance.env
source "$provenance_file"

# Height zero of Monero Stagenet (lez_xmr_monero_adapter::MONERO_STAGENET_GENESIS_HASH).
readonly stagenet_genesis="76ee3cc98646292206cd3e86f74d88b4dcc1d937088645e9b0cbca84b7ce74eb"
# Public Stagenet peers the daemon starts from besides its built-in seeds.
# They are untrusted: monerod checks every block it takes from them against
# the fixed Stagenet genesis, the consensus rules and its checkpoints.
readonly stagenet_peers=(
  node.monerodevs.org:38080
  node2.monerodevs.org:38080
  node.sethforprivacy.com:38080
  stagenet.xmr-tw.org:38080
)

run_id="${RUN_ID:-local-stagenet-$(date -u +%Y%m%d%H%M%S)-$$}"
if [[ ! "$run_id" =~ ^[a-z0-9][a-z0-9_-]{7,63}$ ]]; then
  echo "RUN_ID must be 8..64 lowercase letters, numbers, underscores, or hyphens" >&2
  exit 1
fi
keep_running="${MONERO_STAGENET_KEEP_RUNNING:-0}"
if [[ "$keep_running" != "0" && "$keep_running" != "1" ]]; then
  echo "MONERO_STAGENET_KEEP_RUNNING must be 0 or 1" >&2
  exit 1
fi
sync_height="${MONERO_STAGENET_SYNC_HEIGHT:-1000}"
sync_seconds="${MONERO_STAGENET_SYNC_SECONDS:-900}"
for value in "$sync_height" "$sync_seconds"; do
  if [[ ! "$value" =~ ^[1-9][0-9]{0,6}$ ]]; then
    echo "MONERO_STAGENET_SYNC_HEIGHT and MONERO_STAGENET_SYNC_SECONDS are positive integers" >&2
    exit 1
  fi
done

readonly image="lez-atomic-swaps-monero-stagenet:${run_id}"
readonly network="lez-atomic-swaps-monero-stagenet-${run_id}"
readonly daemon_container="lez-atomic-swaps-monero-stagenet-${run_id}-monerod"
readonly wallet_container="lez-atomic-swaps-monero-stagenet-${run_id}-wallet-rpc"
# The wallet-rpc reaches monerod by this alias on the run's own network. The
# container name is no hostname: with a GitHub RUN_ID it is longer than the
# 63 characters a DNS label may have, and it does not resolve.
readonly daemon_alias="monerod"
repo_root="$(pwd)"
readonly repo_root
readonly run_dir="${repo_root}/.e2e/${run_id}/monero-stagenet"
readonly cache_dir="${MONERO_CACHE_DIR:-${repo_root}/.e2e/cache/monero-${MONERO_VERSION}}"
readonly build_context="${run_dir}/build-context"
readonly config_dir="${run_dir}/configs"
readonly credentials_dir="${run_dir}/credentials"
readonly evidence_dir="${run_dir}/evidence"
readonly logs_dir="${run_dir}/logs"
readonly labels=(
  --label "org.logos-co.atomic-swaps.run=${run_id}"
  --label 'org.logos-co.atomic-swaps.scope=monero-stagenet-ci'
)

for command_name in cargo chmod curl date docker gpg gpgconf jq mkdir openssl perl rg rm sha256sum sleep tar; do
  command -v "$command_name" >/dev/null || {
    echo "missing Monero Stagenet CI tool: ${command_name}" >&2
    exit 1
  }
done
docker info >/dev/null
if [[ -e "$run_dir" || -L "$run_dir" ]]; then
  echo "refusing to reuse Monero Stagenet run state: ${run_dir}" >&2
  exit 1
fi
for container in "$daemon_container" "$wallet_container"; do
  if docker container inspect "$container" >/dev/null 2>&1; then
    echo "refusing to reuse container ${container}" >&2
    exit 1
  fi
done
if docker image inspect "$image" >/dev/null 2>&1 ||
   docker network inspect "$network" >/dev/null 2>&1; then
  echo "refusing to reuse the image or network of run ${run_id}" >&2
  exit 1
fi

mkdir -p "$config_dir" "$credentials_dir" "$evidence_dir" "$logs_dir"
chmod 0700 "$run_dir" "$config_dir" "$credentials_dir" "$evidence_dir" "$logs_dir"

daemon_secret="$(openssl rand -hex 24)"
wallet_secret="$(openssl rand -hex 24)"

write_config() {
  local path="$1"
  shift
  printf '%s\n' "$@" >"$path"
  chmod 0444 "$path"
}

write_curl_config() {
  local path="$1"
  local user="$2"
  local password="$3"
  {
    printf '%s\n' 'digest'
    printf 'user = "%s:%s"\n' "$user" "$password"
    printf '%s\n' 'connect-timeout = 2' 'max-time = 30' 'silent' 'show-error' 'fail-with-body'
    printf '%s\n' 'header = "content-type: application/json"'
  } >"$path"
  chmod 0600 "$path"
}

peer_lines=()
for peer in "${stagenet_peers[@]}"; do
  peer_lines+=("add-peer=${peer}")
done
write_config "${config_dir}/monerod.conf" \
  'stagenet=1' \
  'data-dir=/var/lib/monero' \
  'p2p-bind-ip=0.0.0.0' \
  'p2p-bind-port=38080' \
  'in-peers=0' \
  'out-peers=8' \
  "${peer_lines[@]}" \
  'rpc-bind-ip=0.0.0.0' \
  'rpc-bind-port=38081' \
  'confirm-external-bind=1' \
  "rpc-login=daemon:${daemon_secret}" \
  'rpc-ssl=disabled' \
  'no-zmq=1' \
  'no-igd=1' \
  'check-updates=disabled' \
  'max-concurrency=2' \
  'log-file=/dev/stdout' \
  'log-level=0'
write_config "${config_dir}/wallet.conf" \
  'stagenet=1' \
  'wallet-dir=/var/lib/monero-wallet' \
  'rpc-bind-ip=0.0.0.0' \
  'rpc-bind-port=38083' \
  'confirm-external-bind=1' \
  "rpc-login=operator:${wallet_secret}" \
  'rpc-ssl=disabled' \
  "daemon-address=${daemon_alias}:38081" \
  "daemon-login=daemon:${daemon_secret}" \
  'daemon-ssl=disabled' \
  'trusted-daemon=1' \
  'no-initial-sync=1' \
  'non-interactive=1' \
  'max-concurrency=1' \
  'log-file=/dev/stdout' \
  'log-level=0'
printf 'daemon:%s\n' "$daemon_secret" >"${credentials_dir}/daemon.login"
printf 'operator:%s\n' "$wallet_secret" >"${credentials_dir}/wallet.login"
chmod 0600 "${credentials_dir}/daemon.login" "${credentials_dir}/wallet.login"
write_curl_config "${credentials_dir}/daemon.curlrc" daemon "$daemon_secret"
write_curl_config "${credentials_dir}/wallet.curlrc" operator "$wallet_secret"

collect_logs() {
  local name
  for name in monerod wallet-rpc; do
    if docker container inspect "lez-atomic-swaps-monero-stagenet-${run_id}-${name}" >/dev/null 2>&1; then
      docker logs "lez-atomic-swaps-monero-stagenet-${run_id}-${name}" \
        >"${logs_dir}/${name}.log" 2>&1 || true
    fi
  done
}

cleanup() {
  local status=$?
  local failed=0
  trap - EXIT
  set +e
  collect_logs
  for secret in "$daemon_secret" "$wallet_secret"; do
    if rg -Fq -- "$secret" "$logs_dir" "$evidence_dir"; then
      echo "Monero Stagenet logs or evidence contain a generated secret" >&2
      failed=1
    fi
  done
  # A failed run shows the end of both logs in the job output: nothing else
  # keeps them once the runner is gone. Only logs free of the secrets.
  if [[ "$status" != "0" && "$failed" == "0" ]]; then
    for name in monerod wallet-rpc; do
      if [[ -f "${logs_dir}/${name}.log" ]]; then
        echo "--- last lines of the Stagenet ${name} log ---" >&2
        tail -n 40 "${logs_dir}/${name}.log" >&2
      fi
    done
  fi
  if [[ -d "$build_context" && ! -L "$build_context" ]]; then
    rm -rf -- "$build_context" || failed=1
  fi
  if [[ "$keep_running" == "1" && "$status" == "0" ]]; then
    echo "Monero Stagenet remains running for RUN_ID=${run_id}:"
    echo "  daemon ${daemon_endpoint:-} (login ${credentials_dir}/daemon.login)"
    echo "  wallet-rpc ${wallet_endpoint:-} (login ${credentials_dir}/wallet.login)"
    echo "To remove it: docker rm -f ${daemon_container} ${wallet_container};" \
      "docker network rm ${network}; docker image rm ${image}"
  else
    docker container rm --force "$daemon_container" "$wallet_container" >/dev/null 2>&1
    docker network rm "$network" >/dev/null 2>&1
    docker image rm "$image" >/dev/null 2>&1
    if [[ -n "$(docker container ls --all --quiet \
          --filter "label=org.logos-co.atomic-swaps.run=${run_id}")" ]] ||
       docker network inspect "$network" >/dev/null 2>&1 ||
       docker image inspect "$image" >/dev/null 2>&1; then
      echo "Monero Stagenet CI resources of ${run_id} remain" >&2
      failed=1
    fi
  fi
  [[ "$failed" == "0" ]] || status=1
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

rpc() {
  local credentials="$1"
  local endpoint="$2"
  local request="$3"
  printf '%s' "$request" | curl --config "$credentials" --data-binary @- "${endpoint}/json_rpc"
}

wait_for() {
  local seconds="$1"
  local credentials="$2"
  local endpoint="$3"
  local request="$4"
  local predicate="$5"
  local response=""
  local deadline=$(( $(date +%s) + seconds ))
  while (( $(date +%s) < deadline )); do
    if response="$(rpc "$credentials" "$endpoint" "$request" 2>/dev/null)" &&
       jq -e "$predicate" <<<"$response" >/dev/null; then
      printf '%s\n' "$response"
      return 0
    fi
    sleep 2
  done
  echo "Monero Stagenet RPC did not satisfy ${predicate} within ${seconds} s: ${endpoint}" >&2
  [[ -z "$response" ]] || jq -c '.result // .' <<<"$response" >&2 || true
  return 1
}

allocate_ports() {
  perl -MIO::Socket::INET -e '
    @s = map {
      IO::Socket::INET->new(LocalAddr => "127.0.0.1", LocalPort => 0, Proto => "tcp", Listen => 1)
        or die "loopback port allocation failed: $!\n"
    } 1..2;
    print join(" ", map { $_->sockport } @s), "\n";
  '
}

export MONERO_CACHE_DIR="$cache_dir"
export MONERO_BUILD_CONTEXT="$build_context"
export MONERO_PROVENANCE_EVIDENCE="${evidence_dir}/provenance.json"
./scripts/verify-monero-release.sh

# The verifier took the archive of this host's architecture (MONERO_ARCH), and
# the image is built for the same one (TARGETARCH needs BuildKit).
DOCKER_BUILDKIT=1 docker build --quiet --file "$dockerfile" "${labels[@]}" \
  --tag "$image" "$build_context" >/dev/null
docker network create --driver bridge "${labels[@]}" "$network" >/dev/null

read -r daemon_port wallet_port < <(allocate_ports)
readonly daemon_endpoint="http://127.0.0.1:${daemon_port}"
readonly wallet_endpoint="http://127.0.0.1:${wallet_port}"
hardening=(
  --user 65532:65532 --read-only --cap-drop ALL
  --security-opt no-new-privileges:true --tmpfs '/tmp:rw,noexec,nosuid,size=64m'
)

docker run --detach --name "$daemon_container" --network "$network" \
  --network-alias "$daemon_alias" "${labels[@]}" \
  "${hardening[@]}" --pids-limit 256 --cpus 2 --memory 2g \
  --tmpfs /var/lib/monero:rw,noexec,nosuid,size=2g,mode=1777 \
  --volume "${config_dir}/monerod.conf:/run-config/monerod.conf:ro" \
  --publish "127.0.0.1:${daemon_port}:38081" \
  --entrypoint /usr/local/bin/monerod "$image" \
  --config-file=/run-config/monerod.conf --non-interactive >/dev/null
docker run --detach --name "$wallet_container" --network "$network" "${labels[@]}" \
  "${hardening[@]}" --pids-limit 128 --cpus 1 --memory 1g \
  --tmpfs /var/lib/monero-wallet:rw,noexec,nosuid,size=256m,mode=1777 \
  --volume "${config_dir}/wallet.conf:/run-config/wallet.conf:ro" \
  --publish "127.0.0.1:${wallet_port}:38083" \
  --entrypoint /usr/local/bin/monero-wallet-rpc "$image" \
  --config-file=/run-config/wallet.conf >/dev/null
started_epoch="$(date +%s)"

daemon_rc="${credentials_dir}/daemon.curlrc"
wallet_rc="${credentials_dir}/wallet.curlrc"
info="$(wait_for 120 "$daemon_rc" "$daemon_endpoint" \
  '{"jsonrpc":"2.0","id":"ci","method":"get_info"}' \
  '.error == null and .result.status == "OK" and .result.nettype == "stagenet"
   and .result.stagenet == true and .result.mainnet == false and .result.testnet == false
   and .result.version == "0.18.5.1-release"')"
genesis="$(rpc "$daemon_rc" "$daemon_endpoint" \
  '{"jsonrpc":"2.0","id":"ci","method":"on_get_block_hash","params":[0]}' | jq -er '.result')"
if [[ "$genesis" != "$stagenet_genesis" ]]; then
  echo "monerod --stagenet answered genesis ${genesis}, not the Stagenet genesis" >&2
  exit 1
fi
wait_for 120 "$wallet_rc" "$wallet_endpoint" \
  '{"jsonrpc":"2.0","id":"ci","method":"get_version"}' \
  '.error == null and .result.release == true and .result.version == 65567' >/dev/null
wrong_login="$(printf '%s' '{"jsonrpc":"2.0","id":"ci","method":"get_version"}' |
  curl --silent --output /dev/null --write-out '%{http_code}' --digest \
    --user "operator:${wallet_secret}-wrong" --data-binary @- "${wallet_endpoint}/json_rpc" || true)"
if [[ "$wrong_login" != "401" ]]; then
  echo "the Stagenet wallet-rpc did not refuse a wrong login (HTTP ${wrong_login})" >&2
  exit 1
fi
rpc "$wallet_rc" "$wallet_endpoint" \
  '{"jsonrpc":"2.0","id":"ci","method":"create_wallet","params":{"filename":"ci-role","password":"","language":"English"}}' |
  jq -e '.error == null' >/dev/null
role_address="$(rpc "$wallet_rc" "$wallet_endpoint" \
  '{"jsonrpc":"2.0","id":"ci","method":"get_address","params":{"account_index":0}}' |
  jq -er '.result.address')"
if [[ ! "$role_address" =~ ^5[1-9A-HJ-NP-Za-km-z]{94}$ ]]; then
  echo "the Stagenet wallet-rpc made a non-Stagenet address" >&2
  exit 1
fi
echo "monerod --stagenet ${MONERO_VERSION} and wallet-rpc --stagenet answer; genesis ${genesis}"

MONERO_STAGENET_DAEMON_URL="${daemon_endpoint}/" \
MONERO_STAGENET_DAEMON_LOGIN_FILE="${credentials_dir}/daemon.login" \
MONERO_STAGENET_WALLET_URL="${wallet_endpoint}/" \
MONERO_STAGENET_WALLET_LOGIN_FILE="${credentials_dir}/wallet.login" \
MONERO_STAGENET_ROLE_WALLET=ci-role \
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" \
  cargo test --locked -p lez-xmr-monero-adapter --test stagenet_daemon -- \
    --ignored --nocapture --test-threads=1

synced="$(wait_for "$sync_seconds" "$daemon_rc" "$daemon_endpoint" \
  '{"jsonrpc":"2.0","id":"ci","method":"get_info"}' \
  ".error == null and .result.nettype == \"stagenet\" and .result.outgoing_connections_count > 0
   and .result.height >= ${sync_height}")"
synced_epoch="$(date +%s)"
height="$(jq -r '.result.height' <<<"$synced")"
target="$(jq -r '.result.target_height' <<<"$synced")"
peers="$(jq -r '.result.outgoing_connections_count' <<<"$synced")"
echo "monerod --stagenet synced ${height} blocks from ${peers} public peers in $(( synced_epoch - started_epoch )) s (network tip ${target})"

jq -n \
  --arg run_id "$run_id" \
  --arg version "$(jq -r '.result.version' <<<"$info")" \
  --arg genesis "$genesis" \
  --argjson height "$height" \
  --argjson target "$target" \
  --argjson peers "$peers" \
  --argjson seconds "$(( synced_epoch - started_epoch ))" \
  '{
    schema_version: 1,
    run_id: $run_id,
    scope: "self-hosted Monero Stagenet daemon and wallet-rpc; adapter and SDK Stagenet checks; bounded sync, not a full sync and not a swap",
    monerod_version: $version,
    nettype: "stagenet",
    genesis: $genesis,
    synced_height: $height,
    network_tip_height: $target,
    outgoing_peers: $peers,
    seconds_from_start_to_synced_height: $seconds
  }' >"${evidence_dir}/runtime.json"
echo "Monero Stagenet CI passed for RUN_ID=${run_id}; evidence ${evidence_dir}"
