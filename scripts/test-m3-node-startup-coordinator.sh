#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly runner="scripts/run-m3-actor-local-poc.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/m3-node-startup-contract.XXXXXX")"
readonly test_root
declare -a test_process_groups=()
lez_pid=""
owned_pid=""
foreign_pid=""
mutated_pid=""
signal_foreign_pid=""

cleanup_test_root() {
  local pid initial_start current_start initial_cmdline current_cmdline process_snapshot
  set +e
  process_snapshot="$(ps -eo pid=,args=)"
  while IFS= read -r pid; do
    [[ "$pid" =~ ^[1-9][0-9]*$ && -r "/proc/${pid}/stat" ]] || continue
    initial_start="$(awk '{print $22}' "/proc/${pid}/stat" 2>/dev/null || true)"
    initial_cmdline="$(tr '\0' ' ' <"/proc/${pid}/cmdline" 2>/dev/null || true)"
    [[ "$initial_start" =~ ^[0-9]+$ && "$initial_cmdline" == *"$test_root"* ]] ||
      continue
    current_start="$(awk '{print $22}' "/proc/${pid}/stat" 2>/dev/null || true)"
    current_cmdline="$(tr '\0' ' ' <"/proc/${pid}/cmdline" 2>/dev/null || true)"
    [[ "$current_start" == "$initial_start" &&
       "$current_cmdline" == *"$test_root"* ]] ||
      continue
    kill -TERM "$pid" 2>/dev/null || true
    for _ in {1..100}; do
      [[ -r "/proc/${pid}/stat" ]] || break
      current_start="$(awk '{print $22}' "/proc/${pid}/stat" 2>/dev/null || true)"
      current_cmdline="$(tr '\0' ' ' <"/proc/${pid}/cmdline" 2>/dev/null || true)"
      [[ "$current_start" == "$initial_start" &&
         "$current_cmdline" == *"$test_root"* ]] ||
        break
      sleep 0.01
    done
    if [[ -r "/proc/${pid}/stat" ]]; then
      current_start="$(awk '{print $22}' "/proc/${pid}/stat" 2>/dev/null || true)"
      current_cmdline="$(tr '\0' ' ' <"/proc/${pid}/cmdline" 2>/dev/null || true)"
      if [[ "$current_start" == "$initial_start" &&
            "$current_cmdline" == *"$test_root"* ]]; then
        kill -KILL "$pid" 2>/dev/null || true
      fi
    fi
    wait "$pid" 2>/dev/null || true
  done < <(awk -v root="$test_root" 'index($0, root) { print $1 }' <<<"$process_snapshot")
  rm -rf -- "$test_root"
}
trap cleanup_test_root EXIT

fail() {
  echo "M3 node-startup coordinator contract failed: $*" >&2
  exit 1
}

extract_function() {
  local name="$1"
  local source
  source="$(sed -n "/^${name}() {$/,/^}$/p" "$runner")"
  [[ -n "$source" ]] || fail "runner is missing ${name}"
  printf '%s\n' "$source"
}

readonly extracted="${test_root}/coordinator-functions.sh"
for function_name in process_matches_registry process_group_matches_registry \
  register_owned_process stop_provisional_owned_process process_group_has_live_members \
  process_group_anchor_matches_registry stop_trusted_process_group_members \
  service_launcher_hashes_stable \
  stop_owned_processes \
  assert_exact_owned_resource \
  single_owned_container_id \
  remove_exact_container_file remove_exact_resource remove_exact_resource_file \
  collect_owned_containers collect_owned_resources \
  reconcile_node_resource_inventories wait_for_node_child wait_for_node_children \
  remove_secure_state_root write_cleanup_attestation cleanup manifest_value \
  start_actual_nodes; do
  extract_function "$function_name" >>"$extracted"
done
# shellcheck source=/dev/null
source "$extracted"
# This contract exercises only fresh, per-run node startup. Attach-mode
# behavior has its own runner contracts and would bypass the child lifecycle.
# shellcheck disable=SC2034 # consumed by the extracted start_actual_nodes function
readonly attach_mode=0

start_source="$(sed -n '/^start_actual_nodes() {$/,/^}$/p' "$runner")"
[[ -n "$start_source" ]] || fail "runner is missing start_actual_nodes"
bitcoin_line="$(rg -n -F '"$bitcoin_service_driver"' <<<"$start_source" | cut -d: -f1)"
lez_line="$(rg -n -F '"$lez_service_driver"' <<<"$start_source" | cut -d: -f1)"
wait_line="$(rg -n -F 'wait_for_node_child' <<<"$start_source" | cut -d: -f1)"
# Attach mode reconciles without launching children; the final occurrence is
# the fresh-start reconciliation that must follow the exact child wait.
reconcile_line="$(rg -n -F 'reconcile_node_resource_inventories' \
  <<<"$start_source" | cut -d: -f1 | tail -n 1)"
[[ "$bitcoin_line" =~ ^[0-9]+$ && "$lez_line" =~ ^[0-9]+$ &&
   "$wait_line" =~ ^[0-9]+$ && "$reconcile_line" =~ ^[0-9]+$ ]] ||
  fail "node startup lacks fixed child, wait, or reconciliation calls"
(( bitcoin_line < wait_line && lez_line < wait_line && wait_line < reconcile_line )) ||
  fail "both fixed launchers must start before exact wait and reconciliation"
[[ "$(rg -Fc 'setsid ' <<<"$start_source")" == 2 ]] ||
  fail "both service launchers must receive distinct exact process groups"
[[ "$(rg -Fc 'register_owned_process node-' <<<"$start_source")" == 2 ]] ||
  fail "both service launchers must be registered immediately"
rg -Fq 'wait_for_node_children' <<<"$start_source" ||
  fail "actual coordinator does not explicitly wait and reap both launchers"
rg -Fq 'register_owned_process node-lez startup' <<<"$start_source" ||
  fail "background LEZ is not registered before foreground Core"
if rg -q 'NODE.*(DRIVER|COMMAND)|eval|bash -c' <<<"$start_source"; then
  fail "node startup admits a command override or string evaluation"
fi
for provenance_term in bitcoin_service_driver_sha_at_start lez_service_driver_sha_at_start \
  'certified_executable_scripts.bitcoin_service_driver.sha256' \
  'certified_executable_scripts.lez_service_driver.sha256'; do
  rg -Fq "$provenance_term" "$runner" ||
    fail "service-launcher provenance omits ${provenance_term}"
done
rg -Fq 'M3_POC_BITCOIN_CONTAINER_ID="$bitcoin_container_id"' "$runner" ||
  fail "actor handoff does not export the parsed Docker container ID"
with_direction_source="$(sed -n '/^with_direction_environment() {$/,/^}$/p' "$runner")"
rg -Fq 'assert_exact_owned_resource container "$bitcoin_container_id"' \
  <<<"$with_direction_source" ||
  fail "actor handoff does not revalidate the parsed container's live labels"
if rg -Fq 'M3_POC_BITCOIN_CONTAINER_ID="$(sed' "$runner"; then
  fail "actor handoff still exports an unparsed inventory record"
