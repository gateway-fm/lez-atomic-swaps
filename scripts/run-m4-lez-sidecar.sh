#!/usr/bin/env bash
set -euo pipefail

export LC_ALL=C
umask 077

readonly MANIFEST_SCHEMA="lez_m4_role_sidecar_pid_manifest_v2"
readonly RESULT_SCHEMA="lez_m4_role_sidecar_launcher_v1"
readonly CAPABILITY_SCOPE="full_role_sidecar_rpc_surface_not_release_only"
readonly TERMS_ENFORCEMENT="launcher_manifest_binding_not_server_method_restriction"
readonly MAX_CONFIG_BYTES=1048576

fail() {
  echo "M4 LEZ sidecar launcher failed: $*" >&2
  exit 1
}

usage() {
  cat >&2 <<'USAGE'
usage:
  run-m4-lez-sidecar.sh start --root NEW_0700_ROOT --role maker|taker --run-id RUN \
    --sidecar-bin BINARY --sequencer-url http://127.0.0.1:PORT \
    --indexer-url http://127.0.0.1:PORT --runtime-file PRIVATE_JSON \
    --terms-file PRIVATE_JSON --private-key-file PRIVATE_KEY \
    --authenticated-transfer-program-id HEX32 [--adopt-state-directory ABSOLUTE_0700_DIR --tag13-handoff-receipt PRIVATE_JSON]
  run-m4-lez-sidecar.sh status --root EXISTING_0700_ROOT
  run-m4-lez-sidecar.sh stop --root EXISTING_0700_ROOT
USAGE
  exit 64
}

require_commands() {
  local command_name
  for command_name in awk chmod cp curl dirname grep head id jq kill ln mkdir mktemp mv openssl \
    perl readlink rm sha256sum sleep stat tr; do
    command -v "$command_name" >/dev/null || fail "missing command: $command_name"
  done
}

