#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

for command in awk cargo install jq readlink sha256sum stat strip systemctl systemd-run; do
  command -v "$command" >/dev/null || {
    echo "required command is unavailable: $command" >&2
    exit 1
  }
done

started_at="$SECONDS"
readonly started_at
readonly run_id="lez-m5-systemd-${UID}-$$-${RANDOM}"
readonly unit_name="${run_id}.service"
readonly runtime_name="$run_id"
readonly user_runtime="${XDG_RUNTIME_DIR:-/run/user/${UID}}"
readonly runtime="$user_runtime/$runtime_name"
readonly credential_directory="$user_runtime/credentials/$unit_name"
run_root="$(mktemp -d "${TMPDIR:-/tmp}/${run_id}.XXXXXX")"
readonly run_root
readonly database="$run_root/maker.sqlite3"
readonly delivery="$run_root/delivery"
readonly signing_key="$run_root/delivery-signing.key"
readonly claim_key="$run_root/maker-claim-recovery.key"
readonly preimage="$run_root/maker-claim-preimage.key"
readonly socket="$runtime/maker.sock"
readonly chat_socket="$runtime/chat.sock"
readonly ready="$runtime/ready"
readonly pause_marker="$run_root/actor-paused.json"

cleanup() {
  systemctl --user stop "$unit_name" >/dev/null 2>&1 || true
  systemctl --user reset-failed "$unit_name" >/dev/null 2>&1 || true
  rm -rf -- "$run_root"
}
trap cleanup EXIT

write_secret() {
  local path="$1"
  local byte="$2"
  local index
  umask 077
  : >"$path"
  for ((index = 0; index < 32; index += 1)); do
    printf '%b' "\\x${byte}" >>"$path"
  done
  chmod 0600 "$path"
}

wait_active() {
  local deadline=$((SECONDS + 15))
  local state
  while ((SECONDS < deadline)); do
    state="$(systemctl --user show "$unit_name" --property=ActiveState --value 2>/dev/null || true)"
    if [[ "$state" == active && -S "$socket" && -f "$ready" ]]; then
      return
    fi
    if [[ "$state" == failed ]]; then
      systemctl --user status "$unit_name" --no-pager >&2 || true
      return 1
    fi
    sleep 0.05
  done
  echo "timed out waiting for $unit_name readiness" >&2
  return 1
}

wait_restarted() {
  local deadline=$((SECONDS + 15))
  local restarts state
  while ((SECONDS < deadline)); do
    state="$(systemctl --user show "$unit_name" --property=ActiveState --value 2>/dev/null || true)"
    restarts="$(systemctl --user show "$unit_name" --property=NRestarts --value 2>/dev/null || true)"
    if [[ "$state" == active && "$restarts" =~ ^[1-9][0-9]*$ && -S "$socket" && -f "$ready" ]]; then
      return
    fi
    if [[ "$state" == failed ]]; then
      systemctl --user status "$unit_name" --no-pager >&2 || true
      return 1
    fi
    sleep 0.05
  done
  echo "timed out waiting for $unit_name crash restart" >&2
  return 1
}

wait_for_pause_marker() {
  local deadline=$((SECONDS + 15))
  while ((SECONDS < deadline)); do
    if [[ -f "$pause_marker" ]] && jq -e --arg crash "$crash_swap_id" '
      .schema_version == 1
      and .state == "paused_after_submitted_before_stdout"
      and .role == "maker"
      and .operation == "zcash_fund"
      and .swap_id == $crash
      and (.process_id | numbers) > 1
    ' "$pause_marker" >/dev/null; then
      return
    fi
    sleep 0.05
  done
  echo "timed out waiting for submitted-effect pause" >&2
  return 1
}

wait_for_terminal_actors() {
  local deadline=$((SECONDS + 15)) evidence
  while ((SECONDS < deadline)); do
    evidence="$("$actor_store" inspect "$database")"
    if jq -e --arg crash "$crash_swap_id" --arg peer "$peer_swap_id" '
      ([.actors[] | select(.swap_id == $crash and .schedule_state == "terminal"
        and .lease_generation == 2 and .attempt_count == 2
        and .child_identity == null)] | length) == 1
      and ([.actors[] | select(.swap_id == $peer and .schedule_state == "terminal"
        and .lease_generation == 1 and .attempt_count == 1
        and .child_identity == null)] | length) == 1
    ' <<<"$evidence" >/dev/null; then
      printf '%s\n' "$evidence"
      return
    fi
    sleep 0.05
  done
  echo "timed out waiting for recovered and disjoint actor evidence" >&2
  return 1
}