fi
rg -Fq 'actual_sid' "$runner" || fail "process registration does not validate SID"
register_source="$(sed -n '/^register_owned_process() {$/,/^}$/p' "$runner")"
for registrar_term in first_observed_start first_observed_ppid \
  expected_parent_pid 'first_observed_ppid" == "$expected_parent_pid"' \
  current_start current_ppid; do
  rg -Fq "$registrar_term" <<<"$register_source" ||
    fail "process registration does not pin and preserve ${registrar_term}"
done
for label_term in \
  'org.logos-co.atomic-swaps.scope=bitcoin-core-regtest-e2e' \
  'org.logos-co.atomic-swaps.component=bitcoin-core-image' \
  'org.logos-co.atomic-swaps.scope=lez-v0.2-local-devnet' \
  'org.logos-co.atomic-swaps.component=lez-v0.2-network' \
  'org.logos-co.atomic-swaps.component=lez-v0.2-image'; do
  rg -Fq "$label_term" scripts/run-bitcoin-core-e2e.sh scripts/run-lez-v02-stack.sh ||
    fail "service resource identity omits ${label_term}"
done

readonly fake_child_script="${test_root}/fake-child.sh"
cat >"$fake_child_script" <<'FAKE_CHILD'
#!/usr/bin/env bash
set -euo pipefail
root="$1"
name="$2"
status="$3"
trap 'printf "terminated\n" >"${root}/${name}.terminated"; exit 143' TERM
printf 'started\n' >"${root}/${name}.started"
if [[ "$name" == owned-INT || "$name" == owned-TERM ]]; then
  bash -c 'trap "" TERM; printf "%s\n" "$$" >"$1"; while :; do sleep 0.05; done' \
    _ "${root}/${name}.grandchild-pid" &
fi
IFS= read -r _ <"${root}/${name}.release" || true
printf 'completed\n' >"${root}/${name}.completed"
exit "$status"
FAKE_CHILD
chmod 0700 "$fake_child_script"

readonly fake_service_common="${test_root}/fake-service-common.sh"
cat >"$fake_service_common" <<'FAKE_SERVICE'
#!/usr/bin/env bash
set -euo pipefail
role="$1"
root="$(dirname "$0")"
base="${RUN_ID%-btc-core}"
[[ "$role" == bitcoin ]] || base="${RUN_ID%-lez-v02}"
state="${root}/service-${RUN_ID}"
other_run="${base}-lez-v02"
[[ "$role" == bitcoin ]] || other_run="${base}-btc-core"
status="$(<"${root}/${base}-${role}.status")"
trap 'printf "terminated\n" >"${state}.terminated"; exit 143' TERM
trap 'printf "interrupted\n" >"${state}.interrupted"; exit 130' INT
if [[ "$role" == bitcoin && -f "${root}/${base}.spawn-grandchild" ]]; then
  bash -c 'trap "" TERM; printf "%s\n" "$$" >"$1"; while :; do sleep 0.05; done' \
    _ "${state}.grandchild-pid" &
  for _ in {1..500}; do
    [[ -f "${state}.grandchild-pid" ]] && break
    sleep 0.01
  done
  [[ -f "${state}.grandchild-pid" ]] || exit 97
fi
printf '%s\n' "$$" >"${state}.pid"
printf 'started\n' >"${state}.started"
if [[ "$role" == bitcoin && -f "${root}/${base}.signal" ]]; then
  signal_name="$(<"${root}/${base}.signal")"
  kill -s "$signal_name" "$PPID"
  printf 'sent\n' >"${state}.signal-sent"
  while :; do sleep 0.05; done
fi
for _ in {1..1000}; do
  [[ -f "${root}/service-${other_run}.started" ]] && break
  sleep 0.01
done
[[ -f "${root}/service-${other_run}.started" ]] || exit 98
printf 'overlap\n' >"${state}.overlap"
if [[ "$role" == bitcoin && -f "${root}/${base}.mutate-registry" ]]; then
  registry="$(<"${root}/${base}.mutate-registry")"
  for _ in {1..500}; do
    [[ -f "$registry" ]] && [[ "$(jq -s 'length' "$registry" 2>/dev/null || true)" == 2 ]] &&
      break
    sleep 0.01
  done
  [[ -f "$registry" && "$(jq -s 'length' "$registry")" == 2 ]] || exit 96
  jq -c '.start_ticks = "0"' "$registry" >"${registry}.mutated"
  mv "${registry}.mutated" "$registry"
fi
if [[ "$status" == 0 ]]; then
  if [[ "$role" == bitcoin ]]; then
    printf 'RUN_ID=%s\nBITCOIN_CORE_E2E_MODE=service\n' "$RUN_ID" >"${root}/${RUN_ID}.env"
  else
    printf 'RUN_ID=%s\nLEZ_V02_SLOT_DURATION_SECONDS=%s\n' \
      "$RUN_ID" "$LEZ_V02_SLOT_DURATION_SECONDS" >"${root}/${RUN_ID}.env"
  fi
  chmod 0600 "${root}/${RUN_ID}.env"
fi
sleep 0.05
printf '%s\n' "$status" >"${state}.completed-status"
exit "$status"
FAKE_SERVICE
chmod 0700 "$fake_service_common"
readonly fake_bitcoin_service="${test_root}/fake-bitcoin-service.sh"
readonly fake_lez_service="${test_root}/fake-lez-service.sh"
cat >"$fake_bitcoin_service" <<FAKE_BITCOIN
#!/usr/bin/env bash
exec "$fake_service_common" bitcoin
FAKE_BITCOIN
cat >"$fake_lez_service" <<FAKE_LEZ
#!/usr/bin/env bash
exec "$fake_service_common" lez
FAKE_LEZ
chmod 0700 "$fake_bitcoin_service" "$fake_lez_service"

bitcoin_service_driver="$fake_bitcoin_service"
lez_service_driver="$fake_lez_service"
# The extracted terminal-provenance function dynamically consumes these globals.
# shellcheck disable=SC2034
bitcoin_service_driver_sha_at_start="$(sha256sum "$bitcoin_service_driver" | sed 's/ .*//')"
# shellcheck disable=SC2034
lez_service_driver_sha_at_start="$(sha256sum "$lez_service_driver" | sed 's/ .*//')"
service_launcher_hashes_stable ||
  fail "unchanged fake service launchers failed the terminal provenance gate"
cp "$fake_bitcoin_service" "${fake_bitcoin_service}.clean"
printf '# terminal drift\n' >>"$fake_bitcoin_service"
if service_launcher_hashes_stable; then
  fail "Bitcoin service-launcher mutation passed terminal publication validation"
fi
mv "${fake_bitcoin_service}.clean" "$fake_bitcoin_service"
cp "$fake_lez_service" "${fake_lez_service}.clean"
printf '# terminal drift\n' >>"$fake_lez_service"
if service_launcher_hashes_stable; then
  fail "LEZ service-launcher mutation passed terminal publication validation"
fi
mv "${fake_lez_service}.clean" "$fake_lez_service"
chmod 0700 "$fake_bitcoin_service" "$fake_lez_service"
service_launcher_hashes_stable ||
  fail "restored fake service launchers failed the terminal provenance gate"

