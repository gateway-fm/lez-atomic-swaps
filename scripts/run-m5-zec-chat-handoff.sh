#!/usr/bin/env bash
set -euo pipefail

export LC_ALL=C
umask 077

fail() {
  printf 'M5 ZEC Chat handoff failed: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf '%s\n' \
    'usage: run-m5-zec-chat-handoff.sh' \
    '  --run-id ID --source-actors-root DIR --source-provision-summary FILE' \
    '  --output-actors-root NEW_DIR --application-root NEW_DIR --evidence-dir DIR' \
    '  --maker-daemon-bin FILE --maker-cli-bin FILE --taker-bin FILE' \
    '  --draft-bin FILE --finalize-bin FILE'
}

run_id=''
source_actors_root=''
source_provision_summary=''
output_actors_root=''
application_root=''
evidence_dir=''
maker_daemon_bin=''
maker_cli_bin=''
taker_bin=''
draft_bin=''
finalize_bin=''
while (( $# > 0 )); do
  case "$1" in
    --run-id) run_id="${2:-}"; shift 2 ;;
    --source-actors-root) source_actors_root="${2:-}"; shift 2 ;;
    --source-provision-summary) source_provision_summary="${2:-}"; shift 2 ;;
    --output-actors-root) output_actors_root="${2:-}"; shift 2 ;;
    --application-root) application_root="${2:-}"; shift 2 ;;
    --evidence-dir) evidence_dir="${2:-}"; shift 2 ;;
    --maker-daemon-bin) maker_daemon_bin="${2:-}"; shift 2 ;;
    --maker-cli-bin) maker_cli_bin="${2:-}"; shift 2 ;;
    --taker-bin) taker_bin="${2:-}"; shift 2 ;;
    --draft-bin) draft_bin="${2:-}"; shift 2 ;;
    --finalize-bin) finalize_bin="${2:-}"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; fail "unknown argument $1" ;;
  esac
done

for value in run_id source_actors_root source_provision_summary output_actors_root \
  application_root evidence_dir maker_daemon_bin maker_cli_bin taker_bin draft_bin \
  finalize_bin; do
  [[ -n "${!value}" ]] || fail "missing --${value//_/-}"
