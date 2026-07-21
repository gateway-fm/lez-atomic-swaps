#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly launcher="scripts/run-m4-lez-sidecar.sh"

fail() {
  echo "M4 LEZ sidecar launcher contract failed: $*" >&2
  exit 1
}

for command_name in awk cc chmod cp curl jq kill mkdir mktemp mv readlink rm sha256sum sleep stat tail wc; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done
[[ -x "$launcher" && ! -L "$launcher" ]] || fail "launcher is missing or unsafe"

test_root="$(mktemp -d)"
readonly test_root
target_pid=""
sentinel_pid=""
cleanup() {
  local pid state
  for pid in "$target_pid" "$sentinel_pid"; do
    [[ "$pid" =~ ^[1-9][0-9]*$ ]] || continue
    kill -TERM "$pid" 2>/dev/null || true
    for _ in {1..20}; do
      [[ -r "/proc/$pid/stat" ]] || break
      state="$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null || true)"
      [[ "$state" == Z ]] && break
      sleep 0.05
    done
    if [[ -r "/proc/$pid/stat" && "$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null || true)" != Z ]]; then
      kill -KILL "$pid" 2>/dev/null || true
    fi
    wait "$pid" 2>/dev/null || true
  done
  rm -rf -- "$test_root"
}
trap cleanup EXIT

assert_process_stopped() {
  local pid="$1" label="$2" state
  for _ in {1..100}; do
    if [[ ! -r "/proc/$pid/stat" ]]; then return 0; fi
    state="$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null || true)"
    [[ "$state" == Z ]] && return 0
    sleep 0.05
  done
  fail "$label left PID $pid alive"
}

readonly fake_bin_root="$test_root/bin"
readonly source_root="$test_root/source"
readonly sidecar_root="$test_root/maker-sidecar"
readonly launch_audit="$test_root/launch-audit"
readonly curl_audit="$test_root/curl-audit"
real_ln="$(command -v ln)"
readonly real_ln
mkdir -m 0700 -- "$fake_bin_root" "$source_root"

cat >"$fake_bin_root/fake-sidecar.c" <<'FAKE_SIDECAR'
#define _POSIX_C_SOURCE 200809L
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static volatile sig_atomic_t stopping = 0;

static void stop(int signal_number) {
  (void)signal_number;
  stopping = 1;
}

static void configure_signals(void) {
  if (getenv("TEST_IGNORE_SIGNALS") != NULL) {
    signal(SIGINT, SIG_IGN);
    signal(SIGTERM, SIG_IGN);
  } else {
    signal(SIGINT, stop);
    signal(SIGTERM, stop);
  }
}

static void wait_for_stop(void) {
  while (!stopping) pause();
}

