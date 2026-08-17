#!/usr/bin/env bash
# down.sh — stop the stack. --wipe also removes runtime state and volumes.
set -euo pipefail
DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DEPLOY_ROOT"

# compose interpolation requires the variable; a placeholder suffices for teardown
export BTC_RPC_PASSWORD="${BTC_RPC_PASSWORD:-teardown}"

if [[ "${1:-}" == "--wipe" ]]; then
  docker compose down -v --remove-orphans
  rm -rf runtime
  echo "stack down; state wiped"
else
  docker compose down --remove-orphans
  echo "stack down (state retained; use --wipe to remove)"
fi