start_fake_child() {
  local name="$1" status="$2" output_variable="$3" pid
  mkfifo -m 0600 "${test_root}/${name}.release"
  setsid "$fake_child_script" "$test_root" "$name" "$status" &
  pid=$!
  test_process_groups+=("$pid")
  printf -v "$output_variable" '%s' "$pid"
}

release_fake_child() {
  local name="$1"
  printf 'release\n' >"${test_root}/${name}.release"
}

untrack_test_process_group() {
  local reaped_pid="$1" tracked_pid
  local -a retained_groups=()
  for tracked_pid in "${test_process_groups[@]}"; do
    [[ "$tracked_pid" == "$reaped_pid" ]] || retained_groups+=("$tracked_pid")
  done
  test_process_groups=("${retained_groups[@]}")
}

wait_for_file() {
  local path="$1"
  for _ in {1..500}; do
    [[ -f "$path" ]] && return 0
    sleep 0.01
  done
  fail "timed out waiting for ${path}"
}

test_bash_executable="$(readlink -f "$(command -v bash)")"
readonly test_bash_executable
process_registry="${test_root}/owned-processes.ndjson"
: >"$process_registry"
chmod 0600 "$process_registry"

start_fake_child success-lez 0 lez_pid
wait_for_file "${test_root}/success-lez.started"
register_owned_process node-lez startup "$lez_pid" "$test_bash_executable" true true ||
  fail "could not register the LEZ fake child"
jq -e --argjson expected_parent "$BASHPID" '
  .ppid == $expected_parent and .pgid == .pid and .sid == .pid
' "$process_registry" >/dev/null ||
  fail "registrar did not bind the direct child to its owning shell and exact session"
bitcoin_status=0
release_fake_child success-lez
lez_status=""
wait_for_node_child "$lez_pid" lez_status || fail "both-success exact LEZ wait failed"
untrack_test_process_group "$lez_pid"
[[ "$bitcoin_status" == 0 && "$lez_status" == 0 ]] ||
  fail "both-success statuses were not retained"
[[ ! -e "/proc/${lez_pid}" ]] || fail "successful LEZ child was not reaped"

start_fake_child sibling-lez 0 lez_pid
wait_for_file "${test_root}/sibling-lez.started"
register_owned_process node-lez startup "$lez_pid" "$test_bash_executable" true true ||
  fail "could not register the successful LEZ sibling"
bitcoin_status=7
kill -0 "$lez_pid" 2>/dev/null || fail "foreground Core failure killed its LEZ sibling"
release_fake_child sibling-lez
lez_status=""
wait_for_node_child "$lez_pid" lez_status || fail "Core-failure path lost successful LEZ wait"
untrack_test_process_group "$lez_pid"
[[ "$bitcoin_status" == 7 && "$lez_status" == 0 ]] ||
  fail "one-child failure lost an exact child status"
[[ -f "${test_root}/sibling-lez.completed" ]] ||
  fail "first failure did not wait for the successful sibling"

start_fake_child failed-lez 9 lez_pid
wait_for_file "${test_root}/failed-lez.started"
register_owned_process node-lez startup "$lez_pid" "$test_bash_executable" true true ||
  fail "could not register the failing LEZ child"
bitcoin_status=0
release_fake_child failed-lez
lez_status=""
if wait_for_node_child "$lez_pid" lez_status; then fail "failing LEZ returned success"; fi
untrack_test_process_group "$lez_pid"
[[ "$bitcoin_status" == 0 && "$lez_status" == 9 ]] ||
  fail "LEZ-failure path lost an exact status"

start_fake_child both-fail-lez 13 lez_pid
wait_for_file "${test_root}/both-fail-lez.started"
register_owned_process node-lez startup "$lez_pid" "$test_bash_executable" true true ||
  fail "could not register the both-fail LEZ child"
bitcoin_status=12
release_fake_child both-fail-lez
lez_status=""
if wait_for_node_child "$lez_pid" lez_status; then fail "both-fail LEZ returned success"; fi
untrack_test_process_group "$lez_pid"
[[ "$bitcoin_status" == 12 && "$lez_status" == 13 ]] ||
  fail "both-fail path lost an exact status"

: >"$process_registry"
start_fake_child owned-stop 0 owned_pid
start_fake_child foreign-stop 0 foreign_pid
wait_for_file "${test_root}/owned-stop.started"
wait_for_file "${test_root}/foreign-stop.started"
register_owned_process node-bitcoin startup "$owned_pid" "$test_bash_executable" true true ||
  fail "could not register the owned stop target"
stop_owned_processes || fail "exact process-group cleanup failed"
untrack_test_process_group "$owned_pid"
wait_for_file "${test_root}/owned-stop.terminated"
kill -0 "$foreign_pid" 2>/dev/null || fail "foreign process-group sentinel was killed"
release_fake_child foreign-stop
wait "$foreign_pid" || true
untrack_test_process_group "$foreign_pid"

: >"$process_registry"
start_fake_child mutated-identity 0 mutated_pid
wait_for_file "${test_root}/mutated-identity.started"
register_owned_process node-lez startup "$mutated_pid" "$test_bash_executable" true true ||
  fail "could not register the identity-mutation sentinel"
jq -c '.start_ticks = "0"' "$process_registry" >"${process_registry}.mutated"
mv "${process_registry}.mutated" "$process_registry"
stop_owned_processes || fail "mismatched registry cleanup returned failure"
kill -0 "$mutated_pid" 2>/dev/null || fail "mismatched process identity was signalled"
release_fake_child mutated-identity
wait "$mutated_pid" || true
untrack_test_process_group "$mutated_pid"

readonly signal_harness="${test_root}/signal-harness.sh"
cat >"$signal_harness" <<'SIGNAL_HARNESS'
#!/usr/bin/env bash
set -euo pipefail
functions_file="$1"
fake_child="$2"
root="$3"
signal_name="$4"
bash_executable="$5"
process_registry="${root}/owned-processes.ndjson"
: >"$process_registry"
chmod 0600 "$process_registry"
# shellcheck source=/dev/null
source "$functions_file"
cleanup_supervisor() {
  local status=$? cleanup_status=0
  trap - EXIT
  set +e
  stop_owned_processes
  cleanup_status=$?
  printf '%s\n' "$cleanup_status" >"${root}/cleanup-status"
  exit "$status"
}
trap cleanup_supervisor EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkfifo -m 0600 "${root}/owned-${signal_name}.release"
setsid "$fake_child" "$root" "owned-${signal_name}" 0 &
child_pid=$!
register_owned_process node-lez startup "$child_pid" "$bash_executable" true true
printf '%s\n' "$child_pid" >"${root}/child-pid"
printf 'ready\n' >"${root}/ready"
while :; do sleep 0.05; done
SIGNAL_HARNESS
chmod 0700 "$signal_harness"

