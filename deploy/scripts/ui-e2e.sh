#!/usr/bin/env bash
# ui-e2e.sh — the node-e2e.py scenarios clicked through the parties' desks
# against the running stack. Same Nodes, same methods (the desks call
# taker_swap_lock_v1 / _claim_v1 / _refund_v1 and watch the Maker's actor);
# only the driver differs: every step drives the desk that the party's own
# Basecamp has open (lez-maker-node, lez-taker-node: the desks people use over
# VNC), pressing its buttons through the QML inspector (ui-tests/verify.mjs).
# The Nodes' APIs are read here only to learn the new swap's id and to log.
#
#   scripts/ui-e2e.sh <scenario> [--direction TakerSellsForeign|TakerSellsLez] [--record]
#
#   --record  every desk step is screen-grabbed on its party's Basecamp display
#             (ui-tests/record-step.sh); the desk's narration is burned in as
#             subtitles and the segments are joined into
#             runtime/evidence/videos/<direction>-<scenario>.mp4
#
# A party's Basecamp runs its desk, its LEZ signer and its Node: a scenario that
# takes the party offline stops or restarts that container (the desk opens again
# when it starts); node-offline holds only the Nodes down (scripts/node-hold.sh),
# so the desks can show them gone and back.
#
#   happy          take → lock → (Maker locks) → claim → (Maker claims) → completed
#   restart-taker  Taker's Basecamp (and Node) restarted between its lock and its claim
#   restart-maker  Maker's Basecamp (and Node) restarted between the Taker's lock and its own
#   survivor       Taker's Basecamp stopped right after its claim; the Maker completes alone
#   concurrent     two swaps taken, locked and claimed interleaved
#   taker-refund   Maker's Basecamp stopped before it locks; the Taker refunds past the cutoff
#   maker-refund   the Taker never claims; the Maker refunds, then the Taker refunds
#   lez-finality-stall  the LEZ sequencer stopped until both Nodes report the stall
#                  and both desks show it, then started again (#112; not in `all`)
#   logos-happy    happy on the Logos transport (compose.logos.yaml): the Taker has no
#                  Delivery directory and no Maker socket; the offer and every Chat
#                  round travel through the two Basecamps' Logos Delivery and Chat
#                  modules (Bitcoin, or Zcash TakerSellsLez from a desk-published
#                  offer; not in `all`)
#   zec-desk-offer the Maker desk's form publishes a Zcash offer, the Taker desk takes
#                  it, the Nodes agree at take time (no zec-provision.sh); TakerSellsLez
#   all            every scenario in sequence (stops at the first failure)
#
# replay and wrong-inputs have no desk equivalent: a desk sends one request per
# click and offers only the actions the Node reports. The refund scenarios need
# the fast timing profile (LEZ_TIMING_PROFILE=fast scripts/up.sh).
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1
set -a; source runtime/runtime.env; set +a
# The pair of regression lane LEZ_LANE (lane 1, the stack itself, when unset).
# shellcheck source=lane.sh
source scripts/lane.sh
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
videos="$PWD/$LEZ_LANE_RUNTIME/evidence/videos"; segments=(); segment_index=0; step_note=""
# The stack this runs against. The defaults are the local stack; the public
# testnet stack (docs/testnet-run.md) sets, for example:
#   LEZ_STACK_COMPOSE_ARGS="-p lez-testnet --env-file testnet.env -f compose.yaml -f compose.testnet.yaml"
#   LEZ_CONTAINER_PREFIX=lez-testnet LEZ_REPAIR_INDEXER=0 LEZ_EXPORT_EVIDENCE=0
read -r -a stack_compose <<< "${LEZ_STACK_COMPOSE_ARGS:---env-file runtime/runtime.env}"
container_prefix="${LEZ_CONTAINER_PREFIX:-lez}"
maker_node="$container_prefix-maker-node"; taker_node="$container_prefix-taker-node"
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
# set (REVIEW 21:08Z): zec-taker-refund (the Maker's Basecamp stopped after it
# saw the first lock), zec-restart-taker / zec-restart-maker, zec-survivor (the
# Taker's Basecamp stopped after its claim), zec-node-offline (both Nodes held
# down with a Zcash swap open). Host steps between desk presses are node-e2e.py zec-wait/zec-step.
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
# A scenario that stops a party's Basecamp registers it here, and one that
# holds the Nodes down sets nodes_held; either is undone however the scenario
# ends.
stopped_node=""; nodes_held=0; logos_on=0
trap '[[ -z "$stopped_node" ]] || docker start "$stopped_node" >/dev/null 2>&1
  [[ "$nodes_held" == 0 ]] || bash scripts/node-hold.sh release maker taker >/dev/null 2>&1
  [[ "$logos_on" == 0 ]] || { chat_log; transport local; }' EXIT
