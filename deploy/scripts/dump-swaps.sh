#!/usr/bin/env bash
# dump-swaps.sh <label> — copy everything both Nodes know about their swaps
# into runtime/e2e/dumps/<label>-<utc>/ before a reset destroys it: the owner
# views (swap lists, actor monitor), each swap directory (journals, sidecar
# logs, state databases) and both Nodes' container logs. Read only.
set -euo pipefail
cd "$(dirname "$0")/.."
label="${1:-manual}"
out="runtime/e2e/dumps/${label}-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$out"
owner() { # role method
  docker exec "lez-$1-node" curl -sS --max-time 60 --unix-socket "/run/lez/$1/node.sock" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":[{\"schema_version\":1}]}" http://localhost/ \
    >"$out/$1-$2.json" 2>"$out/$1-$2.err" || true
}
owner taker taker_swap_list_v1
owner maker maker_actor_monitor_v1
for role in taker maker; do
  docker logs --since 6h "lez-$role-node" >"$out/$role-node.log" 2>&1 || true
  docker cp "lez-$role-node:/var/lib/lez/$role/btc/swaps" "$out/$role-swaps" 2>/dev/null || true
done
du -sh "$out" | cut -f1 | xargs -I{} echo "swap state dumped to $out ({})"
