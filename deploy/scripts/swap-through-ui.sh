#!/usr/bin/env bash
# swap-through-ui.sh — one complete BTC → LEZ swap driven through the two
# Basecamp apps against the running stack, settled by the two Nodes alone:
#   Taker takes an offer → Taker locks BTC → Maker Node funds LEZ →
#   Taker claims LEZ → Maker Node claims BTC.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1
set -a; source runtime/runtime.env; set +a
export BTC_RPC_PASSWORD

log() { printf '\n[%s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
ui() { # ui <role> [ENV=VALUE...]
  local role="$1"; shift; local envs=()
  for kv in "$@"; do envs+=(-e "$kv"); done
  docker compose --env-file runtime/runtime.env run --rm --no-deps "${envs[@]}" \
    --entrypoint node basecamp-ui /ui-tests/verify.mjs "$role" 2>&1 |
    grep -E '✓|✗|interactive|Expected|passed|failed|has not|Error' | grep -viE 'locale'
}
taker_swaps() { # the Taker Node's own view of its swaps: "<swap_id> <state> <generation> <action>"
  local reply
  for _ in 1 2 3 4 5; do
    reply="$(docker exec lez-taker-node curl -sS --max-time 20 --unix-socket /run/lez/taker/node.sock \
      -H 'content-type: application/json' \
      --data '{"jsonrpc":"2.0","id":1,"method":"taker_swap_list_v1","params":[{"schema_version":1}]}' http://localhost/ 2>/dev/null)"
    [ -n "$reply" ] && break
    sleep 3
  done
  printf '%s' "$reply" | python3 -c '
import json, sys
raw = sys.stdin.read()
for s in (json.loads(raw) if raw else {}).get("result", {}).get("swaps", []):
    print(s["swap_id"], s["state"], s.get("progress_generation", 0), s.get("available_action") or "-")'
}
swap_state() { taker_swaps | grep -v -F -f <(printf "%s\n" "${baseline_swaps[@]:-__none__}") | sed 's/^/   /'; }
step() { log "$1"; shift; if ! ui "$@"; then swap_state; log "step failed"; exit 1; fi; swap_state; }
bash scripts/repair-indexer.sh || exit 1
# Swaps that exist before this run are not this run's; ignore them throughout.
mapfile -t baseline_swaps < <(taker_swaps | awk "{print \$1}")
step "0/5 Maker desk publishes offers until two are pending" maker
step "1/5 Taker takes an offer (the Taker Node reserves, plans, signs and activates)" taker PREPARE_INTERACTIVE_BTC=1
swap_id="$(swap_state | awk 'NR == 1 {print $1}')"
[ -n "$swap_id" ] || { log "the Taker Node lists no new swap"; exit 1; }
step "2/5 Taker: Lock 0.01 BTC" taker INTERACTIVE_ACTION=lock_btc
step "3/5 Maker Node funds 1,000 LEZ once the BTC lock confirms" maker INTERACTIVE_ACTION=fund_lez "INTERACTIVE_SWAP_ID=$swap_id"
step "4/5 Taker: Claim 1,000 LEZ (waits for LEZ funding to finalize)" taker INTERACTIVE_ACTION=claim_lez
step "5/5 Maker Node claims Bitcoin after the revealing claim" maker INTERACTIVE_ACTION=claim_btc "INTERACTIVE_SWAP_ID=$swap_id"
log "waiting for the Taker Node to see the swap complete"
for _ in $(seq 1 60); do
  state="$(taker_swaps | awk -v id="$swap_id" '$1 == id {print $2}')"; echo "   $swap_id $state"
  [[ "$state" == completed ]] && break
  [[ "$state" == attention_required || "$state" == refunded ]] && { echo "swap ended without completing" >&2; exit 1; }
  sleep 20
done
[[ "$state" == completed ]] || { echo "swap did not complete in time" >&2; exit 1; }
log "swap completed on both Nodes"
python3 scripts/export-node-evidence.py --swap "$swap_id" || exit 1
