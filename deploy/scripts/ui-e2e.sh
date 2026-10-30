#!/usr/bin/env bash
# ui-e2e.sh — the node-e2e.py scenarios clicked through the two Basecamp
# desks against the running stack. Same Nodes, same methods (the desks call
# taker_swap_lock_v1 / _claim_v1 / _refund_v1 and watch the Maker's actor);
# only the driver differs: every step spawns the real desk offscreen and
# presses its buttons through the QML inspector (ui-tests/verify.mjs). The
# Nodes' APIs are read here only to learn the new swap's id and to log.
#
#   scripts/ui-e2e.sh <scenario> [--direction TakerSellsForeign|TakerSellsLez] [--record]
#
#   --record  every desk step renders on a private virtual display in its own
#             UI container and is screen-grabbed (ui-tests/record-step.sh);
#             the desk's narration is burned in as subtitles and the segments
#             are joined into runtime/evidence/videos/<direction>-<scenario>.mp4
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

scenario="${1:-}"; direction="TakerSellsForeign"; pair="Bitcoin"; record=0; zec_direction_given=0
shift || true
while [[ $# -gt 0 ]]; do
  case "$1" in
    --direction) direction="$2"; zec_direction_given=1; shift 2 ;;
    --pair) pair="$2"; shift 2 ;;
    --record) record=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done
videos="$PWD/runtime/evidence/videos"; segments=(); segment_index=0; step_note=""
# The stack this runs against. The defaults are the local stack; the public
# testnet stack (docs/testnet-run.md) sets, for example:
#   LEZ_STACK_COMPOSE_ARGS="-p lez-testnet --env-file testnet.env -f compose.yaml -f compose.testnet.yaml"
#   LEZ_CONTAINER_PREFIX=lez-testnet LEZ_REPAIR_INDEXER=0 LEZ_EXPORT_EVIDENCE=0
read -r -a stack_compose <<< "${LEZ_STACK_COMPOSE_ARGS:---env-file runtime/runtime.env}"
container_prefix="${LEZ_CONTAINER_PREFIX:-lez}"
case "$direction" in TakerSellsForeign|TakerSellsLez) ;; *) echo "--direction must be TakerSellsForeign or TakerSellsLez" >&2; exit 64 ;; esac
if [[ "$direction" == TakerSellsLez ]]; then
  lock_action=lock_lez; claim_action=claim_btc; refund_action=refund_lez; lock_wallet=lez-maker
else
  lock_action=lock_btc; claim_action=claim_lez; refund_action=refund_btc; lock_wallet=lez-taker
fi
# Zcash: a prepared corridor (scripts/zec-provision.sh --direction …) in either
# direction, TakerSellsLez unless --direction says TakerSellsForeign. The
# Taker's first lock (the LEZ escrow, or the Zcash HTLC when it sells ZEC) has
# no desk button; the Taker Node drives its actor itself, and node-e2e.py
# --pair Zcash zec-wait polls (and mines zebra) between desk steps. Selling
# LEZ the Taker claims Zcash and refunds LEZ; selling ZEC it claims LEZ (the
# revealing claim) and refunds its Zcash HTLC.
# Zcash desk journeys, both directions: zec-happy, zec-concurrent,
# zec-concurrent-staggered (selling LEZ), zec-maker-refund, and the BTC-parity
# set (REVIEW 21:08Z): zec-taker-refund (the Maker Node stopped after it saw the
# first lock), zec-restart-taker / zec-restart-maker, zec-survivor (the Taker
# Node stopped after its claim), zec-node-offline (both Nodes down with a Zcash
# swap open). Host steps between desk presses are node-e2e.py zec-wait/zec-step.
case "$pair" in
  Bitcoin) ;;
  Zcash)
    [[ "$zec_direction_given" == 1 ]] || direction=TakerSellsLez
    lock_action=""; lock_wallet=lez-maker
    if [[ "$direction" == TakerSellsLez ]]; then claim_action=claim_zec; refund_action=refund_lez
    else claim_action=claim_lez; refund_action=refund_zec; fi ;;
  *) echo "--pair must be Bitcoin or Zcash" >&2; exit 64 ;;
esac

