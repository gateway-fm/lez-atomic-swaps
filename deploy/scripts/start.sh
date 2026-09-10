#!/usr/bin/env bash
# start.sh — bring the prebuilt LEZ ↔ BTC swap stack up from a release bundle
# (or from a checkout) without building anything: pull the release images,
# mint the four wallet identities, generate the runtime, start the stack,
# bootstrap the settlement market and verify the Basecamp UI against both
# Nodes. Rerunning it is safe: every step skips what already exists.
#
#   ./scripts/start.sh           pull → identities → config → stack → bootstrap → UI suites
#   ./scripts/start.sh --quick   the same without the Basecamp UI suites
#   ./scripts/start.sh --swap    … then one full BTC → LEZ swap through the two apps
#
# The image names come from release.env next to compose.yaml (written into
# the bundle by scripts/package-dist.sh) or from the environment:
#   LEZ_IMAGE_PREFIX   registry path prefix, e.g. ghcr.io/gateway-fm/lez-atomic-swaps/lez
#   LEZ_IMAGE_TAG      release tag, e.g. v0.2.1
#   LEZ_ESCROW_PROGRAM_ID  the pinned escrow program the deployer in the tools
#                      image carries (the bundle records it; a checkout reads
#                      it from scripts/verify-lez-v02-provisional.sh)
# State lives next to this file: runtime/ (chains, configs, secrets) and
# market/ (wallet identities, the escrow deployment manifest). Keep both;
# ./scripts/down.sh --wipe removes runtime/ and the named volumes.
set -euo pipefail

DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DEPLOY_ROOT"

QUICK=0
RUN_SWAP=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --quick) QUICK=1; shift ;;
    --swap) RUN_SWAP=1; shift ;;
    -h|--help) sed -n 2,22p "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

