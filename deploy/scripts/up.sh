#!/usr/bin/env bash
# up.sh — one-shot bring-up of the full local LEZ swap stack.
#   ./scripts/up.sh          build images + generate config + start everything
#   ./scripts/up.sh --fresh  wipe runtime/ and named volumes first
set -euo pipefail

DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DEPLOY_ROOT"

if [[ "${1:-}" == "--fresh" ]]; then
  echo "wiping runtime state and volumes…"
  docker compose down -v --remove-orphans >/dev/null 2>&1 || true
  rm -rf runtime
fi

mkdir -p runtime

for tool in docker jq openssl xxd; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 1; }
done

echo "[1/4] generating runtime config…"
bash scripts/gen-config.sh runtime
set -a; source runtime/runtime.env; set +a
export BTC_RPC_PASSWORD

echo "[2/4] building images…"
docker compose build

echo "[3/4] starting stack…"
docker compose up -d

echo "[4/4] waiting for chains…"
timeout=240
elapsed=0
until docker exec lez-bitcoin-core /usr/local/bin/bitcoin-cli -conf=/run-config/bitcoin.conf -datadir=/var/lib/bitcoin getblockcount >/dev/null 2>&1; do
  sleep 3; elapsed=$((elapsed + 3))
  [[ $elapsed -lt $timeout ]] || { echo "bitcoin-core RPC never became ready"; docker compose logs --tail 30 bitcoin-core; exit 1; }
done
until docker exec lez-maker-node lez-maker-cli --socket /run/lez/maker/node.sock health >/dev/null 2>&1; do
  sleep 3; elapsed=$((elapsed + 3))
  [[ $elapsed -lt $timeout ]] || { echo "Maker Node never became ready"; docker compose logs --tail 30 maker-node; exit 1; }
done
until docker exec lez-taker-node curl -sf --max-time 3 --unix-socket /run/lez/taker/node.sock \
    --header 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"taker_health","params":[{}]}' \
    http://localhost/ >/dev/null 2>&1; do
  sleep 3; elapsed=$((elapsed + 3))
  [[ $elapsed -lt $timeout ]] || { echo "Taker Node never became ready"; docker compose logs --tail 30 taker-node; exit 1; }
done

if [[ "${SKIP_UI_VERIFY:-0}" != "1" ]]; then
  echo "[5/5] verifying Basecamp UI against both real Nodes…"
  set -a; source runtime/runtime.env; set +a
  docker compose run --rm --no-deps --entrypoint node basecamp-ui /ui-tests/verify.mjs maker \
    | grep -viE "locale|Qt depends|reconfigure|manual"
  docker compose run --rm --no-deps --entrypoint node basecamp-ui /ui-tests/verify.mjs taker \
    | grep -viE "locale|Qt depends|reconfigure|manual"
fi

role_now="${BASECAMP_ROLE:-both}"
vnc_password="${VNC_PASSWORD:-lezswap}"
cat <<BANNER

──────────────────────────────────────────────────────────────────
 LEZ swap stack is up (all dockerized, all native arm64)

   Bitcoin regtest RPC   http://127.0.0.1:18443   (auto-mining)
   BTC explorer          http://127.0.0.1:3002
   LEZ + M3 proof UI     http://127.0.0.1:3003/#/evidence
   Basecamp UI (VNC)     vnc://127.0.0.1:5901     (password: ${vnc_password}; role: ${role_now})
   BTC swap flow         Maker publishes → Taker takes → four role-owned actions
   Maker Node            docker exec lez-maker-node lez-maker-cli --socket /run/lez/maker/node.sock health
   UI verification       docker compose run --rm --no-deps --entrypoint node basecamp-ui /ui-tests/verify.mjs [maker|taker]

 switch UI role:  BASECAMP_ROLE=taker docker compose up -d basecamp-ui
 logs:  docker compose logs -f <service>     down+wipe:  ./scripts/down.sh --wipe
──────────────────────────────────────────────────────────────────
BANNER