stop_node() { stopped_node="$1"; docker stop "$1" >/dev/null; }
start_node() { # start_node <container>: the party's Basecamp again, its Node healthy and its desk open
  local role="${1#"$container_prefix"-}"; role="${role%-node}"
  docker start "$1" >/dev/null; stopped_node=""; wait_healthy "$1"; wait_desk "$role"
}
restart_node() { # restart_node <role>: the party's whole Basecamp (desk, signer, Node) restarted
  docker restart "$container_prefix-$1-node" >/dev/null; wait_healthy "$container_prefix-$1-node"; wait_desk "$1"
}
# Each desk step is a `docker exec` with no TTY and stdin closed. A pseudo-TTY
# with stdin attached would make a runner that is not in the terminal's
# foreground process group -- a detached tmux session, CI -- take SIGTTIN on
# the first read, and the whole scenario suspends. It looks exactly like a
# hang: the step prints, nothing follows, and the Nodes stay healthy. `ps`
# shows state T.
ui() { # ui <role> [ENV=VALUE...]: one step on the desk the role's own Basecamp has open
  local role="$1"; shift; local envs=(-e "M3_UI_DIRECTION=$direction" -e "M3_UI_PAIR=$pair" -e "DESK_DEBUG=${DESK_DEBUG:-0}")
  local container="$container_prefix-$role-node"
  # verify.mjs reads the trade size from these, so they have to cross into the
  # container: the desk runs inside it. Unset means the local stack's defaults
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
    # The segment is recorded inside the role's Basecamp and copied out; the
    # join (join_video) runs as uid 4713 into this directory, so it must be
    # writable by that user on a host where it did not exist yet.
    mkdir -p "$videos" && chmod 0777 "$videos"
    local status
    docker exec "${envs[@]}" -e "STEP_TITLE=${step_title:-}" -e RECORDINGS=/tmp/lez-recordings \
      "$container" bash /ui-tests/record-step.sh "$role" "$segment" </dev/null 2>&1 |
      grep -E '✓|✗|^    [a-zA-Z]|»|interactive|Expected|passed|failed|has not|Error|DESK|reached|refused|not ready|label:|RETRIED-PRESS|pressing again' | grep -viE 'locale'
    status="${PIPESTATUS[0]}"
    docker cp "$container:/tmp/lez-recordings/$segment.mp4" "$videos/$segment.mp4" >/dev/null 2>&1 || true
    docker exec "$container" sh -c "rm -f /tmp/lez-recordings/$segment.*" >/dev/null 2>&1 || true
    return "$status"
  fi
  docker exec "${envs[@]}" "$container" node /ui-tests/verify.mjs "$role" </dev/null 2>&1 |
    grep -E '✓|✗|^    [a-zA-Z]|interactive|Expected|passed|failed|has not|Error|DESK|reached|refused|not ready|label:|RETRIED-PRESS|pressing again' | grep -viE 'locale'
}
# The same desk runner for the offline status check (#91): its own entry point,
# because verify.mjs assumes a reachable Node from its first step.
ui_offline() { # ui_offline <role> <down|up>
  docker exec -e "M3_UI_DIRECTION=$direction" "$container_prefix-$1-node" \
    node /ui-tests/verify-node-offline.mjs "$1" "$2" </dev/null 2>&1 |
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
  # ffmpeg from the Maker's image, in a throwaway container of it.
  docker compose "${stack_compose[@]}" run --rm -T --no-deps -v "$videos:/recordings" -w /recordings \
    --entrypoint ffmpeg maker-node -hide_banner -loglevel error -y -f concat -safe 0 -i "$direction-$scenario.txt" \
      -c:v libx264 -preset veryfast -crf 24 -pix_fmt yuv420p -r "${RECORD_FPS:-6}" -fps_mode cfr -an "$direction-$scenario.mp4" </dev/null >/dev/null 2>&1 \
    && log "video: $LEZ_LANE_RUNTIME/evidence/videos/$direction-$scenario.mp4" \
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
wait_desk() { # wait_desk <role>: the role's Basecamp has opened its desk again (open-desks.mjs, at container start)
  for _ in $(seq 1 100); do
    docker exec "$container_prefix-$1-node" grep -q ' passed, 0 failed' /tmp/open-desks.log 2>/dev/null && return 0
    sleep 3
  done
  fail "the $1 desk was not opened again after its Basecamp started"
}
owner_ready() { # owner_ready <role> <method> <params>: the Node answers its owner
  docker exec "$container_prefix-$1-node" curl -sS --max-time 30 --unix-socket "/run/lez/$1/node.sock" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":[$3]}" http://localhost/ 2>/dev/null | grep -q '"result"'
}
wait_owner() { # wait_owner: both Nodes answer their owner health calls (the container health lags a held Node)
  for _ in $(seq 1 200); do
    owner_ready maker maker_health '{}' && owner_ready taker taker_health '{"schema_version":1}' && return 0
    sleep 3
  done
  fail "the Nodes did not answer their owner health calls"
}
btc() { docker exec "$container_prefix-bitcoin-core" bitcoin-cli -conf=/run-config/bitcoin.conf -datadir=/var/lib/bitcoin "$@"; }
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
require_fast() { [[ "${LEZ_TIMING_PROFILE:-local}" =~ ^(refund|fast)$ ]] || fail "this scenario needs LEZ_TIMING_PROFILE=refund or fast (scripts/up.sh)"; }

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
# The refund journeys' ZEC profile (zec-swap-sdk profile.rs, node-e2e.py
# ZEC_REFUND_PROFILE): a 4-minute LEZ refund delay, as nobody claims there.
zec_refund_profile=deterministic-local-devnet-v6
zec_provision() { # zec_provision [swaps] [profile]: one fresh corridor run with that many prepared entries; recreates both Nodes (zec-provision.sh step 8)
  if zec_take_time "${2:-}"; then zec_publish_offers --swaps "${1:-1}"; return; fi
  log "provisioning a ZEC corridor with ${1:-1} prepared swap(s)${2:+ under $2} (zec-provision.sh: offer, draft, catalog, Nodes recreated)"
  bash scripts/zec-provision.sh ${LEZ_ZEC_SKIP_CHAIN_CHECK:+--skip-chain-check} --direction "$direction" --swaps "${1:-1}" ${2:+--profile "$2"} 2>&1 | tail -4 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "zec-provision.sh"
  wait_healthy "$maker_node"; wait_healthy "$taker_node"; wait_desk maker; wait_desk taker
  # The desk shows the corridor's lot; verify.mjs asserts the amounts it is given.
  local prepared; prepared="$LEZ_LANE_RUNTIME/zec/$(cat "$LEZ_LANE_RUNTIME/zec/current")/prepared.json"
  LEZ_UI_ZEC_AMOUNT="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[0]["foreign_units"] / 1e8)' "$prepared")"
  LEZ_UI_ZEC_LEZ_AMOUNT="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[0]["lez_units"])' "$prepared")"
  export LEZ_UI_ZEC_AMOUNT LEZ_UI_ZEC_LEZ_AMOUNT
  log "corridor lot: $LEZ_UI_ZEC_AMOUNT ZEC for $LEZ_UI_ZEC_LEZ_AMOUNT LEZ units"
}
zec_provision_add() { # a second corridor beside the live one (its own Zcash anchor); recreates both Nodes with both
  if zec_take_time ""; then zec_publish_offers --swaps 1 --add; return; fi
  log "adding a second ZEC corridor beside the live one (zec-provision.sh --add: offer, draft, catalog of both, Nodes recreated)"
  bash scripts/zec-provision.sh ${LEZ_ZEC_SKIP_CHAIN_CHECK:+--skip-chain-check} --direction "$direction" --add 2>&1 | tail -4 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "zec-provision.sh --add"
  wait_healthy "$maker_node"; wait_healthy "$taker_node"; wait_desk maker; wait_desk taker
}
# LEZ_ZEC_AGREEMENT=take-time (node-e2e.py): the Maker publishes the offers on
# its running Node and the Nodes agree when the desk takes one; a corridor of
# another profile than the Nodes take under stays the dealer's.
zec_take_time() { [[ "${LEZ_ZEC_AGREEMENT:-dealer}" == take-time && "${1:-${LEZ_ZEC_PROFILE:-deterministic-local-devnet-v5}}" == "${LEZ_ZEC_PROFILE:-deterministic-local-devnet-v5}" ]]; }
zec_publish_offers() { # zec_publish_offers <node-e2e.py zec-publish arguments>
  log "publishing Zcash offer(s) on the running Maker Node (take-time agreement: no corridor, no Node recreated)"
  python3 scripts/node-e2e.py --pair Zcash --direction "$direction" zec-publish "$@" 2>&1 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "zec-publish"
  export LEZ_UI_ZEC_AMOUNT=1.0 LEZ_UI_ZEC_LEZ_AMOUNT=50000  # node-e2e.py ZEC_FOREIGN_UNITS / 1e8, ZEC_LEZ_UNITS
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
zec_offer_ids() {
  if zec_take_time ""; then cat "$LEZ_LANE_RUNTIME/zec/take-time-offers"; return; fi
  python3 -c 'import json,sys; print("\n".join(e["offer_id"] for e in json.load(open(sys.argv[1]))))' "$LEZ_LANE_RUNTIME/zec/$(cat "$LEZ_LANE_RUNTIME/zec/current")/prepared.json";
}
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
zec_refund_delay() { # the LEZ refund delay of the corridor the Nodes run with, seconds (node-e2e.py's mapping)
  python3 -c 'import importlib.util,sys; sys.argv=["x"]; s=importlib.util.spec_from_file_location("ne","scripts/node-e2e.py"); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); print(m.zec_current_refund_delay())'
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
  while (( $(date -u +%s) < $1 )); do sleep 2; done
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
  note "The $1's Basecamp, with its Node, is restarted now, between the Taker's lock and its claim; swaps survive a restart"; restart_node "$1"
  maker_wait awaiting_taker_claim "$swap"
  taker_act "$claim_action" "$swap"
  finish "$swap"
}
scenario_survivor() {
  local swap; publish; take swap
  taker_act "$lock_action" "$swap"
  maker_wait awaiting_taker_claim "$swap"
  taker_act "$claim_action" "$swap"
  note "The Taker's Basecamp, with its Node, is stopped right after its revealing claim; the Maker Node completes the swap on its own"; stop_node "$taker_node"
  maker_wait completed "$swap"
  note "The Taker's Basecamp is started again; its desk catches up with the completed swap"; start_node "$taker_node"
  taker_wait completed "$swap"
  python3 scripts/export-node-evidence.py --swap "$swap" || fail "evidence export"
}
# #91: the desk's connection state must follow its Node, not its own loading.
# Both Nodes are held down inside their Basecamps (scripts/node-hold.sh) while
# the desks stay open, and released again, so one desk session is watched
# through "down" and back to "up".
scenario_node_offline() {
  for role in maker taker; do
    note "Both Nodes are held down while the $role desk stays open; it must report its Node unavailable, never connected"
    nodes_held=1
    bash scripts/node-hold.sh hold maker taker || fail "holding both Nodes down"
    ui_offline "$role" down || fail "the $role desk misreported its Node connection"
    note "Both Nodes are released; the $role desk must pick its Node up again"
    bash scripts/node-hold.sh release maker taker || fail "releasing both Nodes"
    nodes_held=0
    ui_offline "$role" up || fail "the $role desk did not see its Node come back"
    wait_owner
  done
}
# #112: a frozen LEZ finality is reported by both Nodes and shown on both desks.
# The stack's sequencer is stopped (no new blocks, nothing left to finalize, no
# tip to read) until both Nodes' health turns degraded with
# lez_finality_stalled and both desks show the line above their swaps; it is
# started again and both recover. The Nodes' threshold is
# LEZ_NODE_FINALITY_STALL_SECONDS (the heartbeat's 60 s on the devnet). Every
# lane shares the one sequencer, so this runs on a stack nothing else uses.
lez_chain() { # lez_chain <role> <method> <params>: the Node's health lez_chain, one line of JSON
  docker exec "$container_prefix-$1-node" curl -sS --max-time 30 --unix-socket "/run/lez/$1/node.sock" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":[$3]}" http://localhost/ 2>/dev/null |
    python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; print(json.dumps({"degraded": r.get("degraded"), "lez_chain": r.get("lez_chain")}))' 2>/dev/null
}
wait_lez_chain() { # wait_lez_chain <stalled|moving> <seconds>: both Nodes report it
  local want="$1" deadline=$(( $(date +%s) + $2 )) maker taker
  while :; do
    maker="$(lez_chain maker maker_health '{}')"; taker="$(lez_chain taker taker_health '{"schema_version":1}')"
    if python3 - "$want" "$maker" "$taker" <<'PY'
import json, sys
want = sys.argv[1]
for raw in sys.argv[2:]:
    health = json.loads(raw or "{}")
    chain = health.get("lez_chain") or {}
    if want == "stalled" and not (health.get("degraded") is True and chain.get("reason") == "lez_finality_stalled"):
        sys.exit(1)
    if want == "moving" and not (chain and chain.get("degraded") is False):
        sys.exit(1)
PY
    then
      log "maker_health: $maker"; log "taker_health: $taker"; return 0
    fi
    (( $(date +%s) < deadline )) || { log "maker_health: $maker"; log "taker_health: $taker"; fail "the Nodes did not report the LEZ chain $want within $2 s"; }
    sleep 5
  done
}
scenario_lez_finality_stall() {
  local sequencer="$LEZ_STACK_PREFIX-sequencer" threshold="${LEZ_NODE_FINALITY_STALL_SECONDS:-600}" stopped_at
  note "Both Nodes report LEZ finality moving before the stall"
  wait_lez_chain moving 180
  note "The LEZ sequencer is stopped: finality stands still and both Nodes must turn degraded after ${threshold} s"
  stopped_node="$sequencer"; docker stop "$sequencer" >/dev/null || fail "stopping $sequencer"; stopped_at="$(date +%s)"
  wait_lez_chain stalled $(( threshold + 180 ))
  log "degraded $(( $(date +%s) - stopped_at )) s after the sequencer stopped (threshold ${threshold} s)"
  for role in maker taker; do
    docker exec "$container_prefix-$role-node" node /ui-tests/verify-lez-chain.mjs "$role" stalled </dev/null 2>&1 |
      grep -E '✓|✗|^    [a-zA-Z]|^  [a-z]+ desk:|passed|failed|Error'
    [[ "${PIPESTATUS[0]}" == 0 ]] || fail "the $role desk did not show the LEZ finality stall"
  done
  note "The sequencer is started again: finality moves and both Nodes and desks recover"
  docker start "$sequencer" >/dev/null || fail "starting $sequencer"; stopped_node=""
  wait_lez_chain moving 600
  for role in maker taker; do
    docker exec "$container_prefix-$role-node" node /ui-tests/verify-lez-chain.mjs "$role" clear </dev/null 2>&1 |
      grep -E '✓|✗|^    [a-zA-Z]|^  [a-z]+ desk:|passed|failed|Error'
    [[ "${PIPESTATUS[0]}" == 0 ]] || fail "the $role desk still shows the LEZ finality stall"
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
  note "The Maker's Basecamp, with its Node, is stopped before it can lock; the Taker locks anyway and must recover its funds later"; stop_node "$maker_node"
  taker_act "$lock_action" "$swap"
  note "The Maker's lock cutoff (${LEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS} s after the take) passes with no Maker lock; the desk then offers Refund"
  taker_act "$refund_action" "$swap"
  note "The Maker's Basecamp is started again; its Node reconciles the refunded swap"; start_node "$maker_node"
  taker_wait refunded "$swap"
  maker_wait refunded "$swap"
}
scenario_zec_happy() {
  local swap maker_pid taker_pid
  if [[ "$record" == 1 ]]; then
    log "a clean Taker store for the recording (reset-swaps.sh): no stale row above the order book"
    bash scripts/reset-swaps.sh --abandon-open >/dev/null 2>&1 || fail "reset-swaps.sh"
    wait_healthy "$maker_node"; wait_healthy "$taker_node"
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
  local swap_a swap_b offers maker_a taker_a
  if [[ "$record" == 1 ]]; then
    log "a clean Taker store for the recording (reset-swaps.sh): no stale row above the order book"
    bash scripts/reset-swaps.sh --abandon-open >/dev/null 2>&1 || fail "reset-swaps.sh"
    wait_healthy "$maker_node"; wait_healthy "$taker_node"
  fi
  zec_provision 2
  mapfile -t offers < <(zec_offer_ids)
  [[ "${#offers[@]}" == 2 ]] || fail "the corridor run lists ${#offers[@]} offers, not 2"
  take swap_a "${offers[0]}"
  note "A second Zcash row is on the book: the Taker takes it too, and the Taker Node runs a second prepared ceremony with the Maker"
  take swap_b "${offers[1]}"
  note "The Taker Node drives both LEZ locks on its own, their steps interleaving on one account; the Maker Node funds Zcash for each from its own coinbase output and claims LEZ on each while the Maker desk watches both rows"
  # One driver on the Maker desk at a time: two at once press its buttons
  # between each other's checks (one's "Node ready" replaced by the other's
  # "Market refreshed"). The second row is checked once the first has been.
  maker_wait_bg maker_a awaiting_taker_claim "$swap_a"
  desk_head_start
  zec_wait_both "$swap_a" "$swap_b" claim_available
  wait "$maker_a" || fail "Maker desk: awaiting_taker_claim ($swap_a)"
  maker_wait awaiting_taker_claim "$swap_b"
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
    bash scripts/reset-swaps.sh --abandon-open >/dev/null 2>&1 || fail "reset-swaps.sh"
    wait_healthy "$maker_node"; wait_healthy "$taker_node"
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
  # refund time (the profile's delay after provisioning: 4 min on the devnet V4), the Maker
  # Node refunds its Zcash at the refund height, the Taker Node's supervisor runs the
  # admitted refund on its own until it has observed that; both desks show the
  # swap refunded. One press of Refund; no Retry.
  local swap maker_pid taker_pid opens
  if [[ "$direction" != TakerSellsLez ]]; then scenario_zec_maker_refund_foreign; return; fi
  if [[ "$record" == 1 ]]; then
    log "a clean Taker store for the recording (reset-swaps.sh): no stale row above the order book"
    bash scripts/reset-swaps.sh --abandon-open >/dev/null 2>&1 || fail "reset-swaps.sh"
    wait_healthy "$maker_node"; wait_healthy "$taker_node"
  fi
  zec_provision 1 "$zec_refund_profile"; take swap
  note "The Taker Node drives the LEZ lock on its own; the Maker Node funds Zcash, and zebra is mined only for what waits in the mempool, so the funding stays one block deep and the Maker cannot claim yet"
  python3 scripts/node-e2e.py --pair Zcash zec-wait --swap-id "$swap" --until-phase "both_legs_locked|awaiting_maker_confirmations" --mine-pending-only 2>&1 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "zec-wait to both_legs_locked"
  # The stamp is its second in ten digits; lanes other than the first append
  # their two-digit lane number (zec-provision.sh), which is not part of the time.
  opens=$(( $(sed 's/.*-//' "$LEZ_LANE_RUNTIME/zec/current" | cut -c1-10) + $(zec_refund_delay) + 15 ))
  wait_until_unix "$opens" "for the agreement's LEZ refund time (the profile's delay after provisioning)"
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
  note "The $1's Basecamp, with its Node, is restarted now, between the Taker's first lock and its claim; swaps survive a restart"
  restart_node "$1"
  zec_claim_and_complete "$swap"
}
scenario_zec_survivor() {
  local swap maker_pid; zec_provision; take swap
  maker_wait_bg maker_pid awaiting_taker_claim "$swap"
  desk_head_start
  zec_wait "$swap" claim_available
  wait "$maker_pid" || fail "Maker desk: awaiting_taker_claim"
  taker_act "$claim_action" "$swap"
  note "The Taker's Basecamp, with its Node, is stopped right after its claim; the Maker Node completes the swap on its own"
  stop_node "$taker_node"
  zec_step "$swap" maker-completes 1500
  maker_wait completed "$swap"
  note "The Taker's Basecamp is started again; its desk catches up with the completed swap"
  start_node "$taker_node"
  zec_wait "$swap" completed 900
  taker_wait completed "$swap"
  export_evidence "$swap"
}
scenario_zec_taker_refund() {
  local swap; zec_provision 1 "$zec_refund_profile"; take swap
  note "The Maker Node observes the Taker's first lock and is stopped before it can lock its own leg; the Taker must recover its funds"
  zec_step "$swap" maker-down-after-first-lock 1500; stopped_node="$maker_node"
  zec_step "$swap" refund-point
  zec_wait "$swap" refund_available 900
  note "The Taker's refund is due with no Maker lock; the Taker desk offers Refund"
  taker_act "$refund_action" "$swap"
  zec_wait "$swap" refunded 1800
  taker_wait refunded "$swap"
  note "The Maker Node is started again and reconciles the refunded swap"
  zec_step "$swap" maker-up-reconcile 1200; stopped_node=""; wait_desk maker
  maker_wait refunded "$swap"
}
scenario_zec_maker_refund_foreign() {
  # Selling ZEC: both legs locked, the Taker never presses its LEZ claim; at the
  # LEZ refund time the Maker refunds its LEZ escrow, then the Taker presses
  # Refund in the claim window and its Zcash comes back at the refund height.
  local swap maker_pid; zec_provision 1 "$zec_refund_profile"; take swap
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
scenario_zec_stale_window() {
  # FINDINGS 26, selling ZEC: the Taker never claims; when the revealing claim
  # window closes (the LEZ refund time minus 30 s) the Taker row stops
  # offering the claim and offers Refund; one press; the Maker refunds its LEZ
  # escrow and the Taker's Zcash refund lands at the refund height.
  local swap maker_pid
  [[ "$direction" == TakerSellsForeign ]] || fail "zec-stale-window is the selling-ZEC journey (--direction TakerSellsForeign)"
  zec_provision 1 "$zec_refund_profile"; take swap
  maker_wait_bg maker_pid awaiting_taker_claim "$swap"
  desk_head_start
  zec_wait "$swap" claim_available
  wait "$maker_pid" || fail "Maker desk: awaiting_taker_claim"
  note "The Taker never claims. When the claim window closes (the LEZ refund time minus 30 s) the Taker row stops offering the claim and offers Refund"
  zec_step "$swap" claim-window-closed 300
  step "Taker desk shows ${swap:0:12} as the closed claim window" taker INTERACTIVE_ACTION=wait INTERACTIVE_STATE=refund_ready "INTERACTIVE_SWAP_ID=$swap" "INTERACTIVE_EXPECT_LABEL=Claim window closed"
  taker_act "$refund_action" "$swap"
  note "Refund pressed once; the Maker Node refunds its LEZ escrow and the Taker's Zcash refund lands at the refund height"
  zec_step "$swap" maker-lez-refund 1800
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
  note "A Zcash swap is open at claim_available on both desks while both Nodes are held down"
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

# ---- the Logos transport (compose.logos.yaml, node-entrypoint.sh) ----------------
chat_log() { # both Basecamps' Chat module lines (UTC): conversations, every message sent and received with its size
  local role
  for role in maker taker; do
    log "$role Chat module:"
    docker exec "$container_prefix-$role-node" sh -c "cat /var/lez/basecamp-$role-user/module_data/chat_module/*/chat_module_*.log" 2>/dev/null |
      grep -E 'created|sent|received|failed|online' | sed 's/^/   /'
  done
  log "the Taker Node's last lines:"
  docker exec "$taker_node" tail -n 15 /run/lez/taker/node.log 2>/dev/null | cut -c1-400 | sed 's/^/   /'
}
transport() { # transport <logos|local>: both Basecamps recreated on that transport, the Maker's first (as up.sh)
  local args=("${stack_compose[@]}")
  if [[ "$1" == logos ]]; then
    [[ " ${args[*]} " == *" -f "* ]] || args+=(-f compose.yaml)
    args+=(-f compose.logos.yaml)
    # What a Taker on another host is told: the Maker's public Delivery identity.
    LEZ_MAKER_DELIVERY_IDENTITY="$(docker exec "$maker_node" cat /delivery/maker-delivery-identity.pub)" ||
      fail "the Maker's Delivery identity is unavailable"
    export LEZ_MAKER_DELIVERY_IDENTITY
  fi
  logos_on=1
  log "both Basecamps recreated on the $1 transport"
  docker compose "${args[@]}" up -d --no-build --force-recreate --no-deps maker-node </dev/null >/dev/null 2>&1 ||
    fail "recreating the Maker's Basecamp on the $1 transport"
  if [[ "$1" == logos ]]; then
    # The Taker's Delivery node dials the Maker's: its libp2p address, by the
    # Maker container's IP on the Docker network (the Delivery node's own DNS
    # resolver does not ask Docker's), with the peer id its node logs.
    local peer=""
    for _ in $(seq 1 60); do
      peer="$(docker logs "$maker_node" 2>&1 | sed -n 's/.*Started libp2p node.*(peerId: \([0-9A-Za-z]*\).*/\1/p' | tail -1)"
      [[ -n "$peer" ]] && break
      sleep 2
    done
    [[ -n "$peer" ]] || fail "the Maker's Delivery node did not start"
    LEZ_MAKER_DELIVERY_PEER="/ip4/$(docker exec "$maker_node" hostname -i | awk '{print $1}')/tcp/60000/p2p/$peer"
    export LEZ_MAKER_DELIVERY_PEER
    log "the Taker's Delivery node dials the Maker's at $LEZ_MAKER_DELIVERY_PEER"
  fi
  docker compose "${args[@]}" up -d --no-build --force-recreate --no-deps taker-node </dev/null >/dev/null 2>&1 ||
    fail "recreating the Taker's Basecamp on the $1 transport"
  wait_healthy "$maker_node"; wait_healthy "$taker_node"; wait_desk maker; wait_desk taker
  [[ "$1" == logos ]] || logos_on=0
}
gateway() { # gateway <role> <method> <params>: the role's Chat gateway answers on its control socket
  docker exec "$container_prefix-$1-node" curl -sS --max-time 10 --unix-socket "/run/lez/$1/logos-chat-control.sock" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":[$3]}" http://localhost/ 2>/dev/null
}
# A Zcash offer published from the Maker desk's form and agreed at take time
# (zec_reserve_v1): no zec-provision.sh, no corridor, no Node recreated.
# Selling ZEC the Taker funds from its own key: node-e2e.py zec-fund pays it
# (and renews a worn Zebra) before the desk publishes.
zec_publish() {
  python3 scripts/node-e2e.py --pair Zcash --direction "$direction" zec-fund 2>&1 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "zec-fund"
  step "Maker desk publishes $direction Zcash offers from its form until two are pending" maker LEZ_UI_PUBLISH_ZEC=1
}
scenario_zec_desk_offer() {
  local swap; zec_publish; take swap
  zec_claim_and_complete "$swap"
}
scenario_logos_happy() {
  local swap started route node
  transport logos
  log "the Taker's Basecamp, as the Node's user: no Delivery directory, no Maker socket"
  docker exec "$taker_node" sh -c 'id -u; ! grep -q " /delivery " /proc/self/mountinfo && [ -z "$(ls -A /delivery)" ] &&
    ! test -e /run/lez/maker && jq -c "{delivery_sources, chat_socket}" /var/lib/lez/taker/role.json' 2>&1 | sed 's/^/   /'
  [[ "${PIPESTATUS[0]}" == 0 ]] || fail "the Taker still sees the Maker's Delivery directory or socket"
  if [[ "$pair" == Zcash ]]; then zec_publish; else publish; fi
  started="$(date -u +%s)"; take swap
  log "taken over Logos Delivery and Chat in $(( $(date -u +%s) - started )) s (discovery, conversation, reservation, ceremony)"
  if [[ "$pair" == Zcash ]]; then
    zec_claim_and_complete "$swap"
  else
    taker_act "$lock_action" "$swap"
    maker_wait awaiting_taker_claim "$swap"
    taker_act "$claim_action" "$swap"
    finish "$swap"
  fi
  # What carried it: the Taker gateway holds the Maker's announcement from
  # Delivery and one bound Chat session; the Maker gateway binds a session only
  # from a Chat message's authenticated sender.
  route="{\"pair\":\"$pair\",\"direction\":\"$direction\"}"
  log "Chat gateways: Taker $(gateway taker logos_chat_status_v1 '{"schema_version":1}'); Maker $(gateway maker logos_chat_status_v1 '{"schema_version":1}')"
  log "Taker gateway offers from Delivery: $(gateway taker logos_offer_list_v1 "{\"schema_version\":1,\"route\":$route}" |
    python3 -c 'import json,sys; o=json.load(sys.stdin)["result"]["offers"]; print(len(o), "offer(s) of", len({e["maker_identity"] for e in o}), "Maker(s):", sorted({e["maker_identity"][:12] for e in o}))')"
  gateway taker logos_chat_status_v1 '{"schema_version":1}' | grep -q '"session_bound":true' || fail "the Taker gateway has no bound Chat session"
  gateway maker logos_chat_status_v1 '{"schema_version":1}' | grep -q '"session_count":[1-9]' || fail "the Maker gateway never received a Chat message"
  gateway taker logos_offer_list_v1 "{\"schema_version\":1,\"route\":$route}" | grep -q "\"$LEZ_MAKER_DELIVERY_IDENTITY\"" ||
    log "(the Maker's offers are no longer listed: its pending offers were taken or withdrawn)"
  for node in "$maker_node" "$taker_node"; do
    docker logs --since "$started" "$node" 2>&1 | grep -q 'Chat gateway exited' && log "the Chat gateway in $node restarted during the swap"
  done
  chat_log
  transport local
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
    lez-finality-stall) scenario_lez_finality_stall ;;
    logos-happy) scenario_logos_happy ;;
    zec-desk-offer) [[ "$pair" == Zcash ]] || fail "zec-desk-offer needs --pair Zcash"; scenario_zec_desk_offer ;;
    zec-happy) [[ "$pair" == Zcash ]] || fail "zec-happy needs --pair Zcash"; scenario_zec_happy ;;
    zec-maker-refund) [[ "$pair" == Zcash ]] || fail "zec-maker-refund needs --pair Zcash"; scenario_zec_maker_refund ;;
    zec-concurrent) [[ "$pair" == Zcash ]] || fail "zec-concurrent needs --pair Zcash"; scenario_zec_concurrent ;;
    zec-concurrent-staggered) [[ "$pair" == Zcash ]] || fail "zec-concurrent-staggered needs --pair Zcash"; scenario_zec_concurrent_staggered ;;
    zec-taker-refund) [[ "$pair" == Zcash ]] || fail "zec-taker-refund needs --pair Zcash"; scenario_zec_taker_refund ;;
    zec-restart-taker) [[ "$pair" == Zcash ]] || fail "zec-restart-taker needs --pair Zcash"; scenario_zec_restart taker ;;
    zec-restart-maker) [[ "$pair" == Zcash ]] || fail "zec-restart-maker needs --pair Zcash"; scenario_zec_restart maker ;;
    zec-survivor) [[ "$pair" == Zcash ]] || fail "zec-survivor needs --pair Zcash"; scenario_zec_survivor ;;
    zec-node-offline) [[ "$pair" == Zcash ]] || fail "zec-node-offline needs --pair Zcash"; scenario_zec_node_offline ;;
    zec-stale-window) [[ "$pair" == Zcash ]] || fail "zec-stale-window needs --pair Zcash"; scenario_zec_stale_window ;;
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
