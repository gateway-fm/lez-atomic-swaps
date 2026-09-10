#!/usr/bin/env bash
# down.sh — stop the whole stack, chains, Nodes and UI. --wipe also removes
# runtime state and this stack's named volumes (Bitcoin data, Node stores,
# sockets), so the next up.sh --fresh starts from nothing.
set -euo pipefail
DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DEPLOY_ROOT"

# compose interpolation needs the same variables the stack was started with;
# the volume prefix in particular selects which volumes --wipe removes.
if [[ -f runtime/runtime.env ]]; then set -a; source runtime/runtime.env; set +a; fi
export BTC_RPC_PASSWORD="${BTC_RPC_PASSWORD:-teardown}"
docker rm -f lez-basecamp-ui >/dev/null 2>&1 || true

if [[ "${1:-}" == "--wipe" ]]; then
  docker compose down -v --remove-orphans
  rm -rf runtime
  echo "stack down; runtime state and ${LEZ_VOLUME_PREFIX:-lez}-* volumes wiped"
else
  docker compose down --remove-orphans
  echo "stack down (state retained; use --wipe to remove)"
fi