start_fake_child signal-foreign 0 signal_foreign_pid
wait_for_file "${test_root}/signal-foreign.started"
for signal_name in INT TERM; do
  signal_root="${test_root}/signal-${signal_name,,}"
  mkdir -m 0700 "$signal_root"
  signal_status=""
  if timeout --preserve-status --kill-after=20 --signal="$signal_name" 1 \
      "$signal_harness" "$extracted" "$fake_child_script" "$signal_root" \
      "$signal_name" "$test_bash_executable"; then
    fail "${signal_name} supervisor unexpectedly returned success"
  else
    signal_status=$?
  fi
  if [[ "$signal_name" == INT ]]; then
    [[ "$signal_status" == 130 ]] || fail "INT supervisor returned ${signal_status}, expected 130"
  else
    [[ "$signal_status" == 143 ]] || fail "TERM supervisor returned ${signal_status}, expected 143"
  fi
  wait_for_file "${signal_root}/cleanup-status"
  [[ "$(<"${signal_root}/cleanup-status")" == 0 ]] ||
    fail "${signal_name} supervisor cleanup failed"
  signal_child_pid="$(<"${signal_root}/child-pid")"
  signal_grandchild_pid="$(<"${signal_root}/owned-${signal_name}.grandchild-pid")"
  [[ ! -e "/proc/${signal_child_pid}" ]] ||
    fail "${signal_name} supervisor did not reap its exact child"
  [[ ! -e "/proc/${signal_grandchild_pid}" ]] ||
    fail "${signal_name} supervisor left a TERM-ignoring group grandchild"
  [[ -f "${signal_root}/owned-${signal_name}.terminated" ]] ||
    fail "${signal_name} supervisor did not terminate its exact process group"
  kill -0 "$signal_foreign_pid" 2>/dev/null ||
    fail "${signal_name} supervisor killed the foreign sentinel"
done
release_fake_child signal-foreign
wait "$signal_foreign_pid" || true
untrack_test_process_group "$signal_foreign_pid"

false_boolean_registry="${test_root}/false-boolean-processes.ndjson"
jq -nc '{role:"maker",phase:"planning",pid:999999999,start_ticks:"1",
  executable:"/bin/false",group_owned:false,reap_child:false}' \
  >"$false_boolean_registry"
jq -nc '{role:"taker",phase:"planning",pid:999999998,start_ticks:"1",
  executable:"/bin/false"}' >>"$false_boolean_registry"
chmod 0600 "$false_boolean_registry"
process_registry="$false_boolean_registry"
stop_owned_processes ||
  fail "valid false or omitted process-ownership booleans failed cleanup"
invalid_boolean_registry="${test_root}/invalid-boolean-processes.ndjson"
jq -nc '{role:"maker",phase:"planning",pid:999999999,start_ticks:"1",
  executable:"/bin/false",group_owned:"false",reap_child:false}' \
  >"$invalid_boolean_registry"
chmod 0600 "$invalid_boolean_registry"
process_registry="$invalid_boolean_registry"
if stop_owned_processes; then
  fail "non-boolean process ownership passed cleanup"
fi

bitcoin_run_id="startup-test-btc"
lez_run_id="startup-test-lez"
bitcoin_container_ids="${test_root}/bitcoin-containers.ids"
lez_container_ids="${test_root}/lez-containers.ids"
network_resources="${test_root}/owned-networks.tsv"
volume_resources="${test_root}/owned-volumes.tsv"
image_resources="${test_root}/owned-images.tsv"

declare -A fake_lists=()
declare -A fake_runs=()
declare -A fake_scopes=()
declare -A fake_components=()
fake_fail_query=""
fake_remove_log=""
set_fake_identity() {
  local resource="$1" run="$2" scope="$3" component="$4"
  fake_runs["$resource"]="$run"
  fake_scopes["$resource"]="$scope"
  fake_components["$resource"]="$component"
}
fake_lists["container:${bitcoin_run_id}"]="btc-container"
fake_lists["network:${bitcoin_run_id}"]="btc-network"
fake_lists["volume:${bitcoin_run_id}"]="btc-volume"
fake_lists["image:${bitcoin_run_id}"]="btc-image:tag"
fake_lists["container:${lez_run_id}"]=$'lez-bedrock\nlez-indexer\nlez-sequencer'
fake_lists["network:${lez_run_id}"]="lez-network"
fake_lists["volume:${lez_run_id}"]=""
fake_lists["image:${lez_run_id}"]="lez-image:tag"
set_fake_identity btc-container "$bitcoin_run_id" bitcoin-core-regtest-e2e bitcoin-core
set_fake_identity btc-network "$bitcoin_run_id" bitcoin-core-regtest-e2e bitcoin-core-network
set_fake_identity btc-volume "$bitcoin_run_id" bitcoin-core-regtest-e2e bitcoin-core-data
set_fake_identity btc-image:tag "$bitcoin_run_id" bitcoin-core-regtest-e2e bitcoin-core-image
set_fake_identity lez-bedrock "$lez_run_id" lez-v0.2-local-devnet bedrock
set_fake_identity lez-indexer "$lez_run_id" lez-v0.2-local-devnet indexer
set_fake_identity lez-sequencer "$lez_run_id" lez-v0.2-local-devnet sequencer
set_fake_identity lez-network "$lez_run_id" lez-v0.2-local-devnet lez-v0.2-network
set_fake_identity lez-image:tag "$lez_run_id" lez-v0.2-local-devnet lez-v0.2-image

