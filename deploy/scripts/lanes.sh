#!/usr/bin/env bash
# lanes.sh — regression lanes 2..N beside the stack (lane 1).
#
# A lane is one more Maker/Taker pair, each party's own Basecamp as in the
# stack, with its own Bitcoin Core, miner and Zebra, on the stack's one LEZ
# network (scripts/lane.sh names it; compose.lane.yaml runs it as the Compose
# project lez-lane-<k>). Lane 2 settles as maker-basel-02 / taker-limmat-02,
# which every genesis funds; lanes 3 and up need their own funded wallets.
#
#   scripts/lanes.sh up <N>        lanes 2..N: their files (gen-config.sh with
#                                  LEZ_LANE), chains, wallets, and Nodes recreated
#                                  on the current LEZ chain; idempotent
#   scripts/lanes.sh stop <N>      lanes 2..N: Nodes stopped, their swaps
#                                  forgotten (a new LEZ chain is coming)
#   scripts/lanes.sh down <N> [--wipe]
#                                  lanes 2..N removed; --wipe also their volumes
#                                  and runtime/lanes/<k>
#   scripts/lanes.sh status <N>
#   scripts/lanes.sh mint <N>      the market wallets of lanes 3..N, minted into the
#                                  market root when missing (the tools image); the
#                                  next LEZ chain funds them (gen-config.sh,
#                                  market-bootstrap.sh): up.sh --fresh-lez
#
# up.sh calls stop and up around a new LEZ chain when LEZ_LANES > 1, so one
# chain serves every lane. Lanes come up side by side; each logs to
# runtime/lanes/<k>/lanes.log.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1
command="${1:-}"; count="${2:-}"
[[ "$command" =~ ^(up|stop|down|status|mint)$ && "$count" =~ ^[1-9]$ ]] ||
  { sed -n 2,26p "${BASH_SOURCE[0]}"; exit 64; }
