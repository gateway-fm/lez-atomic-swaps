#!/usr/bin/env bash
# ui-e2e.sh — the node-e2e.py scenarios clicked through the two Basecamp
# desks against the running stack. Same Nodes, same methods (the desks call
# taker_swap_lock_v1 / _claim_v1 / _refund_v1 and watch the Maker's actor);
# only the driver differs: every step spawns the real desk offscreen and
# presses its buttons through the QML inspector (ui-tests/verify.mjs). The
# Nodes' APIs are read here only to learn the new swap's id and to log.
#
#   scripts/ui-e2e.sh <scenario> [--direction TakerSellsForeign|TakerSellsLez]
#
#   happy          take → lock → (Maker locks) → claim → (Maker claims) → completed
#   restart-taker  Taker Node restarted between its lock and its claim
#   restart-maker  Maker Node restarted between the Taker's lock and its own
#   survivor       Taker Node stopped right after its claim; the Maker completes alone
#   concurrent     two swaps taken, locked and claimed interleaved
#   taker-refund   Maker Node stopped before it locks; the Taker refunds past the cutoff
#   maker-refund   the Taker never claims; the Maker refunds, then the Taker refunds
#   all            every scenario in sequence (stops at the first failure)
#
# replay and wrong-inputs have no desk equivalent: a desk sends one request per
# click and offers only the actions the Node reports. The refund scenarios need
# the fast timing profile (LEZ_TIMING_PROFILE=fast scripts/up.sh).
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1
set -a; source runtime/runtime.env; set +a
export BTC_RPC_PASSWORD

scenario="${1:-}"; direction="TakerSellsForeign"
shift || true
while [[ $# -gt 0 ]]; do
  case "$1" in
    --direction) direction="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done
case "$direction" in TakerSellsForeign|TakerSellsLez) ;; *) echo "--direction must be TakerSellsForeign or TakerSellsLez" >&2; exit 64 ;; esac
if [[ "$direction" == TakerSellsLez ]]; then
  lock_action=lock_lez; claim_action=claim_btc; refund_action=refund_lez; lock_wallet=lez-maker
else
  lock_action=lock_btc; claim_action=claim_lez; refund_action=refund_btc; lock_wallet=lez-taker
fi