log() { printf '\n[%s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
fail() { log "FAILED: $*"; exit 1; }
# A scenario that stops a Node registers it here; it is started again however
# the scenario ends.
stopped_node=""
trap '[[ -z "$stopped_node" ]] || docker start "$stopped_node" >/dev/null 2>&1' EXIT
stop_node() { stopped_node="$1"; docker stop "$1" >/dev/null; }
start_node() { docker start "$1" >/dev/null; stopped_node=""; wait_healthy "$1"; }
# Every desk runs through `compose run -T` with stdin closed. Without -T,
# compose allocates a pseudo-TTY and attaches stdin; a runner that is not in
# the terminal's foreground process group -- a detached tmux session, CI --
# then takes SIGTTIN on the first read and the whole scenario suspends. It
# looks exactly like a hang: the step prints, nothing follows, and the Nodes
# stay healthy. `ps` shows state T.
ui() { # ui <role> [ENV=VALUE...]
  local role="$1"; shift; local envs=(-e "M3_UI_DIRECTION=$direction" -e "M3_UI_PAIR=$pair" -e "DESK_DEBUG=${DESK_DEBUG:-0}")
  # verify.mjs reads the trade size from these, so they have to cross into the
  # container: the desks run inside it. Unset means the local stack's defaults
  # (1000 LEZ, 0.01 BTC); the public testnets pass smaller amounts, which is the
  # difference between a Maker BTC reservation that fits its wallet and one that
  # cannot.
  [[ -z "${LEZ_UI_LEZ_AMOUNT:-}" ]] || envs+=(-e "LEZ_UI_LEZ_AMOUNT=$LEZ_UI_LEZ_AMOUNT")
  [[ -z "${LEZ_UI_BTC_AMOUNT:-}" ]] || envs+=(-e "LEZ_UI_BTC_AMOUNT=$LEZ_UI_BTC_AMOUNT")
  [[ -z "${LEZ_UI_ZEC_AMOUNT:-}" ]] || envs+=(-e "LEZ_UI_ZEC_AMOUNT=$LEZ_UI_ZEC_AMOUNT")
  [[ -z "${LEZ_UI_ZEC_LEZ_AMOUNT:-}" ]] || envs+=(-e "LEZ_UI_ZEC_LEZ_AMOUNT=$LEZ_UI_ZEC_LEZ_AMOUNT")
  # How long a desk waits for the Node to reach the next state. verify.mjs defaults to
  # 30 minutes, which is barely 1.5 testnet4 block intervals and does time out on the
  # public networks, so docs/testnet-run.md tells the operator to raise it. That only
  # works if it crosses into the container.
  [[ -z "${INTERACTIVE_TIMEOUT_MS:-}" ]] || envs+=(-e "INTERACTIVE_TIMEOUT_MS=$INTERACTIVE_TIMEOUT_MS")
  for kv in "$@"; do envs+=(-e "$kv"); done
  if [[ "$record" == 1 ]]; then
    local segment
    if [[ -n "${UI_SEGMENT:-}" ]]; then segment="$UI_SEGMENT"   # a background step: assigned by step_bg in the parent
    else
      segment_index=$((segment_index + 1))
      segment="$(printf '%s-%s-%02d-%s' "$direction" "$scenario" "$segment_index" "$role")"
      segments+=("$segment")
    fi
    # The desk container records as uid 4713 into this bind mount: it must be
    # writable by that user on a host where the directory did not exist yet.
    mkdir -p "$videos" && chmod 0777 "$videos"
    docker compose "${stack_compose[@]}" run --rm -T --no-deps "${envs[@]}" \
      -e "STEP_TITLE=${step_title:-}" -v "$videos:/recordings" \
      --entrypoint bash basecamp-ui /ui-tests/record-step.sh "$role" "$segment" </dev/null 2>&1 |
      grep -E '✓|✗|^    [a-zA-Z]|»|interactive|Expected|passed|failed|has not|Error|DESK|reached|refused|not ready|label:' | grep -viE 'locale'
    return "${PIPESTATUS[0]}"
  fi
  docker compose "${stack_compose[@]}" run --rm -T --no-deps "${envs[@]}" \
    --entrypoint node basecamp-ui /ui-tests/verify.mjs "$role" </dev/null 2>&1 |
    grep -E '✓|✗|^    [a-zA-Z]|interactive|Expected|passed|failed|has not|Error|DESK|reached|refused|not ready|label:' | grep -viE 'locale'
}
# The same desk runner for the offline status check (#91): its own entry point,
# because verify.mjs assumes a reachable Node from its first step.
ui_offline() {
  docker compose "${stack_compose[@]}" run --rm -T --no-deps \
    -e "M3_UI_DIRECTION=$direction" \
    --entrypoint node basecamp-ui /ui-tests/verify-node-offline.mjs "$1" </dev/null 2>&1 |
    grep -E '✓|✗|^    [a-zA-Z]|passed|failed|Error'
  return "${PIPESTATUS[0]}"
}
# The scenario video: its segments joined in order and re-encoded once, so the
# trimmed segments' timestamps start clean (a stream copy of trimmed segments
# can leave the first frames out of order for players).
join_video() {
  [[ "$record" == 1 && "${#segments[@]}" -gt 0 ]] || return 0
  local list="$videos/$direction-$scenario.txt" out="$videos/$direction-$scenario.mp4" seg
  : > "$list"
  for seg in "${segments[@]}"; do [[ -s "$videos/$seg.mp4" ]] && printf "file '%s'\n" "$seg.mp4" >> "$list"; done
  docker compose "${stack_compose[@]}" run --rm -T --no-deps -v "$videos:/recordings" -w /recordings \
    --entrypoint ffmpeg basecamp-ui -hide_banner -loglevel error -y -f concat -safe 0 -i "$direction-$scenario.txt" \
      -c:v libx264 -preset veryfast -crf 24 -pix_fmt yuv420p -r "${RECORD_FPS:-6}" -fps_mode cfr -an "$direction-$scenario.mp4" </dev/null >/dev/null 2>&1 \
    && log "video: runtime/evidence/videos/$direction-$scenario.mp4" \
    || log "video join failed for $direction-$scenario"
  for seg in "${segments[@]}"; do rm -f "$videos/$seg.mp4" "$videos/$seg.events" "$videos/$seg.srt" "$videos/$seg.raw.mp4"; done
  rm -f "$list"
}
taker_swaps() { # the Taker Node's own view: "<swap_id> <state> <generation> <action>"
  local reply
  for _ in 1 2 3 4 5; do
    reply="$(docker exec "$container_prefix-taker-node" curl -sS --max-time 240 --unix-socket /run/lez/taker/node.sock \
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
# Whole-id comparison: a fixed-string filter would also drop a swap whose id
# extends a baseline id (the concurrent fixture's "<id>-b").
new_swaps() { taker_swaps | awk -v b="${baseline_swaps[*]:-__none__}" 'BEGIN { n = split(b, a, " "); for (i = 1; i <= n; i++) seen[a[i]] = 1 } !seen[$1]'; }
show() { new_swaps | sed 's/^/   /'; }
step() { log "$1"; step_title="${step_note:+$step_note · }$1"; step_note=""; shift; if ! ui "$@"; then show; fail "step failed"; fi; show; }
note() { log "$1"; step_note="$1"; }  # a scripted action, narrated at the start of the next recorded step
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
take() { # take <var> [offer_id]: the Taker desk takes one offer of this direction (that offer's row when named); the new swap id lands in <var>
  local -n out="$1"
  mapfile -t baseline_swaps < <(taker_swaps | awk '{print $1}')
  step "Taker desk takes an offer${2:+ ($2)} (the Taker Node reserves, plans, signs and activates)" taker PREPARE_INTERACTIVE_BTC=1 "INTERACTIVE_OFFER_ID=${2:-}"
  out="$(new_swaps | awk 'NR == 1 {print $1}')"
  [ -n "$out" ] || fail "the Taker Node lists no new swap"
  mapfile -t baseline_swaps < <(taker_swaps | awk '{print $1}')
}
taker_act() { step "Taker desk: $1 on ${2:0:12}" taker "INTERACTIVE_ACTION=$1" "INTERACTIVE_SWAP_ID=$2"; }
taker_wait() { step "Taker desk shows ${2:0:12} at $1" taker INTERACTIVE_ACTION=wait "INTERACTIVE_STATE=$1" "INTERACTIVE_SWAP_ID=$2"; }
maker_wait() { # the awaiting-claim label names the asset the Taker claims: what the Maker locked
  local label=""
  if [[ "$1" == awaiting_taker_claim ]]; then
    if [[ "$pair" == Zcash && "$direction" == TakerSellsLez ]]; then label="Waiting for the Taker's Zcash claim"
    elif [[ "$pair" == Zcash ]]; then label="Waiting for the Taker's LEZ claim"
    elif [[ "$direction" == TakerSellsLez ]]; then label="Waiting for the Taker's Bitcoin claim"
    else label="Waiting for the Taker's LEZ claim"; fi
  fi
  step "Maker desk shows ${2:0:12} at $1" maker INTERACTIVE_ACTION=wait "INTERACTIVE_STATE=$1" "INTERACTIVE_SWAP_ID=$2" "INTERACTIVE_EXPECT_LABEL=$label"
}
maker_wait_label() { # the awaiting-claim label names the asset the Taker claims: what the Maker locked
  [[ "$1" == awaiting_taker_claim ]] || { echo ""; return; }
  if [[ "$pair" == Zcash && "$direction" == TakerSellsLez ]]; then echo "Waiting for the Taker's Zcash claim"
  elif [[ "$pair" == Zcash ]]; then echo "Waiting for the Taker's LEZ claim"
  elif [[ "$direction" == TakerSellsLez ]]; then echo "Waiting for the Taker's Bitcoin claim"
  else echo "Waiting for the Taker's LEZ claim"; fi
}
# A desk step in the background: the desk opens now and waits on screen while
# the chain moves (the Taker Node drives the ZEC lifecycle itself, not a desk
# button, so the only way to see it happen on a desk is to have the desk open
# first); the caller `wait`s for it. Its recording segment is assigned
# here, in the parent, so join_video keeps the step order.
step_bg() { # step_bg <pid-var> <title> <role> [ENV=VALUE...]
  local pid_var="$1"; shift
  local segment=""
  if [[ "$record" == 1 ]]; then
    segment_index=$((segment_index + 1))
    segment="$(printf '%s-%s-%02d-%s' "$direction" "$scenario" "$segment_index" "$2")"
    segments+=("$segment")
  fi
  # A failed step leaves through fail's exit 1; a passed one must not report
  # the status of step's trailing `show` (the foreground path never checks it).
  ( UI_SEGMENT="$segment" step "$@"; exit 0 ) &
  printf -v "$pid_var" '%s' "$!"
  step_note=""
}
maker_wait_bg() { local pid_var="$1"; shift; step_bg "$pid_var" "Maker desk shows ${2:0:12} at $1" maker INTERACTIVE_ACTION=wait "INTERACTIVE_STATE=$1" "INTERACTIVE_SWAP_ID=$2" "INTERACTIVE_EXPECT_LABEL=$(maker_wait_label "$1")"; }
taker_wait_bg() { local pid_var="$1"; shift; step_bg "$pid_var" "Taker desk shows ${2:0:12} at $1" taker INTERACTIVE_ACTION=wait "INTERACTIVE_STATE=$1" "INTERACTIVE_SWAP_ID=$2"; }
# Let a background desk render before the chain is moved under it (recording only).
desk_head_start() { [[ "$record" == 1 ]] && sleep "${RECORD_DESK_HEAD_START:-40}" || true; }
finish() { # finish <swap_id>: both desks show the swap completed, then export the chain evidence
  maker_wait completed "$1"
  taker_wait completed "$1"
  export_evidence "$1"
}
export_evidence() { # export_evidence <swap_id>
  [[ "${LEZ_EXPORT_EVIDENCE:-1}" == 1 ]] || return 0
  if [[ "$pair" == Bitcoin ]]; then
    python3 scripts/export-node-evidence.py --swap "$1" || fail "evidence export"
  else
    # export-node-evidence.py reads the Bitcoin swap stores; a ZEC swap's record
    # is the corridor, both Nodes' views, the Taker actor's status and the zebra
    # transactions, written by node-e2e.py to runtime/evidence/zec/<swap>.json.
    LEZ_ZEC_REUSE_CORRIDOR=1 python3 scripts/node-e2e.py --pair Zcash --direction "$direction" zec-export --swap-id "$1" || fail "ZEC evidence export"
  fi
}
# ---- ZEC host-side steps (the Node offers only claim/refund for ZEC) ----------------
zec_provision() { # zec_provision [swaps]: one fresh corridor run with that many prepared entries; recreates both Nodes (zec-provision.sh step 8)
  log "provisioning a ZEC corridor with ${1:-1} prepared swap(s) (zec-provision.sh: offer, draft, catalog, Nodes recreated)"
  bash scripts/zec-provision.sh ${LEZ_ZEC_SKIP_CHAIN_CHECK:+--skip-chain-check} --direction "$direction" --swaps "${1:-1}" 2>&1 | tail -4 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "zec-provision.sh"
  wait_healthy lez-maker-node; wait_healthy lez-taker-node
  # The desk shows the corridor's lot; verify.mjs asserts the amounts it is given.
  local prepared; prepared="runtime/zec/$(cat runtime/zec/current)/prepared.json"
  LEZ_UI_ZEC_AMOUNT="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[0]["foreign_units"] / 1e8)' "$prepared")"
  LEZ_UI_ZEC_LEZ_AMOUNT="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[0]["lez_units"])' "$prepared")"
  export LEZ_UI_ZEC_AMOUNT LEZ_UI_ZEC_LEZ_AMOUNT
  log "corridor lot: $LEZ_UI_ZEC_AMOUNT ZEC for $LEZ_UI_ZEC_LEZ_AMOUNT LEZ units"
}
zec_provision_add() { # a second corridor beside the live one (its own Zcash anchor); recreates both Nodes with both
  log "adding a second ZEC corridor beside the live one (zec-provision.sh --add: offer, draft, catalog of both, Nodes recreated)"
  bash scripts/zec-provision.sh ${LEZ_ZEC_SKIP_CHAIN_CHECK:+--skip-chain-check} --direction "$direction" --add 2>&1 | tail -4 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "zec-provision.sh --add"
  wait_healthy lez-maker-node; wait_healthy lez-taker-node
}
zec_wait() { # zec_wait <swap_id> <taker state> [timeout]: wait (the Taker Node drives its actor; zebra is mined on demand) until the Node reports that state
  log "driving the Taker's ZEC actor on the Node until ${1:0:12} shows $2"
  python3 scripts/node-e2e.py --pair Zcash --direction "$direction" zec-wait --swap-id "$1" --until "$2" --timeout "${3:-2400}" 2>&1 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "zec-wait to $2"
}

zec_wait_both() { # zec_wait_both <swap_a> <swap_b> <taker state> [timeout]: wait until both swaps show that state (the Taker Node drives both actors)
  log "driving both Taker ZEC actors on the Node until ${1:0:12} and ${2:0:12} show $3"
  python3 scripts/node-e2e.py --pair Zcash --direction "$direction" zec-wait --swap-id "$1" --swap-id "$2" --until "$3" --timeout "${4:-3000}" 2>&1 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "zec-wait (both) to $3"
}
zec_offer_ids() { python3 -c 'import json,sys; print("\n".join(e["offer_id"] for e in json.load(open(sys.argv[1]))))' "runtime/zec/$(cat runtime/zec/current)/prepared.json"; }
zec_refund_tail() { # zec_refund_tail <swap_id>: zebra to the refund height, the Maker's refund queued, its Zcash refund in a block
  log "mining zebra to the agreement's refund height; the Maker Node refunds its Zcash (operator action) once it sees the LEZ leg refunded"
  python3 scripts/node-e2e.py --pair Zcash --direction "$direction" zec-refund-tail --swap-id "$1" 2>&1 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "zec-refund-tail"
}
zec_wait_phase() { # zec_wait_phase <swap_id> <Taker actor phases, '|'-separated> [timeout]: zebra mined for a reason
  python3 scripts/node-e2e.py --pair Zcash --direction "$direction" zec-wait --swap-id "$1" --until-phase "$2" --timeout "${3:-2400}" 2>&1 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "zec-wait to phase $2"
}
zec_step() { # zec_step <swap_id> <step> [timeout]: a host step of a ZEC desk journey (node-e2e.py zec-step)
  python3 scripts/node-e2e.py --pair Zcash --direction "$direction" zec-step --swap-id "$1" --step "$2" --timeout "${3:-1800}" 2>&1 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "zec-step $2"
}
zec_first_lock_phases="taker_lock_confirmed|awaiting_maker_confirmations|both_legs_locked"
zec_claim_and_complete() { # zec_claim_and_complete <swap_id>: from the open swap to completed on both desks
  local maker_pid taker_pid
  maker_wait_bg maker_pid awaiting_taker_claim "$1"
  desk_head_start
  zec_wait "$1" claim_available
  wait "$maker_pid" || fail "Maker desk: awaiting_taker_claim"
  taker_act "$claim_action" "$1"
  maker_wait_bg maker_pid completed "$1"
  taker_wait_bg taker_pid completed "$1"
  desk_head_start
  zec_wait "$1" completed 900
  wait "$maker_pid" || fail "Maker desk: completed"
  wait "$taker_pid" || fail "Taker desk: completed"
  export_evidence "$1"
}
wait_until_unix() { # wait_until_unix <unix seconds> <what for>
  local now; now="$(date -u +%s)"
  (( now >= $1 )) && return 0
  log "waiting $(( $1 - now ))s $2"
  while (( $(date -u +%s) < $1 )); do sleep 15; done
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
  note "The $1 Node is restarted now, between the Taker's lock and its claim; swaps survive a Node restart"; docker restart "lez-$1-node" >/dev/null; wait_healthy "lez-$1-node"
  maker_wait awaiting_taker_claim "$swap"
  taker_act "$claim_action" "$swap"
  finish "$swap"
}
scenario_survivor() {
  local swap; publish; take swap
  taker_act "$lock_action" "$swap"
  maker_wait awaiting_taker_claim "$swap"
  taker_act "$claim_action" "$swap"
  note "The Taker Node is stopped right after its revealing claim; the Maker Node completes the swap on its own"; stop_node lez-taker-node
  maker_wait completed "$swap"
  note "The Taker Node is started again; its desk catches up with the completed swap"; start_node lez-taker-node
  taker_wait completed "$swap"
  python3 scripts/export-node-evidence.py --swap "$swap" || fail "evidence export"
}
# #91: the desk's connection state must follow its Node, not its own loading.
# The Nodes are down before the desk opens and come back while it waits, so the
# view is watched through "checking", "down" and "up" in one session.
scenario_node_offline() {
  local status=0
  for role in maker taker; do
    note "Both Nodes are stopped before the $role desk opens; a desk must not report a connection it does not have"
    stop_node lez-maker-node; stop_node lez-taker-node
    ( sleep 75; docker start lez-maker-node lez-taker-node >/dev/null 2>&1 ) &
    local resumer=$!
    ui_offline "$role" || status=1
    wait "$resumer" 2>/dev/null || true
    wait_healthy lez-maker-node; wait_healthy lez-taker-node
    stopped_node=""
    [[ $status == 0 ]] || fail "the $role desk misreported its Node connection"
  done
}
scenario_concurrent() {
  local a b; ensure_coins "$lock_wallet" 2
  if [[ "$direction" == TakerSellsLez ]]; then
    publish; take a; take b
    taker_act "$lock_action" "$a"; taker_act "$lock_action" "$b"
    maker_wait awaiting_taker_claim "$a"; maker_wait awaiting_taker_claim "$b"
  else
    # The Maker locks LEZ one swap at a time and refuses a second such take
    # until the first lock is confirmed (node-e2e.py asserts the refusal), so
    # the second swap is taken then; the two overlap from there on.
    publish; take a
    taker_act "$lock_action" "$a"; maker_wait awaiting_taker_claim "$a"
    take b
    taker_act "$lock_action" "$b"; maker_wait awaiting_taker_claim "$b"
  fi
  taker_act "$claim_action" "$a"; taker_act "$claim_action" "$b"
  finish "$a"; finish "$b"
}
scenario_taker_refund() {
  require_fast; local swap; publish; take swap
  note "The Maker Node is stopped before it can lock; the Taker locks anyway and must recover its funds later"; stop_node lez-maker-node
  taker_act "$lock_action" "$swap"
  note "The Maker's lock cutoff (${LEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS} s after the take) passes with no Maker lock; the desk then offers Refund"
  taker_act "$refund_action" "$swap"
  note "The Maker Node is started again and reconciles the refunded swap"; start_node lez-maker-node
  taker_wait refunded "$swap"
  maker_wait refunded "$swap"
}
scenario_zec_happy() {
  local swap maker_pid taker_pid
  if [[ "$record" == 1 ]]; then
    log "a clean Taker store for the recording (reset-swaps.sh): no stale row above the order book"
    bash scripts/reset-swaps.sh >/dev/null 2>&1 || fail "reset-swaps.sh"
    wait_healthy lez-maker-node; wait_healthy lez-taker-node
  fi
  zec_provision; take swap
  if [[ "$direction" == TakerSellsLez ]]; then
    note "The Taker Node drives the LEZ lock on its own, straight after the take; the Maker desk watches its row while the Maker Node funds Zcash and claims LEZ with the secret"
  else
    note "The Taker Node funds the Zcash HTLC on its own, straight after the take; the Maker desk watches its row while the Maker Node locks the LEZ escrow"
  fi
  maker_wait_bg maker_pid awaiting_taker_claim "$swap"
  desk_head_start
  zec_wait "$swap" claim_available
  wait "$maker_pid" || fail "Maker desk: awaiting_taker_claim"
  taker_act "$claim_action" "$swap"
  if [[ "$direction" == TakerSellsLez ]]; then
    note "The Zcash claim waits for a zebra block; both desks watch the swap complete while the Taker Node observes it"
  else
    note "The Taker's LEZ claim revealed the secret; the Maker Node's Zcash claim waits for a zebra block, and both desks watch the swap complete"
  fi
  maker_wait_bg maker_pid completed "$swap"
  taker_wait_bg taker_pid completed "$swap"
  desk_head_start
  zec_wait "$swap" completed 900
  wait "$maker_pid" || fail "Maker desk: completed"
  wait "$taker_pid" || fail "Taker desk: completed"
  export_evidence "$swap"
}
scenario_zec_concurrent() {
  # The BTC concurrent shape: two prepared entries of one corridor run, both
  # taken from the desk, both actors driven by the Taker Node with their LEZ
  # steps interleaving on the same role accounts, both Maker fundings (distinct
  # coinbase outputs), both revealing LEZ claims, both Zcash claims from the
  # Taker desk; both desks show two rows completed.
  local swap_a swap_b offers maker_a maker_b taker_a
  if [[ "$record" == 1 ]]; then
    log "a clean Taker store for the recording (reset-swaps.sh): no stale row above the order book"
    bash scripts/reset-swaps.sh >/dev/null 2>&1 || fail "reset-swaps.sh"
    wait_healthy lez-maker-node; wait_healthy lez-taker-node
  fi
  zec_provision 2
  mapfile -t offers < <(zec_offer_ids)
  [[ "${#offers[@]}" == 2 ]] || fail "the corridor run lists ${#offers[@]} offers, not 2"
  take swap_a "${offers[0]}"
  note "A second Zcash row is on the book: the Taker takes it too, and the Taker Node runs a second prepared ceremony with the Maker"
  take swap_b "${offers[1]}"
  note "The Taker Node drives both LEZ locks on its own, their steps interleaving on one account; the Maker Node funds Zcash for each from its own coinbase output and claims LEZ on each while the Maker desk watches both rows"
  maker_wait_bg maker_a awaiting_taker_claim "$swap_a"
  maker_wait_bg maker_b awaiting_taker_claim "$swap_b"
  desk_head_start
  zec_wait_both "$swap_a" "$swap_b" claim_available
  wait "$maker_a" || fail "Maker desk: awaiting_taker_claim ($swap_a)"
  wait "$maker_b" || fail "Maker desk: awaiting_taker_claim ($swap_b)"
  taker_act "$claim_action" "$swap_a"
  taker_act "$claim_action" "$swap_b"
  note "Both Zcash claims wait for zebra blocks; both desks watch the two swaps complete while the Taker Node observes them"
  maker_wait_bg maker_a completed "$swap_a"
  taker_wait_bg taker_a completed "$swap_a"
  desk_head_start
  zec_wait_both "$swap_a" "$swap_b" completed 1200
  wait "$maker_a" || fail "Maker desk: completed ($swap_a)"
  wait "$taker_a" || fail "Taker desk: completed ($swap_a)"
  maker_wait completed "$swap_b"
  taker_wait completed "$swap_b"
  export_evidence "$swap_a"
  export_evidence "$swap_b"
}
scenario_zec_concurrent_staggered() {
  # Two prepared ZEC swaps open side by side: the first provisioned, taken and
  # brought to claim_available; a second corridor added beside it (its own
  # Zcash anchor and refund window, Nodes recreated with both), taken and
  # brought there too; both Zcash claims from the Taker desk; both desks show
  # two rows completed. Staggered because each role's LEZ transactions share
  # one account (a second lock or claim prepared before the first is final
  # goes out with a stale nonce) and because one anchor's four-block Zcash
  # window does not hold two swaps.
  local swap_a swap_b offers maker_a maker_b taker_a
  if [[ "$record" == 1 ]]; then
    log "a clean Taker store for the recording (reset-swaps.sh): no stale row above the order book"
    bash scripts/reset-swaps.sh >/dev/null 2>&1 || fail "reset-swaps.sh"
    wait_healthy lez-maker-node; wait_healthy lez-taker-node
  fi
  zec_provision
  take swap_a
  note "The Taker Node drives the first swap's LEZ lock on its own; the Maker Node funds Zcash from its first coinbase output and claims LEZ while the Maker desk watches the row"
  maker_wait_bg maker_a awaiting_taker_claim "$swap_a"
  desk_head_start
  zec_wait "$swap_a" claim_available
  wait "$maker_a" || fail "Maker desk: awaiting_taker_claim ($swap_a)"
  zec_provision_add
  mapfile -t offers < <(zec_offer_ids)
  [[ "${#offers[@]}" == 2 ]] || fail "the catalog lists ${#offers[@]} offers after --add, not 2"
  note "A second corridor is live beside the first: a new Zcash row is on the book while the first swap waits for its Zcash claim; the Taker takes it too"
  take swap_b "${offers[1]}"
  note "The second swap follows the same path from its own corridor: its LEZ lock, the Maker's Zcash funding from a second coinbase output, the Maker's second LEZ claim; the first row stays open beside it"
  maker_wait_bg maker_b awaiting_taker_claim "$swap_b"
  desk_head_start
  zec_wait "$swap_b" claim_available
  wait "$maker_b" || fail "Maker desk: awaiting_taker_claim ($swap_b)"
  taker_act "$claim_action" "$swap_a"
  taker_act "$claim_action" "$swap_b"
  note "Both Zcash claims wait for zebra blocks; both desks watch the two swaps complete while the Taker Node observes them"
  maker_wait_bg maker_a completed "$swap_a"
  taker_wait_bg taker_a completed "$swap_a"
  desk_head_start
  zec_wait_both "$swap_a" "$swap_b" completed 1200
  wait "$maker_a" || fail "Maker desk: completed ($swap_a)"
  wait "$taker_a" || fail "Taker desk: completed ($swap_a)"
  maker_wait completed "$swap_b"
  taker_wait completed "$swap_b"
  export_evidence "$swap_a"
  export_evidence "$swap_b"
}
scenario_zec_maker_refund() {
  # Both legs locked, the Maker's Zcash funding kept at one confirmation (below
  # the pre-reveal gate), the Taker refunds LEZ from its desk at the agreement's
  # refund time (30 min after provisioning on the devnet profile), the Maker
  # Node refunds its Zcash at the refund height, the Taker Node's supervisor runs the
  # admitted refund on its own until it has observed that; both desks show the
  # swap refunded. One press of Refund; no Retry.
  local swap maker_pid taker_pid opens
  if [[ "$direction" != TakerSellsLez ]]; then scenario_zec_maker_refund_foreign; return; fi
  if [[ "$record" == 1 ]]; then
    log "a clean Taker store for the recording (reset-swaps.sh): no stale row above the order book"
    bash scripts/reset-swaps.sh >/dev/null 2>&1 || fail "reset-swaps.sh"
    wait_healthy lez-maker-node; wait_healthy lez-taker-node
  fi
  zec_provision; take swap
  note "The Taker Node drives the LEZ lock on its own; the Maker Node funds Zcash, and zebra is mined only for what waits in the mempool, so the funding stays one block deep and the Maker cannot claim yet"
  python3 scripts/node-e2e.py --pair Zcash zec-wait --swap-id "$swap" --until-phase "both_legs_locked|awaiting_maker_confirmations" --mine-pending-only 2>&1 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "zec-wait to both_legs_locked"
  opens=$(( $(cat runtime/zec/current | sed 's/.*-//') + 1800 + 15 ))
  wait_until_unix "$opens" "for the agreement's LEZ refund time (devnet profile: 30 min after provisioning)"
  note "The LEZ refund time has passed and the Maker never claimed; the Taker desk offers Refund and the Maker desk watches its Zcash come back"
  maker_wait_bg maker_pid refunded "$swap"
  desk_head_start
  taker_act "$refund_action" "$swap"
  note "Refund pressed once: the Taker Node's supervisor runs the admitted refund until it is final; the Maker Node refunds its Zcash at the refund height and the Taker observes it"
  taker_wait_bg taker_pid refunded "$swap"
  zec_refund_tail "$swap"
  wait "$maker_pid" || fail "Maker desk: refunded"
  wait "$taker_pid" || fail "Taker desk: refunded"
  export_evidence "$swap"
}
# ---- ZEC desk journeys for BTC parity (REVIEW 21:08Z), both directions ----------
scenario_zec_restart() { # scenario_zec_restart <maker|taker>
  local swap; zec_provision; take swap
  note "The Taker Node drives its first lock on its own; the $1 Node is restarted once it is confirmed"
  zec_wait_phase "$swap" "$zec_first_lock_phases"
  note "The $1 Node is restarted now, between the Taker's first lock and its claim; swaps survive a Node restart"
  docker restart "lez-$1-node" >/dev/null; wait_healthy "lez-$1-node"
  zec_claim_and_complete "$swap"
}
scenario_zec_survivor() {
  local swap maker_pid; zec_provision; take swap
  maker_wait_bg maker_pid awaiting_taker_claim "$swap"
  desk_head_start
  zec_wait "$swap" claim_available
  wait "$maker_pid" || fail "Maker desk: awaiting_taker_claim"
  taker_act "$claim_action" "$swap"
  note "The Taker Node is stopped right after its claim; the Maker Node completes the swap on its own"
  stop_node lez-taker-node
  zec_step "$swap" maker-completes 1500
  maker_wait completed "$swap"
  note "The Taker Node is started again; its desk catches up with the completed swap"
  start_node lez-taker-node
  zec_wait "$swap" completed 900
  taker_wait completed "$swap"
  export_evidence "$swap"
}
scenario_zec_taker_refund() {
  local swap; zec_provision; take swap
  note "The Maker Node observes the Taker's first lock and is stopped before it can lock its own leg; the Taker must recover its funds"
  zec_step "$swap" maker-down-after-first-lock 1500; stopped_node=lez-maker-node
  zec_step "$swap" refund-point
  zec_wait "$swap" refund_available 900
  note "The Taker's refund is due with no Maker lock; the Taker desk offers Refund"
  taker_act "$refund_action" "$swap"
  zec_wait "$swap" refunded 1800
  taker_wait refunded "$swap"
  note "The Maker Node is started again and reconciles the refunded swap"
  zec_step "$swap" maker-up-reconcile 1200; stopped_node=""
  maker_wait refunded "$swap"
}
scenario_zec_maker_refund_foreign() {
  # Selling ZEC: both legs locked, the Taker never presses its LEZ claim; at the
  # LEZ refund time the Maker refunds its LEZ escrow, then the Taker presses
  # Refund in the claim window and its Zcash comes back at the refund height.
  local swap maker_pid; zec_provision; take swap
  maker_wait_bg maker_pid awaiting_taker_claim "$swap"
  desk_head_start
  zec_wait "$swap" claim_available
  wait "$maker_pid" || fail "Maker desk: awaiting_taker_claim"
  note "The Taker never claims; at the LEZ refund time the Maker Node refunds its LEZ escrow"
  zec_step "$swap" maker-lez-refund 1800
  zec_wait "$swap" refund_available 900
  note "The Maker's LEZ is back; the Taker desk presses Refund and its Zcash comes back at the refund height"
  taker_act "$refund_action" "$swap"
  zec_wait "$swap" refunded 1800
  maker_wait refunded "$swap"
  taker_wait refunded "$swap"
  export_evidence "$swap"
}
scenario_zec_node_offline() {
  # The desk link check is the same code for both pairs; its ZEC run holds an
  # open Zcash swap on both desks through the outage, then completes it.
  local swap maker_pid; zec_provision; take swap
  maker_wait_bg maker_pid awaiting_taker_claim "$swap"
  desk_head_start
  zec_wait "$swap" claim_available
  wait "$maker_pid" || fail "Maker desk: awaiting_taker_claim"
  note "A Zcash swap is open at claim_available on both desks while both Nodes go down"
  scenario_node_offline
  note "Both Nodes are back; the Taker claims and the swap completes"
  zec_claim_and_complete "$swap"
}
scenario_maker_refund() {
  require_fast; local swap; publish; take swap
  taker_act "$lock_action" "$swap"
  maker_wait awaiting_taker_claim "$swap"
  note "The Taker never claims; after its refund deadline (${LEZ_BTC_EARLIER_REFUND_SECONDS} s after the take) the Maker Node refunds its own lock"
  # One refunded leg is not an unwound swap: the Maker desk reads "refunding"
  # until the Taker's leg is back too, and only then "refunded".
  maker_wait refunding "$swap"
  taker_act "$refund_action" "$swap"
  taker_wait refunded "$swap"
  maker_wait refunded "$swap"
}

run_one() {
  local name="$1" started; started="$(date -u +%s)"
  log "=== $name ($direction)"
  if [[ "${LEZ_REPAIR_INDEXER:-1}" == 1 ]]; then bash scripts/repair-indexer.sh >/dev/null || fail "repair-indexer.sh"; fi
  case "$name" in
    happy) scenario_happy ;;
    restart-taker) scenario_restart taker ;;
    restart-maker) scenario_restart maker ;;
    survivor) scenario_survivor ;;
    concurrent) scenario_concurrent ;;
    taker-refund) scenario_taker_refund ;;
    maker-refund) scenario_maker_refund ;;
    node-offline) scenario_node_offline ;;
    zec-happy) [[ "$pair" == Zcash ]] || fail "zec-happy needs --pair Zcash"; scenario_zec_happy ;;
    zec-maker-refund) [[ "$pair" == Zcash ]] || fail "zec-maker-refund needs --pair Zcash"; scenario_zec_maker_refund ;;
    zec-concurrent) [[ "$pair" == Zcash ]] || fail "zec-concurrent needs --pair Zcash"; scenario_zec_concurrent ;;
    zec-concurrent-staggered) [[ "$pair" == Zcash ]] || fail "zec-concurrent-staggered needs --pair Zcash"; scenario_zec_concurrent_staggered ;;
    zec-taker-refund) [[ "$pair" == Zcash ]] || fail "zec-taker-refund needs --pair Zcash"; scenario_zec_taker_refund ;;
    zec-restart-taker) [[ "$pair" == Zcash ]] || fail "zec-restart-taker needs --pair Zcash"; scenario_zec_restart taker ;;
    zec-restart-maker) [[ "$pair" == Zcash ]] || fail "zec-restart-maker needs --pair Zcash"; scenario_zec_restart maker ;;
    zec-survivor) [[ "$pair" == Zcash ]] || fail "zec-survivor needs --pair Zcash"; scenario_zec_survivor ;;
    zec-node-offline) [[ "$pair" == Zcash ]] || fail "zec-node-offline needs --pair Zcash"; scenario_zec_node_offline ;;
    *) echo "unknown scenario: $name" >&2; exit 64 ;;
  esac
  join_video
  log "=== $name ($direction): passed in $(( $(date -u +%s) - started ))s"
}
all=(happy restart-taker restart-maker survivor concurrent taker-refund maker-refund node-offline)
case "$scenario" in
  all) for name in "${all[@]}"; do run_one "$name"; done ;;
  "") echo "usage: scripts/ui-e2e.sh <scenario|all> [--direction TakerSellsForeign|TakerSellsLez] [--pair Bitcoin|Zcash] [--record]" >&2; exit 64 ;;
  *) run_one "$scenario" ;;
esac