wipe=0; [[ "${3:-}" != --wipe ]] || wipe=1
set -a
# shellcheck source=/dev/null
source runtime/runtime.env
set +a
export BTC_RPC_PASSWORD
log() { printf '[lanes %s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }

wait_healthy() { # wait_healthy <seconds> <container>...
  local deadline=$(( $(date +%s) + $1 )) c; shift
  for c in "$@"; do
    until [[ "$(docker inspect -f '{{.State.Health.Status}}' "$c" 2>/dev/null)" == healthy ]]; do
      (( $(date +%s) < deadline )) || { log "$c never became healthy"; docker logs --tail 20 "$c" 2>&1; return 1; }
      sleep 3
    done
  done
}
owner_ready() { # owner_ready <role> <method> <params>: the Node's own owner call
  docker exec "$LEZ_CONTAINER_PREFIX-$1-node" curl -sS --max-time 30 --unix-socket "/run/lez/$1/node.sock" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":[$3]}" http://localhost/ 2>/dev/null | grep -q '"result"'
}

# One lane, in a subshell with its names (lane.sh: lane_compose and the LEZ_* names).
# shellcheck disable=SC2154
lane_up() {
  local market_root identity wallet balance
  market_root="$(sed -n 's/^LEZ_MARKET_ROOT=//p' runtime/runtime.env | head -1)"
  for wallet in "$LEZ_LANE_MAKER_WALLET" "$LEZ_LANE_TAKER_WALLET"; do
    identity="$market_root/identities/$wallet/identity.json"
    [[ -s "$identity" ]] || { log "lane $LEZ_LANE: no $wallet identity in the market root (scripts/lanes.sh mint $LEZ_LANE, then up.sh --fresh-lez funds it)"; return 1; }
    balance="$(curl -s --max-time 60 -H 'content-type: application/json' \
      --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccount\",\"params\":[\"$(jq -r .account_id "$identity")\"]}" \
      http://127.0.0.1:8779/ | jq -r '.result.balance // 0')"
    [[ "${balance:-0}" != 0 ]] || { log "lane $LEZ_LANE: $wallet holds no LEZ on this chain (a chain made before its identity: up.sh --fresh-lez)"; return 1; }
  done
  bash scripts/gen-config.sh runtime || return 1
  # (with their init containers, which own the new volumes to the chains' uid)
  docker compose "${lane_compose[@]}" up -d bitcoin-core zebra || return 1
  wait_healthy 600 "$LEZ_CONTAINER_PREFIX-bitcoin-core" "$LEZ_CONTAINER_PREFIX-zebra" || return 1
  docker compose "${lane_compose[@]}" up -d --no-deps btc-miner || return 1
  python3 scripts/seed-btc-wallets.py || return 1
  # Recreated: the LEZ chain or the lane's files may be new since they started.
  # The Taker pins the Maker's Delivery identity at start, so the Maker first.
  docker compose "${lane_compose[@]}" up -d --no-deps --force-recreate maker-node || return 1
  wait_healthy 1200 "$LEZ_CONTAINER_PREFIX-maker-node" || return 1
  docker compose "${lane_compose[@]}" up -d --no-deps --force-recreate taker-node || return 1
  wait_healthy 1200 "$LEZ_CONTAINER_PREFIX-taker-node" || return 1
  local deadline=$(( $(date +%s) + 300 ))
  until owner_ready maker maker_health '{}' && owner_ready taker taker_health '{"schema_version":1}'; do
    (( $(date +%s) < deadline )) || { log "lane $LEZ_LANE: the Nodes do not answer their owner health calls"; return 1; }
    sleep 3
  done
  log "lane $LEZ_LANE up: $LEZ_CONTAINER_PREFIX-{maker,taker}-node ($LEZ_LANE_MAKER_WALLET, $LEZ_LANE_TAKER_WALLET), Zebra on 127.0.0.1:$LEZ_ZEBRA_PORT, VNC $LEZ_MAKER_VNC_PORT/$LEZ_TAKER_VNC_PORT"
}
lane_stop() {
  docker compose "${lane_compose[@]}" stop maker-node taker-node >/dev/null 2>&1 || true
  if docker volume inspect "$LEZ_VOLUME_PREFIX-maker-state" >/dev/null 2>&1; then
    bash scripts/reset-swaps.sh --offline --abandon-open 2>&1 | tail -1
  fi
  log "lane $LEZ_LANE stopped"
}
lane_down() {
  local flags=(--remove-orphans)
  (( wipe == 0 )) || flags+=(-v)
  docker compose "${lane_compose[@]}" down "${flags[@]}" >/dev/null 2>&1 || return 1
  (( wipe == 0 )) || rm -rf "$LEZ_LANE_RUNTIME"
  if (( wipe )); then log "lane $LEZ_LANE down, volumes and files wiped"; else log "lane $LEZ_LANE down"; fi
}
lane_mint() {
  local market_root wallet
  market_root="$(sed -n 's/^LEZ_MARKET_ROOT=//p' runtime/runtime.env | head -1)"
  for wallet in "$LEZ_LANE_MAKER_WALLET" "$LEZ_LANE_TAKER_WALLET"; do
    [[ ! -f "$market_root/identities/$wallet/identity.json" ]] || continue
    log "minting the $wallet identity"
    rm -rf "${market_root:?}/identities/$wallet"
    docker compose --env-file runtime/runtime.env --profile tools run --rm --no-deps tools \
      "lez-v02-local-actor-identity --output-directory /market/identities/$wallet" >/dev/null || return 1
    cp "$market_root/identities/$wallet/identity.json" "$market_root/identities/$wallet.json"
  done
}
lane_status() {
  local c
  for c in maker-node taker-node bitcoin-core btc-miner zebra; do
    printf '  lane %s %-28s %s\n' "$LEZ_LANE" "$LEZ_CONTAINER_PREFIX-$c" \
      "$(docker inspect -f '{{.State.Status}} {{if .State.Health}}{{.State.Health.Status}}{{end}}' "$LEZ_CONTAINER_PREFIX-$c" 2>/dev/null || echo absent)"
  done
}

in_lane() { # in_lane <k> <function>
  # shellcheck source=lane.sh
  ( export LEZ_LANE="$1"; source scripts/lane.sh; "$2" )
}

(( count >= 2 )) || { log "one lane: nothing to do"; exit 0; }
status=0
case "$command" in
  up)
    pids=()
    for (( k = 2; k <= count; k++ )); do
      mkdir -p "runtime/lanes/$k"
      in_lane "$k" lane_up >"runtime/lanes/$k/lanes.log" 2>&1 &
      pids+=("$!")
    done
    for (( k = 2; k <= count; k++ )); do
      if wait "${pids[k - 2]}"; then tail -1 "runtime/lanes/$k/lanes.log"
      else log "lane $k did not come up (runtime/lanes/$k/lanes.log):"; tail -5 "runtime/lanes/$k/lanes.log"; status=1; fi
    done ;;
  stop) for (( k = 2; k <= count; k++ )); do in_lane "$k" lane_stop || status=1; done ;;
  down) for (( k = 2; k <= count; k++ )); do in_lane "$k" lane_down || status=1; done ;;
  status) for (( k = 2; k <= count; k++ )); do in_lane "$k" lane_status; done ;;
  mint) for (( k = 3; k <= count; k++ )); do in_lane "$k" lane_mint || status=1; done ;;
esac
exit "$status"
