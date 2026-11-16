#!/usr/bin/env bash
# Runs the full non-settlement verification set over the running stack:
#   1. container health
#   2. settlement-chain state (both chains + the four persistent wallets)
#   3. block explorers actually display every certified swap transaction
#   4. Node market behaviour (publication, discovery, replay, withdrawal)
#   5. desk regression suites, each in its party's own Basecamp (maker and taker)
#
# It starts no swaps and submits no chain effects. The market check publishes
# and withdraws one uniquely named offer; the Maker Node keeps it in its history.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit

env_file=runtime/runtime.env
[[ -f "$env_file" ]] || { echo "missing $env_file; run ./scripts/up.sh first" >&2; exit 1; }
set -a
# shellcheck source=/dev/null
source "$env_file"
set +a
# The stack this directory runs (scripts/lane.sh): its containers and ports.
# shellcheck source=lane.sh
source scripts/lane.sh || exit 1
compose=(docker compose --env-file "$env_file")
failures=0
section() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
report() {
  if [[ "$1" == 0 ]]; then printf '  \033[32mOK\033[0m   %s\n' "$2"
  else printf '  \033[31mFAIL\033[0m %s\n' "$2"; failures=$((failures + 1)); fi
}

section "containers"
unhealthy="$("${compose[@]}" ps --format '{{.Service}} {{.Status}}' |
  grep -viE 'up .*(healthy)?|running' || true)"
"${compose[@]}" ps --format '  {{.Service}}\t{{.Status}}' | sed 's/^/ /'
report "$([[ -z "$unhealthy" ]] && echo 0 || echo 1)" "all services up"

section "settlement chains"
btc_height="$(docker exec "$LEZ_CONTAINER_PREFIX-bitcoin-core" bitcoin-cli -conf=/run-config/bitcoin.conf \
  -datadir=/var/lib/bitcoin getblockcount 2>/dev/null)"
report "$([[ "${btc_height:-0}" -gt 100 ]] && echo 0 || echo 1)" \
  "Bitcoin chain at height ${btc_height:-unknown}"
indexes="$(docker exec "$LEZ_CONTAINER_PREFIX-bitcoin-core" bitcoin-cli -conf=/run-config/bitcoin.conf \
  -datadir=/var/lib/bitcoin getindexinfo 2>/dev/null)"
grep -q txospenderindex <<<"$indexes"
report "$?" "Bitcoin spender index present (actor lock observation)"
lez_head="$(curl -sf -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' "$LEZ_STACK_INDEXER_URL" |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["result"])' 2>/dev/null)"
report "$([[ "${lez_head:-None}" != None ]] && echo 0 || echo 1)" "LEZ chain finalized at block ${lez_head:-unknown}"

section "block explorers"
# Off by default (Compose profile "explorers"): started here for their checks.
# Pulled images (LEZ_IMAGES=pull, a release bundle) are started as they are:
# start.sh and up.sh pulled the explorers with the rest, and a bundle has no
# images/ build contexts. A checkout builds them here when they are missing,
# also on its own images (LEZ_IMAGES=present): up.sh builds and checks only
# the services outside the "explorers" profile.
explorers_up=(--profile explorers up -d --wait --wait-timeout 180)
if [[ "${LEZ_IMAGES:-build}" == pull || ! -d images/btc-explorer || ! -d images/lez-explorer ]]; then
  explorers_up+=(--no-build)
fi
"${compose[@]}" "${explorers_up[@]}" btc-explorer lez-explorer >/dev/null 2>&1
# Neither explorer has a health check, so `up --wait` returns when the
# containers run, before they listen (the Bitcoin explorer closed the first
# connection of a fresh start: "Remote end closed connection without
# response"). Ask until both answer, for up to two minutes.
explorers_deadline=$(( $(date +%s) + 120 ))
until curl -sf -o /dev/null --max-time 5 "http://127.0.0.1:$LEZ_BTC_EXPLORER_PORT/" &&
      curl -sf -o /dev/null --max-time 5 "http://127.0.0.1:$LEZ_LEZ_EXPLORER_PORT/"; do
  (( $(date +%s) < explorers_deadline )) || break
  sleep 2
done
python3 scripts/verify-explorers.py --evidence-dir runtime/evidence \
  --btc "http://127.0.0.1:$LEZ_BTC_EXPLORER_PORT" --lez "http://127.0.0.1:$LEZ_LEZ_EXPLORER_PORT"
report "$?" "explorer transaction display"

section "Node market"
python3 scripts/verify-market.py
report "$?" "Node market behaviour"

section "desk regressions (each party's own Basecamp)"
for role in maker taker; do
  docker exec "$LEZ_CONTAINER_PREFIX-$role-node" node /ui-tests/verify.mjs "$role" </dev/null 2>&1 | grep -E '✓|✗|passed|^    [A-Za-z]'
  report "${PIPESTATUS[0]}" "desk suite: $role"
done

printf '\n'
if [[ "$failures" == 0 ]]; then
  printf '\033[32mall verification stages passed\033[0m\n'
else
  printf '\033[31m%d verification stage(s) failed\033[0m\n' "$failures"
fi
exit "$((failures > 0))"