done
[[ "$run_id" =~ ^[a-z0-9][a-z0-9_-]{7,47}$ ]] || fail 'run ID is unsafe'
for path in "$source_actors_root" "$source_provision_summary" \
  "$output_actors_root" "$application_root" "$evidence_dir" \
  "$maker_daemon_bin" "$maker_cli_bin" "$taker_bin" "$draft_bin" \
  "$finalize_bin"; do
  [[ "$path" == /* ]] || fail "path must be absolute: $path"
done
[[ -d "$source_actors_root" && ! -L "$source_actors_root" ]] || \
  fail 'source actors root is unavailable or unsafe'
[[ -d "$evidence_dir" && ! -L "$evidence_dir" ]] || \
  fail 'evidence directory is unavailable or unsafe'
[[ ! -e "$output_actors_root" && ! -L "$output_actors_root" ]] || \
  fail 'output actors root already exists'
[[ ! -e "$application_root" && ! -L "$application_root" ]] || \
  fail 'application root already exists'
for binary in "$maker_daemon_bin" "$maker_cli_bin" "$taker_bin" "$draft_bin" \
  "$finalize_bin"; do
  [[ -f "$binary" && -x "$binary" && ! -L "$binary" ]] || \
    fail "binary is unavailable or unsafe: $binary"
done
for private_file in \
  "$source_provision_summary" \
  "$source_actors_root/shared/agreement-v2.borsh" \
  "$source_actors_root/maker/actor-config.json" \
  "$source_actors_root/taker/actor-config.json" \
  "$source_actors_root/maker/zcash.key" \
  "$source_actors_root/taker/zcash.key" \
  "$source_actors_root/maker/claim-recovery.key" \
  "$source_actors_root/maker/claim-preimage.key"; do
  [[ -f "$private_file" && ! -L "$private_file" ]] || \
    fail "private input is unavailable or unsafe: $private_file"
  [[ "$(stat -c %a -- "$private_file")" == 600 ]] || \
    fail "private input is not mode 0600: $private_file"
  [[ "$(stat -c %h -- "$private_file")" == 1 ]] || \
    fail "private input has multiple links: $private_file"
done

for command in awk chmod cut date jq kill mkdir mv readlink sha256sum sleep stat; do
  command -v "$command" >/dev/null || fail "required command unavailable: $command"
done

mkdir -m 0700 "$application_root"
runtime_root="$application_root/runtime"
mkdir -m 0700 "$runtime_root"
maker_socket="$runtime_root/maker.sock"
chat_socket="$runtime_root/chat.sock"
ready_file="$runtime_root/ready"
restart_ready_file="$runtime_root/ready-restart"
database="$application_root/maker.sqlite3"
delivery_directory="$application_root/delivery"
delivery_offline="$application_root/delivery-offline"
daemon_log="$evidence_dir/m5-maker-daemon.log"
daemon_restart_log="$evidence_dir/m5-maker-daemon-restart.log"
draft_file="$application_root/unsigned-draft.borsh"
agreement_file="$application_root/final-agreement.borsh"
draft_receipt="$evidence_dir/m5-chat-draft.json"
finalize_receipt="$evidence_dir/m5-chat-finalize.json"
discovery_receipt="$evidence_dir/m5-delivery-discovery.json"
taker_receipt="$evidence_dir/m5-taker-acceptance.json"
result_receipt="$evidence_dir/m5-chat-handoff.json"

token="$(printf '%s' "$run_id" | sha256sum)"
token="${token%% *}"
token="${token:0:16}"
offer_id="m5-offer-${token}"
reservation_id="m5-reserve-${token}"
readonly token offer_id reservation_id
readonly foreign_units=100000000
readonly offer_ttl_seconds=300

daemon_pid=''
daemon_start_ticks=''

process_start_ticks() {
  local pid="$1"
  awk '{print $22}' "/proc/${pid}/stat" 2>/dev/null
}

daemon_is_owned() {
  [[ -n "$daemon_pid" && -n "$daemon_start_ticks" && \
     -r "/proc/${daemon_pid}/stat" ]] || return 1
  [[ "$(process_start_ticks "$daemon_pid")" == "$daemon_start_ticks" ]] || return 1
  [[ "$(readlink -f "/proc/${daemon_pid}/exe" 2>/dev/null)" == "$maker_daemon_bin" ]]
}

stop_daemon() {
  if ! daemon_is_owned; then
    daemon_pid=''
    daemon_start_ticks=''
    return 0
  fi
  kill -INT "$daemon_pid" 2>/dev/null || true
  for _ in {1..200}; do
    if ! daemon_is_owned; then
      wait "$daemon_pid" 2>/dev/null || true
      daemon_pid=''
      daemon_start_ticks=''
      return 0
    fi
    sleep 0.05
  done
  kill -TERM "$daemon_pid" 2>/dev/null || true
  for _ in {1..100}; do
    if ! daemon_is_owned; then
      wait "$daemon_pid" 2>/dev/null || true
      daemon_pid=''
      daemon_start_ticks=''
      return 0
    fi
    sleep 0.05
  done
  kill -KILL "$daemon_pid" 2>/dev/null || true
  for _ in {1..100}; do
    if ! daemon_is_owned; then
      wait "$daemon_pid" 2>/dev/null || true
      daemon_pid=''
      daemon_start_ticks=''
      return 0
    fi
    sleep 0.05
  done
  fail 'maker daemon did not terminate after SIGKILL'
}

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  if daemon_is_owned; then
    kill -INT "$daemon_pid" 2>/dev/null || true
    for _ in {1..100}; do
      daemon_is_owned || break
      sleep 0.05
    done
    if daemon_is_owned; then
      kill -TERM "$daemon_pid" 2>/dev/null || true
      for _ in {1..100}; do
        daemon_is_owned || break
        sleep 0.05
      done
    fi
    if daemon_is_owned; then
      kill -KILL "$daemon_pid" 2>/dev/null || true
      for _ in {1..100}; do
        daemon_is_owned || break
        sleep 0.05
      done
    fi
    if daemon_is_owned; then
      printf 'M5 owned maker daemon did not terminate after SIGKILL\n' >&2
    else
      wait "$daemon_pid" 2>/dev/null || true
    fi
  fi
  if (( status != 0 )); then
    printf 'M5 private handoff diagnostics retained at %s\n' "$application_root" >&2
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

start_daemon() {
  local ready="$1"
  local log="$2"
  "$maker_daemon_bin" \
    --socket "$maker_socket" \
    --chat-socket "$chat_socket" \
    --database "$database" \
    --ready-file "$ready" \
    --delivery-directory "$delivery_directory" \
    --delivery-signing-key-file "$source_actors_root/maker/zcash.key" \
    --maker-claim-key-id "${run_id}-maker-claim" \
    --maker-claim-key-file "$source_actors_root/maker/claim-recovery.key" \
    --maker-claim-preimage-file "$source_actors_root/maker/claim-preimage.key" \
    >"$log" 2>&1 &
  daemon_pid=$!
  daemon_start_ticks="$(process_start_ticks "$daemon_pid")"
  [[ -n "$daemon_start_ticks" ]] || fail 'maker daemon identity unavailable'
  for _ in {1..200}; do
    if [[ -s "$ready" ]]; then
      [[ "$(<"$ready")" == "$maker_socket" ]] || fail 'maker readiness path mismatch'
      return 0
    fi
    daemon_is_owned || fail 'maker daemon exited before readiness'
    sleep 0.05
  done
  fail 'maker daemon readiness timed out'
}

maker_public_key="$(jq -er '.maker_zcash_public_key | strings | select(test("^[0-9a-f]{66}$"))' \
  "$source_provision_summary")"
taker_public_key="$(jq -er '.taker_zcash_public_key | strings | select(test("^[0-9a-f]{66}$"))' \
  "$source_provision_summary")"
source_swap_id="$(jq -er '.agreement_file as $agreement | .signed_agreement_sha256 as $sha | .direction as $direction | select($agreement | strings) | select($sha | test("^[0-9a-f]{64}$")) | select($direction == "taker_sells_lez") | $agreement' \
  "$source_provision_summary")"
[[ "$source_swap_id" == "$source_actors_root/shared/agreement-v2.borsh" ]] || \
  fail 'source agreement path or direction mismatch'
[[ "$maker_public_key" != "$taker_public_key" ]] || fail 'role public keys are not distinct'

start_daemon "$ready_file" "$daemon_log"

"$maker_cli_bin" --socket "$maker_socket" configure-pair \
  --request-id "m5-config-off-${token}" --pair zcash --direction taker-sells-lez \
  --enabled false --price-source local --minimum-foreign-units "$foreign_units" \
  --maximum-foreign-units "$foreign_units" --offer-ttl-seconds "$offer_ttl_seconds" \
  >"$evidence_dir/m5-configure-disabled.json"
"$maker_cli_bin" --socket "$maker_socket" set-local-price \
  --request-id "m5-price-${token}" --pair zcash --direction taker-sells-lez \
  --lez-units-per-lot 1 --foreign-units-per-lot 2000 \
  >"$evidence_dir/m5-set-price.json"
"$maker_cli_bin" --socket "$maker_socket" configure-pair \
  --request-id "m5-config-on-${token}" --expected-revision 1 --pair zcash \
  --direction taker-sells-lez --enabled true --price-source local \
  --minimum-foreign-units "$foreign_units" --maximum-foreign-units "$foreign_units" \
  --offer-ttl-seconds "$offer_ttl_seconds" \
  >"$evidence_dir/m5-configure-enabled.json"
"$maker_cli_bin" --socket "$maker_socket" publish-offer \
  --request-id "m5-publish-${token}" --offer-id "$offer_id" --pair zcash \
  --direction taker-sells-lez >"$evidence_dir/m5-publish-offer.json"

accepted_at="$(date -u +%s)"
"$taker_bin" --delivery-directory "$delivery_directory" \
  --maker-public-key "$maker_public_key" --now-unix-seconds "$accepted_at" \
  --pair zcash --direction taker-sells-lez >"$discovery_receipt"
jq -e --arg offer "$offer_id" --arg maker "$maker_public_key" '
  .schema_version == 1 and (.offers | length) == 1
  and .offers[0].offer.id == $offer
  and .offers[0].maker_public_key == $maker
  and (.offers[0].signed_envelope_sha256 | test("^[0-9a-f]{64}$"))
  and .offers[0].offer.pair_configuration.route.pair == "Zcash"
  and .offers[0].offer.pair_configuration.route.direction == "TakerSellsLez"
' "$discovery_receipt" >/dev/null
offer_commitment="$(jq -er '.offers[0].signed_envelope_sha256' "$discovery_receipt")"
offer_expires="$(jq -er '.offers[0].offer.expires_at_unix_seconds | numbers' "$discovery_receipt")"

"$draft_bin" --source-agreement-file "$source_actors_root/shared/agreement-v2.borsh" \
  --now-unix-seconds "$accepted_at" --reservation-id "$reservation_id" \
  --offer-commitment "$offer_commitment" \
  --offer-expires-at-unix-seconds "$offer_expires" --output-file "$draft_file" \
  >"$draft_receipt"
jq -e --arg maker "$maker_public_key" --arg reservation "$reservation_id" \
  --arg commitment "$offer_commitment" '
  .schema_version == 1 and .maker_public_key == $maker
  and .reservation_id == $reservation and .offer_commitment == $commitment
  and .private_material_disclosed == false
' "$draft_receipt" >/dev/null

"$taker_bin" --delivery-directory "$delivery_directory" \
  --maker-public-key "$maker_public_key" --now-unix-seconds "$accepted_at" \
  --pair zcash --direction taker-sells-lez --accept-zec-offer "$offer_id" \
  --chat-socket "$chat_socket" --reservation-id "$reservation_id" \
  --foreign-units "$foreign_units" --unsigned-draft-file "$draft_file" \
  --taker-signing-key-file "$source_actors_root/taker/zcash.key" \
  --agreement-output-file "$agreement_file" >"$taker_receipt"
jq -e --arg swap "$(jq -er '.swap_id' "$draft_receipt")" '
  .schema_version == 1 and .swap_id == $swap and .offer_revision == 3
  and .replay == {proposal:false,completion:false,agreement_file:false}
  and .private_material_disclosed == false
  and (.agreement_sha256 | test("^[0-9a-f]{64}$"))
' "$taker_receipt" >/dev/null

"$finalize_bin" --source-maker-config "$source_actors_root/maker/actor-config.json" \
  --source-taker-config "$source_actors_root/taker/actor-config.json" \
  --final-agreement-file "$agreement_file" --accepted-at-unix-seconds "$accepted_at" \
  --output-root "$output_actors_root" >"$finalize_receipt"
jq -e --arg swap "$(jq -er '.swap_id' "$taker_receipt")" '
  .schema_version == 1 and .swap_id == $swap
  and .private_material_disclosed == false and .actor_pair_validated == true
  and (.signed_agreement_sha256 | test("^[0-9a-f]{64}$"))
' "$finalize_receipt" >/dev/null
[[ "$(sha256sum "$agreement_file" | cut -d ' ' -f1)" == \
    "$(jq -er '.signed_agreement_sha256' "$finalize_receipt")" ]] || \
  fail 'final agreement hash mismatch'

stop_daemon
start_daemon "$restart_ready_file" "$daemon_restart_log"
"$maker_cli_bin" --socket "$maker_socket" pairs >"$evidence_dir/m5-pairs-after-restart.json"
"$maker_cli_bin" --socket "$maker_socket" prices >"$evidence_dir/m5-prices-after-restart.json"
"$maker_cli_bin" --socket "$maker_socket" offers >"$evidence_dir/m5-offers-after-restart.json"
"$maker_cli_bin" --socket "$maker_socket" history >"$evidence_dir/m5-history-after-restart.json"
jq -e '
  length == 1 and .[0].revision == 2 and .[0].value.enabled == true
  and .[0].value.route.pair == "Zcash"
  and .[0].value.route.direction == "TakerSellsLez"
' "$evidence_dir/m5-pairs-after-restart.json" >/dev/null
jq -e '
  length == 1 and .[0].revision == 1
  and .[0].value.route.pair == "Zcash"
  and .[0].value.route.direction == "TakerSellsLez"
  and .[0].value.lez_units_per_lot == 1
  and .[0].value.foreign_units_per_lot == 2000
' "$evidence_dir/m5-prices-after-restart.json" >/dev/null
jq -e --arg offer "$offer_id" '
  length == 1 and .[0].offer.id == $offer and .[0].status == "consumed"
' "$evidence_dir/m5-offers-after-restart.json" >/dev/null
jq -e --arg swap "$(jq -er '.swap_id' "$taker_receipt")" '
  length == 1 and .[0].id == $swap
' "$evidence_dir/m5-history-after-restart.json" >/dev/null

jq -n \
  --arg run_id "$run_id" --arg offer_id "$offer_id" --arg reservation_id "$reservation_id" \
  --arg swap_id "$(jq -er '.swap_id' "$taker_receipt")" \
  --arg agreement_sha256 "$(jq -er '.signed_agreement_sha256' "$finalize_receipt")" \
  --arg maker_public_key "$maker_public_key" --arg taker_public_key "$taker_public_key" \
  --arg maker_daemon_bin "$maker_daemon_bin" --arg maker_socket "$maker_socket" \
  --arg chat_socket "$chat_socket" --arg delivery_directory "$delivery_directory" \
  --arg delivery_offline "$delivery_offline" --argjson daemon_pid "$daemon_pid" \
  --arg daemon_start_ticks "$daemon_start_ticks" --argjson accepted_at "$accepted_at" '
  {
    schema_version: 1,
    kind: "m5_zec_chat_actor_handoff",
    result: "passed",
    run_id: $run_id,
    offer_id: $offer_id,
    reservation_id: $reservation_id,
    swap_id: $swap_id,
    accepted_at_unix_seconds: $accepted_at,
    agreement_sha256: $agreement_sha256,
    role_public_keys: {maker:$maker_public_key,taker:$taker_public_key},
    real_processes: {maker_daemon:true,maker_cli:true,taker_cli:true},
    actor_pair_validated: true,
    daemon_restart_history_validated: true,
    transport_cutover: {
      state:"armed_after_restart",
      maker_daemon_bin:$maker_daemon_bin,
      maker_daemon_pid:$daemon_pid,
      maker_daemon_start_ticks:$daemon_start_ticks,
      maker_socket:$maker_socket,
      chat_socket:$chat_socket,
      delivery_directory:$delivery_directory,
      delivery_offline:$delivery_offline
    },
    public_rpc_or_faucet_used: false,
    private_material_disclosed: false
  }' >"$result_receipt"
chmod 0600 "$result_receipt"
jq -e '.result == "passed" and .actor_pair_validated == true
  and .daemon_restart_history_validated == true
  and .transport_cutover.state == "armed_after_restart"
  and (.transport_cutover.maker_daemon_pid | numbers) > 1
  and (.transport_cutover.maker_daemon_start_ticks | test("^[0-9]+$"))
  and .public_rpc_or_faucet_used == false
  and .private_material_disclosed == false' "$result_receipt" >/dev/null

# Ownership passes to the corridor runner, which removes both transports only
# after observing and confirming the first lock.
daemon_pid=''
daemon_start_ticks=''
printf '%s\n' "$result_receipt"