int main(int argc, char **argv) {
  const char *listen = NULL;
  const char *run_id = NULL;
  const char *runtime_path = NULL;
  const char *sequencer_url = NULL;
  const char *indexer_url = NULL;
  const char *audit_path;
  FILE *runtime;
  FILE *audit;
  char runtime_json[8192];
  size_t runtime_size;
  int index;

  configure_signals();

  if (argc == 2 && strcmp(argv[1], "--sentinel") == 0) {
    wait_for_stop();
    return 0;
  }

  if (getenv("TEST_INTERRUPT_SIGNAL") != NULL) {
    const char *pid_path = getenv("TEST_INTERRUPT_PID_FILE");
    FILE *pid_file;
    int parent_signal = strcmp(getenv("TEST_INTERRUPT_SIGNAL"), "INT") == 0 ? SIGINT : SIGTERM;
    if (pid_path == NULL) return 67;
    pid_file = fopen(pid_path, "w");
    if (pid_file == NULL) return 67;
    fprintf(pid_file, "%ld\n", (long)getpid());
    fclose(pid_file);
    kill(getppid(), parent_signal);
    wait_for_stop();
    return 0;
  }

  for (index = 1; index < argc; index += 2) {
    if (index + 1 >= argc) return 64;
    if (strcmp(argv[index], "--listen-address") == 0) listen = argv[index + 1];
    else if (strcmp(argv[index], "--run-id") == 0) run_id = argv[index + 1];
    else if (strcmp(argv[index], "--runtime-file") == 0) runtime_path = argv[index + 1];
    else if (strcmp(argv[index], "--sequencer-url") == 0) sequencer_url = argv[index + 1];
    else if (strcmp(argv[index], "--indexer-url") == 0) indexer_url = argv[index + 1];
    else if (
             strcmp(argv[index], "--node-profile") != 0 &&
             strcmp(argv[index], "--capability-file") != 0 &&
             strcmp(argv[index], "--private-key-file") != 0 &&
             strcmp(argv[index], "--state-directory") != 0 &&
             strcmp(argv[index], "--authenticated-transfer-program-id") != 0) return 64;
  }
  if (listen == NULL || strncmp(listen, "127.0.0.1:", 10) != 0 || run_id == NULL ||
      runtime_path == NULL || sequencer_url == NULL || indexer_url == NULL) return 64;
  if (getenv("TEST_SEQUENCER_URL") == NULL || getenv("TEST_INDEXER_URL") == NULL ||
      strcmp(sequencer_url, getenv("TEST_SEQUENCER_URL")) != 0 ||
      strcmp(indexer_url, getenv("TEST_INDEXER_URL")) != 0) return 64;
  audit_path = getenv("TEST_LAUNCH_AUDIT");
  if (audit_path == NULL) return 65;
  audit = fopen(audit_path, "a");
  if (audit == NULL) return 65;
  fprintf(audit, "%ld\t%s\t%s\t%s\n", (long)getpid(), listen, sequencer_url, indexer_url);
  fclose(audit);
  runtime = fopen(runtime_path, "r");
  if (runtime == NULL) return 66;
  runtime_size = fread(runtime_json, 1, sizeof(runtime_json) - 1, runtime);
  if (ferror(runtime) || !feof(runtime)) return 66;
  fclose(runtime);
  while (runtime_size > 0 && (runtime_json[runtime_size - 1] == '\n' ||
                              runtime_json[runtime_size - 1] == '\r')) runtime_size--;
  runtime_json[runtime_size] = '\0';
  printf("{\"event\":\"ready\",\"endpoint\":\"http://%s\",\"run_id\":\"%s\","
         "\"runtime\":%s,\"node_profile\":\"local\","
         "\"sequencer_observation\":\"bounded_canonical_inclusion_and_same_tip_accounts\","
         "\"indexer_health\":\"stable_finalized_tip_bound_to_runtime_genesis\","
         "\"finality\":\"exact_genesis_bound_finalized_indexer_clock_available\"}\n",
         listen, run_id, runtime_json);
  fflush(stdout);
  wait_for_stop();
  return 0;
}
FAKE_SIDECAR
cc -std=c11 -O2 -Wall -Wextra -Werror "$fake_bin_root/fake-sidecar.c" \
  -o "$fake_bin_root/fake-sidecar"
chmod 0700 "$fake_bin_root/fake-sidecar"

cat >"$fake_bin_root/ln" <<'FAKE_LN'
#!/usr/bin/env bash
set -euo pipefail
destination="${!#}"
if [[ -n "${TEST_LN_COLLISION_TARGET:-}" && "$destination" == "$TEST_LN_COLLISION_TARGET" &&
      ! -e "$destination" && ! -L "$destination" ]]; then
  printf 'collision-must-survive\n' >"$destination"
  chmod 0600 "$destination"
fi
exec "$TEST_REAL_LN" "$@"
FAKE_LN
chmod 0700 "$fake_bin_root/ln"

cat >"$fake_bin_root/curl" <<'FAKE_CURL'
#!/usr/bin/env bash
set -euo pipefail
config="$(cat)"
output="" request=""
while (($#)); do
  case "$1" in
    --output) output="$2"; shift 2 ;;
    --data-binary) request="${2#@}"; shift 2 ;;
    --write-out) shift 2 ;;
    --connect-timeout|--max-time|--noproxy|--config) shift 2 ;;
    --silent|--show-error) shift ;;
    http://127.0.0.1:*) shift ;;
    *) shift ;;
  esac
