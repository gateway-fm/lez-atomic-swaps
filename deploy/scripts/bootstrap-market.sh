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

market_root="$(sed -n 's/^LEZ_MARKET_ROOT=//p' runtime/runtime.env | head -1)"
[[ -d "$market_root/identities" ]] || { echo "LEZ_MARKET_ROOT in runtime/runtime.env must hold the wallet identities" >&2; exit 65; }
[[ -x "$PROVISION/escrow-artifact/debug/lez-zec-escrow-v02-deployer" ]] || { echo "escrow deployer missing under $PROVISION; run from-scratch.sh --only build" >&2; exit 65; }
escrow_pin="$(sed -n 's/^ *expected_image_id="\([0-9a-f]\{64\}\)".*/\1/p' "$REPO_ROOT/scripts/verify-lez-v02-provisional.sh" | head -1)"
[[ "$escrow_pin" =~ ^[0-9a-f]{64}$ ]] || { echo "cannot read the escrow pin" >&2; exit 65; }

docker run --rm --network lez-swap-chains --user "$(id -u):$(id -g)" \
  -v "$PROVISION:/provision:ro" -v "$market_root:/market" -v "$DEPLOY_ROOT/scripts:/scripts:ro" \
  -e MARKET_ROOT=/market -e ESCROW_PROGRAM_ID="$escrow_pin" \
  -e DEPLOYER=/provision/escrow-artifact/debug/lez-zec-escrow-v02-deployer \
  -e VAULT_CLAIM_BIN=/provision/sidecar/lez-v02-vault-claim-poc \
  lez-builder:local bash -c 'socat TCP-LISTEN:3040,bind=127.0.0.1,fork,reuseaddr TCP:sequencer:3040 &
    socat TCP-LISTEN:8779,bind=127.0.0.1,fork,reuseaddr TCP:indexer:8779 &
    sleep 1; bash /scripts/market-bootstrap.sh'

# The Nodes read the manifest once at start; up.sh recreates them when it changed.
install -m 0644 "$market_root/market-bootstrap.env" runtime/market-bootstrap.env
