#!/usr/bin/env bash
# repair-indexer.sh — rebuild the LEZ indexer's database when it can no longer
# serve historical account reads.
#
# The v0.2 indexer takes a state breakpoint every 10 blocks
# (deploy/builder/patches, breakpoint-interval; 100 upstream) and serves
# getAccountAtBlock by replaying from the last breakpoint. On a long-lived
# chain it occasionally fails to write one; from then on every historical
# read past the gap fails, a restart refuses to start, and a swap's Maker
# actor cannot observe its own LEZ initialization. The index is derived from
# the node, so wiping it and re-indexing restores it (~2-3 min per 1000 blocks).
# The indexer and the node share the LEZ network's Basecamp (lez-network, the
# stack's <prefix>-network: scripts/lane.sh): its
# LEZ Network desk's Re-index has the indexer module delete its database and
# start again while the node keeps producing blocks.
#
#   scripts/repair-indexer.sh [--check]    --check only reports, exit 1 when broken
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1
# shellcheck source=lane.sh
source scripts/lane.sh

indexer_url="$LEZ_STACK_INDEXER_URL"
# A historical read replays from the last breakpoint, so its cost grows with the
# distance to it: on LEZ v0.2.4 a read 99 blocks past a breakpoint takes ~16 s
# (indexer_service and the indexer module alike). A shorter limit would call a
# slow read a broken indexer and re-index a healthy chain.
rpc() { curl -sf -m "${2:-10}" -H 'content-type: application/json' -d "$1" "$indexer_url"; }
tip() { rpc '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' | jq -r '.result // empty'; }
historical_read_ok() {
  local height="$1" account
  account="$(sed -n 's/^LEZ_V02_MAKER_ACCOUNT_ID=//p' runtime/runtime.env)"
  rpc "$(jq -cn --arg a "$account" --argjson h "$height" '{jsonrpc:"2.0",id:1,method:"getAccountAtBlock",params:[$a,$h]}')" 60 |
    jq -e '.error == null' >/dev/null 2>&1
}

current="$(tip)"
if [[ -n "$current" && "$current" -gt 1 ]] && historical_read_ok "$((current - 1))"; then
  echo "indexer serves historical reads at the tip ($current)"
  exit 0
fi
# A chain provisioned from scratch sits at the genesis block until something
# writes to the zone, and the first writer is the market bootstrap in the next
# phase. There is no history to repair yet, and re-indexing cannot create any,
# so waiting here would deadlock the bring-up against a step that runs later.
if [[ "$current" == 1 ]]; then
  echo "indexer is at the genesis block; nothing to repair on a fresh chain"
  exit 0
fi
echo "indexer cannot serve a historical read at its tip (${current:-unreachable})"
[[ "${1:-}" != "--check" ]] || exit 1

echo "re-indexing from the node…"
docker exec "$LEZ_STACK_PREFIX-network" node /opt/lez-network-reindex.mjs >/dev/null ||
  { echo "the LEZ Network desk did not take the Re-index" >&2; exit 1; }
previous=0
for _ in $(seq 1 360); do
  sleep 10
  current="$(tip || true)"
  [[ -n "$current" ]] || continue
  # Caught up: the height moves at the chain's pace again, not at a replay's.
  # The devnet makes a block a second, so a caught-up indexer moves ~10 in
  # these 10 s (it moved 4-5 on the hosts this was first run on, and "fewer
  # than 5" then never came true on a faster one: an hour's wait).
  if [[ "$current" -gt 1 && $((current - previous)) -lt 25 ]] && historical_read_ok "$((current - 1))"; then
    echo "re-indexed to $current; historical reads restored"
    exit 0
  fi
  previous="$current"
done
echo "indexer did not catch up in time" >&2
exit 1
