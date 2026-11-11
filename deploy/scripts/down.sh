#!/usr/bin/env bash
# down.sh — stop the whole stack: chains, and each party's Basecamp and Node. --wipe also removes
# runtime state and this stack's named volumes (Bitcoin data, Node stores,
# sockets), so the next up.sh --fresh starts from nothing. The market root
# keeps its wallet identities (they outlive any chain), but its escrow
# deployment and bootstrap manifest belong to the chain being wiped: --wipe
# retires them, and the next bootstrap deploys again on the new chain.
set -euo pipefail
DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DEPLOY_ROOT"

# compose interpolation needs the same variables the stack was started with;
# the volume prefix in particular selects which volumes --wipe removes.
if [[ -f runtime/runtime.env ]]; then set -a; source runtime/runtime.env; set +a; fi
export BTC_RPC_PASSWORD="${BTC_RPC_PASSWORD:-teardown}"
# The stack this directory runs (its Compose project and container names).
# shellcheck source=lane.sh
source scripts/lane.sh
# A stack from before the parties' Basecamps took the desks also ran this one.
docker rm -f "$LEZ_STACK_PREFIX-basecamp-ui" >/dev/null 2>&1 || true

if [[ "${1:-}" == "--wipe" ]]; then
  docker compose down -v --remove-orphans
  # start.sh uses market/ next to this file when runtime.env names no root.
  market_root="${LEZ_MARKET_ROOT:-$DEPLOY_ROOT/market}"
  stamp="$(date +%s)"
  for f in bootstrap/deployment.json market-bootstrap.env; do
    [[ ! -f "$market_root/$f" ]] || mv "$market_root/$f" "$market_root/$f.chain-$stamp.bak"
  done
  rm -rf runtime
  echo "stack down; runtime state and ${LEZ_VOLUME_PREFIX:-$LEZ_STACK_PREFIX}-* volumes wiped; the escrow deployment in $market_root retired (identities kept)"
else
  docker compose down --remove-orphans
  echo "stack down (state retained; use --wipe to remove)"
fi