cargo build --locked -p lez-maker-node --features test-crash-hooks \
  --bin lez-maker-daemon --bin lez-maker \
  --example m5-systemd-fault-actor --example m5-systemd-actor-store
chmod 0700 "$run_root"
write_secret "$signing_key" 08
write_secret "$claim_key" 7a
write_secret "$preimage" 44
install -D -m 0700 target/debug/examples/m5-systemd-fault-actor \
  "$run_root/bin/m5-systemd-fault-actor"
strip --strip-debug "$run_root/bin/m5-systemd-fault-actor"
chmod 0500 "$run_root/bin/m5-systemd-fault-actor"

daemon="$(realpath target/debug/lez-maker-daemon)"
readonly daemon
maker="$(realpath target/debug/lez-maker)"
readonly maker
actor_program="$(realpath "$run_root/bin/m5-systemd-fault-actor")"
readonly actor_program
actor_store="$(realpath target/debug/examples/m5-systemd-actor-store)"
readonly actor_store
actor_fixture_json="$($actor_store setup "$run_root" "$database" "$actor_program")"
readonly actor_fixture_json
actor_program_sha256="$(jq -er '.program_sha256' <<<"$actor_fixture_json")"
readonly actor_program_sha256
actor_source_config="$(jq -er '.source_config' <<<"$actor_fixture_json")"
readonly actor_source_config
actor_root="$(jq -er '.actor_root' <<<"$actor_fixture_json")"
readonly actor_root
crash_swap_id="$(jq -er '.crash_swap_id' <<<"$actor_fixture_json")"
readonly crash_swap_id
peer_swap_id="$(jq -er '.peer_swap_id' <<<"$actor_fixture_json")"
readonly peer_swap_id
effect_file="$(jq -er '.effect_file' <<<"$actor_fixture_json")"
readonly effect_file
systemd-run --user \
  --unit="$unit_name" \
  --property=Type=notify \
  --property=NotifyAccess=main \
  --property=Restart=on-failure \
  --property=RestartSec=100ms \
  --property=KillMode=control-group \
  --property="RuntimeDirectory=$runtime_name" \
  --property=RuntimeDirectoryMode=0700 \
  --property="LoadCredential=delivery-signing.key:$signing_key" \
  --property="LoadCredential=maker-claim-recovery.key:$claim_key" \
  --property="LoadCredential=maker-claim-preimage.key:$preimage" \
  "$daemon" \
  --socket "$socket" \
  --database "$database" \
  --ready-file "$ready" \
  --delivery-directory "$delivery" \
  --delivery-signing-key-file "$credential_directory/delivery-signing.key" \
  --chat-socket "$chat_socket" \
  --maker-claim-key-id transient-v1 \
  --maker-claim-key-file "$credential_directory/maker-claim-recovery.key" \
  --maker-claim-preimage-file "$credential_directory/maker-claim-preimage.key" \
  --actor-supervisor \
  --actor-attempt-timeout-milliseconds 30000 \
  --actor-poll-milliseconds 10 \
  --actor-requeue-delay-seconds 60 \
  --actor-failure-backoff-seconds 60 \
  --actor-test-pause-swap-id "$crash_swap_id" \
  --actor-test-pause-operation zcash_fund \
  --actor-test-pause-marker "$pause_marker" \
  --zec-source-maker-config "$actor_source_config" \
  --zec-maker-actor-root "$actor_root" \
  --zec-actor-program "$actor_program" \
  --zec-actor-program-sha256 "$actor_program_sha256"

wait_active
test "$(stat -c '%a' "$runtime")" = 700
test "$(stat -c '%a' "$socket")" = 600
test "$(stat -c '%a' "$ready")" = 600
"$maker" --socket "$socket" health |
  jq -e '.schema_version == 1 and .ready == true' >/dev/null

"$maker" --socket "$socket" configure-pair \
  --request-id transient-route-v1 \
  --pair zcash \
  --direction taker-sells-lez \
  --enabled false \
  --price-source local \
  --minimum-foreign-units 1 \
  --maximum-foreign-units 100000000 \
  --offer-ttl-seconds 300 >/dev/null