done
[[ -n "$output" && -n "$request" ]]
if ! jq -e '
  .jsonrpc == "2.0" and .id == 1 and .method == "lez_bridge.v1.describe_runtime" and
  (.params | type == "array" and length == 1) and
  (.params[0].context.schema_version == 1) and
  (.params[0].context.request_id == "launcher-health-0001")
' "$request" >/dev/null; then
  : >"$output"
  printf '400'
  exit 0
fi
expected_capability="$(tr -d '\r\n' <"$TEST_EXPECTED_ROOT/capability")"
role="$(jq -er '.sidecar_role' "$TEST_RUNTIME_FILE")"
run_id="$TEST_RUN_ID"
kind="valid"
grep -Fq "authorization: Bearer ${expected_capability}" <<<"$config" || kind="wrong_capability"
grep -Fq "x-lez-bridge-run-id: ${run_id}" <<<"$config" || kind="wrong_run"
grep -Fq "x-lez-bridge-sidecar-role: ${role}" <<<"$config" || kind="wrong_role"
printf '%s\n' "$kind" >>"$TEST_CURL_AUDIT"
if [[ "$kind" != valid ]]; then
  : >"$output"
  printf '401'
  exit 0
fi
if [[ "${TEST_BAD_RPC:-0}" == 1 ]]; then
  jq -cn --slurpfile request "$request" \
    '{jsonrpc:"2.0",id:1,result:{context:$request[0].params[0].context,runtime:{}}}' >"$output"
else
  jq -cn --slurpfile request "$request" --slurpfile runtime "$TEST_RUNTIME_FILE" \
    '{jsonrpc:"2.0",id:1,result:{context:$request[0].params[0].context,runtime:$runtime[0]}}' >"$output"
fi
printf '200'
FAKE_CURL
chmod 0700 "$fake_bin_root/curl"

hex() { printf '%064d' 0 | tr '0' "$1"; }
escrow_program="$(hex 1)"
transfer_program="$(hex 2)"
signer="$(hex 3)"
chain="$(hex 4)"
genesis="$(hex 5)"
readonly escrow_program transfer_program signer chain genesis
readonly run_id="m4-sidecar-contract-run"
readonly runtime_file="$source_root/runtime.json"
readonly terms_file="$source_root/terms.json"
readonly private_key_file="$source_root/signer.key"

jq -cn --arg escrow "$escrow_program" --arg signer "$signer" --arg chain "$chain" \
  --arg genesis "$genesis" \
  '{sidecar_role:"maker",compatibility:"lee_v0_2_0",chain_id:$chain,channel_id:$chain,
    genesis_block_hash:$genesis,escrow_program_id:$escrow,signer_account_id:$signer}' \
  >"$runtime_file"
jq -cn --arg escrow "$escrow_program" --arg transfer "$transfer_program" \
  '{version:3,depositor:"taker",claimant:"maker",escrow_program_id:$escrow,
    authenticated_transfer_program_id:$transfer,swap_id:("6"*64),activation_commitment:("7"*64)}' \
  >"$terms_file"
printf '%064d\n' 8 >"$private_key_file"
chmod 0600 "$runtime_file" "$terms_file" "$private_key_file"

export PATH="$fake_bin_root:$PATH"
export TEST_EXPECTED_ROOT="$sidecar_root"
export TEST_RUNTIME_FILE="$runtime_file"
export TEST_RUN_ID="$run_id"
export TEST_LAUNCH_AUDIT="$launch_audit"
export TEST_CURL_AUDIT="$curl_audit"
export TEST_SEQUENCER_URL="http://127.0.0.1:3040"
export TEST_INDEXER_URL="http://127.0.0.1:8779"
export TEST_REAL_LN="$real_ln"

start_output="$test_root/start.json"
"$launcher" start --root "$sidecar_root" --role maker --run-id "$run_id" \
  --sidecar-bin "$fake_bin_root/fake-sidecar" --sequencer-url http://127.0.0.1:3040 \
  --indexer-url http://127.0.0.1:8779 --runtime-file "$runtime_file" \
  --terms-file "$terms_file" --private-key-file "$private_key_file" \
  --authenticated-transfer-program-id "$transfer_program" >"$start_output"

