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
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
set -a
# shellcheck source=/dev/null
source runtime/runtime.env
set +a
export BTC_RPC_PASSWORD
compose=(docker compose --env-file runtime/runtime.env)
log() { printf '[reset %s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
# Runs a command inside a fresh container of the role image with the role's
# state volume mounted, as the Node uid, while the Node itself is stopped.
in_role() { local role="$1"; shift; "${compose[@]}" run --rm --no-deps --entrypoint "$1" "$role-node" "${@:2}"; }

log "stopping both Nodes (their per-swap sidecars stop with them)"
"${compose[@]}" stop maker-node taker-node >/dev/null

log "Taker: swap directories and registry rows"
in_role taker bash -c 'rm -rf /var/lib/lez/taker/btc/swaps/* /var/lib/lez/taker/btc/sidecar-state /var/lib/lez/taker/btc/sidecar.log'
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
in_role maker bash -c 'rm -rf /var/lib/lez/maker/btc/swaps/* /var/lib/lez/maker/btc/sidecar-state /var/lib/lez/maker/btc/sidecar.log /var/lib/lez/maker/btc/lez-btc-maker-actor-trace'
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

log "starting both Nodes"
"${compose[@]}" up -d --no-deps maker-node taker-node >/dev/null
until [[ "$("${compose[@]}" ps --format '{{.Service}} {{.Status}}' | grep -E '^(maker|taker)-node ' | grep -c healthy)" == 2 ]]; do sleep 3; done
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
