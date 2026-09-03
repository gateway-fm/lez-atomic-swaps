#!/usr/bin/env bash
# repair-indexer.sh — rebuild the LEZ indexer's database when it can no longer
# serve historical account reads.
#
# The v0.2 indexer takes a state breakpoint every 100 blocks and serves
# getAccountAtBlock by replaying from the last breakpoint. On a long-lived
# chain it occasionally fails to write one; from then on every historical
# read past the gap fails, a restart refuses to start, and a swap's Maker
# actor cannot observe its own LEZ initialization. The index is derived from
# bedrock, so wiping it and re-indexing restores it (~2-3 min per 1000 blocks).
#
#   scripts/repair-indexer.sh [--check]    --check only reports, exit 1 when broken
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

indexer_url="http://127.0.0.1:8779"
rpc() { curl -sf -m 10 -H 'content-type: application/json' -d "$1" "$indexer_url"; }
tip() { rpc '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' | jq -r '.result // empty'; }
historical_read_ok() {
  local height="$1" account
  account="$(sed -n 's/^LEZ_V02_MAKER_ACCOUNT_ID=//p' runtime/runtime.env)"
  rpc "$(jq -cn --arg a "$account" --argjson h "$height" '{jsonrpc:"2.0",id:1,method:"getAccountAtBlock",params:[$a,$h]}')" |
    jq -e '.error == null' >/dev/null 2>&1
}

current="$(tip)"
if [[ -n "$current" && "$current" -gt 1 ]] && historical_read_ok "$((current - 1))"; then
  echo "indexer serves historical reads at the tip ($current)"
  exit 0
fi
echo "indexer cannot serve a historical read at its tip (${current:-unreachable})"
[[ "${1:-}" != "--check" ]] || exit 1

echo "re-indexing from bedrock…"
docker stop lez-indexer >/dev/null
backup="runtime/indexer-broken-$(date -u +%Y%m%d-%H%M%S)"
mkdir -p "$backup"
mv runtime/indexer/rocksdb-* "$backup"/
docker start lez-indexer >/dev/null
previous=0
for _ in $(seq 1 360); do
  sleep 10
  current="$(tip || true)"
  [[ -n "$current" ]] || continue
  if [[ "$current" -gt 1 && $((current - previous)) -lt 5 ]] && historical_read_ok "$((current - 1))"; then
    echo "re-indexed to $current; historical reads restored (old database kept in $backup)"
    exit 0
  fi
  previous="$current"
done
echo "indexer did not catch up in time" >&2
exit 1