log() { printf '\n[%s start] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
fail() { echo "start.sh failed: $*" >&2; exit 1; }

# ---- host --------------------------------------------------------------------
if [[ "$(uname -m)" != "arm64" && "$(uname -m)" != "aarch64" ]]; then
  fail "the prebuilt images are linux/arm64 (Apple silicon or an arm64 Linux host)"
fi
for tool in docker jq openssl xxd python3; do
  command -v "$tool" >/dev/null || fail "install $tool first"
done
docker info >/dev/null 2>&1 || fail "the Docker daemon is not reachable"
docker compose version >/dev/null 2>&1 || fail "docker compose plugin is required"

# ---- release pins ---------------------------------------------------------------
if [[ -f release.env ]]; then
  set -a
  # shellcheck source=/dev/null
  source release.env
  set +a
fi
[[ -n "${LEZ_IMAGE_PREFIX:-}" && -n "${LEZ_IMAGE_TAG:-}" ]] ||
  fail "LEZ_IMAGE_PREFIX and LEZ_IMAGE_TAG select the release images (release.env in a bundle)"
if [[ -z "${LEZ_ESCROW_PROGRAM_ID:-}" && -f ../scripts/verify-lez-v02-provisional.sh ]]; then
  LEZ_ESCROW_PROGRAM_ID="$(sed -n 's/^ *expected_image_id="\([0-9a-f]\{64\}\)".*/\1/p' ../scripts/verify-lez-v02-provisional.sh | head -1)"
fi
[[ "${LEZ_ESCROW_PROGRAM_ID:-}" =~ ^[0-9a-f]{64}$ ]] || fail "LEZ_ESCROW_PROGRAM_ID must name the pinned escrow program"
export LEZ_IMAGE_PREFIX LEZ_IMAGE_TAG LEZ_ESCROW_PROGRAM_ID
export LEZ_IMAGES=pull

# ---- market root: wallet identities + the escrow deployment manifest -------------
export LEZ_MARKET_ROOT="${LEZ_MARKET_ROOT:-$DEPLOY_ROOT/market}"
export LEZ_WALLET_IDENTITIES="$LEZ_MARKET_ROOT/identities"
mkdir -p "$LEZ_WALLET_IDENTITIES"
chmod 0700 "$LEZ_MARKET_ROOT" "$LEZ_WALLET_IDENTITIES"
LEZ_HOST_UID="$(id -u)"; LEZ_HOST_GID="$(id -g)"
export LEZ_HOST_UID LEZ_HOST_GID
# compose interpolation needs a password before gen-config.sh has minted one
export BTC_RPC_PASSWORD="${BTC_RPC_PASSWORD:-pending}"

# one throwaway container of the tools image on the stack network
tools() { docker compose --profile tools run --rm --no-deps --quiet-pull tools "$*"; }

log "pulling ${LEZ_IMAGE_PREFIX}-tools:${LEZ_IMAGE_TAG}"
docker compose --profile tools pull --quiet tools
for wallet in maker-munich-01 maker-basel-02 taker-zurich-01 taker-limmat-02; do
  [[ -f "$LEZ_WALLET_IDENTITIES/$wallet/identity.json" ]] && continue
  log "minting the $wallet identity"
  rm -rf "${LEZ_WALLET_IDENTITIES:?}/$wallet"
  tools "lez-v02-local-actor-identity --output-directory /market/identities/$wallet" >/dev/null
  cp "$LEZ_WALLET_IDENTITIES/$wallet/identity.json" "$LEZ_WALLET_IDENTITIES/$wallet.json"
done

# ---- stack -------------------------------------------------------------------------
log "runtime config, image pull, stack start"
SKIP_UI_VERIFY=1 LEZ_NO_BANNER=1 bash scripts/up.sh
set -a
# shellcheck source=/dev/null
source runtime/runtime.env
set +a
bash scripts/repair-indexer.sh

# ---- market bootstrap (escrow program, vault claims, bootstrap manifest) --------
# The deployer and the vault-claim tool accept only literal-loopback URLs, so
# the container forwards 127.0.0.1:3040/8779 to sequencer/indexer for the run.
log "market bootstrap (idempotent)"
tools 'socat TCP-LISTEN:3040,bind=127.0.0.1,fork,reuseaddr TCP:sequencer:3040 &
  socat TCP-LISTEN:8779,bind=127.0.0.1,fork,reuseaddr TCP:indexer:8779 &
  sleep 1; bash /scripts/market-bootstrap.sh' | tail -4
# The first start precedes the deployment; entrypoints read the manifest once,
# so both roles restart with its public fields in the runtime.
cat "$LEZ_MARKET_ROOT/market-bootstrap.env" > runtime/market-bootstrap.env
chmod 0644 runtime/market-bootstrap.env
docker compose up -d --no-deps --force-recreate --wait --wait-timeout 180 maker-node taker-node

# ---- verification --------------------------------------------------------------------
if [[ "$QUICK" != 1 ]]; then
  log "Basecamp suites against both Nodes (the Maker suite also seeds the order book)"
  for role in maker taker; do
    docker exec lez-basecamp-ui node /ui-tests/verify.mjs "$role" 2>&1 | grep -E '✓|✗|passed' || fail "$role UI suite failed"
  done
  bash scripts/verify-all.sh 2>&1 | grep -E 'OK|FAIL|checks|failed' || fail "verify-all.sh reported a failed stage"
fi
if [[ "$RUN_SWAP" == 1 ]]; then
  log "one full BTC → LEZ swap through the two Basecamp apps"
  bash scripts/swap-through-ui.sh
fi

vnc_password="${VNC_PASSWORD:-lezswap}"
cat <<BANNER

──────────────────────────────────────────────────────────────────
 LEZ swap stack is up from ${LEZ_IMAGE_PREFIX}-*:${LEZ_IMAGE_TAG}

   Basecamp UI (VNC)     vnc://127.0.0.1:5901     (password: ${vnc_password})
   Bitcoin regtest RPC   http://127.0.0.1:18443   (auto-mining)
   BTC explorer          http://127.0.0.1:3002
   LEZ explorer + proof  http://127.0.0.1:3003/#/evidence
   swap                  ./scripts/swap-through-ui.sh   (or click through the two desks)
   verify                ./scripts/verify-all.sh
   logs                  docker compose --env-file runtime/runtime.env logs -f <service>
   stop                  ./scripts/down.sh            (--wipe removes chains and Node state)

 wallet identities and the escrow deployment live in ${LEZ_MARKET_ROOT}
──────────────────────────────────────────────────────────────────
BANNER
