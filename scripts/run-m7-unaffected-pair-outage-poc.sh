#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export LC_ALL=C
umask 077

fail() {
  printf 'M7 unaffected-pair outage PoC failed: %s\n' "$*" >&2
  exit 1
}

run_id="${RUN_ID:-m7outage-$(date -u +%Y%m%d%H%M%S)-$$}"
[[ "$run_id" =~ ^[a-z0-9][a-z0-9_-]{7,39}$ ]] ||
  fail 'RUN_ID must be 8..40 lowercase safe characters'
readonly run_id
readonly bitcoin_run_id="${run_id}-btc"
readonly proof_root="${M7_OUTAGE_PROOF_ROOT:-/tmp/lez-m7-outage-${run_id}}"
readonly bitcoin_run_root="${PWD}/.e2e/${bitcoin_run_id}/bitcoin-core"
readonly bitcoin_runtime="$bitcoin_run_root/evidence/runtime.json"
readonly bitcoin_manifest="$bitcoin_run_root/run.env"
readonly route_health_config="$proof_root/route-health.json"
readonly bitcoin_healthy="$proof_root/m7-bitcoin-healthy-before-stop.json"
readonly bitcoin_unavailable="$proof_root/m7-bitcoin-unavailable-after-stop.json"
readonly result="$proof_root/result.json"
readonly corridor_evidence="${POC_OUTPUT_ROOT:-/tmp/lez-atomic-swaps-${run_id}}/evidence"

for command_name in awk chmod curl date docker git id install jq mkdir readlink rg sha256sum stat; do
  command -v "$command_name" >/dev/null || fail "missing command ${command_name}"
done
[[ ! -e "$proof_root" && ! -L "$proof_root" ]] ||
  fail "refusing to reuse proof root ${proof_root}"
mkdir -m 0700 "$proof_root"
mkdir -m 0700 "$proof_root/bin"