jq -e --arg root "$sidecar_root" '
  .schema == "lez_m4_role_sidecar_launcher_v1" and .action == "start" and .status == "running" and
  .root == $root and .role == "maker" and .health_and_authentication_proved == true
' "$start_output" >/dev/null || fail "start result is incomplete"

for path in "$sidecar_root" "$sidecar_root/state"; do
  [[ "$(stat -c '%a' "$path")" == 700 ]] || fail "directory is not exact 0700: $path"
done
for path in runtime.json terms.json capability sidecar.log pid-manifest.json; do
  [[ -f "$sidecar_root/$path" && ! -L "$sidecar_root/$path" ]] || fail "missing private artifact: $path"
  [[ "$(stat -c '%a' "$sidecar_root/$path")" == 600 ]] || fail "artifact is not exact 0600: $path"
done

jq -e --arg run "$run_id" --arg runtime_sha "$(sha256sum "$runtime_file" | awk '{print $1}')" \
  --arg terms_sha "$(sha256sum "$terms_file" | awk '{print $1}')" \
  --arg binary_sha "$(sha256sum "$fake_bin_root/fake-sidecar" | awk '{print $1}')" '
  .schema == "lez_m4_role_sidecar_pid_manifest_v1" and .run_id == $run and .role == "maker" and
  (.endpoint | test("^http://127\\.0\\.0\\.1:[1-9][0-9]*$")) and
  .runtime_sha256 == $runtime_sha and .terms_sha256 == $terms_sha and .binary_sha256 == $binary_sha and
  .executable_sha256 == $binary_sha and (.executable_device | test("^[0-9]+$")) and
  (.executable_inode | test("^[1-9][0-9]*$")) and
  (.pid | type == "number") and (.start_ticks | test("^[1-9][0-9]*$")) and
  .listener_scope == "dynamic_literal_loopback" and
  .release_only_capability_enforced == false and
  .capability_scope == "full_role_sidecar_rpc_surface_not_release_only" and
  .terms_enforcement == "launcher_manifest_binding_not_server_method_restriction" and
  .authentication.wrong_capability_rejected and .authentication.wrong_run_rejected and
  .authentication.wrong_role_rejected and .authentication.authenticated_runtime_matched
' "$sidecar_root/pid-manifest.json" >/dev/null || fail "PID manifest binding is incomplete"

[[ "$(wc -l <"$launch_audit")" == 1 ]] || fail "launcher did not start exactly one sidecar"
target_pid="$(jq -er '.pid' "$sidecar_root/pid-manifest.json")"
[[ "$(awk 'NR == 1 { print $1 }' "$launch_audit")" == "$target_pid" ]] ||
  fail "launcher PID does not match the exact spawned process"
[[ -r "/proc/$target_pid/stat" ]] || fail "target sidecar is not running"

status_output="$test_root/status.json"
"$launcher" status --root "$sidecar_root" >"$status_output"
jq -e '.action == "status" and .status == "running" and .identity_matched and .authenticated' \
  "$status_output" >/dev/null || fail "status did not revalidate identity and authentication"
[[ "$(tr '\n' ',' <"$curl_audit")" == \
  wrong_capability,wrong_run,wrong_role,valid,wrong_capability,wrong_run,wrong_role,valid, ]] ||
  fail "start/status did not perform the exact negative and positive authentication probes"

"$fake_bin_root/fake-sidecar" --sentinel &
sentinel_pid=$!
sleep 0.1
[[ -r "/proc/$sentinel_pid/stat" ]] || fail "foreign same-binary sentinel did not start"

cp -p "$sidecar_root/pid-manifest.json" "$test_root/manifest.saved"
jq -c '.start_ticks = "1"' "$sidecar_root/pid-manifest.json" >"$test_root/manifest.tampered"
chmod 0600 "$test_root/manifest.tampered"
mv "$test_root/manifest.tampered" "$sidecar_root/pid-manifest.json"
if "$launcher" stop --root "$sidecar_root" >"$test_root/tampered-stop.json" 2>&1; then
  fail "stop accepted a tampered PID identity"