docker() {
  local kind="$1" action="$2"
  shift 2
  if [[ "$action" == ls ]]; then
    local argument child_run="" query_key
    while (( $# > 0 )); do
      argument="$1"
      shift
      if [[ "$argument" == label=org.logos-co.atomic-swaps.run=* ]]; then
        child_run="${argument##*=}"
      fi
    done
    query_key="${kind}:${child_run}"
    [[ "$fake_fail_query" != "$query_key" ]] || return 42
    printf '%s\n' "${fake_lists["${kind}:${child_run}"]:-}" | sed '/^$/d'
    return 0
  fi
  if [[ "$action" == inspect ]]; then
    local resource="${*: -1}"
    [[ -n "${fake_runs[$resource]:-}" ]] || return 1
    printf '%s|%s|%s\n' "${fake_runs[$resource]}" "${fake_scopes[$resource]}" \
      "${fake_components[$resource]}"
    return 0
  fi
  if [[ "$action" == rm ]]; then
    local resource="${*: -1}" key current retained=""
    [[ -z "$fake_remove_log" ]] || printf '%s\n' "$resource" >>"$fake_remove_log"
    for key in "${!fake_lists[@]}"; do
      while IFS= read -r current; do
        [[ -n "$current" && "$current" != "$resource" ]] &&
          retained+="${retained:+$'\n'}${current}"
      done <<<"${fake_lists[$key]}"
      fake_lists["$key"]="$retained"
      retained=""
    done
    unset 'fake_runs[$resource]' 'fake_scopes[$resource]' 'fake_components[$resource]'
    return 0
  fi
  return 1
}

reconcile_node_resource_inventories passed passed ||
  fail "exact success resource reconciliation failed"
[[ "$(wc -l <"$bitcoin_container_ids")" == 1 &&
   "$(wc -l <"$lez_container_ids")" == 3 ]] ||
  fail "success resource reconciliation lost container cardinality"
[[ "$(wc -l <"$network_resources")" == 2 &&
   "$(wc -l <"$volume_resources")" == 1 &&
   "$(wc -l <"$image_resources")" == 2 ]] ||
  fail "success resource reconciliation lost resource cardinality"

container_parser_root="${test_root}/container-parser"
mkdir -m 0700 "$container_parser_root"
printf '%s\t%s\n' b47a11d3deea bitcoin-core >"$container_parser_root/valid.tsv"
chmod 0600 "$container_parser_root/valid.tsv"
[[ "$(single_owned_container_id "$container_parser_root/valid.tsv" bitcoin-core)" == \
    b47a11d3deea ]] ||
  fail "single owned-container parser did not return only the Docker object ID"
full_container_id="b47a11d3deea8915a402601a7db8da28875f64d521c2a625a024c17614f7a0e6"
printf '%s\t%s\n' "$full_container_id" bitcoin-core \
  >"$container_parser_root/valid-full.tsv"
chmod 0600 "$container_parser_root/valid-full.tsv"
[[ "$(single_owned_container_id "$container_parser_root/valid-full.tsv" bitcoin-core)" == \
    "$full_container_id" ]] ||
  fail "single owned-container parser rejected a canonical full Docker object ID"
printf '%s\n' b47a11d3deea >"$container_parser_root/legacy-id-only.tsv"
printf '%s\t%s\n' b47a11d3deea unexpected-component \
  >"$container_parser_root/wrong-component.tsv"
printf '%s\t%s\textra\n' b47a11d3deea bitcoin-core \
  >"$container_parser_root/extra-field.tsv"
printf '%s\t%s\n%s\t%s\n' b47a11d3deea bitcoin-core c47a11d3deea bitcoin-core \
  >"$container_parser_root/duplicate.tsv"
printf '%s\t\t%s\n' b47a11d3deea bitcoin-core \
  >"$container_parser_root/double-tab.tsv"
printf '\t%s\t%s\n' b47a11d3deea bitcoin-core \
  >"$container_parser_root/leading-tab.tsv"
printf '%s\t%s\t\n' b47a11d3deea bitcoin-core \
  >"$container_parser_root/trailing-tab.tsv"
printf '%s\t%s\r\n' b47a11d3deea bitcoin-core \
  >"$container_parser_root/crlf.tsv"
printf '%s\t%s\njunk' b47a11d3deea bitcoin-core \
  >"$container_parser_root/trailing-bytes.tsv"
printf '%s\t%s\n\n' b47a11d3deea bitcoin-core \
  >"$container_parser_root/extra-blank-line.tsv"
printf '%s\t%s' b47a11d3deea bitcoin-core \
  >"$container_parser_root/no-final-newline.tsv"
printf '%s\t%s\n' --help bitcoin-core >"$container_parser_root/option-id.tsv"
printf '%s\t%s\n' not-hex bitcoin-core >"$container_parser_root/nonhex-id.tsv"
printf '%s\t%s\n' b47a11d3deea8 bitcoin-core \
  >"$container_parser_root/noncanonical-id-length.tsv"
: >"$container_parser_root/empty.tsv"
for invalid_inventory in legacy-id-only wrong-component extra-field duplicate double-tab \
  leading-tab trailing-tab crlf trailing-bytes extra-blank-line no-final-newline option-id \
  nonhex-id noncanonical-id-length empty; do
  chmod 0600 "$container_parser_root/${invalid_inventory}.tsv"
  invalid_output=""
  if invalid_output="$(single_owned_container_id \
      "$container_parser_root/${invalid_inventory}.tsv" bitcoin-core 2>/dev/null)"; then
    fail "single owned-container parser accepted ${invalid_inventory} inventory"
  fi
  [[ -z "$invalid_output" ]] ||
    fail "single owned-container parser emitted output for ${invalid_inventory} inventory"
done
printf '%s\t%s\n' b47a11d3deea bitcoin-core >"$container_parser_root/wrong-mode.tsv"
chmod 0644 "$container_parser_root/wrong-mode.tsv"
if single_owned_container_id "$container_parser_root/wrong-mode.tsv" \
    bitcoin-core >/dev/null 2>&1; then
  fail "single owned-container parser accepted a non-private inventory"
fi
ln -s valid.tsv "$container_parser_root/symlink.tsv"
if single_owned_container_id "$container_parser_root/symlink.tsv" \
    bitcoin-core >/dev/null 2>&1; then
  fail "single owned-container parser accepted a symlink inventory"
fi
mkdir "$container_parser_root/directory.tsv"
if single_owned_container_id "$container_parser_root/directory.tsv" \
    bitcoin-core >/dev/null 2>&1; then
  fail "single owned-container parser accepted a directory inventory"
fi
if single_owned_container_id "$container_parser_root/missing.tsv" \
    bitcoin-core >/dev/null 2>&1; then
  fail "single owned-container parser accepted a missing inventory"
fi

fake_lists["container:${bitcoin_run_id}"]=""
fake_lists["network:${bitcoin_run_id}"]=""
fake_lists["volume:${bitcoin_run_id}"]=""
fake_lists["image:${bitcoin_run_id}"]=""
reconcile_node_resource_inventories failed passed ||
  fail "bounded failed-child reconciliation rejected zero Bitcoin resources"

fake_lists["container:${bitcoin_run_id}"]="btc-container"
fake_lists["network:${bitcoin_run_id}"]="btc-network"
fake_lists["volume:${bitcoin_run_id}"]="btc-volume"
fake_lists["image:${bitcoin_run_id}"]="btc-image:tag"
fake_lists["container:${lez_run_id}"]="lez-bedrock"
fake_lists["network:${lez_run_id}"]="lez-network"
fake_lists["volume:${lez_run_id}"]=""
fake_lists["image:${lez_run_id}"]="lez-image:tag"
reconcile_node_resource_inventories passed failed ||
  fail "bounded failed-LEZ reconciliation rejected exact partial resources"

for child_run in "$bitcoin_run_id" "$lez_run_id"; do
  fake_lists["container:${child_run}"]=""
  fake_lists["network:${child_run}"]=""
  fake_lists["volume:${child_run}"]=""
  fake_lists["image:${child_run}"]=""
done
reconcile_node_resource_inventories failed failed ||
  fail "both-failed reconciliation rejected zero exact resources"

fake_lists["container:${bitcoin_run_id}"]="btc-container"
fake_lists["network:${bitcoin_run_id}"]="btc-network"
fake_lists["volume:${bitcoin_run_id}"]="btc-volume"
fake_lists["image:${bitcoin_run_id}"]="btc-image:tag"
fake_lists["container:${lez_run_id}"]=$'lez-bedrock\nlez-indexer\nlez-sequencer'
fake_lists["network:${lez_run_id}"]="lez-network"
fake_lists["volume:${lez_run_id}"]=""
fake_lists["image:${lez_run_id}"]="lez-image:tag"
fake_fail_query="volume:${lez_run_id}"
if reconcile_node_resource_inventories passed passed; then
  fail "expected-zero LEZ volume inventory masked a Docker query failure"
fi
fake_fail_query="container:${bitcoin_run_id}"
if reconcile_node_resource_inventories failed passed; then
  fail "failed-child inventory masked a Docker query failure"
fi
fake_fail_query=""

fake_lists["container:${lez_run_id}"]=$'lez-bedrock\nlez-bedrock'
fake_lists["network:${lez_run_id}"]=""
fake_lists["image:${lez_run_id}"]=""
if reconcile_node_resource_inventories passed failed; then
  fail "failed-child reconciliation accepted a duplicate within its count bound"
fi

fake_lists["container:${lez_run_id}"]=$'lez-bedrock\nlez-indexer\nlez-sequencer'
fake_lists["network:${lez_run_id}"]="lez-network"
fake_lists["image:${lez_run_id}"]="lez-image:tag"
fake_lists["container:${bitcoin_run_id}"]=$'btc-container\nbtc-container-extra'
set_fake_identity btc-container-extra "$bitcoin_run_id" bitcoin-core-regtest-e2e bitcoin-core
if reconcile_node_resource_inventories failed passed; then
  fail "failed-child reconciliation accepted an over-count"
fi
[[ "$(wc -l <"$bitcoin_container_ids")" -ge 2 ]] ||
  fail "over-count discovery did not retain every verified Bitcoin container for cleanup"

fake_lists["container:${bitcoin_run_id}"]="foreign-container"
set_fake_identity foreign-container foreign-run bitcoin-core-regtest-e2e bitcoin-core
if reconcile_node_resource_inventories failed passed; then
  fail "failed-child reconciliation accepted a foreign label"
fi
[[ "${fake_runs[foreign-container]}" == foreign-run ]] ||
  fail "foreign resource sentinel was mutated"

for child_run in "$bitcoin_run_id" "$lez_run_id"; do
  fake_lists["container:${child_run}"]=""
  fake_lists["network:${child_run}"]=""
  fake_lists["volume:${child_run}"]=""
  fake_lists["image:${child_run}"]=""
done
# These globals are consumed by the extracted cleanup-attestation function.
# shellcheck disable=SC2034
run_id="startup-attestation"
# shellcheck disable=SC2034
journey="claim"
# shellcheck disable=SC2034
secure_state_root="${test_root}/absent-secure-state"
cleanup_attestation="${test_root}/cleanup-attestation.json"
fake_fail_query="volume:${lez_run_id}"
if write_cleanup_attestation failed; then
  fail "cleanup attestation masked an expected-zero Docker query failure"
fi
[[ ! -e "$cleanup_attestation" ]] ||
  fail "cleanup attestation published false absence after a query failure"
fake_fail_query=""
write_cleanup_attestation passed || fail "clean empty-resource attestation failed"
jq -e '.result == "passed" and .all_exact_run_resources_absent == true' \
  "$cleanup_attestation" >/dev/null ||
  fail "clean empty-resource attestation did not prove exact absence"

configure_behavior_resources() {
  local btc_container="container-$bitcoin_run_id"
  local btc_network="network-$bitcoin_run_id"
  local btc_volume="volume-$bitcoin_run_id"
  local btc_image="image-$bitcoin_run_id:tag"
  local lez_bedrock="bedrock-$lez_run_id"
  local lez_indexer="indexer-$lez_run_id"
  local lez_sequencer="sequencer-$lez_run_id"
  local lez_network="network-$lez_run_id"
  local lez_image="image-$lez_run_id:tag"
  fake_lists=()
  fake_runs=()
  fake_scopes=()
  fake_components=()
  fake_fail_query=""
  fake_lists["container:$bitcoin_run_id"]="$btc_container"
  fake_lists["network:$bitcoin_run_id"]="$btc_network"
  fake_lists["volume:$bitcoin_run_id"]="$btc_volume"
  fake_lists["image:$bitcoin_run_id"]="$btc_image"
  fake_lists["container:$lez_run_id"]="$lez_bedrock"$'\n'"$lez_indexer"$'\n'"$lez_sequencer"
  fake_lists["network:$lez_run_id"]="$lez_network"
  fake_lists["volume:$lez_run_id"]=""
  fake_lists["image:$lez_run_id"]="$lez_image"
  set_fake_identity "$btc_container" "$bitcoin_run_id" \
    bitcoin-core-regtest-e2e bitcoin-core
  set_fake_identity "$btc_network" "$bitcoin_run_id" \
    bitcoin-core-regtest-e2e bitcoin-core-network
  set_fake_identity "$btc_volume" "$bitcoin_run_id" \
    bitcoin-core-regtest-e2e bitcoin-core-data
  set_fake_identity "$btc_image" "$bitcoin_run_id" \
    bitcoin-core-regtest-e2e bitcoin-core-image
  set_fake_identity "$lez_bedrock" "$lez_run_id" lez-v0.2-local-devnet bedrock
  set_fake_identity "$lez_indexer" "$lez_run_id" lez-v0.2-local-devnet indexer
  set_fake_identity "$lez_sequencer" "$lez_run_id" lez-v0.2-local-devnet sequencer
  set_fake_identity "$lez_network" "$lez_run_id" \
    lez-v0.2-local-devnet lez-v0.2-network
  set_fake_identity "$lez_image" "$lez_run_id" lez-v0.2-local-devnet lez-v0.2-image
  set_fake_identity behavior-foreign foreign-run foreign-scope foreign-component
}

# These globals are consumed dynamically by the extracted production functions.
# shellcheck disable=SC2034
prepare_behavior_case() {
  local label="$1"
  local bitcoin_exit="$2"
  local lez_exit="$3"
  local suffix="${test_root##*.}"
  local case_root="$test_root/behavior-$label"
  run_id="behavior-$label-$suffix"
  journey=claim
  bitcoin_run_id="$run_id-btc-core"
  lez_run_id="$run_id-lez-v02"
  evidence_dir="$case_root/evidence"
  bitcoin_manifest="$test_root/$bitcoin_run_id.env"
  lez_manifest="$test_root/$lez_run_id.env"
  bitcoin_container_ids="$case_root/bitcoin-containers.tsv"
  lez_container_ids="$case_root/lez-containers.tsv"
  network_resources="$case_root/networks.tsv"
  volume_resources="$case_root/volumes.tsv"
  image_resources="$case_root/images.tsv"
  process_registry="$case_root/processes.ndjson"
  cleanup_attestation="$evidence_dir/cleanup-attestation.json"
  secure_state_root="/tmp/lez-atomic-swaps-m3-$run_id-secure-state"
  bitcoin_service_driver="$fake_bitcoin_service"
  lez_service_driver="$fake_lez_service"
  lez_source_dir="$test_root/fake-lez-source"
  lez_slot_duration_seconds=1.0
  mkdir -m 0700 "$case_root" "$evidence_dir" "$secure_state_root"
  : >"$process_registry"
  chmod 0600 "$process_registry"
  jq -n '{account_id:"maker",vault_account_id:"maker-vault"}' \
    >"$evidence_dir/maker-lez-identity.json"
  jq -n '{account_id:"taker",vault_account_id:"taker-vault"}' \
    >"$evidence_dir/taker-lez-identity.json"
  chmod 0600 "$evidence_dir/maker-lez-identity.json" \
    "$evidence_dir/taker-lez-identity.json"
  printf '%s\n' "$bitcoin_exit" >"$test_root/$run_id-bitcoin.status"
  printf '%s\n' "$lez_exit" >"$test_root/$run_id-lez.status"
  fake_remove_log="$case_root/removed-resources"
  : >"$fake_remove_log"
  configure_behavior_resources
}

assert_behavior_cleanup() {
  local label="$1"
  local child_pid
  jq -e '
    .result == "passed"
    and .all_exact_run_resources_absent == true
    and .foreign_resources_targeted == false
  ' "$cleanup_attestation" >/dev/null ||
    fail "behavior $label did not attest exact cleanup"
  [[ "$(wc -l <"$fake_remove_log")" == 9 ]] ||
    fail "behavior $label did not remove all nine exact resources"
  ! rg -Fxq behavior-foreign "$fake_remove_log" ||
    fail "behavior $label targeted the foreign sentinel"
  while IFS= read -r child_pid; do
    [[ ! -e "/proc/$child_pid" ]] ||
      fail "behavior $label left registered launcher $child_pid alive"
  done < <(jq -r '.pid' "$process_registry")
}

run_behavior_case() {
  local label="$1"
  local bitcoin_exit="$2"
  local lez_exit="$3"
  local expected_status="$4"
  local actual_status=0
  local expected_role_status role service_run
  local case_root="$test_root/behavior-$label"
  prepare_behavior_case "$label" "$bitcoin_exit" "$lez_exit"
  if (
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    start_actual_nodes
  ) >"$case_root/supervisor.log" 2>&1; then
    actual_status=0
  else
    actual_status=$?
  fi
  if [[ "$actual_status" != "$expected_status" ]]; then
    sed 's/^/behavior supervisor: /' "$case_root/supervisor.log" >&2
    sed 's/^/behavior Bitcoin: /' "$evidence_dir/bitcoin-service.log" >&2
    sed 's/^/behavior LEZ: /' "$evidence_dir/lez-service.log" >&2
    find "$test_root" -maxdepth 1 -type f -name 'service-*' -printf \
      'behavior state: %f\n' >&2
    fail "behavior $label returned $actual_status, expected $expected_status"
  fi
  jq -s -e '
    length == 2
    and ([.[].role] | sort) == ["node-bitcoin","node-lez"]
    and all(.[]; .group_owned == true and .reap_child == true
      and .pgid == .pid and .sid == .pid)
  ' "$process_registry" >/dev/null ||
    fail "behavior $label did not register both exact launcher sessions"
  jq -e --argjson bitcoin_status "$bitcoin_exit" --argjson lez_status "$lez_exit" '
    .bitcoin_status == $bitcoin_status
    and .lez_status == $lez_status
    and .bitcoin_registered == true
    and .lez_registered == true
    and .both_children_waited_and_reaped == true
    and .exact_process_groups_absent_after_wait == true
    and .inventory_reconciled == true
  ' "$evidence_dir/node-startup-status.json" >/dev/null ||
    fail "behavior $label lost exact launcher statuses"
  for role in bitcoin lez; do
    expected_role_status="$lez_exit"
    [[ "$role" != bitcoin ]] || expected_role_status="$bitcoin_exit"
    if [[ "$role" == bitcoin ]]; then
      service_run="$bitcoin_run_id"
    else
      service_run="$lez_run_id"
    fi
    [[ -f "$test_root/service-$service_run.overlap" ]] ||
      fail "behavior $label did not overlap both launchers"
    [[ "$(<"$test_root/service-$service_run.completed-status")" == \
       "$expected_role_status" ]] ||
      fail "behavior $label lost the $role completion status"
  done
  assert_behavior_cleanup "$label"
}

run_behavior_inventory_failure_case() {
  local mode="$1"
  local label="inventory-$mode"
  local case_root="$test_root/behavior-$label"
  local actual_status=0 bitcoin_container extra_container
  prepare_behavior_case "$label" 0 0
  case "$mode" in
    overcount)
      extra_container="extra-container-$bitcoin_run_id"
      fake_lists["container:$bitcoin_run_id"]+=$'\n'"$extra_container"
      set_fake_identity "$extra_container" "$bitcoin_run_id" \
        bitcoin-core-regtest-e2e bitcoin-core
      ;;
    component)
      bitcoin_container="${fake_lists["container:$bitcoin_run_id"]}"
      fake_components["$bitcoin_container"]=wrong-bitcoin-component
      ;;
    query)
      fake_fail_query="volume:$lez_run_id"
      ;;
    *)
      fail "unknown behavior inventory failure mode: $mode"
      ;;
  esac
  if (
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    start_actual_nodes
  ) >"$case_root/supervisor.log" 2>&1; then
    actual_status=0
  else
    actual_status=$?
  fi
  [[ "$actual_status" == 1 ]] ||
    fail "behavior inventory $mode returned $actual_status, expected 1"
  jq -e '
    .bitcoin_status == 0
    and .lez_status == 0
    and .bitcoin_registered == true
    and .lez_registered == true
    and .both_children_waited_and_reaped == true
    and .inventory_reconciled == false
  ' "$evidence_dir/node-startup-status.json" >/dev/null ||
    fail "behavior inventory $mode did not fail closed after exact child success"
  jq -s -e '
    length == 2
    and all(.[]; .group_owned == true and .reap_child == true
      and .pgid == .pid and .sid == .pid)
  ' "$process_registry" >/dev/null ||
    fail "behavior inventory $mode lost exact launcher registration"
  case "$mode" in
    overcount)
      [[ "$(wc -l <"$fake_remove_log")" == 10 ]] ||
        fail "behavior overcount did not remove every individually safe exact resource"
      rg -Fxq "$extra_container" "$fake_remove_log" ||
        fail "behavior overcount did not retain the extra exact resource for cleanup"
      jq -e '
        .result == "failed"
        and .all_exact_run_resources_absent == true
        and .foreign_resources_targeted == false
      ' "$cleanup_attestation" >/dev/null ||
        fail "behavior overcount cleanup attestation is incorrect"
      ;;
    component)
      [[ "$(wc -l <"$fake_remove_log")" == 8 ]] ||
        fail "behavior component mismatch targeted an unverified resource"
      ! rg -Fxq "$bitcoin_container" "$fake_remove_log" ||
        fail "behavior component mismatch removed the wrong-component resource"
      jq -e '
        .result == "failed"
        and .all_exact_run_resources_absent == false
        and .foreign_resources_targeted == false
      ' "$cleanup_attestation" >/dev/null ||
        fail "behavior component mismatch attestation masked the retained resource"
      ;;
    query)
      [[ "$(wc -l <"$fake_remove_log")" == 9 ]] ||
        fail "behavior query failure lost individually verified cleanup resources"
      [[ ! -e "$cleanup_attestation" ]] ||
        fail "behavior query failure published an unverified absence attestation"
      rg -Fq 'could not publish cleanup attestation' "$case_root/supervisor.log" ||
        fail "behavior query failure did not surface the attestation write failure"
      ;;
  esac
  ! rg -Fxq behavior-foreign "$fake_remove_log" ||
    fail "behavior inventory $mode targeted the foreign sentinel"
}

