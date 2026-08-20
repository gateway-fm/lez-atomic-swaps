#!/usr/bin/env bash
# Runs the full non-settlement verification set over the running stack:
#   1. container health
#   2. settlement-chain state (both chains + the four persistent wallets)
#   3. block explorers actually display every certified swap transaction
#   4. wallet-market controller behaviour (validation, replay, role gating)
#   5. Basecamp UI regression suites (maker and taker)
#
# It starts no swaps and submits no chain effects. The market check creates and
# withdraws one uniquely named offer; the controller retains that audit row.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

env_file=runtime/runtime.env
[[ -f "$env_file" ]] || { echo "missing $env_file; run ./scripts/up.sh first" >&2; exit 1; }
set -a
source "$env_file"
set +a
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
btc_height="$(docker exec lez-bitcoin-core bitcoin-cli -conf=/run-config/bitcoin.conf \
  -datadir=/var/lib/bitcoin getblockcount 2>/dev/null)"
report "$([[ "${btc_height:-0}" -gt 100 ]] && echo 0 || echo 1)" \
  "Bitcoin chain at height ${btc_height:-unknown}"
indexes="$(docker exec lez-bitcoin-core bitcoin-cli -conf=/run-config/bitcoin.conf \
  -datadir=/var/lib/bitcoin getindexinfo 2>/dev/null)"
grep -q txospenderindex <<<"$indexes"
report "$?" "Bitcoin spender index present (actor lock observation)"
lez_head="$(curl -sf http://127.0.0.1:3003/api/overview |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["health"]["latest_block"])' 2>/dev/null)"
report "$([[ -n "${lez_head:-}" ]] && echo 0 || echo 1)" "LEZ chain at block ${lez_head:-unknown}"

section "block explorers"
python3 scripts/verify-explorers.py --runs "$LEZ_M3_RUNNER_REPO/.e2e"
report "$?" "explorer transaction display"

section "wallet market controller"
docker exec -i lez-btc-demo-controller python3 - < scripts/verify-market.py
report "$?" "market controller behaviour"

section "Basecamp UI regressions"
for role in maker taker; do
  "${compose[@]}" run --rm --no-deps --entrypoint node basecamp-ui \
    /ui-tests/verify.mjs "$role" 2>&1 | grep -E '✓|✗|passed'
  report "${PIPESTATUS[0]}" "UI suite: $role"
done

printf '\n'
if [[ "$failures" == 0 ]]; then
  printf '\033[32mall verification stages passed\033[0m\n'
else
  printf '\033[31m%d verification stage(s) failed\033[0m\n' "$failures"
fi
exit "$((failures > 0))"
