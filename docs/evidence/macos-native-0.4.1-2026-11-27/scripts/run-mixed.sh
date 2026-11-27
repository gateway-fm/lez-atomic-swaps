#!/usr/bin/env bash
# run-mixed.sh <log> <scenario> — one Monero scenario of the stack's harness
# in the mixed layout (mixed-e2e.py), its output to <log>, bounded like
# regress.sh bounds it (timeout -k), stdin closed.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
log="${1:?log}"; scenario="${2:?scenario}"
: "${MIXED_STACK:?}" "${MIXED_NATIVE_ROLE:?}" "${MIXED_NATIVE_HOME:?}" "${MIXED_FUNDING_PORT:?}"
case "$scenario" in xmr-punish) seconds=1340 ;; xmr-taker-refund*) seconds=1020 ;; *) seconds=600 ;; esac
cd "$MIXED_STACK"
export PATH=/opt/homebrew/bin:$PATH
{ echo "# $(date -u +%FT%TZ) $scenario, $MIXED_NATIVE_ROLE native, bound ${seconds}s"
  timeout -k 30 "$seconds" python3 "$here/mixed-e2e.py" --pair Monero --direction TakerSellsLez "$scenario" </dev/null
  echo "# exit $? at $(date -u +%FT%TZ)"; } >"$log" 2>&1
