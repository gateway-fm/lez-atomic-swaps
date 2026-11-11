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
#   reset-swaps.sh                 stop both Nodes, forget the swaps, start them again
#   reset-swaps.sh --offline       the same, but leave the Nodes stopped (up.sh
#                                  --fresh-lez forgets the old chain's swaps this way,
#                                  while the Nodes still run, before it starts them
#                                  on the new chain)
#   reset-swaps.sh --abandon-open  also when a swap is still open (below)
#
# A swap that is still open may have funds locked, and its per-swap directory
# holds the only copy of its Bitcoin refund key (the Core wallet never sees
# it). The reset refuses while a Taker swap is not completed or refunded, a
# Maker actor is not terminal, or the Taker cannot be asked (its Node down).
# --abandon-open forgets them anyway, for test runs and a recreated chain: the
# Nodes no longer drive those swaps, and their per-swap directories and store
# databases are moved to /var/lib/lez/<role>/.reset-archive/<time>/ in the
# role's state volume instead of being deleted.
#
# Each Node runs inside its party's Basecamp, supervised by lez_swap_node. When
# both Basecamps are up only the Nodes stop: each is held down (the supervisor's
# hold file) while its swaps are forgotten and released afterwards, so Basecamp,
# its modules and the role's LEZ signer keep running and a reset takes seconds.
# With a Basecamp down (or LEZ_RESET_IN_PLACE=0) the containers are stopped and
# started instead.
set -euo pipefail
offline=0; abandon_open=0
for arg in "$@"; do
  case "$arg" in
    --offline) offline=1 ;;
    --abandon-open) abandon_open=1 ;;
    *) echo "unknown argument: $arg" >&2; exit 64 ;;
  esac
