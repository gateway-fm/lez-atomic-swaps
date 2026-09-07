#!/usr/bin/env bash
# node-swap.sh — one BTC→LEZ swap settled by the two Nodes alone (ADR 0213):
# the Maker publishes an offer, the Taker takes it through
# taker_swap_initiate_v1 (reservation, funding plan, draft, ceremony, actor),
# locks its Bitcoin with taker_swap_lock_v1, the Maker's supervised actor funds
# LEZ and later claims Bitcoin, and the Taker claims LEZ. No runner, no demo
# controller. Usage: scripts/node-swap.sh [--no-wait]
set -euo pipefail
DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DEPLOY_ROOT"
wait_for_completion=1; [[ "${1:-}" == "--no-wait" ]] && wait_for_completion=0

rpc() { # rpc <container> <socket> <method> <params-json>
  docker exec "$1" curl -sS --max-time 120 --unix-socket "$2" -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$3\",\"params\":[$4]}" http://localhost/
}
mk() { rpc lez-maker-node /run/lez/maker/node.sock "$@"; }
tk() { rpc lez-taker-node /run/lez/taker/node.sock "$@"; }
result() { python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(1) if "error" in d and print(json.dumps(d["error"])) else print(json.dumps(d["result"]))'; }
stamp="$(date -u +%s)"
route='{"pair":"Bitcoin","direction":"TakerSellsForeign"}'
foreign_units=1000000

echo "== Maker: enable the BTC route and publish an offer"
pair_revision="$(mk maker_pair_list '{}' | python3 -c 'import json,sys; print([p["revision"] for p in json.load(sys.stdin)["result"] if p["value"]["route"]["pair"]=="Bitcoin"][0])')"
price_revision="$(mk maker_local_price_list '{}' | python3 -c 'import json,sys; print([p["revision"] for p in json.load(sys.stdin)["result"] if p["value"]["route"]["pair"]=="Bitcoin"][0])')"
mk maker_local_route_save_v1 "{\"request_id\":\"node-route-$stamp\",\"expected_pair_revision\":$pair_revision,\"expected_price_revision\":$price_revision,
  \"configuration\":{\"route\":$route,\"enabled\":true,\"price_source\":\"local\",\"minimum_foreign_units\":$foreign_units,\"maximum_foreign_units\":$foreign_units,\"offer_ttl_seconds\":3600},
  \"price\":{\"route\":$route,\"lez_units_per_lot\":1,\"foreign_units_per_lot\":1000}}" | result >/dev/null
offer_id="offer-node-btc-$stamp"
mk maker_offer_publish "{\"request_id\":\"node-publish-$stamp\",\"offer_id\":\"$offer_id\",\"route\":$route}" | result | cut -c1-200

echo "== Taker: discover the offer over Delivery"
sleep 2
offer_view="$(tk taker_offer_list_v1 "{\"schema_version\":1,\"route\":$route}" | python3 -c "
import json,sys
offers=json.load(sys.stdin)['result']['offers']
match=[o for o in offers if o['offer']['id']=='$offer_id']
assert match, 'offer not discovered: ' + json.dumps([o['offer']['id'] for o in offers])
print(json.dumps(match[0]))")"
maker_identity="$(printf '%s' "$offer_view" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["maker_identity"]))')"
envelope_sha="$(printf '%s' "$offer_view" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["signed_envelope_sha256"]))')"
lez_units="$(printf '%s' "$offer_view" | python3 -c "
import json,sys; o=json.load(sys.stdin)['offer']; p=o['price']
print($foreign_units * p['lez_units_per_lot'] // p['foreign_units_per_lot'])")"
echo "offer $offer_id quotes $lez_units LEZ for $foreign_units sat"

echo "== Taker: initiate (reservation, funding plan, draft, ceremony, actor)"
request_id="node-take-$stamp"
initiate="$(tk taker_swap_initiate_v1 "{\"schema_version\":1,\"request_id\":\"$request_id\",\"offer_id\":\"$offer_id\",\"route\":$route,
  \"maker_identity\":$maker_identity,\"signed_envelope_sha256\":$envelope_sha,\"foreign_units\":$foreign_units,\"expected_lez_units\":$lez_units}")"
printf '%s\n' "$initiate" | cut -c1-600
swap_id="$(printf '%s' "$initiate" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["result"]["swap"]["swap_id"] if "result" in d else "")')"
[[ -n "$swap_id" ]] || { echo "initiation failed; Taker log:"; docker logs lez-taker-node 2>&1 | tail -20; echo "Maker log:"; docker logs lez-maker-node 2>&1 | tail -20; exit 1; }
echo "swap $swap_id"

echo "== Taker: lock Bitcoin"
tk taker_swap_lock_v1 "{\"schema_version\":1,\"swap_id\":\"$swap_id\"}" | cut -c1-300
[[ "$wait_for_completion" == 1 ]] || exit 0

echo "== waiting for both actors"
deadline=$(( $(date -u +%s) + 2400 ))
claimed=0
while (( $(date -u +%s) < deadline )); do
  taker="$(tk taker_swap_monitor_v1 "{\"schema_version\":1,\"swap_id\":\"$swap_id\"}" | python3 -c 'import json,sys; d=json.load(sys.stdin); r=d.get("result",{}); print(r.get("state","?"), r.get("progress_generation","?"), r.get("available_action") or "-") if r else print("error", json.dumps(d.get("error"))[:120])')"
  maker="$(mk maker_actor_monitor_v1 "{\"id\":\"$swap_id\"}" | python3 -c 'import json,sys; d=json.load(sys.stdin); r=d.get("result"); print((r["schedule_state"], (r.get("progress") or {}).get("observation")) if r else ("error", json.dumps(d.get("error"))[:120]))')"
  echo "$(date -u +%T) taker: $taker | maker: $maker"
  state="${taker%% *}"
  if [[ "$state" == "claim_available" && "$claimed" == 0 ]]; then
    generation="$(printf '%s' "$taker" | awk '{print $2}')"
    tk taker_swap_claim_v1 "{\"schema_version\":1,\"request_id\":\"node-claim-$stamp\",\"swap_id\":\"$swap_id\",\"expected_generation\":$generation}" | cut -c1-300
    claimed=1
  fi
  [[ "$state" == "completed" ]] && { echo "swap completed"; exit 0; }
  [[ "$state" == "refunded" || "$state" == "attention_required" ]] && { echo "swap ended in $state"; exit 1; }
  sleep 20
done
echo "timed out"; exit 1