require_absolute_path() {
  [[ "$1" == /* ]] || fail "$2 must be an absolute path"
}

require_private_directory() {
  local path="$1" label="$2" canonical
  [[ -d "$path" && ! -L "$path" ]] || fail "$label is not a real directory"
  canonical="$(readlink -f -- "$path" 2>/dev/null)" || fail "$label path cannot be canonicalized"
  [[ "$canonical" == "$path" ]] || fail "$label must use its canonical path without symlinked ancestors"
  [[ "$(stat -c '%u:%a' -- "$path")" == "$(id -u):700" ]] ||
    fail "$label is not exact owner-only 0700"
}

require_private_owned_file() {
  local path="$1" label="$2" canonical
  [[ -f "$path" && ! -L "$path" ]] || fail "$label is not a regular non-symlink file"
  canonical="$(readlink -f -- "$path" 2>/dev/null)" || fail "$label path cannot be canonicalized"
  [[ "$canonical" == "$path" ]] || fail "$label must use its canonical path without symlinked ancestors"
  [[ "$(stat -c '%u:%a:%h' -- "$path")" == "$(id -u):600:1" ]] ||
    fail "$label is not exact owner-only 0600 with one link"
}

require_private_file() {
  local path="$1" label="$2" size
  require_private_owned_file "$path" "$label"
  size="$(stat -c '%s' -- "$path")"
  [[ "$size" =~ ^[1-9][0-9]*$ && "$size" -le "$MAX_CONFIG_BYTES" ]] ||
    fail "$label is empty or oversized"
}

sidecar_binary_is_safe() {
  local path="$1" permissions canonical
  [[ -f "$path" && -x "$path" && ! -L "$path" ]] || return 1
  [[ "$(stat -c '%u:%h' -- "$path")" == "$(id -u):1" ]] || return 1
  permissions="$(stat -c '%a' -- "$path")"
  [[ "$permissions" =~ ^[0-7]{3,4}$ ]] || return 1
  (( (8#$permissions & 0022) == 0 )) || return 1
  canonical="$(readlink -f -- "$path" 2>/dev/null)" || return 1
  [[ "$canonical" == "$path" ]]
}

require_sidecar_binary() {
  sidecar_binary_is_safe "$1" ||
    fail "sidecar binary must be canonical, owner-held, executable, single-link, and not group/other writable"
}

require_safe_run_id() {
  local value="$1"
  [[ "${#value}" -ge 8 && "${#value}" -le 64 &&
    "$value" =~ ^[A-Za-z0-9._-]+$ ]] || fail "run ID is invalid"
}

require_role() {
  [[ "$1" == maker || "$1" == taker ]] || fail "role must be maker or taker"
}

require_hex32() {
  [[ "$1" =~ ^[0-9a-f]{64}$ && "$1" != "$(printf '%064d' 0)" ]] ||
    fail "$2 is not one nonzero lowercase hex32"
}

require_loopback_url() {
  [[ "$1" =~ ^http://127\.0\.0\.1:([1-9][0-9]{0,4})/?$ ]] ||
    fail "$2 must be one literal-loopback HTTP root"
  local port="${BASH_REMATCH[1]}"
  [[ "$port" -le 65535 ]] || fail "$2 port is out of range"
}

validate_private_key() {
  local path="$1" key
  awk 'BEGIN { valid = 1 } NR != 1 || $0 !~ /^[0-9a-f]{64}$/ { valid = 0; exit }
    END { if (NR != 1) valid = 0; exit valid ? 0 : 1 }' "$path" ||
    fail "sidecar private key must be exactly one lowercase hex32 line"
  key="$(head -n 1 -- "$path")"
  require_hex32 "$key" "sidecar private key"
}

process_start_ticks() {
  local record fields
  IFS= read -r record 2>/dev/null <"/proc/$1/stat" || return 1
  fields="${record##*) }"
  read -r -a fields <<<"$fields"
  [[ "${#fields[@]}" -ge 20 ]] || return 1
  printf '%s\n' "${fields[19]}"
}

process_state() {
  local record fields
  IFS= read -r record 2>/dev/null <"/proc/$1/stat" || return 1
  fields="${record##*) }"
  read -r -a fields <<<"$fields"
  [[ "${#fields[@]}" -ge 2 ]] || return 1
  printf '%s\n' "${fields[0]}"
}

process_parent_pid() {
  local record fields
  IFS= read -r record 2>/dev/null <"/proc/$1/stat" || return 1
  fields="${record##*) }"
  read -r -a fields <<<"$fields"
  [[ "${#fields[@]}" -ge 2 ]] || return 1
  printf '%s\n' "${fields[1]}"
}

owned_child_identity_matches() {
  local pid="$1" start_ticks="$2" owner_pid="$3"
  [[ "$pid" =~ ^[1-9][0-9]*$ && -r "/proc/$pid/stat" ]] || return 1
  [[ "$(process_state "$pid")" != Z ]] || return 1
  [[ "$(process_start_ticks "$pid")" == "$start_ticks" ]] || return 1
  [[ "$(process_parent_pid "$pid")" == "$owner_pid" ]]
}

process_identity_matches() {
  local pid="$1" start_ticks="$2" executable_device="$3" executable_inode="$4"
  local executable_sha="$5" observed_identity observed_sha
  [[ "$pid" =~ ^[1-9][0-9]*$ && -r "/proc/$pid/stat" ]] || return 1
  [[ "$(process_state "$pid")" != Z ]] || return 1
  [[ "$(process_start_ticks "$pid")" == "$start_ticks" ]] || return 1
  observed_identity="$(stat -Lc '%d:%i' -- "/proc/$pid/exe" 2>/dev/null)" || return 1
  [[ "$observed_identity" == "$executable_device:$executable_inode" ]] || return 1
  observed_sha="$(sha256sum "/proc/$pid/exe" 2>/dev/null | awk '{print $1}')" || return 1
  [[ "$observed_sha" == "$executable_sha" ]]
}

terminate_owned_child() {
  local pid="$1" start_ticks="$2" owner_pid="$3"
  if [[ ! -r "/proc/$pid/stat" || "$(process_state "$pid" 2>/dev/null || true)" == Z ]]; then
    wait "$pid" 2>/dev/null || true
    return 0
  fi
  owned_child_identity_matches "$pid" "$start_ticks" "$owner_pid" || return 1
  kill -TERM "$pid" 2>/dev/null || return 1
  for _ in {1..20}; do
    if [[ ! -r "/proc/$pid/stat" || "$(process_state "$pid" 2>/dev/null || true)" == Z ]]; then
      wait "$pid" 2>/dev/null || true
      return 0
    fi
    owned_child_identity_matches "$pid" "$start_ticks" "$owner_pid" || return 1
    sleep 0.05
  done
  owned_child_identity_matches "$pid" "$start_ticks" "$owner_pid" || return 1
  kill -KILL "$pid" 2>/dev/null || return 1
  for _ in {1..100}; do
    if [[ ! -r "/proc/$pid/stat" || "$(process_state "$pid" 2>/dev/null || true)" == Z ]]; then
      wait "$pid" 2>/dev/null || true
      return 0
    fi
    sleep 0.05
  done
  return 1
}

allocate_loopback_port() {
  perl -MIO::Socket::INET -e '
    $socket = IO::Socket::INET->new(LocalAddr => "127.0.0.1", LocalPort => 0,
      Proto => "tcp", Listen => 1) or die "loopback allocation failed: $!\n";
    print $socket->sockport, "\n";
  '
}

publish_private_output() {
  local destination="$1"
  shift
  [[ ! -e "$destination" && ! -L "$destination" ]] || fail "refusing to replace $destination"
  local partial
  partial="$(mktemp "${destination}.partial.XXXXXXXX")" || fail "cannot create private temporary file"
  if ! "$@" >"$partial"; then
    rm -f -- "$partial"
    fail "cannot produce private artifact: $destination"
  fi
  chmod 0600 "$partial"
  if ! ln -- "$partial" "$destination"; then
    rm -f -- "$partial"
    fail "refusing to replace raced destination: $destination"
  fi
  rm -f -- "$partial"
  require_private_file "$destination" "published private artifact"
}

copy_private_input() {
  local source="$1" destination="$2" label="$3" before after partial
  require_private_file "$source" "$label"
  [[ ! -e "$destination" && ! -L "$destination" ]] || fail "$label destination exists"
  before="$(sha256sum "$source" | awk '{print $1}')"
  partial="$(mktemp "${destination}.partial.XXXXXXXX")" || fail "cannot create private copy temporary"
  if ! cp -- "$source" "$partial"; then
    rm -f -- "$partial"
    fail "$label copy failed"
  fi
  chmod 0600 "$partial"
  after="$(sha256sum "$source" | awk '{print $1}')"
  if [[ "$before" != "$after" || "$(sha256sum "$partial" | awk '{print $1}')" != "$before" ]]; then
    rm -f -- "$partial"
    fail "$label changed while copied"
  fi
  if ! ln -- "$partial" "$destination"; then
    rm -f -- "$partial"
    fail "refusing to replace raced $label destination"
  fi
  rm -f -- "$partial"
  require_private_file "$destination" "bound $label"
}

validate_runtime_and_terms() {
  local runtime="$1" terms="$2" role="$3" transfer_program="$4"
  jq -e --arg role "$role" '
    type == "object" and .sidecar_role == $role and .compatibility == "lee_v0_2_0" and
    (.chain_id | test("^[0-9a-f]{64}$")) and .chain_id != ("0" * 64) and
    .channel_id == .chain_id and
    (.genesis_block_hash | test("^[0-9a-f]{64}$")) and .genesis_block_hash != ("0" * 64) and
    (.escrow_program_id | test("^[0-9a-f]{64}$")) and .escrow_program_id != ("0" * 64) and
    (.signer_account_id | test("^[0-9a-f]{64}$")) and .signer_account_id != ("0" * 64) and
    (keys | sort) == (["chain_id","channel_id","compatibility","escrow_program_id",
      "genesis_block_hash","sidecar_role","signer_account_id"] | sort)
  ' "$runtime" >/dev/null || fail "runtime does not exactly bind the requested role and v0.2 profile"
  jq -e --arg transfer "$transfer_program" --slurpfile runtime "$runtime" '
    type == "object" and .version == 3 and .depositor == "taker" and .claimant == "maker" and
    .escrow_program_id == $runtime[0].escrow_program_id and
    .authenticated_transfer_program_id == $transfer and
    (.swap_id | test("^[0-9a-f]{64}$")) and .swap_id != ("0" * 64) and
    (.activation_commitment | test("^[0-9a-f]{64}$")) and .activation_commitment != ("0" * 64)
  ' "$terms" >/dev/null || fail "terms do not bind the runtime and authenticated-transfer program"
}

create_probe_request() {
  local destination="$1" run_id="$2" role="$3"
  publish_private_output "$destination" jq -cn --arg run "$run_id" --arg role "$role" '
    {jsonrpc:"2.0",id:1,method:"lez_bridge.v1.describe_runtime",params:[
      {context:{schema_version:1,run_id:$run,request_id:"launcher-health-0001",sidecar_role:$role}}
    ]}
  '
}

rpc_probe() {
  local endpoint="$1" capability="$2" header_run="$3" header_role="$4"
  local request="$5" response="$6"
  local code partial="${response}.partial.$$"
  : >"$partial"
  chmod 0600 "$partial"
  code="$({
    printf 'header = "authorization: Bearer %s"\n' "$capability"
    printf 'header = "x-lez-bridge-run-id: %s"\n' "$header_run"
    printf 'header = "x-lez-bridge-sidecar-role: %s"\n' "$header_role"
    printf 'header = "content-type: application/json"\n'
  } | curl --config - --silent --show-error --noproxy '*' --connect-timeout 2 --max-time 10 \
    --output "$partial" --write-out '%{http_code}' \
    --data-binary "@$request" "$endpoint")" || {
      rm -f -- "$partial"
      return 1
    }
  mv -f -- "$partial" "$response"
  printf '%s' "$code"
}

prove_authentication() {
  local root="$1" endpoint="$2" run_id="$3" role="$4" capability response code
  capability="$(tr -d '\r\n' <"$root/capability")"
  [[ "${#capability}" -ge 32 && "${#capability}" -le 128 &&
    "$capability" =~ ^[A-Za-z0-9._-]+$ ]] || fail "capability is invalid"
  response="$root/probe-response.json"

  code="$(rpc_probe "$endpoint" "launcher-wrong-capability-00000001" "$run_id" "$role" \
    "$root/probe-request.json" "$response")" || fail "wrong-capability probe failed"
  [[ "$code" == 401 || "$code" == 403 ]] || fail "wrong-capability probe returned HTTP $code instead of 401 or 403"
  code="$(rpc_probe "$endpoint" "$capability" "launcher-wrong-run" "$role" \
    "$root/probe-request.json" "$response")" || fail "wrong-run probe failed"
  [[ "$code" == 401 || "$code" == 403 ]] || fail "wrong-run probe returned HTTP $code instead of 401 or 403"
  local wrong_role=maker
  [[ "$role" == maker ]] && wrong_role=taker
  code="$(rpc_probe "$endpoint" "$capability" "$run_id" "$wrong_role" \
    "$root/probe-request.json" "$response")" || fail "wrong-role probe failed"
  [[ "$code" == 401 || "$code" == 403 ]] || fail "wrong-role probe returned HTTP $code instead of 401 or 403"
  code="$(rpc_probe "$endpoint" "$capability" "$run_id" "$role" \
    "$root/probe-request.json" "$response")" || fail "authenticated runtime probe failed"
  [[ "$code" == 200 ]] || fail "authenticated runtime probe returned HTTP $code"
  jq -e --slurpfile request "$root/probe-request.json" --slurpfile runtime "$root/runtime.json" '
    .jsonrpc == "2.0" and .id == 1 and .result.context == $request[0].params[0].context and
    .result.runtime == $runtime[0] and (.error | not)
  ' "$response" >/dev/null || fail "authenticated runtime response differs from bound inputs"
  rm -f -- "$response"
}

load_manifest() {
  local root="$1" manifest state_directory state_mode state_device state_inode role
  local tag13_handoff_receipt tag13_handoff_receipt_sha
  manifest="$root/pid-manifest.json"
  require_absolute_path "$root" "root"
  require_private_directory "$root" "sidecar root"
  require_private_file "$manifest" "PID manifest"
  jq -e --arg schema "$MANIFEST_SCHEMA" --arg root "$root" '
    .schema == $schema and (.pid | type == "number") and
    .root == $root and (.state_directory | startswith("/")) and
    (.state_directory_mode == "fresh_supervisor_owned" or
      .state_directory_mode == "adopted_exact_existing_tag13") and
    (.state_directory_device | test("^[0-9]+$")) and
    (.state_directory_inode | test("^[1-9][0-9]*$")) and
    (((.state_directory_mode == "adopted_exact_existing_tag13") and
      ((.tag13_handoff_receipt | type) == "string") and
      (.tag13_handoff_receipt | startswith("/")) and
      (.tag13_handoff_receipt_sha256 | test("^[0-9a-f]{64}$"))) or
     ((.state_directory_mode == "fresh_supervisor_owned") and
      .tag13_handoff_receipt == null and .tag13_handoff_receipt_sha256 == null)) and
    (.start_ticks | type == "string" and test("^[1-9][0-9]*$")) and
    (.endpoint | test("^http://127\\.0\\.0\\.1:[1-9][0-9]*$")) and
    (.binary_sha256 | test("^[0-9a-f]{64}$")) and
    (.executable_sha256 | test("^[0-9a-f]{64}$")) and .executable_sha256 == .binary_sha256 and
    (.executable_device | test("^[0-9]+$")) and
    (.executable_inode | test("^[1-9][0-9]*$")) and
    (.executable | startswith("/")) and .sidecar_binary == .executable and
    (.runtime_sha256 | test("^[0-9a-f]{64}$")) and
    (.terms_sha256 | test("^[0-9a-f]{64}$")) and
    (.role == "maker" or .role == "taker") and
    .listener_scope == "dynamic_literal_loopback" and
    .release_only_capability_enforced == false and
    .capability_scope == "full_role_sidecar_rpc_surface_not_release_only" and
    .terms_enforcement == "launcher_manifest_binding_not_server_method_restriction"
  ' "$manifest" >/dev/null || fail "PID manifest is malformed or unsupported"
  state_directory="$(jq -er .state_directory "$manifest")"
  state_mode="$(jq -er .state_directory_mode "$manifest")"
  state_device="$(jq -er .state_directory_device "$manifest")"
  state_inode="$(jq -er .state_directory_inode "$manifest")"
  role="$(jq -er .role "$manifest")"
  require_private_directory "$state_directory" "bound sidecar state directory"
  [[ "$(stat --format=%d:%i -- "$state_directory")" == "$state_device:$state_inode" ]] ||
    fail "bound sidecar state directory identity changed"
  if [[ "$state_mode" == fresh_supervisor_owned ]]; then
    [[ "$state_directory" == "$root/state" ]] || fail "fresh state is outside its supervisor root"
  else
    [[ "$role" == taker ]] || fail "only Taker may own adopted Tag13 state"
    require_private_file "$state_directory/m4-xmr-stage-a-tag13-evidence.v2.json" \
      "bound adopted Tag13 evidence"
    tag13_handoff_receipt="$(jq -er .tag13_handoff_receipt "$manifest")"
    tag13_handoff_receipt_sha="$(jq -er .tag13_handoff_receipt_sha256 "$manifest")"
    require_private_file "$tag13_handoff_receipt" "bound typed Tag13 handoff receipt"
    [[ "$tag13_handoff_receipt" != "$state_directory/"* ]] ||
      fail "bound typed Tag13 handoff receipt moved inside adopted state"
    [[ "$(sha256sum "$tag13_handoff_receipt" | awk '{print $1}')" == "$tag13_handoff_receipt_sha" ]] || fail "typed Tag13 handoff receipt hash changed"
    [[ "$state_directory" != "$root" && "$state_directory" != "$root/"* &&
      "$root" != "$state_directory/"* ]] ||
      fail "adopted Tag13 state overlaps its supervisor root"
  fi
  require_private_file "$root/runtime.json" "bound runtime"
  require_private_file "$root/terms.json" "bound terms"
  require_private_file "$root/capability" "sidecar capability"
  require_private_file "$root/probe-request.json" "runtime probe request"
  require_private_owned_file "$root/sidecar.log" "sidecar log"
  [[ "$(sha256sum "$root/runtime.json" | awk '{print $1}')" == "$(jq -er '.runtime_sha256' "$manifest")" ]] ||
    fail "runtime hash differs from PID manifest"
  [[ "$(sha256sum "$root/terms.json" | awk '{print $1}')" == "$(jq -er '.terms_sha256' "$manifest")" ]] ||
    fail "terms hash differs from PID manifest"
}

emit_result() {
  local action="$1" status="$2" root="$3" identity="$4" authenticated="$5"
  local role run endpoint pid state_directory state_mode tag13_handoff_receipt_sha
  role="$(jq -er '.role' "$root/pid-manifest.json")"
  run="$(jq -er '.run_id' "$root/pid-manifest.json")"
  endpoint="$(jq -er '.endpoint' "$root/pid-manifest.json")"
  pid="$(jq -er '.pid' "$root/pid-manifest.json")"
  state_directory="$(jq -er .state_directory "$root/pid-manifest.json")"
  state_mode="$(jq -er .state_directory_mode "$root/pid-manifest.json")"
  tag13_handoff_receipt_sha="$(jq -r '.tag13_handoff_receipt_sha256 // ""' \
    "$root/pid-manifest.json")"
  jq -cn --arg schema "$RESULT_SCHEMA" --arg action "$action" --arg status "$status" \
    --arg root "$root" --arg role "$role" --arg run "$run" --arg endpoint "$endpoint" \
    --arg state_directory "$state_directory" --arg state_mode "$state_mode" \
    --arg receipt_sha "$tag13_handoff_receipt_sha" \
    --argjson pid "$pid" --argjson identity "$identity" --argjson authenticated "$authenticated" '
    {schema:$schema,action:$action,status:$status,root:$root,role:$role,run_id:$run,
     endpoint:$endpoint,pid:$pid,state_directory:$state_directory,
     state_directory_mode:$state_mode,
     tag13_handoff_receipt_bound:($state_mode == "adopted_exact_existing_tag13"),
     tag13_handoff_receipt_sha256:(if $receipt_sha == "" then null else $receipt_sha end),
     identity_matched:$identity,authenticated:$authenticated,
     health_and_authentication_proved:($status == "running" and $identity and $authenticated)}
  '
}

start_sidecar() {
  local root="" role="" run_id="" sidecar_bin="" sequencer_url="" indexer_url=""
  local runtime_file="" terms_file="" private_key_file="" transfer_program="" adopted_state_directory=""
  local tag13_handoff_receipt=""
  local option
  declare -A seen_options=()
  while (($#)); do
    option="$1"
    case "$option" in
      --root|--role|--run-id|--sidecar-bin|--sequencer-url|--indexer-url|--runtime-file|\
        --terms-file|--private-key-file|--authenticated-transfer-program-id|--adopt-state-directory|--tag13-handoff-receipt)
        [[ -z "${seen_options[$option]+present}" ]] || fail "duplicate start option: $option"
        seen_options["$option"]=1
        [[ $# -ge 2 ]] || usage
        ;;
      *) usage ;;
    esac
    case "$option" in
      --root) root="$2" ;;
      --role) role="$2" ;;
      --run-id) run_id="$2" ;;
      --sidecar-bin) sidecar_bin="$2" ;;
      --sequencer-url) sequencer_url="$2" ;;
      --indexer-url) indexer_url="$2" ;;
      --runtime-file) runtime_file="$2" ;;
      --terms-file) terms_file="$2" ;;
      --private-key-file) private_key_file="$2" ;;
      --authenticated-transfer-program-id) transfer_program="$2" ;;
      --adopt-state-directory) adopted_state_directory="$2" ;;
      --tag13-handoff-receipt) tag13_handoff_receipt="$2" ;;
    esac
    shift 2
  done
  [[ -n "$root" && -n "$role" && -n "$run_id" && -n "$sidecar_bin" &&
    -n "$sequencer_url" && -n "$indexer_url" && -n "$runtime_file" &&
    -n "$terms_file" && -n "$private_key_file" && -n "$transfer_program" ]] || usage
  require_absolute_path "$root" "root"
  require_absolute_path "$sidecar_bin" "sidecar binary"
  require_absolute_path "$runtime_file" "runtime file"
  require_absolute_path "$terms_file" "terms file"
  require_absolute_path "$private_key_file" "private key file"
  if [[ -n "$adopted_state_directory" ]]; then
    require_absolute_path "$adopted_state_directory" "adopted state directory"
    [[ -n "$tag13_handoff_receipt" ]] ||
      fail "adopted state requires one typed Tag13 handoff receipt"
  else
    [[ -z "$tag13_handoff_receipt" ]] ||
      fail "typed Tag13 handoff receipt is forbidden without adopted state"
  fi
  if [[ -n "$tag13_handoff_receipt" ]]; then
    require_absolute_path "$tag13_handoff_receipt" "Tag13 handoff receipt"
  fi
  require_role "$role"
  require_safe_run_id "$run_id"
  require_loopback_url "$sequencer_url" "sequencer URL"
  require_loopback_url "$indexer_url" "indexer URL"
  sequencer_url="${sequencer_url%/}"
  indexer_url="${indexer_url%/}"
  [[ "$sequencer_url" != "$indexer_url" ]] || fail "sequencer and indexer URLs must be distinct"
  require_hex32 "$transfer_program" "authenticated-transfer program ID"
  require_sidecar_binary "$sidecar_bin"
  require_private_file "$runtime_file" "runtime input"
  require_private_file "$terms_file" "terms input"
  require_private_file "$private_key_file" "sidecar private key"
  validate_private_key "$private_key_file"
  validate_runtime_and_terms "$runtime_file" "$terms_file" "$role" "$transfer_program"

  local parent root_name planned_root state_directory state_mode state_identity
  local state_device state_inode tag13_handoff_receipt_sha=""
  local tag13_artifact_directory tag13_artifact_identity
  parent="$(dirname "$root")"
  require_private_directory "$parent" "sidecar-root parent"
  root_name="${root##*/}"
  [[ -n "$root_name" && "$root_name" != . && "$root_name" != .. ]] ||
    fail "sidecar root must name one new directory"
  planned_root="${parent%/}/${root_name}"
  [[ "$root" == "$planned_root" ]] ||
    fail "sidecar root must use its canonical path"
  [[ ! -e "$root" && ! -L "$root" ]] || fail "sidecar root already exists"
  if [[ -n "$adopted_state_directory" ]]; then
    [[ "$role" == taker ]] || fail "only Taker may adopt existing Tag13 state"
    require_private_directory "$adopted_state_directory" "adopted Tag13 state directory"
    require_private_file "$adopted_state_directory/m4-xmr-stage-a-tag13-evidence.v2.json" \
      "adopted Tag13 evidence"
    require_private_file "$tag13_handoff_receipt" "typed Tag13 handoff receipt"
    tag13_artifact_directory="$(dirname "$tag13_handoff_receipt")"
    require_private_directory "$tag13_artifact_directory" "typed Tag13 artifact directory"
    [[ "$tag13_handoff_receipt" == "$tag13_artifact_directory/tag13-handoff-receipt.json" ]] ||
      fail "typed Tag13 handoff receipt does not use its fixed artifact path"
    [[ "$runtime_file" == "$tag13_artifact_directory/taker-runtime.json" ]] ||
      fail "adopted runtime is not the fixed typed Taker artifact"
    [[ "$terms_file" == "$tag13_artifact_directory/terms.json" ]] ||
      fail "adopted terms are not the fixed typed artifact beside the receipt"
    tag13_artifact_identity="$(stat --format=%d:%i -- "$tag13_artifact_directory")"
    [[ "$(stat --format=%d:%i -- "$(dirname "$runtime_file")")" == "$tag13_artifact_identity" &&
      "$(stat --format=%d:%i -- "$(dirname "$terms_file")")" == "$tag13_artifact_identity" ]] ||
      fail "typed Tag13 runtime, terms, and receipt do not share one artifact directory identity"
    [[ "$tag13_handoff_receipt" != "$adopted_state_directory/"* ]] ||
      fail "typed Tag13 handoff receipt must be outside adopted state"
    tag13_handoff_receipt_sha="$(sha256sum "$tag13_handoff_receipt" | awk '{print $1}')"
    [[ "$adopted_state_directory" != "$planned_root" &&
      "$adopted_state_directory" != "$planned_root/"* &&
      "$planned_root" != "$adopted_state_directory/"* ]] ||
      fail "adopted Tag13 state must be separate from its supervisor root"
    state_directory="$adopted_state_directory"
    state_mode=adopted_exact_existing_tag13
  else
    state_directory="$root/state"
    state_mode=fresh_supervisor_owned
  fi
  mkdir -m 0700 -- "$root"
  if [[ "$state_mode" == fresh_supervisor_owned ]]; then
    mkdir -m 0700 -- "$state_directory"
  fi
  require_private_directory "$state_directory" "sidecar state directory"
  state_identity="$(stat --format=%d:%i -- "$state_directory")"
  [[ "$state_identity" =~ ^[0-9]+:[1-9][0-9]*$ ]] || fail "invalid state directory identity"
  IFS=: read -r state_device state_inode <<<"$state_identity"
  local -a tag13_handoff_arguments=()
  local sidecar_runtime_file="$root/runtime.json"
  if [[ "$state_mode" == adopted_exact_existing_tag13 ]]; then
    tag13_handoff_arguments=(--tag13-handoff-receipt "$tag13_handoff_receipt")
    sidecar_runtime_file="$runtime_file"
  fi
  copy_private_input "$runtime_file" "$root/runtime.json" "runtime input"
  copy_private_input "$terms_file" "$root/terms.json" "terms input"
  publish_private_output "$root/capability" openssl rand -hex 32
  : >"$root/sidecar.log"
  chmod 0600 "$root/sidecar.log"
  create_probe_request "$root/probe-request.json" "$run_id" "$role"

  local port endpoint binary_sha pid="" start_ticks="" executable="" executable_device=""
  local executable_inode="" executable_sha="" readiness="" readiness_valid=false
  local launcher_pid="$$" spawn_pending=false owned_identity_captured=false
  port="$(allocate_loopback_port)"
  [[ "$port" =~ ^[1-9][0-9]*$ && "$port" -le 65535 ]] || fail "invalid allocated port"
  endpoint="http://127.0.0.1:$port"
  binary_sha="$(sha256sum "$sidecar_bin" | awk '{print $1}')"

  capture_owned_spawn_identity() {
    local candidate="$pid" observed_start observed_parent
    if [[ -z "$candidate" && "$spawn_pending" == true ]]; then candidate="$!"; fi
    [[ "$candidate" =~ ^[1-9][0-9]*$ ]] || return 1
    for _ in {1..20}; do
      if [[ -r "/proc/$candidate/stat" ]]; then
        observed_start="$(process_start_ticks "$candidate" 2>/dev/null || true)"
        observed_parent="$(process_parent_pid "$candidate" 2>/dev/null || true)"
        if [[ "$observed_start" =~ ^[1-9][0-9]*$ && "$observed_parent" == "$launcher_pid" ]]; then
          if [[ -n "$start_ticks" && "$start_ticks" != "$observed_start" ]]; then return 1; fi
          pid="$candidate"
          start_ticks="$observed_start"
          owned_identity_captured=true
          return 0
        fi
      else
        return 1
      fi
      sleep 0.01
    done
    return 1
  }

  cleanup_start_process() {
    if [[ "$owned_identity_captured" != true ]]; then
      capture_owned_spawn_identity || {
        if [[ -n "$pid" && -r "/proc/$pid/stat" ]]; then
          echo "M4 LEZ sidecar launcher: refusing unproven failed-start process cleanup for PID $pid" >&2
          return 1
        fi
        return 0
      }
    fi
    terminate_owned_child "$pid" "$start_ticks" "$launcher_pid"
  }

  cleanup_failed_start() {
    local result=$?
    trap - EXIT INT TERM
    if [[ "$result" -ne 0 ]]; then
      cleanup_start_process ||
        echo "M4 LEZ sidecar launcher: exact failed-start cleanup could not be completed" >&2
    fi
    exit "$result"
  }

  handle_start_signal() {
    local signal_name="$1" result="$2"
    trap - INT TERM
    cleanup_start_process ||
      echo "M4 LEZ sidecar launcher: exact cleanup after $signal_name could not be completed" >&2
    trap - EXIT
    exit "$result"
  }

  trap cleanup_failed_start EXIT
  trap 'handle_start_signal INT 130' INT
  trap 'handle_start_signal TERM 143' TERM
  spawn_pending=true
  "$sidecar_bin" --listen-address "127.0.0.1:$port" --node-profile local \
    --sequencer-url "$sequencer_url" --indexer-url "$indexer_url" --run-id "$run_id" \
    --runtime-file "$sidecar_runtime_file" --terms-file "$root/terms.json" --capability-file "$root/capability" \
    --private-key-file "$private_key_file" --state-directory "$state_directory" \
    --authenticated-transfer-program-id "$transfer_program" "${tag13_handoff_arguments[@]}" >"$root/sidecar.log" 2>&1 &
  pid=$!
  spawn_pending=false
  capture_owned_spawn_identity || fail "sidecar child ownership could not be captured immediately after spawn"

  local observed_identity observed_sha
  for _ in {1..200}; do
    owned_child_identity_matches "$pid" "$start_ticks" "$launcher_pid" ||
      fail "sidecar exited or lost owned-child identity before executable capture"
    executable="$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)"
    observed_identity="$(stat -Lc '%d:%i' -- "/proc/$pid/exe" 2>/dev/null || true)"
    observed_sha="$(sha256sum "/proc/$pid/exe" 2>/dev/null | awk '{print $1}' || true)"
    if [[ "$executable" == "$sidecar_bin" && "$observed_identity" =~ ^[0-9]+:[1-9][0-9]*$ &&
          "$observed_sha" == "$binary_sha" ]]; then
      IFS=: read -r executable_device executable_inode <<<"$observed_identity"
      executable_sha="$observed_sha"
      break
    fi
    sleep 0.05
  done
  [[ -n "$executable_device" && -n "$executable_inode" && -n "$executable_sha" ]] ||
    fail "immutable sidecar executable identity is unavailable"

  for _ in {1..400}; do
    process_identity_matches "$pid" "$start_ticks" "$executable_device" "$executable_inode" \
      "$executable_sha" ||
      fail "sidecar exited or changed identity before readiness"
    readiness="$(head -n 1 "$root/sidecar.log" 2>/dev/null || true)"
    if jq -e --arg endpoint "$endpoint" --arg run "$run_id" --arg role "$role" \
      --slurpfile runtime "$root/runtime.json" '
      .event == "ready" and .endpoint == $endpoint and .run_id == $run and
      .runtime == $runtime[0] and .runtime.sidecar_role == $role and .node_profile == "local" and
      .sequencer_observation == "bounded_canonical_inclusion_and_same_tip_accounts" and
      .indexer_health == "stable_finalized_tip_bound_to_runtime_genesis" and
      .finality == "exact_genesis_bound_finalized_indexer_clock_available"
    ' <<<"$readiness" >/dev/null 2>&1; then
      readiness_valid=true
      break
    fi
    sleep 0.05
  done
  [[ "$readiness_valid" == true ]] || fail "sidecar did not emit exact bound readiness"
  require_private_directory "$state_directory" "sidecar state directory after readiness"
  [[ "$(stat --format=%d:%i -- "$state_directory")" == "$state_device:$state_inode" ]] ||
    fail "sidecar state directory identity changed during launch"
  if [[ "$state_mode" == adopted_exact_existing_tag13 ]]; then
    require_private_file "$tag13_handoff_receipt" "typed Tag13 handoff receipt after readiness"
    [[ "$(sha256sum "$tag13_handoff_receipt" | awk '{print $1}')" == "$tag13_handoff_receipt_sha" ]] ||
      fail "typed Tag13 handoff receipt changed during launch"
  fi
  prove_authentication "$root" "$endpoint" "$run_id" "$role"
  local capability
  capability="$(tr -d '\r\n' <"$root/capability")"
  if grep -Fq "$capability" "$root/sidecar.log"; then fail "sidecar log disclosed capability"; fi

  local runtime_sha terms_sha readiness_sha manifest="$root/pid-manifest.json"
  runtime_sha="$(sha256sum "$root/runtime.json" | awk '{print $1}')"
  terms_sha="$(sha256sum "$root/terms.json" | awk '{print $1}')"
  readiness_sha="$(printf '%s\n' "$readiness" | sha256sum | awk '{print $1}')"
  publish_private_output "$manifest" jq -cn --arg schema "$MANIFEST_SCHEMA" --arg role "$role" \
    --arg run "$run_id" --arg endpoint "$endpoint" --arg root "$root" \
    --arg state_directory "$state_directory" --arg state_mode "$state_mode" \
    --arg state_device "$state_device" --arg state_inode "$state_inode" \
    --arg tag13_receipt "$tag13_handoff_receipt" \
    --arg tag13_receipt_sha "$tag13_handoff_receipt_sha" \
    --argjson pid "$pid" --arg start "$start_ticks" --arg executable "$executable" \
    --arg binary "$sidecar_bin" --arg binary_sha "$binary_sha" \
    --arg executable_device "$executable_device" --arg executable_inode "$executable_inode" \
    --arg executable_sha "$executable_sha" --arg runtime_sha "$runtime_sha" \
    --arg terms_sha "$terms_sha" --arg readiness_sha "$readiness_sha" \
    --arg capability_scope "$CAPABILITY_SCOPE" --arg terms_enforcement "$TERMS_ENFORCEMENT" '
    {schema:$schema,role:$role,run_id:$run,root:$root,endpoint:$endpoint,
     state_directory:$state_directory,state_directory_mode:$state_mode,
     state_directory_device:$state_device,state_directory_inode:$state_inode,
     tag13_handoff_receipt:(if $state_mode == "adopted_exact_existing_tag13"
       then $tag13_receipt else null end),
     tag13_handoff_receipt_sha256:(if $state_mode == "adopted_exact_existing_tag13"
       then $tag13_receipt_sha else null end),
     listener_scope:"dynamic_literal_loopback",pid:$pid,start_ticks:$start,
     executable:$executable,sidecar_binary:$binary,binary_sha256:$binary_sha,
     executable_device:$executable_device,executable_inode:$executable_inode,
     executable_sha256:$executable_sha,
     runtime_sha256:$runtime_sha,terms_sha256:$terms_sha,readiness_sha256:$readiness_sha,
     health_gate:"sidecar_ready_after_genesis_bound_finalized_indexer_and_sequencer_checks",
     authentication:{wrong_capability_rejected:true,wrong_run_rejected:true,wrong_role_rejected:true,
       authenticated_runtime_matched:true},capability_scope:$capability_scope,
     release_only_capability_enforced:false,terms_enforcement:$terms_enforcement,
     public_resources_used:false}
  '
  if grep -Fq "$capability" "$manifest"; then fail "PID manifest disclosed capability"; fi
  trap - EXIT INT TERM
  emit_result start running "$root" true true
}

status_sidecar() {
  local root=""
  [[ "${1:-}" == --root && -n "${2:-}" && $# -eq 2 ]] || usage
  root="$2"
  load_manifest "$root"
  local pid start executable_device executable_inode executable_sha endpoint run role
  pid="$(jq -er '.pid' "$root/pid-manifest.json")"
  start="$(jq -er '.start_ticks' "$root/pid-manifest.json")"
  executable_device="$(jq -er '.executable_device' "$root/pid-manifest.json")"
  executable_inode="$(jq -er '.executable_inode' "$root/pid-manifest.json")"
  executable_sha="$(jq -er '.executable_sha256' "$root/pid-manifest.json")"
  endpoint="$(jq -er '.endpoint' "$root/pid-manifest.json")"
  run="$(jq -er '.run_id' "$root/pid-manifest.json")"
  role="$(jq -er '.role' "$root/pid-manifest.json")"
  if [[ ! -r "/proc/$pid/stat" || "$(process_state "$pid" || true)" == Z ]]; then
    emit_result status stopped "$root" false false
    return 3
  fi
  process_identity_matches "$pid" "$start" "$executable_device" "$executable_inode" \
    "$executable_sha" || {
    emit_result status identity_mismatch "$root" false false
    return 4
  }
  prove_authentication "$root" "$endpoint" "$run" "$role"
  emit_result status running "$root" true true
}

stop_sidecar() {
  local root=""
  [[ "${1:-}" == --root && -n "${2:-}" && $# -eq 2 ]] || usage
  root="$2"
  load_manifest "$root"
  local pid start executable_device executable_inode executable_sha signal
  pid="$(jq -er '.pid' "$root/pid-manifest.json")"
  start="$(jq -er '.start_ticks' "$root/pid-manifest.json")"
  executable_device="$(jq -er '.executable_device' "$root/pid-manifest.json")"
  executable_inode="$(jq -er '.executable_inode' "$root/pid-manifest.json")"
  executable_sha="$(jq -er '.executable_sha256' "$root/pid-manifest.json")"
  if [[ ! -r "/proc/$pid/stat" || "$(process_state "$pid" || true)" == Z ]]; then
    emit_result stop already_stopped "$root" false false
    return 0
  fi
  process_identity_matches "$pid" "$start" "$executable_device" "$executable_inode" \
    "$executable_sha" ||
    fail "refusing to signal a process whose exact identity does not match the manifest"
  for signal in INT TERM KILL; do
    process_identity_matches "$pid" "$start" "$executable_device" "$executable_inode" \
      "$executable_sha" || break
    kill -s "$signal" "$pid" || fail "exact $signal signal failed"
    for _ in {1..100}; do
      if [[ ! -r "/proc/$pid/stat" || "$(process_state "$pid" || true)" == Z ]]; then break 2; fi
      sleep 0.05
    done
  done
  if process_identity_matches "$pid" "$start" "$executable_device" "$executable_inode" \
    "$executable_sha"; then
    fail "exact sidecar process survived scoped stop"
  fi
  emit_result stop stopped "$root" true false
}

main() {
  require_commands
  local action="${1:-}"
  [[ -n "$action" ]] || usage
  shift
  case "$action" in
    start) start_sidecar "$@" ;;
    status) status_sidecar "$@" ;;
    stop) stop_sidecar "$@" ;;
    *) usage ;;
  esac
}

main "$@"
