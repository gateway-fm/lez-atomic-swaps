#!/usr/bin/env bash
# up.sh — bring the whole local LEZ ↔ BTC swap stack up: chains, market,
# both Nodes and the Basecamp UI. Idempotent on a standing stack.
#
#   ./scripts/up.sh               start or resume everything
#   ./scripts/up.sh --build       also rebuild the images from the staged payloads
#   ./scripts/up.sh --fresh-lez   recreate the LEZ chain only (Bedrock cannot
#                                 restart after long uptime); Bitcoin, wallets
#                                 and identities are kept, persisted swaps are
#                                 reset because they reference the old chain
#   ./scripts/up.sh --fresh       wipe runtime state and volumes first, then start
#                                 under a new volume prefix (LEZ_VOLUME_PREFIX to pick one)
#
#   LEZ_IMAGES=pull          pull the images named by LEZ_IMAGE_PREFIX and
#                            LEZ_IMAGE_TAG instead of building them (the
#                            prebuilt-release path; scripts/start.sh sets it,
#                            bootstraps the market itself afterwards and
#                            prints its own banner: LEZ_NO_BANNER=1)
#   LEZ_API_ONLY=1           chains, market and Nodes only, wallets seeded
#                            for the evidence run; no Basecamp UI
#
# Requires the payloads staged by from-scratch.sh (images/*/ contexts, the
# provision directory, the market root). SKIP_UI_VERIFY=1 skips the UI suites.
set -euo pipefail
DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DEPLOY_ROOT"

BUILD=0; FRESH_LEZ=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --build) BUILD=1; shift ;;
    --fresh-lez) FRESH_LEZ=1; shift ;;
    --fresh)
      # A wipe recreates both chains, so the previous chain's escrow deployment
      # in the market root is retired too, as --fresh-lez does; the market
      # bootstrap then deploys again on the new chain.
      if [[ -f runtime/runtime.env ]]; then
        market_root="$(sed -n 's/^LEZ_MARKET_ROOT=//p' runtime/runtime.env | head -1)"
        if [[ -n "$market_root" && -f "$market_root/bootstrap/deployment.json" ]]; then
          mv "$market_root/bootstrap/deployment.json" "$market_root/bootstrap/deployment.json.chain-$(date +%s).bak"
        fi
      fi
      echo "wiping runtime state and volumes…"
      bash scripts/down.sh --wipe >/dev/null 2>&1 || true
      # A fresh stack gets its own volume namespace, so it can never adopt
      # volumes another run or another checkout left behind under the default
      # prefix (a wipe under the old prefix does not touch those). gen-config
      # records it in runtime.env; the next down.sh --wipe removes exactly it.
      export LEZ_VOLUME_PREFIX="${LEZ_VOLUME_PREFIX:-lez-$(date +%Y%m%d%H%M%S)}"
      shift ;;
    -h|--help) sed -n 2,15p "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

for tool in docker jq openssl xxd; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 1; }
done
mkdir -p runtime
load_env() { set -a; source runtime/runtime.env; set +a; export BTC_RPC_PASSWORD; }

wait_healthy() { # wait_healthy <seconds> <container>...
  local timeout="$1"; shift
  local elapsed=0 c
  for c in "$@"; do
    until [[ "$(docker inspect -f '{{.State.Health.Status}}' "$c" 2>/dev/null)" == healthy ]]; do
      sleep 3; elapsed=$((elapsed + 3))
      [[ $elapsed -lt $timeout ]] || { echo "$c never became healthy" >&2; docker logs --tail 30 "$c" 2>&1 | tail -30; exit 1; }
    done
  done
}

if [[ "$FRESH_LEZ" == 1 && -f runtime/runtime.env ]]; then
  load_env
  stamp="$(date +%s)"
  echo "[lez] recreating the LEZ chain; old state → runtime/lez-broken-$stamp"
  docker compose stop maker-node taker-node lez-explorer indexer sequencer bedrock >/dev/null 2>&1 || true
  mkdir -p "runtime/lez-broken-$stamp"
  for d in bedrock sequencer indexer; do [[ -d "runtime/$d" ]] && mv "runtime/$d" "runtime/lez-broken-$stamp/"; done
  sed -i.bak '/^LEZ_V02_GENESIS_TIME_EPOCH=/d' runtime/runtime.env && rm -f runtime/runtime.env.bak
  market_root="$(sed -n 's/^LEZ_MARKET_ROOT=//p' runtime/runtime.env | head -1)"
  [[ -f "$market_root/bootstrap/deployment.json" ]] && mv "$market_root/bootstrap/deployment.json" "$market_root/bootstrap/deployment.json.chain-$stamp.bak"
fi

echo "[1/6] generating runtime config…"
env_before="$(sha256sum runtime/runtime.env 2>/dev/null | cut -c1-64 || true)"
bash scripts/gen-config.sh runtime
env_after="$(sha256sum runtime/runtime.env 2>/dev/null | cut -c1-64 || true)"
load_env