wait_for_pause_marker
test "$(stat -c '%a' "$pause_marker")" = 600
before_actor_evidence="$("$actor_store" inspect "$database")"
actor_pid="$(jq -er --arg crash "$crash_swap_id" '.actors[] | select(.swap_id == $crash) | .child_identity.pid' <<<"$before_actor_evidence")"
actor_start_ticks="$(jq -er --arg crash "$crash_swap_id" '.actors[] | select(.swap_id == $crash) | .child_identity.start_ticks' <<<"$before_actor_evidence")"
marker_pid="$(jq -er '.process_id | numbers' "$pause_marker")"
test "$marker_pid" = "$actor_pid"
jq -e --arg crash "$crash_swap_id" --arg peer "$peer_swap_id" --argjson pid "$actor_pid" '
  ([.actors[] | select(.swap_id == $crash and .schedule_state == "leased"
    and .lease_generation == 1 and .attempt_count == 1
    and .child_identity.pid == $pid)] | length) == 1
  and ([.actors[] | select(.swap_id == $peer and .schedule_state == "queued"
    and .lease_generation == 0 and .attempt_count == 0
    and .child_identity == null)] | length) == 1
' <<<"$before_actor_evidence" >/dev/null
test "$(awk '{print $22}' "/proc/${actor_pid}/stat")" = "$actor_start_ticks"
actor_executable="$(readlink "/proc/${actor_pid}/exe")"
[[ "$actor_executable" == "/memfd:lez-maker-actor-program (deleted)" ]]
read -r actor_executable_sha256 _ < <(sha256sum "/proc/${actor_pid}/exe")
test "$actor_executable_sha256" = "$actor_program_sha256"
test -e "/proc/${actor_pid}/fd/198"
test "$(stat -c '%a' "$effect_file")" = 600
jq -e --arg swap "$crash_swap_id" '
  .schema_version == 1
  and .kind == "node_free_scheduler_fixture_effect"
  and .swap_id == $swap
  and .submission_count == 1
' "$effect_file" >/dev/null
effect_sha256_before="$(sha256sum "$effect_file")"
effect_sha256_before="${effect_sha256_before%% *}"
effect_inode_before="$(stat -c '%d:%i' "$effect_file")"

systemctl --user kill --kill-whom=main --signal=SIGKILL "$unit_name"
wait_restarted
after_actor_evidence="$(wait_for_terminal_actors)"
if [[ -r "/proc/${actor_pid}/stat" ]] \
  && [[ "$(awk '{print $22}' "/proc/${actor_pid}/stat")" == "$actor_start_ticks" ]]; then
  echo "old actor PID/start-ticks identity survived control-group restart" >&2
  exit 1
fi
effect_sha256_after="$(sha256sum "$effect_file")"
effect_sha256_after="${effect_sha256_after%% *}"
effect_inode_after="$(stat -c '%d:%i' "$effect_file")"
effect_identity_preserved=false
if [[ "$effect_sha256_after" == "$effect_sha256_before" \
  && "$effect_inode_after" == "$effect_inode_before" ]]; then
  effect_identity_preserved=true
fi
disjoint_peer_progressed="$(jq -r --arg peer "$peer_swap_id" '
  any(.actors[]; .swap_id == $peer and .schedule_state == "terminal"
    and .lease_generation == 1 and .attempt_count == 1)
' <<<"$after_actor_evidence")"
jq -n --argjson effect_identity_preserved "$effect_identity_preserved" \
  --argjson disjoint_peer_progressed "$disjoint_peer_progressed" '
  {effect_identity_preserved:$effect_identity_preserved,
   disjoint_peer_progressed:$disjoint_peer_progressed}
  | select(.effect_identity_preserved == true
    and .disjoint_peer_progressed == true)
' >/dev/null
jq -e 'all(.actors[]; .schedule_state != "leased" and .child_identity == null)' \
  <<<"$after_actor_evidence" >/dev/null
"$maker" --socket "$socket" pairs |
  jq -e 'length == 1 and .[0].value.route.pair == "Zcash"' >/dev/null

restarts="$(systemctl --user show "$unit_name" --property=NRestarts --value)"
readonly restarts
systemctl --user stop "$unit_name"
test ! -e "$runtime"

printf 'M5 node-free scheduler/systemd actor crash proof passed: run_id=%s restarts=%s duration_seconds=%s runtime_external_resources=none actual_zcash_chain_certified=false\n' \
  "$run_id" "$restarts" "$((SECONDS - started_at))"