fi
[[ -r "/proc/$target_pid/stat" ]] || fail "tampered stop killed the target"
cp -p "$test_root/manifest.saved" "$sidecar_root/pid-manifest.json"

stop_output="$test_root/stop.json"
"$launcher" stop --root "$sidecar_root" >"$stop_output"
jq -e '.action == "stop" and .status == "stopped" and .identity_matched' "$stop_output" >/dev/null ||
  fail "exact scoped stop did not succeed"
target_pid=""
[[ -r "/proc/$sentinel_pid/stat" ]] || fail "scoped stop killed the foreign same-binary process"

if "$launcher" status --root "$sidecar_root" >"$test_root/stopped-status.json"; then
  fail "stopped sidecar reported running"
fi
jq -e '.action == "status" and .status == "stopped"' "$test_root/stopped-status.json" >/dev/null ||
  fail "stopped status is not explicit"

if "$launcher" start --root "$sidecar_root" --role maker --run-id "$run_id" \
  --sidecar-bin "$fake_bin_root/fake-sidecar" --sequencer-url http://127.0.0.1:3040 \
  --indexer-url http://127.0.0.1:8779 --runtime-file "$runtime_file" \
  --terms-file "$terms_file" --private-key-file "$private_key_file" \
  --authenticated-transfer-program-id "$transfer_program" >/dev/null 2>&1; then
  fail "launcher reused an existing role root"
fi

wrong_runtime="$source_root/wrong-runtime.json"
jq -c '.sidecar_role = "taker"' "$runtime_file" >"$wrong_runtime"
chmod 0600 "$wrong_runtime"
export TEST_EXPECTED_ROOT="$test_root/wrong-role-root"
if "$launcher" start --root "$TEST_EXPECTED_ROOT" --role maker --run-id "$run_id" \
  --sidecar-bin "$fake_bin_root/fake-sidecar" --sequencer-url http://127.0.0.1:3040 \
  --indexer-url http://127.0.0.1:8779 --runtime-file "$wrong_runtime" \
  --terms-file "$terms_file" --private-key-file "$private_key_file" \
  --authenticated-transfer-program-id "$transfer_program" >/dev/null 2>&1; then
  fail "launcher accepted a cross-role runtime"
fi

duplicate_root="$test_root/duplicate-option-root"
if "$launcher" start --root "$duplicate_root" --role maker --run-id "$run_id" \
  --run-id duplicate-option-run --sidecar-bin "$fake_bin_root/fake-sidecar" \
  --sequencer-url http://127.0.0.1:3040 --indexer-url http://127.0.0.1:8779 \
  --runtime-file "$runtime_file" --terms-file "$terms_file" \
  --private-key-file "$private_key_file" \
  --authenticated-transfer-program-id "$transfer_program" >/dev/null 2>&1; then
  fail "launcher accepted a duplicate start option"
fi
[[ ! -e "$duplicate_root" ]] || fail "duplicate option created a sidecar root"

colliding_endpoint_root="$test_root/colliding-endpoint-root"
if "$launcher" start --root "$colliding_endpoint_root" --role maker --run-id "$run_id" \
  --sidecar-bin "$fake_bin_root/fake-sidecar" --sequencer-url http://127.0.0.1:3040 \
  --indexer-url http://127.0.0.1:3040/ --runtime-file "$runtime_file" \
  --terms-file "$terms_file" --private-key-file "$private_key_file" \
  --authenticated-transfer-program-id "$transfer_program" >/dev/null 2>&1; then
  fail "launcher accepted colliding sequencer/indexer endpoints"
fi
[[ ! -e "$colliding_endpoint_root" ]] || fail "colliding endpoints created a sidecar root"

bad_key="$source_root/bad-signer.key"
printf '%064d\nnot-a-second-key\n' 8 >"$bad_key"
chmod 0600 "$bad_key"
bad_key_root="$test_root/bad-key-root"
if "$launcher" start --root "$bad_key_root" --role maker --run-id "$run_id" \
  --sidecar-bin "$fake_bin_root/fake-sidecar" --sequencer-url http://127.0.0.1:3040 \
  --indexer-url http://127.0.0.1:8779 --runtime-file "$runtime_file" \
  --terms-file "$terms_file" --private-key-file "$bad_key" \
  --authenticated-transfer-program-id "$transfer_program" >/dev/null 2>&1; then
  fail "launcher accepted a multi-line signer key"