run_behavior_registration_failure_case() {
  local signal_name="$1"
  local expected_status="$2"
  local label="registration-failure"
  local actual_status=0 case_root core_pid="" grandchild_pid="" state process_snapshot
  [[ "$signal_name" == NONE ]] || label+="-${signal_name,,}"
  case_root="$test_root/behavior-$label"
  prepare_behavior_case "$label" 0 0
  if [[ "$signal_name" != NONE ]]; then
    printf '%s\n' "$signal_name" >"$test_root/$run_id.signal"
    : >"$test_root/$run_id.spawn-grandchild"
  fi
  if (
    jq() {
      local arguments=" $* "
      if [[ "${1:-}" == -nc &&
            "$arguments" == *' --arg role node-bitcoin '* ]]; then
        if [[ "$signal_name" != NONE ]]; then
          for _ in {1..500}; do
            [[ -f "$test_root/service-$bitcoin_run_id.signal-sent" ]] && break
            sleep 0.01
          done
          [[ -f "$test_root/service-$bitcoin_run_id.signal-sent" ]] || return 99
        fi
        return 1
      fi
      command jq "$@"
    }
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    start_actual_nodes
  ) >"$case_root/supervisor.log" 2>&1; then
    actual_status=0
  else
    actual_status=$?
  fi
  [[ "$actual_status" == "$expected_status" ]] ||
    fail "registration failure $signal_name returned $actual_status, expected $expected_status"
  [[ ! -e "$test_root/service-$lez_run_id.started" ]] ||
    fail "registration failure $signal_name launched LEZ after Core ownership failed"
  if [[ -f "$test_root/service-$bitcoin_run_id.pid" ]]; then
    core_pid="$(<"$test_root/service-$bitcoin_run_id.pid")"
    [[ ! -e "/proc/$core_pid" ]] ||
      fail "registration failure $signal_name left the unregistered Core leader alive"
  fi
  if [[ -f "$test_root/service-$bitcoin_run_id.grandchild-pid" ]]; then
    grandchild_pid="$(<"$test_root/service-$bitcoin_run_id.grandchild-pid")"
    state="$(awk '{print $3}' "/proc/$grandchild_pid/stat" 2>/dev/null || true)"
    [[ ! -e "/proc/$grandchild_pid" || "$state" == Z ]] ||
      fail "registration failure $signal_name left an unregistered group member live"
  fi
  process_snapshot="$(ps -eo args=)"
  ! rg -Fq "$test_root/service-$bitcoin_run_id" <<<"$process_snapshot" ||
    fail "registration failure $signal_name left a process from the exact fake session"
  jq -e '
    .result == "passed"
    and .all_exact_run_resources_absent == true
    and .foreign_resources_targeted == false
  ' "$cleanup_attestation" >/dev/null ||
    fail "registration failure $signal_name did not attest exact resource cleanup"
  [[ "$(wc -l <"$fake_remove_log")" == 9 ]] ||
    fail "registration failure $signal_name lost exact resource cleanup"
}