echo "[2/6] images…"
case "${LEZ_IMAGES:-build}" in
  build)
    missing=0
    for image in $(docker compose config --images 2>/dev/null | sort -u); do
      docker image inspect "$image" >/dev/null 2>&1 || missing=1
    done
    if [[ "$BUILD" == 1 || "$missing" == 1 ]]; then
      # BuildKit resolves every FROM against its registry with a short
      # deadline; pull the base images first, with retries, so a slow registry
      # cannot fail the build of payloads that are already staged.
      for image in $(grep -h '^FROM' images/*/Dockerfile | awk '{print $2}' | sort -u); do
        docker image inspect "$image" >/dev/null 2>&1 && continue
        echo "  pulling $image"
        for _ in 1 2 3; do docker pull -q "$image" >/dev/null && break; sleep 10; done
        docker image inspect "$image" >/dev/null 2>&1 || { echo "cannot pull $image" >&2; exit 1; }
      done
      for attempt in 1 2 3; do
        docker compose build && break
        [[ "$attempt" -lt 3 ]] || { echo "docker compose build failed three times" >&2; exit 1; }
        echo "  image build failed (registry deadline?); retrying in 30 s"
        sleep 30
      done
    else
      echo "  all images present (use --build to rebuild)"
    fi
    ;;
  pull)
    echo "  pulling ${LEZ_IMAGE_PREFIX:-lez}-*:${LEZ_IMAGE_TAG:-local}…"
    docker compose --profile tools pull --quiet
    ;;
  *) echo "LEZ_IMAGES must be build or pull" >&2; exit 64 ;;
esac

echo "[3/6] starting chains…"
docker compose up -d bitcoin-core btc-miner btc-explorer bedrock sequencer indexer lez-explorer
wait_healthy 240 lez-bitcoin-core lez-sequencer lez-indexer
bash scripts/repair-indexer.sh >/dev/null 2>&1 || true
# The Nodes' Core wallets (lez-maker, lez-taker) and the Taker's regtest
# balance; idempotent, and after a wipe nothing else creates them.
python3 scripts/seed-btc-wallets.py

before="$(sha256sum runtime/market-bootstrap.env 2>/dev/null | cut -c1-64 || true)"
if [[ "${LEZ_IMAGES:-build}" == pull ]]; then
  echo "[4/6] market: bootstrapped by start.sh from the tools image after this"
else
  echo "[4/6] market (escrow program, vault claims, bootstrap manifest)…"
  bash scripts/bootstrap-market.sh 2>&1 | tail -3
fi
after="$(sha256sum runtime/market-bootstrap.env 2>/dev/null | cut -c1-64 || true)"

echo "[5/6] Nodes…"
# A changed runtime.env (a timing profile switch, say) reaches the Nodes only
# through a recreate; entrypoints read it once.
if [[ "$before" != "$after" || "$env_before" != "$env_after" || "$FRESH_LEZ" == 1 ]]; then
  docker compose up -d --force-recreate maker-node taker-node
else
  docker compose up -d maker-node taker-node
fi
wait_healthy 240 lez-maker-node lez-taker-node
if [[ "$FRESH_LEZ" == 1 ]]; then
  echo "  forgetting swaps that referenced the old chain"
  bash scripts/reset-swaps.sh 2>&1 | tail -1
fi
[[ "${LEZ_IMAGES:-build}" == pull ]] || python3 scripts/verify-market.py 2>&1 | tail -1

if [[ "${LEZ_API_ONLY:-0}" == 1 ]]; then
  echo "API stack ready; run python3 scripts/record-evidence.py after market bootstrap."
  exit 0
fi
echo "[6/6] Basecamp UI…"
docker compose up -d basecamp-ui
wait_healthy 120 lez-basecamp-ui
ui_failed=0
if [[ "${SKIP_UI_VERIFY:-0}" != "1" ]]; then
  for role in maker taker; do
    if ! docker exec lez-basecamp-ui node /ui-tests/verify.mjs "$role" 2>&1 | grep -E '✓|✗|passed|failed'; then
      ui_failed=1
    fi
  done
fi

# start.sh prints its own banner once the market is bootstrapped.
[[ "${LEZ_NO_BANNER:-0}" != 1 ]] || exit "$ui_failed"
cat <<BANNER

──────────────────────────────────────────────────────────────────
 LEZ swap stack is up (volumes: ${LEZ_VOLUME_PREFIX:-lez}-*)

   Bitcoin regtest RPC   http://127.0.0.1:18443   (auto-mining, 1 block / 120 s)
   BTC explorer          http://127.0.0.1:3002
   LEZ explorer          http://127.0.0.1:3003
   Basecamp UI (VNC)     vnc://127.0.0.1:5901     (password: ${VNC_PASSWORD:-lezswap}; role: ${BASECAMP_ROLE:-both})
   Maker Node            docker exec lez-maker-node lez-maker-cli --socket /run/lez/maker/node.sock health

 logs:  docker compose logs -f <service>      down:  ./scripts/down.sh [--wipe]
 a swap looks stuck?  ./scripts/verify-all.sh, then ./scripts/up.sh --fresh-lez if Bedrock's slot is frozen
──────────────────────────────────────────────────────────────────
BANNER
[[ "$ui_failed" == 0 ]] || { echo "a Basecamp UI suite failed; see above" >&2; exit 1; }