fi
[[ ! -e "$bad_key_root" ]] || fail "invalid signer created a sidecar root"

unsafe_binary="$fake_bin_root/unsafe-sidecar"
cp -- "$fake_bin_root/fake-sidecar" "$unsafe_binary"
chmod 0775 "$unsafe_binary"
unsafe_root="$test_root/unsafe-binary-root"
if "$launcher" start --root "$unsafe_root" --role maker --run-id "$run_id" \
  --sidecar-bin "$unsafe_binary" --sequencer-url http://127.0.0.1:3040 \
  --indexer-url http://127.0.0.1:8779 --runtime-file "$runtime_file" \
  --terms-file "$terms_file" --private-key-file "$private_key_file" \
  --authenticated-transfer-program-id "$transfer_program" >/dev/null 2>&1; then
  fail "launcher accepted a group-writable sidecar binary"
fi
[[ ! -e "$unsafe_root" ]] || fail "unsafe binary created a sidecar root"

source_alias="$test_root/source-alias"
"$real_ln" -s -- "$source_root" "$source_alias"
alias_input_root="$test_root/alias-input-root"
if "$launcher" start --root "$alias_input_root" --role maker --run-id "$run_id" \
  --sidecar-bin "$fake_bin_root/fake-sidecar" --sequencer-url http://127.0.0.1:3040 \
  --indexer-url http://127.0.0.1:8779 --runtime-file "$source_alias/runtime.json" \
  --terms-file "$terms_file" --private-key-file "$private_key_file" \
  --authenticated-transfer-program-id "$transfer_program" >/dev/null 2>&1; then
  fail "launcher accepted a private input through a symlinked ancestor"
fi
[[ ! -e "$alias_input_root" ]] || fail "aliased private input created a sidecar root"

canonical_parent="$test_root/canonical-parent"
mkdir -m 0700 -- "$canonical_parent" "$canonical_parent/nested"
parent_alias="$test_root/parent-alias"
"$real_ln" -s -- "$canonical_parent" "$parent_alias"
alias_parent_root="$parent_alias/nested/role-root"
if "$launcher" start --root "$alias_parent_root" --role maker --run-id "$run_id" \
  --sidecar-bin "$fake_bin_root/fake-sidecar" --sequencer-url http://127.0.0.1:3040 \
  --indexer-url http://127.0.0.1:8779 --runtime-file "$runtime_file" \
  --terms-file "$terms_file" --private-key-file "$private_key_file" \
  --authenticated-transfer-program-id "$transfer_program" >/dev/null 2>&1; then
  fail "launcher accepted a new root beneath a symlinked ancestor"
fi
[[ ! -e "$canonical_parent/nested/role-root" ]] ||
  fail "aliased sidecar-root parent created the canonical root"

copy_collision_root="$test_root/copy-collision-root"
export TEST_LN_COLLISION_TARGET="$copy_collision_root/runtime.json"
if "$launcher" start --root "$copy_collision_root" --role maker --run-id "$run_id" \
  --sidecar-bin "$fake_bin_root/fake-sidecar" --sequencer-url http://127.0.0.1:3040 \
  --indexer-url http://127.0.0.1:8779 --runtime-file "$runtime_file" \
  --terms-file "$terms_file" --private-key-file "$private_key_file" \
  --authenticated-transfer-program-id "$transfer_program" >/dev/null 2>&1; then
  fail "launcher overwrote a raced runtime copy"
fi
unset TEST_LN_COLLISION_TARGET
[[ "$(<"$copy_collision_root/runtime.json")" == collision-must-survive ]] ||
  fail "atomic copying did not preserve the raced runtime destination"
[[ "$(wc -l <"$launch_audit")" == 1 ]] || fail "copy collision started a sidecar"