run_behavior_leader_first_case() {
  local label=leader-first
  local case_root="$test_root/behavior-$label"
  local actual_status=0 core_pgid grandchild_pid state membership_status=0
  prepare_behavior_case "$label" 0 0
  : >"$test_root/$run_id.spawn-grandchild"
  printf '%s\n' "$process_registry" >"$test_root/$run_id.mutate-registry"
  if (
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    start_actual_nodes
  ) >"$case_root/supervisor.log" 2>&1; then
    actual_status=0
  else
    actual_status=$?
  fi
  [[ "$actual_status" == 0 ]] ||
    fail "leader-first authentic coordinator returned $actual_status"
  jq -s -e '
    length == 2
    and all(.[]; .start_ticks == "0" and .pgid == .pid and .sid == .pid)
  ' "$process_registry" >/dev/null ||
    fail "leader-first regression did not mutate the file-only registry identity"
  jq -e '
    .bitcoin_status == 0
    and .lez_status == 0
    and .both_children_waited_and_reaped == true
    and .exact_process_groups_absent_after_wait == true
  ' "$evidence_dir/node-startup-status.json" >/dev/null ||
    fail "leader-first coordinator did not attest in-memory session drain"
  core_pgid="$(jq -er 'select(.role == "node-bitcoin") | .pgid' "$process_registry")"
  process_group_has_live_members "$core_pgid" "$core_pgid" || membership_status=$?
  [[ "$membership_status" == 1 ]] ||
    fail "leader-first coordinator left a live exact group member"
  grandchild_pid="$(<"$test_root/service-$bitcoin_run_id.grandchild-pid")"
  state="$(awk '{print $3}' "/proc/$grandchild_pid/stat" 2>/dev/null || true)"
  [[ ! -e "/proc/$grandchild_pid" || "$state" == Z ]] ||
    fail "leader-first coordinator left the TERM-ignoring grandchild live"
  assert_behavior_cleanup "$label"
}

