#!/usr/bin/env bash
# reset-swaps.sh — forget every swap both Nodes have persisted, keep everything else.
#
# Removes each Node's per-swap directories (role roots, actor stores, sidecar
# state), the Taker registry's swap rows, the Maker store's swap aggregates,
# actor registrations, negotiations and the offers those swaps reserved or
# consumed, and the exported swap evidence (the proof view returns to the
# certified sample). Chains, wallets, identities, the escrow deployment, the
# route preset and the Maker's Delivery signing key are untouched.
#
# Use it after rebuilding the actor programs: persisted swaps pin the program
# hash and stop loading (they list as attention_required) once it changes.
#
#   reset-swaps.sh            stop both Nodes, forget the swaps, start them again
#   reset-swaps.sh --offline  the same, but leave the Nodes stopped (up.sh
#                             --fresh-lez forgets the old chain's swaps this way
#                             before it starts the Nodes on the new one)
#
# Each Node runs inside its party's Basecamp, supervised by lez_swap_node. When
# both Basecamps are up only the Nodes stop: each is held down (the supervisor's
# hold file) while its swaps are forgotten and released afterwards, so Basecamp,
# its modules and the role's LEZ signer keep running and a reset takes seconds.
# With a Basecamp down (or LEZ_RESET_IN_PLACE=0) the containers are stopped and
# started instead.
set -euo pipefail
offline=0
[[ "${1:-}" != --offline ]] || offline=1
cd "$(dirname "${BASH_SOURCE[0]}")/.."
set -a
# shellcheck source=/dev/null
source runtime/runtime.env
set +a
export BTC_RPC_PASSWORD
compose=(docker compose --env-file runtime/runtime.env)
log() { printf '[reset %s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
running() { [[ "$(docker inspect -f '{{.State.Running}}' "lez-$1-node" 2>/dev/null)" == true ]]; }
in_place=0
if [[ "$offline" == 0 && "${LEZ_RESET_IN_PLACE:-1}" == 1 ]] && running maker && running taker; then in_place=1; fi
# Runs a command with the role's state volume, as the Node uid, while the Node
# itself is stopped: inside the role's running Basecamp, or else in a fresh
# container of the role image. stdin stays connected: two calls below feed
# python3 a heredoc.
in_role() {
  local role="$1"; shift
  if [[ "$in_place" == 1 ]]; then docker exec -i "lez-$role-node" "$@"
  else "${compose[@]}" run --rm -T --no-deps --entrypoint "$1" "$role-node" "${@:2}"; fi
}
# Holds a role's Node down inside its Basecamp: the hold file keeps the
# supervisor from restarting it, then the Node's process group (the Node, its
# loopback forwarders and sidecars) is ended.
hold_node() { # hold_node <role> <ROLE>
  docker exec "lez-$1-node" sh -c '
    touch "$LEZ_'"$2"'_NODE_HOLD"
    group="$(pgrep -o -x "lez-'"$1"'-node" || pgrep -o -f node-entrypoint.sh || true)"
    [ -n "$group" ] || exit 0
    pkill -TERM -g "$group"
    for _ in $(seq 75); do pgrep -g "$group" >/dev/null || exit 0; sleep 0.2; done
    pkill -KILL -g "$group"; sleep 1; ! pgrep -g "$group" >/dev/null'
}
release_node() { docker exec "lez-$1-node" sh -c 'rm -f "$LEZ_'"$2"'_NODE_HOLD"' 2>/dev/null || true; }

if [[ "$in_place" == 1 ]]; then
  log "holding both Nodes down inside their Basecamps (their per-swap sidecars stop with them)"
  # a reset that fails must not leave a Node held down
  trap 'release_node maker MAKER; release_node taker TAKER' EXIT
  hold_node maker MAKER; hold_node taker TAKER
else
  log "stopping both Nodes (their per-swap sidecars stop with them)"
  "${compose[@]}" stop maker-node taker-node >/dev/null
fi

log "Taker: swap directories and registry rows"
in_role taker bash -c 'for pair in /var/lib/lez/taker/*/; do
  rm -rf "$pair"swaps/* "$pair"sidecar-state "$pair"sidecar.log
done
# the Zcash actors'"'"' supervision schedule (one row per taken swap) goes with the swaps
rm -f /var/lib/lez/taker/taker-actor-schedule.sqlite3 /var/lib/lez/taker/taker-actor-schedule.sqlite3-wal /var/lib/lez/taker/taker-actor-schedule.sqlite3-shm'
in_role taker python3 - <<'PY'
import sqlite3
store = sqlite3.connect("/var/lib/lez/taker/registry.sqlite3")
for table in ("taker_facade_requests", "taker_facade_authorities", "taker_facade_swaps"):
    store.execute(f'delete from "{table}"')
store.commit()
store.execute("vacuum")
print("  registry: swaps, authorities and requests cleared")
PY

log "Maker: swap directories, actor registrations, negotiations, consumed offers"
in_role maker bash -c 'for pair in /var/lib/lez/maker/*/; do
  rm -rf "$pair"swaps/* "$pair"sidecar-state "$pair"sidecar.log "$pair"lez-*-maker-actor-trace
done'
in_role maker python3 - <<'PY'
import sqlite3
store = sqlite3.connect("/var/lib/lez/maker/maker.sqlite3")
store.execute("pragma foreign_keys = on")
counts = {}
# Negotiations and reserved/consumed lots reference their swap with RESTRICT;
# everything else hangs off `swaps` with CASCADE.
for table in ("maker_btc_negotiations", "maker_zec_negotiations", "maker_xmr_negotiations"):
    counts[table] = store.execute(f'delete from "{table}"').rowcount
counts["maker_offers (reserved/consumed)"] = store.execute(
    "delete from maker_offers where state in ('reserved', 'consumed')").rowcount
counts["swaps (+ actor registrations, progress, events, projections)"] = store.execute("delete from swaps").rowcount
# The Maker's ZEC recovery store lives in this database (zec_sdk_* tables) and
# revalidates every persisted agreement at start; a reset leaves none behind,
# so a profile change between runs cannot keep the Node from starting.
zec_tables = [row[0] for row in store.execute(
    "select name from sqlite_master where type = 'table' and name like 'zec_sdk_%' order by name")]
if "zec_sdk_agreements" in zec_tables:
    counts["zec_sdk_agreements (+ every ZEC recovery row)"] = store.execute("delete from zec_sdk_agreements").rowcount
    for table in zec_tables:
        store.execute(f'delete from "{table}"')
store.commit()
store.execute("vacuum")
for table, count in counts.items():
    if count:
        print(f"  {table}: {count} rows removed")
PY

log "Bitcoin wallets: release coins reserved by plans that never broadcast"
for wallet in lez-taker lez-maker; do
  docker exec lez-bitcoin-core bitcoin-cli -conf=/run-config/bitcoin.conf -datadir=/var/lib/bitcoin \
    -rpcwallet="$wallet" lockunspent true >/dev/null 2>&1 || true
done

log "exported evidence: back to the certified sample"
find runtime/evidence -name '*.json' ! -name 'certified-*.json' -delete
cp assets/certified-evidence-m5arm-08180005-ui.json runtime/m3-btc-ui-evidence.json.tmp
cat runtime/m3-btc-ui-evidence.json.tmp >runtime/m3-btc-ui-evidence.json  # in place: the mount pins the inode
rm -f runtime/m3-btc-ui-evidence.json.tmp

if [[ "$offline" == 1 ]]; then
  log "done: swaps forgotten; the Nodes stay stopped"
  exit 0
fi
if [[ "$in_place" == 1 ]]; then
  log "releasing both Nodes"
  release_node maker MAKER; release_node taker TAKER
else
  log "starting both Nodes"
  "${compose[@]}" up -d --no-deps maker-node taker-node >/dev/null
fi
# The Node's own owner call, not the container's health status: that one is
# read on an interval and still shows the Node that was just stopped.
owner_ready() { # owner_ready <role> <method> <params>
  docker exec "lez-$1-node" curl -sS --max-time 30 --unix-socket "/run/lez/$1/node.sock" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":[$3]}" http://localhost/ 2>/dev/null | grep -q '"result"'
}
deadline=$(( $(date +%s) + 600 ))
until owner_ready maker maker_health '{}' && owner_ready taker taker_health '{"schema_version":1}'; do
  (( $(date +%s) < deadline )) || { log "the Nodes did not answer their owner health calls within 600 s"; exit 1; }
  sleep 1
done
taker_swaps="$(docker exec lez-taker-node curl -sS --unix-socket /run/lez/taker/node.sock \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"taker_swap_list_v1","params":[{"schema_version":1}]}' http://localhost/ |
  python3 -c 'import json,sys; print(len(json.load(sys.stdin)["result"]["swaps"]))')"
maker_swaps="$(docker exec lez-maker-node curl -sS --unix-socket /run/lez/maker/node.sock \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"swap_history","params":[{}]}' http://localhost/ |
  python3 -c 'import json,sys; print(len(json.load(sys.stdin)["result"]))')"
log "done: Taker lists $taker_swaps swaps, Maker lists $maker_swaps"
[[ "$taker_swaps" == 0 && "$maker_swaps" == 0 ]]