collision_root="$test_root/collision-root"
export TEST_EXPECTED_ROOT="$collision_root"
export TEST_LN_COLLISION_TARGET="$collision_root/pid-manifest.json"
if "$launcher" start --root "$collision_root" --role maker --run-id "$run_id" \
  --sidecar-bin "$fake_bin_root/fake-sidecar" --sequencer-url http://127.0.0.1:3040/ \
  --indexer-url http://127.0.0.1:8779/ --runtime-file "$runtime_file" \
  --terms-file "$terms_file" --private-key-file "$private_key_file" \
  --authenticated-transfer-program-id "$transfer_program" >/dev/null 2>&1; then
  fail "launcher overwrote a raced PID manifest"
fi
unset TEST_LN_COLLISION_TARGET
[[ "$(<"$collision_root/pid-manifest.json")" == collision-must-survive ]] ||
  fail "atomic publication did not preserve the raced PID manifest"
collision_pid="$(tail -n 1 "$launch_audit" | awk '{print $1}')"
target_pid="$collision_pid"
assert_process_stopped "$collision_pid" "failed manifest publication"
target_pid=""

for parent_signal in TERM INT; do
  interrupt_root="$test_root/interrupt-${parent_signal,,}-root"
  interrupt_pid_file="$test_root/interrupt-${parent_signal,,}.pid"
  export TEST_EXPECTED_ROOT="$interrupt_root"
  export TEST_RUNTIME_FILE="$runtime_file"
  export TEST_INTERRUPT_SIGNAL="$parent_signal"
  export TEST_INTERRUPT_PID_FILE="$interrupt_pid_file"
  export TEST_IGNORE_SIGNALS=1
  interrupt_succeeded=false
  if "$launcher" start --root "$interrupt_root" --role maker --run-id "$run_id" \
    --sidecar-bin "$fake_bin_root/fake-sidecar" --sequencer-url http://127.0.0.1:3040 \
    --indexer-url http://127.0.0.1:8779 --runtime-file "$runtime_file" \
    --terms-file "$terms_file" --private-key-file "$private_key_file" \
    --authenticated-transfer-program-id "$transfer_program" >/dev/null 2>&1; then
    interrupt_succeeded=true
  fi
  unset TEST_INTERRUPT_SIGNAL TEST_INTERRUPT_PID_FILE TEST_IGNORE_SIGNALS
  [[ "$interrupt_succeeded" == false ]] || fail "launcher ignored immediate $parent_signal after spawn"
  [[ -s "$interrupt_pid_file" ]] || fail "immediate $parent_signal fixture did not publish its PID"
  target_pid="$(<"$interrupt_pid_file")"
  assert_process_stopped "$target_pid" "immediate $parent_signal cleanup"
  target_pid=""
done

failed_rpc_root="$test_root/failed-rpc-root"
export TEST_EXPECTED_ROOT="$failed_rpc_root"
export TEST_RUNTIME_FILE="$runtime_file"
export TEST_BAD_RPC=1
export TEST_IGNORE_SIGNALS=1
failed_rpc_succeeded=false
if "$launcher" start --root "$failed_rpc_root" --role maker --run-id "$run_id" \
  --sidecar-bin "$fake_bin_root/fake-sidecar" --sequencer-url http://127.0.0.1:3040 \
  --indexer-url http://127.0.0.1:8779 --runtime-file "$runtime_file" \
  --terms-file "$terms_file" --private-key-file "$private_key_file" \
  --authenticated-transfer-program-id "$transfer_program" >/dev/null 2>&1; then
  failed_rpc_succeeded=true
fi
unset TEST_BAD_RPC TEST_IGNORE_SIGNALS
[[ "$failed_rpc_succeeded" == false ]] || fail "launcher accepted a mismatched authenticated runtime"
target_pid="$(tail -n 1 "$launch_audit" | awk '{print $1}')"
assert_process_stopped "$target_pid" "TERM-resistant failed-start cleanup"
target_pid=""

mutable_binary="$fake_bin_root/mutable-sidecar"
cp -- "$fake_bin_root/fake-sidecar" "$mutable_binary"
chmod 0700 "$mutable_binary"
mutable_root="$test_root/mutable-binary-root"
export TEST_EXPECTED_ROOT="$mutable_root"
export TEST_RUNTIME_FILE="$runtime_file"
"$launcher" start --root "$mutable_root" --role maker --run-id "$run_id" \
  --sidecar-bin "$mutable_binary" --sequencer-url http://127.0.0.1:3040 \
  --indexer-url http://127.0.0.1:8779 --runtime-file "$runtime_file" \
  --terms-file "$terms_file" --private-key-file "$private_key_file" \
  --authenticated-transfer-program-id "$transfer_program" >"$test_root/mutable-start.json"