log() { printf '\n[%s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
fail() { log "FAILED: $*"; exit 1; }
# A scenario that stops a Node registers it here; it is started again however
# the scenario ends.
stopped_node=""
trap '[[ -z "$stopped_node" ]] || docker start "$stopped_node" >/dev/null 2>&1' EXIT
stop_node() { stopped_node="$1"; docker stop "$1" >/dev/null; }
start_node() { docker start "$1" >/dev/null; stopped_node=""; wait_healthy "$1"; }
ui() { # ui <role> [ENV=VALUE...]
  local role="$1"; shift; local envs=(-e "M3_UI_DIRECTION=$direction" -e "DESK_DEBUG=${DESK_DEBUG:-0}")
  for kv in "$@"; do envs+=(-e "$kv"); done
  docker compose --env-file runtime/runtime.env run --rm --no-deps "${envs[@]}" \
    --entrypoint node basecamp-ui /ui-tests/verify.mjs "$role" 2>&1 |
    grep -E '✓|✗|interactive|Expected|passed|failed|has not|Error|DESK|reached|refused|not ready' | grep -viE 'locale'
}
taker_swaps() { # the Taker Node's own view: "<swap_id> <state> <generation> <action>"
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
new_swaps() { taker_swaps | grep -v -F -f <(printf "%s\n" "${baseline_swaps[@]:-__none__}"); }
show() { new_swaps | sed 's/^/   /'; }
step() { log "$1"; shift; if ! ui "$@"; then show; fail "step failed"; fi; show; }
wait_healthy() { # wait_healthy <container>
  for _ in $(seq 1 60); do
    [[ "$(docker inspect -f '{{.State.Health.Status}}' "$1" 2>/dev/null)" == healthy ]] && return 0
    sleep 3
  done
  fail "$1 never became healthy"
}
btc() { docker exec lez-bitcoin-core bitcoin-cli -conf=/run-config/bitcoin.conf -datadir=/var/lib/bitcoin "$@"; }
ensure_coins() { # ensure_coins <wallet> <count>: two concurrent swaps need two spendable coins
  local wallet="$1" count="$2" have
  have="$(btc "-rpcwallet=$wallet" listunspent | python3 -c 'import json,sys; print(sum(1 for u in json.load(sys.stdin) if float(u["amount"]) >= 0.02))')"
  (( have >= count )) && return 0
  log "splitting the $wallet wallet into $count coins"
  for _ in $(seq 1 $((count - have + 1))); do
    btc "-rpcwallet=$wallet" sendtoaddress "$(btc "-rpcwallet=$wallet" getnewaddress "" bech32m)" 0.05 >/dev/null
  done
  btc generatetoaddress 1 bcrt1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqc8gma6 >/dev/null
  sleep 3
}
require_fast() { [[ "${LEZ_TIMING_PROFILE:-local}" == fast ]] || fail "this scenario needs LEZ_TIMING_PROFILE=fast (scripts/up.sh)"; }

# ---- desk steps -----------------------------------------------------------------
publish() { step "Maker desk publishes $direction offers until two are pending" maker; }
take() { # take <var>: the Taker desk takes one offer of this direction; the new swap id lands in <var>
  local -n out="$1"
  mapfile -t baseline_swaps < <(taker_swaps | awk '{print $1}')
  step "Taker desk takes an offer (the Taker Node reserves, plans, signs and activates)" taker PREPARE_INTERACTIVE_BTC=1
  out="$(new_swaps | awk 'NR == 1 {print $1}')"
  [ -n "$out" ] || fail "the Taker Node lists no new swap"
  mapfile -t baseline_swaps < <(taker_swaps | awk '{print $1}')
}
taker_act() { step "Taker desk: $1 on ${2:0:12}" taker "INTERACTIVE_ACTION=$1" "INTERACTIVE_SWAP_ID=$2"; }
taker_wait() { step "Taker desk shows ${2:0:12} at $1" taker INTERACTIVE_ACTION=wait "INTERACTIVE_STATE=$1" "INTERACTIVE_SWAP_ID=$2"; }
maker_wait() { step "Maker desk shows ${2:0:12} at $1" maker INTERACTIVE_ACTION=wait "INTERACTIVE_STATE=$1" "INTERACTIVE_SWAP_ID=$2"; }
finish() { # finish <swap_id>: both desks show the swap completed, then export the chain evidence
  maker_wait completed "$1"
  taker_wait completed "$1"
  python3 scripts/export-node-evidence.py --swap "$1" || fail "evidence export"
}

# ---- scenarios ------------------------------------------------------------------
scenario_happy() {
  local swap; publish; take swap
  taker_act "$lock_action" "$swap"
  maker_wait awaiting_taker_claim "$swap"
  taker_act "$claim_action" "$swap"
  finish "$swap"
}
scenario_restart() { # scenario_restart <maker|taker>
  local swap; publish; take swap
  taker_act "$lock_action" "$swap"
  log "restarting lez-$1-node"; docker restart "lez-$1-node" >/dev/null; wait_healthy "lez-$1-node"
  maker_wait awaiting_taker_claim "$swap"
  taker_act "$claim_action" "$swap"
  finish "$swap"
}
scenario_survivor() {
  local swap; publish; take swap
  taker_act "$lock_action" "$swap"
  maker_wait awaiting_taker_claim "$swap"
  taker_act "$claim_action" "$swap"
  log "stopping the Taker Node right after its revealing claim"; stop_node lez-taker-node
  maker_wait completed "$swap"
  start_node lez-taker-node
  taker_wait completed "$swap"
  python3 scripts/export-node-evidence.py --swap "$swap" || fail "evidence export"
}
scenario_concurrent() {
  local a b; ensure_coins "$lock_wallet" 2
  publish; take a; take b
  taker_act "$lock_action" "$a"; taker_act "$lock_action" "$b"
  maker_wait awaiting_taker_claim "$a"; maker_wait awaiting_taker_claim "$b"
  taker_act "$claim_action" "$a"; taker_act "$claim_action" "$b"
  finish "$a"; finish "$b"
}
scenario_taker_refund() {
  require_fast; local swap; publish; take swap
  log "stopping the Maker Node so it never locks"; stop_node lez-maker-node
  taker_act "$lock_action" "$swap"
  log "the Refund button appears once the Maker's cutoff (${LEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS}s) passes; the Node then drives the refund"
  taker_act "$refund_action" "$swap"
  start_node lez-maker-node
  taker_wait refunded "$swap"
  maker_wait refunded "$swap"
}
scenario_maker_refund() {
  require_fast; local swap; publish; take swap
  taker_act "$lock_action" "$swap"
  maker_wait awaiting_taker_claim "$swap"
  log "the Taker never claims; the Maker Node refunds its leg after ${LEZ_BTC_EARLIER_REFUND_SECONDS}s"
  maker_wait refunded "$swap"
  taker_act "$refund_action" "$swap"
  taker_wait refunded "$swap"
}

run_one() {
  local name="$1" started; started="$(date -u +%s)"
  log "=== $name ($direction)"
  bash scripts/repair-indexer.sh >/dev/null || fail "repair-indexer.sh"
  case "$name" in
    happy) scenario_happy ;;
    restart-taker) scenario_restart taker ;;
    restart-maker) scenario_restart maker ;;
    survivor) scenario_survivor ;;
    concurrent) scenario_concurrent ;;
    taker-refund) scenario_taker_refund ;;
    maker-refund) scenario_maker_refund ;;
    *) echo "unknown scenario: $name" >&2; exit 64 ;;
  esac
  log "=== $name ($direction): passed in $(( $(date -u +%s) - started ))s"
}
all=(happy restart-taker restart-maker survivor concurrent taker-refund maker-refund)
case "$scenario" in
  all) for name in "${all[@]}"; do run_one "$name"; done ;;
  "") echo "usage: scripts/ui-e2e.sh <scenario|all> [--direction TakerSellsForeign|TakerSellsLez]" >&2; exit 64 ;;
  *) run_one "$scenario" ;;
esac
