#!/usr/bin/env bash
# bootstrap-market.sh — deploy the escrow program and claim the wallet vaults
# on the running LEZ chain, then publish the bootstrap manifest the Nodes
# render into their role configuration. Idempotent: market-bootstrap.sh
# skips every step whose on-chain effect already exists.
#
#   scripts/bootstrap-market.sh            (run from deploy/, after the LEZ
#                                           services are healthy)
#
# Inputs, all resolved from the workspace next to this repository:
#   runtime/runtime.env        LEZ_MARKET_ROOT (wallet identities, bootstrap)
#   ../provision/data          escrow deployer, vault-claim tool, sidecar
#   lez-builder:local          the ephemeral builder image
set -euo pipefail
DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd "$DEPLOY_ROOT/.." && pwd)"
WORKSPACE="${LEZ_WORKSPACE:-$(cd "$REPO_ROOT/.." && pwd)}"
PROVISION="${LEZ_PROVISION:-$WORKSPACE/provision/data}"
cd "$DEPLOY_ROOT"
# The stack's Docker network (scripts/lane.sh).
# shellcheck source=lane.sh
source scripts/lane.sh

market_root="$(sed -n 's/^LEZ_MARKET_ROOT=//p' runtime/runtime.env | head -1)"
# The genesis allocations gen-config.sh wrote; the claims must match them exactly.
wallet_maker_allocation="$(sed -n 's/^LEZ_WALLET_MAKER_ALLOCATION=//p' runtime/runtime.env | head -1)"
wallet_taker_allocation="$(sed -n 's/^LEZ_WALLET_TAKER_ALLOCATION=//p' runtime/runtime.env | head -1)"
[[ "$wallet_maker_allocation" =~ ^[0-9]+$ && "$wallet_taker_allocation" =~ ^[0-9]+$ ]] || { echo "runtime/runtime.env carries no LEZ_WALLET_{MAKER,TAKER}_ALLOCATION; run scripts/gen-config.sh runtime again" >&2; exit 65; }
[[ -d "$market_root/identities" ]] || { echo "LEZ_MARKET_ROOT in runtime/runtime.env must hold the wallet identities" >&2; exit 65; }
if [[ "${LEZ_IMAGES:-build}" == pull || "${LEZ_IMAGES:-build}" == present ]]; then
  # A release bundle (scripts/lane.sh read its release.env): no repository and
  # no provision directory. The deployer is the one in the tools image (as
  # start.sh's bootstrap uses it), the program the one release.env names.
  escrow_pin="${LEZ_ESCROW_PROGRAM_ID:-}"
  deployer=/usr/local/bin/lez-zec-escrow-v02-deployer
  provision_mount=()
else
  [[ -x "$PROVISION/escrow-artifact/debug/lez-zec-escrow-v02-deployer" ]] || { echo "escrow deployer missing under $PROVISION; run from-scratch.sh --only build" >&2; exit 65; }
  escrow_pin="$(sed -n 's/^ *expected_image_id="\([0-9a-f]\{64\}\)".*/\1/p' "$REPO_ROOT/scripts/verify-lez-v02-provisional.sh" | head -1)"
  deployer=/provision/escrow-artifact/debug/lez-zec-escrow-v02-deployer
  provision_mount=(-v "$PROVISION:/provision:ro")
fi
[[ "$escrow_pin" =~ ^[0-9a-f]{64}$ ]] || { echo "cannot read the escrow pin" >&2; exit 65; }

# The bootstrap runs in the LEZ wallet module's image: the vault claims go
# through lez_core (lez-wallet-vault-claim, the proof-of-concept tool's flags
# and evidence fields); the escrow deployment is a program deployment, not a
# wallet operation, and keeps its deployer.
docker image inspect "${LEZ_IMAGE_PREFIX:-lez}-wallet-module:${LEZ_IMAGE_TAG:-local}" >/dev/null 2>&1 ||
  docker compose build heartbeat >/dev/null
docker run --rm --network "$LEZ_STACK_NETWORK" --user "$(id -u):$(id -g)" \
  ${provision_mount[@]+"${provision_mount[@]}"} -v "$market_root:/market" -v "$DEPLOY_ROOT/scripts:/scripts:ro" \
  -e MARKET_ROOT=/market -e ESCROW_PROGRAM_ID="$escrow_pin" \
  -e DEPLOYER="$deployer" \
  -e VAULT_CLAIM_BIN=/usr/local/bin/lez-wallet-vault-claim \
  -e LEZ_BOOTSTRAP_DEFER_CLAIM="${LEZ_BOOTSTRAP_DEFER_CLAIM:-}" \
  -e LEZ_WALLET_MAKER_ALLOCATION="$wallet_maker_allocation" -e LEZ_WALLET_TAKER_ALLOCATION="$wallet_taker_allocation" \
  -e LEZ_HEARTBEAT_ALLOCATION="$(sed -n 's/^LEZ_HEARTBEAT_ALLOCATION=//p' runtime/runtime.env | head -1)" \
  --entrypoint bash "${LEZ_IMAGE_PREFIX:-lez}-wallet-module:${LEZ_IMAGE_TAG:-local}" -c 'socat TCP-LISTEN:3040,bind=127.0.0.1,fork,reuseaddr TCP:sequencer:3040 &
    socat TCP-LISTEN:8779,bind=127.0.0.1,fork,reuseaddr TCP:indexer:8779 &
    sleep 1; bash /scripts/market-bootstrap.sh'

# The Nodes read the manifest once at start; up.sh recreates them when it changed.
# Written in place, and only when it differs: Compose mounts this one file into
# the Node containers, and replacing it (a new inode, as `install` makes) takes
# it away from a running container on Docker Desktop for Mac. The Node then
# exits 2 on its next start, reading /run-market/market-bootstrap.env.
if ! cmp -s "$market_root/market-bootstrap.env" runtime/market-bootstrap.env; then
  cat "$market_root/market-bootstrap.env" > runtime/market-bootstrap.env
  chmod 0644 runtime/market-bootstrap.env
fi