target_pid="$(jq -er '.pid' "$mutable_root/pid-manifest.json")"
cp -p "$mutable_root/pid-manifest.json" "$test_root/mutable-manifest.saved"
jq -c '.executable_inode = "1"' "$mutable_root/pid-manifest.json" \
  >"$test_root/mutable-manifest.tampered"
chmod 0600 "$test_root/mutable-manifest.tampered"
mv "$test_root/mutable-manifest.tampered" "$mutable_root/pid-manifest.json"
if "$launcher" stop --root "$mutable_root" >/dev/null 2>&1; then
  fail "stop accepted a tampered immutable executable identity"
fi
[[ -r "/proc/$target_pid/stat" ]] || fail "tampered executable identity killed the target"
cp -p "$test_root/mutable-manifest.saved" "$mutable_root/pid-manifest.json"

original_image="$fake_bin_root/mutable-sidecar.original"
mv "$mutable_binary" "$original_image"
printf '#!/usr/bin/env bash\nexit 0\n' >"$mutable_binary"
chmod 0700 "$mutable_binary"
"$launcher" status --root "$mutable_root" >"$test_root/replaced-binary-status.json"
jq -e '.status == "running" and .identity_matched and .authenticated' \
  "$test_root/replaced-binary-status.json" >/dev/null ||
  fail "binary path replacement orphaned the kernel-held executable identity"
rm -f -- "$original_image"
"$launcher" status --root "$mutable_root" >"$test_root/deleted-binary-status.json"
jq -e '.status == "running" and .identity_matched and .authenticated' \
  "$test_root/deleted-binary-status.json" >/dev/null ||
  fail "binary image removal orphaned the kernel-held executable identity"
"$launcher" stop --root "$mutable_root" >"$test_root/mutable-stop.json"
target_pid=""
jq -e '.status == "stopped" and .identity_matched' "$test_root/mutable-stop.json" >/dev/null ||
  fail "immutable executable identity did not permit exact stop after path removal"
[[ -r "/proc/$sentinel_pid/stat" ]] ||
  fail "lifecycle failure cleanup or binary mutation killed the foreign same-binary sentinel"

taker_runtime="$source_root/taker-runtime.json"
jq -c '.sidecar_role = "taker"' "$runtime_file" >"$taker_runtime"
chmod 0600 "$taker_runtime"
taker_root="$test_root/taker-sidecar"
export TEST_EXPECTED_ROOT="$taker_root"
export TEST_RUNTIME_FILE="$taker_runtime"
taker_start="$test_root/taker-start.json"
"$launcher" start --root "$taker_root" --role taker --run-id "$run_id" \
  --sidecar-bin "$fake_bin_root/fake-sidecar" --sequencer-url http://127.0.0.1:3040 \
  --indexer-url http://127.0.0.1:8779 --runtime-file "$taker_runtime" \
  --terms-file "$terms_file" --private-key-file "$private_key_file" \
  --authenticated-transfer-program-id "$transfer_program" >"$taker_start"
jq -e '.action == "start" and .status == "running" and .role == "taker" and
  .health_and_authentication_proved' "$taker_start" >/dev/null ||
  fail "taker role did not satisfy the same health/authentication contract"
target_pid="$(jq -er '.pid' "$taker_root/pid-manifest.json")"
"$launcher" stop --root "$taker_root" >"$test_root/taker-stop.json"
target_pid=""
jq -e '.action == "stop" and .status == "stopped" and .role == "taker" and .identity_matched' \
  "$test_root/taker-stop.json" >/dev/null || fail "taker exact scoped stop did not succeed"

kill -TERM "$sentinel_pid"
wait "$sentinel_pid" 2>/dev/null || true
sentinel_pid=""

echo "M4 LEZ sidecar launcher contract passed"