run_behavior_signal_case() {
  local signal_name="$1"
  local expected_status="$2"
  local label="signal-${signal_name,,}"
  local case_root="$test_root/behavior-$label"
  local actual_status=0 grandchild_pid grandchild_state core_pgid membership_status
  prepare_behavior_case "$label" 0 0
  printf '%s\n' "$signal_name" >"$test_root/$run_id.signal"
  : >"$test_root/$run_id.spawn-grandchild"
  if (
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    start_actual_nodes
  ) >"$case_root/supervisor.log" 2>&1; then
    actual_status=0
  else
    actual_status=$?
  fi
  if [[ "$actual_status" != "$expected_status" ]]; then
    sed 's/^/behavior supervisor: /' "$case_root/supervisor.log" >&2
    fail "behavior $signal_name returned $actual_status, expected $expected_status"
  fi
  jq -s -e '
    length == 2
    and ([.[].role] | sort) == ["node-bitcoin","node-lez"]
    and all(.[]; .group_owned == true and .reap_child == true
      and .pgid == .pid and .sid == .pid)
  ' "$process_registry" >/dev/null ||
    fail "behavior $signal_name exposed the spawn-to-registration race"
  grandchild_pid="$(<"$test_root/service-$bitcoin_run_id.grandchild-pid")"
  for _ in {1..500}; do
    [[ ! -e "/proc/$grandchild_pid" ]] && break
    sleep 0.01
  done
  grandchild_state="$(awk '{print $3}' "/proc/$grandchild_pid/stat" 2>/dev/null || true)"
  [[ ! -e "/proc/$grandchild_pid" || "$grandchild_state" == Z ]] ||
    fail "behavior $signal_name left the TERM-ignoring group grandchild live"
  core_pgid="$(jq -er 'select(.role == "node-bitcoin") | .pgid' "$process_registry")"
  membership_status=0
  process_group_has_live_members "$core_pgid" "$core_pgid" || membership_status=$?
  [[ "$membership_status" == 1 ]] ||
    fail "behavior $signal_name did not prove the exact service group has no live member"
  assert_behavior_cleanup "$label"
}

run_behavior_case success 0 0 0
run_behavior_case bitcoin-fails 7 0 1
run_behavior_case lez-fails 0 9 1
run_behavior_case both-fail 12 13 1
run_behavior_inventory_failure_case overcount
run_behavior_inventory_failure_case component
run_behavior_inventory_failure_case query
run_behavior_registration_failure_case NONE 1
run_behavior_registration_failure_case INT 130
run_behavior_registration_failure_case TERM 143
run_behavior_leader_first_case
run_behavior_signal_case INT 130
run_behavior_signal_case TERM 143

printf '%s\n' "M3 node-startup coordinator contract passed"