bitcoin_container=''
bitcoin_image=''
bitcoin_network=''
bitcoin_volume=''
cleanup() {
  local status=$?
  trap - EXIT
  set +e
  if [[ -n "$bitcoin_container" ]]; then
    docker container rm --force "$bitcoin_container" >/dev/null 2>&1
  fi
  if [[ -n "$bitcoin_volume" ]]; then
    docker volume rm "$bitcoin_volume" >/dev/null 2>&1
  fi
  if [[ -n "$bitcoin_network" ]]; then
    docker network rm "$bitcoin_network" >/dev/null 2>&1
  fi
  if [[ -n "$bitcoin_image" ]]; then
    docker image rm "$bitcoin_image" >/dev/null 2>&1
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

RUN_ID="$bitcoin_run_id" BITCOIN_CORE_E2E_MODE=service \
  BITCOIN_CORE_E2E_KEEP_RUNNING=1 ./scripts/run-bitcoin-core-e2e.sh
[[ -s "$bitcoin_runtime" && -s "$bitcoin_manifest" ]] ||
  fail 'Bitcoin service evidence is unavailable'

bitcoin_container="$(jq -er '.isolation.container_id | strings' "$bitcoin_runtime")"
bitcoin_image="$(jq -er '.core.image | strings' "$bitcoin_runtime")"
bitcoin_network="$(jq -er '.isolation.network | strings' "$bitcoin_runtime")"
bitcoin_volume="$(jq -er '.isolation.data_volume | strings' "$bitcoin_runtime")"
bitcoin_rpc_url="$(jq -er '.isolation.rpc_url | strings' "$bitcoin_runtime")"
bitcoin_curl_config="$(jq -er '.actor_rpc.maker_curl_config | strings' "$bitcoin_runtime")"
readonly bitcoin_container bitcoin_image bitcoin_network bitcoin_volume
readonly bitcoin_rpc_url bitcoin_curl_config

jq -e --arg run "$bitcoin_run_id" --arg container "$bitcoin_container" '
  .result == "passed" and .mode == "service"
  and .run_id == $run and .chain.network == "regtest"
  and .chain.genesis == "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206"
  and .chain.network_active == false and .chain.peers_after == 0
  and .isolation.container_id == $container
  and .isolation.rpc_publication == "dynamic_literal_loopback_only"
  and .external_dependencies.runtime_external_resources == []
  and .external_dependencies.public_rpc_used == false
' "$bitcoin_runtime" >/dev/null || fail 'Bitcoin service evidence is not the expected isolated node'

curl --config "$bitcoin_curl_config" \
  --data '{"jsonrpc":"2.0","id":"m7-before-stop","method":"getblockchaininfo","params":[]}' \
  >"$bitcoin_healthy"
chmod 0600 "$bitcoin_healthy"
jq -e '.error == null and .result.chain == "regtest" and .result.blocks == 101' \
  "$bitcoin_healthy" >/dev/null || fail 'Bitcoin node was not semantically healthy before stop'

docker container stop --time 30 "$bitcoin_container" >/dev/null
jq -e --arg run "$bitcoin_run_id" '
  .[0].State.Running == false and .[0].State.Status == "exited"
  and .[0].Config.Labels["org.logos-co.atomic-swaps.run"] == $run
' < <(docker container inspect "$bitcoin_container") >/dev/null ||
  fail 'the exact run-owned Bitcoin container did not stop'

set +e
curl --config "$bitcoin_curl_config" \
  --data '{"jsonrpc":"2.0","id":"m7-after-stop","method":"getblockchaininfo","params":[]}' \
  >"$proof_root/m7-bitcoin-after-stop-unexpected.json" \
  2>"$proof_root/m7-bitcoin-after-stop.stderr"
bitcoin_after_stop_status=$?
set -e
(( bitcoin_after_stop_status != 0 )) || fail 'stopped Bitcoin RPC unexpectedly remained reachable'
jq -n --arg run_id "$bitcoin_run_id" --arg container_id "$bitcoin_container" \
  --arg rpc_url "$bitcoin_rpc_url" --argjson curl_exit "$bitcoin_after_stop_status" \
  --arg healthy_sha256 "$(sha256sum "$bitcoin_healthy" | awk '{print $1}')" '
  {
    schema_version:1,
    result:"passed",
    run_id:$run_id,
    exact_run_owned_container:$container_id,
    rpc_url:$rpc_url,
    healthy_before_stop:true,
    healthy_before_stop_sha256:$healthy_sha256,
    container_running_after_stop:false,
    semantic_rpc_unavailable_after_stop:true,
    curl_exit_after_stop:$curl_exit,
    public_rpc_used:false,
    faucet_used:false
  }
' >"$bitcoin_unavailable"
chmod 0600 "$bitcoin_unavailable"

readonly health_program_source="${PWD}/scripts/probe-local-json-rpc-health.sh"
readonly health_program="$proof_root/bin/probe-local-json-rpc-health"
health_source_sha256="$(sha256sum "$health_program_source" | awk '{print $1}')"
install -m 0500 "$health_program_source" "$health_program"
health_sha256="$(sha256sum "$health_program" | awk '{print $1}')"
readonly health_source_sha256 health_sha256
[[ -x "$health_program" && ! -L "$health_program" \
  && "$health_sha256" =~ ^[0-9a-f]{64}$ ]] ||
  fail 'semantic health-probe executable identity is invalid'
[[ "$health_sha256" == "$health_source_sha256" ]] || fail 'staged health-probe identity changed'
[[ "${ZEBRA_RPC_URL:-}" =~ ^http://127\.0\.0\.1:[1-9][0-9]{0,4}/?$ ]] ||
  fail 'ZEBRA_RPC_URL must identify the fresh surviving loopback node'

jq -n --arg program "$health_program" --arg sha "$health_sha256" \
  --arg zebra "$ZEBRA_RPC_URL" --arg bitcoin_config "$bitcoin_curl_config" '
  {
    schema_version:1,
    commands:[
      {
        route:{pair:"Zcash",direction:"TakerSellsLez"},
        program:$program,
        program_sha256:$sha,
        args:["zcash",$zebra],
        timeout_milliseconds:3000
      },
      {
        route:{pair:"Bitcoin",direction:"TakerSellsForeign"},
        program:$program,
        program_sha256:$sha,
        args:["bitcoin",$bitcoin_config],
        timeout_milliseconds:3000
      }
    ]
  }
' >"$route_health_config"
chmod 0600 "$route_health_config"

export M7_ROUTE_HEALTH_CONFIG="$route_health_config"
export M7_ROUTE_HEALTH_POLL_MILLISECONDS=100
./scripts/run-m5-zec-application-poc.sh

readonly corridor_result="$corridor_evidence/result.json"
readonly before_health="$corridor_evidence/m7-route-health-before-swap.json"
readonly after_health="$corridor_evidence/m7-route-health-after-restart.json"
for path in "$corridor_result" "$before_health" "$after_health"; do
  [[ -s "$path" ]] || fail "missing corridor evidence ${path}"
done
jq -e '
  .result == "completed" and .journey == "claim"
  and .maker_status == "completed" and .taker_status == "completed"
  and .application_plane.enabled == true
  and .effect_owners == {zcash_funder:"maker",lez_claimant:"maker",zcash_claimant:"taker"}
  and .atomic_order_observed == [
    "zcash_funded_and_confirmed",
    "lez_revealing_claim_submitted",
    "zcash_followup_claim_submitted_and_confirmed"
  ]
' "$corridor_result" >/dev/null || fail 'surviving Zcash corridor did not complete atomically'

jq -n --arg run_id "$run_id" --arg commit "$(git rev-parse HEAD)" \
  --arg bitcoin_runtime_sha256 "$(sha256sum "$bitcoin_runtime" | awk '{print $1}')" \
  --arg bitcoin_unavailable_sha256 "$(sha256sum "$bitcoin_unavailable" | awk '{print $1}')" \
  --arg route_health_config_sha256 "$(sha256sum "$route_health_config" | awk '{print $1}')" \
  --arg before_health_sha256 "$(sha256sum "$before_health" | awk '{print $1}')" \
  --arg after_health_sha256 "$(sha256sum "$after_health" | awk '{print $1}')" \
  --arg corridor_result_sha256 "$(sha256sum "$corridor_result" | awk '{print $1}')" '
  {
    schema_version:1,
    kind:"m7_unaffected_pair_actual_node_outage_poc",
    result:"passed",
    run_id:$run_id,
    repository_commit:$commit,
    absent_route:{pair:"Bitcoin",direction:"TakerSellsForeign",actual_local_node:true},
    surviving_route:{pair:"Zcash",direction:"TakerSellsLez",actual_local_node:true},
    absent_route_failed_closed:true,
    route_isolation_survived_maker_restart:true,
    unaffected_pair_swap_completed:true,
    atomic_claim_order_observed:true,
    evidence_sha256:{
      bitcoin_runtime:$bitcoin_runtime_sha256,
      bitcoin_unavailable:$bitcoin_unavailable_sha256,
      route_health_config:$route_health_config_sha256,
      before_swap_health:$before_health_sha256,
      after_restart_health:$after_health_sha256,
      corridor_result:$corridor_result_sha256
    },
    runtime_external_resources:[],
    public_rpc_used:false,
    faucet_used:false,
    public_funds_used:false
  }
' >"$result"
chmod 0600 "$result"
jq -e '.result == "passed" and .unaffected_pair_swap_completed == true
  and .absent_route.actual_local_node == true
  and .surviving_route.actual_local_node == true
  and .runtime_external_resources == []' "$result" >/dev/null

printf '%s\n' "$result"
