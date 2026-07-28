#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

for command in cargo jq stat systemctl systemd-run; do
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

cargo build --locked -p lez-maker-node --bin lez-maker-daemon --bin lez-maker
chmod 0700 "$run_root"
write_secret "$signing_key" 08
write_secret "$claim_key" 7a
write_secret "$preimage" 44

daemon="$(realpath target/debug/lez-maker-daemon)"
readonly daemon
maker="$(realpath target/debug/lez-maker)"
readonly maker
systemd-run --user \
  --unit="$unit_name" \
  --property=Type=notify \
  --property=NotifyAccess=main \
  --property=Restart=on-failure \
  --property=RestartSec=100ms \
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
  --maker-claim-preimage-file "$credential_directory/maker-claim-preimage.key"

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

systemctl --user kill --kill-whom=main --signal=SIGKILL "$unit_name"
wait_restarted
"$maker" --socket "$socket" pairs |
  jq -e 'length == 1 and .[0].value.route.pair == "Zcash"' >/dev/null

restarts="$(systemctl --user show "$unit_name" --property=NRestarts --value)"
readonly restarts
systemctl --user stop "$unit_name"
test ! -e "$runtime"

printf 'M5 actual user-systemd lifecycle passed: run_id=%s restarts=%s duration_seconds=%s runtime_external_resources=none\n' \
  "$run_id" "$restarts" "$((SECONDS - started_at))"
