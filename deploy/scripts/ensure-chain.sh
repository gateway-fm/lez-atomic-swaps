#!/usr/bin/env bash
# ensure-chain.sh — make sure the LEZ chain is finalising before a scenario runs,
# and recreate it when it is not.
#
# Upstream logos-blockchain#3643 freezes the indexer's finalized height while
# Bedrock keeps proposing. Nothing reports it: `maker_health` stays
# `ready: true, degraded: false`, the containers stay healthy, and the only
# visible symptom is that every swap needing a LEZ lock misses its window —
# a desk journey spends half an hour reaching `recovering (Lock window missed)`.
# A chain that carried a full suite an hour ago can be stalled now, so a long
# run has to check rather than assume.
#
# Prints one word and exits 0 for ok/recreated, 1 for broken:
#   ok         finality moved inside the first window
#   recreated  it did not, `up.sh --fresh-lez` brought the chain back
#   broken     still not finalising after the recreate
#
#   scripts/ensure-chain.sh [check_window_seconds] [recheck_window_seconds]
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

check_window="${1:-60}"
recheck_window="${2:-180}"
# The indexer, reached the way node-e2e.py reaches it: the Taker's loopback
# forwarder, so this works whether or not the port is published on the host.
indexer_url="${LEZ_INDEXER_URL:-http://127.0.0.1:8779/}"

finalized() {
  docker exec lez-taker-node curl -sf -m 10 -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' \
    "$indexer_url" 2>/dev/null |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["result"])' 2>/dev/null
}

advances() { # advances <window_seconds>
  local window="$1" first second waited=0
  first="$(finalized)"
  [[ -n "$first" ]] || return 1
  while (( waited < window )); do
    sleep 15; waited=$(( waited + 15 ))
    second="$(finalized)"
    [[ -n "$second" ]] || return 1
    (( second > first )) && return 0
  done
  return 1
}

if advances "$check_window"; then echo ok; exit 0; fi
if bash scripts/up.sh --fresh-lez >"${LEZ_FRESH_LOG:-/dev/null}" 2>&1 && advances "$recheck_window"; then
  echo recreated; exit 0
fi
echo broken; exit 1