done
cd "$(dirname "${BASH_SOURCE[0]}")/.."
set -a
# shellcheck source=/dev/null
source runtime/runtime.env
set +a
export BTC_RPC_PASSWORD
# The pair of regression lane LEZ_LANE (lane 1, the stack itself, when unset).
# shellcheck source=lane.sh
source scripts/lane.sh
node_of() { echo "$LEZ_CONTAINER_PREFIX-$1-node"; }
# shellcheck disable=SC2154  # lane_compose is set by lane.sh, sourced above
compose=(docker compose "${lane_compose[@]}")
log() { printf '[reset %s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
running() { [[ "$(docker inspect -f '{{.State.Running}}' "$(node_of "$1")" 2>/dev/null)" == true ]]; }
in_place=0
if [[ "$offline" == 0 && "${LEZ_RESET_IN_PLACE:-1}" == 1 ]] && running maker && running taker; then in_place=1; fi
owner_call() { # owner_call <role> <method> <params>
  docker exec "$(node_of "$1")" curl -sS --max-time 30 --unix-socket "/run/lez/$1/node.sock" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":[$3]}" http://localhost/ 2>/dev/null
}
# Runs a command with the role's state volume, as the Node uid, while the Node
# itself is stopped: inside the role's running Basecamp, or else in a fresh
# container of the role image. stdin stays connected: two calls below feed
# python3 a heredoc. The fresh container takes the stopped Basecamp's image and
# mounts as they are (`compose run` would first re-check the role image's
# build, a few seconds a call, as it is built on the basecamp-ui service).
in_role() {
  local role="$1"; shift
  if [[ "$in_place" == 1 ]]; then docker exec -i "$(node_of "$role")" "$@"
  elif docker container inspect "$(node_of "$role")" >/dev/null 2>&1; then
    docker run --rm -i --network none --user 4713:4713 --volumes-from "$(node_of "$role")" --entrypoint "$1" \
      "$(docker inspect -f '{{.Config.Image}}' "$(node_of "$role")")" "${@:2}"
  else "${compose[@]}" run --rm -T --no-deps --entrypoint "$1" "$role-node" "${@:2}"; fi
}
# Holds a role's Node down inside its Basecamp (scripts/node-hold.sh: the
# hold file keeps the supervisor from restarting it, and the Node's process
# group, its loopback forwarders and sidecars included, is ended).
hold_node() { bash scripts/node-hold.sh hold "$1"; }
release_node() { bash scripts/node-hold.sh release "$1" 2>/dev/null || true; }

# Swaps that may still have funds locked, one "<role> <swap id> <state>" each.
# The Taker's own list says where each of its swaps stands; the Maker's actor
# schedule (read from its store, as the Node is not needed for it) says which
# of its actors have not reached an absorbing outcome.
open_swaps=()
if running taker && list="$(owner_call taker taker_swap_list_v1 '{"schema_version":1}')" &&
   jq -e '.result.swaps | type == "array"' <<<"$list" >/dev/null 2>&1; then
  mapfile -t -O "${#open_swaps[@]}" open_swaps < <(jq -r '.result.swaps[]
    | select(.state != "completed" and .state != "refunded") | "taker \(.swap_id) \(.state)"' <<<"$list")
else
  open_swaps+=("taker - unknown (its Node does not answer taker_swap_list_v1)")
fi
maker_store_read=(python3 -c '
import os, sqlite3
path = "/var/lib/lez/maker/maker.sqlite3"
if os.path.exists(path):
    try:
        store = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
        for swap, state in store.execute(
                "select swap_id, schedule_state from maker_actor_processes where schedule_state != '"'terminal'"'"):
            print("maker", swap, state)
    except sqlite3.Error as error:
        print("maker - unknown (store unreadable:", error, ")")
')
if running maker; then maker_open="$(docker exec "$(node_of maker)" "${maker_store_read[@]}" 2>/dev/null)" || maker_open="maker - unknown (store unreadable)"
else maker_open="$(in_role maker python3 "${maker_store_read[@]:1}" 2>/dev/null)" || maker_open="maker - unknown (store unreadable)"; fi
[[ -z "$maker_open" ]] || mapfile -t -O "${#open_swaps[@]}" open_swaps <<<"$maker_open"
archive=""
if (( ${#open_swaps[@]} )); then
  if [[ "$abandon_open" != 1 ]]; then
    log "refusing: these swaps may still have funds locked, and their per-swap directories hold the only copy of their refund keys:" >&2
    printf '  %s\n' "${open_swaps[@]}" >&2
    log "let them complete or refund, or pass --abandon-open to forget them anyway (their directories are then archived, not deleted)" >&2
    exit 1
  fi
  archive="$(date -u +%Y%m%dT%H%M%SZ)"
  log "abandoning the open swaps below; their per-swap directories and stores go to /var/lib/lez/<role>/.reset-archive/$archive"
  printf '  %s\n' "${open_swaps[@]}"
fi

if [[ "$in_place" == 1 ]]; then
  log "holding both Nodes down inside their Basecamps (their per-swap sidecars stop with them)"
  # a reset that fails must not leave a Node held down
  trap 'release_node maker; release_node taker' EXIT
  hold_node maker; hold_node taker
else
  log "stopping both Nodes (their per-swap sidecars stop with them)"
  "${compose[@]}" stop maker-node taker-node >/dev/null
fi

log "Taker: swap directories and registry rows"
# keep_swaps <role root> <archive stamp>: with a stamp, moves every pair's
# per-swap directories and sidecar state into the archive (mode 0700, keys).
keep_swaps='keep="${2:+$1/.reset-archive/$2}"
[ -z "$keep" ] || install -d -m 0700 "$1/.reset-archive" "$keep"
for pair in "$1"/*/; do
  [ -n "$keep" ] && [ -d "$pair" ] || continue
  dest="$keep/$(basename "$pair")"
  install -d -m 0700 "$dest/swaps"
  [ ! -d "$pair"swaps ] || find "$pair"swaps -mindepth 1 -maxdepth 1 -exec mv -t "$dest/swaps" {} +
  [ ! -e "$pair"sidecar-state ] || mv "$pair"sidecar-state "$dest/"
done'
in_role taker bash -c "$keep_swaps"'
for pair in /var/lib/lez/taker/*/; do
  rm -rf "$pair"swaps/* "$pair"sidecar-state "$pair"sidecar.log
done
# the Zcash actors'"'"' supervision schedule (one row per taken swap) goes with the swaps
for f in taker-actor-schedule.sqlite3 taker-actor-schedule.sqlite3-wal taker-actor-schedule.sqlite3-shm; do
  if [ -n "$keep" ] && [ -e "/var/lib/lez/taker/$f" ]; then mv "/var/lib/lez/taker/$f" "$keep/"; fi
  rm -f "/var/lib/lez/taker/$f"
done' _ /var/lib/lez/taker "$archive"
in_role taker python3 - "$archive" <<'PY'
import sqlite3, sys
store = sqlite3.connect("/var/lib/lez/taker/registry.sqlite3")
if sys.argv[1]:
    with sqlite3.connect(f"/var/lib/lez/taker/.reset-archive/{sys.argv[1]}/registry.sqlite3") as kept:
        store.backup(kept)
for table in ("taker_facade_requests", "taker_facade_authorities", "taker_facade_swaps"):
    store.execute(f'delete from "{table}"')
store.commit()
store.execute("vacuum")
print("  registry: swaps, authorities and requests cleared")
PY

log "Maker: swap directories, actor registrations, negotiations, consumed offers"
in_role maker bash -c "$keep_swaps"'
for pair in /var/lib/lez/maker/*/; do
  rm -rf "$pair"swaps/* "$pair"sidecar-state "$pair"sidecar.log "$pair"lez-*-maker-actor-trace
done' _ /var/lib/lez/maker "$archive"
in_role maker python3 - "$archive" <<'PY'
import sqlite3, sys
store = sqlite3.connect("/var/lib/lez/maker/maker.sqlite3")
if sys.argv[1]:
    with sqlite3.connect(f"/var/lib/lez/maker/.reset-archive/{sys.argv[1]}/maker.sqlite3") as kept:
        store.backup(kept)
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
  docker exec "$LEZ_CONTAINER_PREFIX-bitcoin-core" bitcoin-cli -conf=/run-config/bitcoin.conf -datadir=/var/lib/bitcoin \
    -rpcwallet="$wallet" lockunspent true >/dev/null 2>&1 || true
done

log "exported evidence: back to the certified sample"
evidence="$LEZ_LANE_RUNTIME"  # a lane's own; lane 1's is the explorer's
mkdir -p "$evidence/evidence"
find "$evidence/evidence" -name '*.json' ! -name 'certified-*.json' -delete
cp assets/certified-evidence-m5arm-08180005-ui.json "$evidence/m3-btc-ui-evidence.json.tmp"
cat "$evidence/m3-btc-ui-evidence.json.tmp" >"$evidence/m3-btc-ui-evidence.json"  # in place: the mount pins the inode
rm -f "$evidence/m3-btc-ui-evidence.json.tmp"

if [[ "$offline" == 1 ]]; then
  log "done: swaps forgotten${archive:+ (open ones archived as .reset-archive/$archive)}; the Nodes stay stopped"
  exit 0
fi
if [[ "$in_place" == 1 ]]; then
  log "releasing both Nodes"
  release_node maker; release_node taker
else
  log "starting both Nodes"
  "${compose[@]}" up -d --no-build --no-deps maker-node taker-node >/dev/null
fi
# The Node's own owner call, not the container's health status: that one is
# read on an interval and still shows the Node that was just stopped.
owner_ready() { # owner_ready <role> <method> <params>
  docker exec "$(node_of "$1")" curl -sS --max-time 30 --unix-socket "/run/lez/$1/node.sock" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":[$3]}" http://localhost/ 2>/dev/null | grep -q '"result"'
}
deadline=$(( $(date +%s) + 600 ))
until owner_ready maker maker_health '{}' && owner_ready taker taker_health '{"schema_version":1}'; do
  (( $(date +%s) < deadline )) || { log "the Nodes did not answer their owner health calls within 600 s"; exit 1; }
  sleep 1
done
taker_swaps="$(docker exec "$(node_of taker)" curl -sS --unix-socket /run/lez/taker/node.sock \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"taker_swap_list_v1","params":[{"schema_version":1}]}' http://localhost/ |
  python3 -c 'import json,sys; print(len(json.load(sys.stdin)["result"]["swaps"]))')"
maker_swaps="$(docker exec "$(node_of maker)" curl -sS --unix-socket /run/lez/maker/node.sock \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"swap_history","params":[{}]}' http://localhost/ |
  python3 -c 'import json,sys; print(len(json.load(sys.stdin)["result"]))')"
log "done: Taker lists $taker_swaps swaps, Maker lists $maker_swaps"
[[ "$taker_swaps" == 0 && "$maker_swaps" == 0 ]]
