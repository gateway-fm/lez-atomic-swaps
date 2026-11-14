#!/bin/bash
# env-linux.sh — NAME=value lines: the environment of ONE native Logos host (one Basecamp
# with both desks) on Linux x86_64 outside any container. Same settings as the macOS run
# (env-both.sh), plus every path the launcher defaults to a container mount on Linux
# (state, run root, identity, ZEC corridor and sidecar dirs, manifest, cookie, Delivery)
# and the chain hosts as 127.0.0.1 (the chains' RPC ports arrive on loopback through an
# SSH reverse tunnel, so the launcher starts no socat forward).
# Inputs: LEZ_NODE_HOME (identity/<role>/, market-bootstrap.env, btc-rpc-cookie) and
# $LEZ_NODE_HOME/timing.env (the devnet's timing and channel lines from runtime.env).
set -euo pipefail
H=${LEZ_NODE_HOME:?}
cat "$H/timing.env"
cat <<ENV
LEZ_NODE_HOME=$H
LEZ_WALLET_DIR=$H/wallet/shared
LEZ_WALLET_SEQUENCER_URL=http://127.0.0.1:3040
LEZ_SEQUENCER_URL=http://127.0.0.1:3040
LEZ_INDEXER_URL=http://127.0.0.1:8779
LEZ_BTC_CLAIM_DESTINATION=wallet
LEZ_NODE_RUN_ROOT=$H/run
LEZ_ZEC_STATE_DIR=$H/zec
LEZ_MARKET_MANIFEST=$H/market-bootstrap.env
LEZ_BTC_RPC_COOKIE_FILE=$H/btc-rpc-cookie
LEZ_DELIVERY_DIR=$H/delivery
LEZ_BITCOIN_CORE_HOST=127.0.0.1
LEZ_SEQUENCER_HOST=127.0.0.1
LEZ_INDEXER_HOST=127.0.0.1
LEZ_ZEBRA_HOST=127.0.0.1
ENV
for role in maker taker; do
  ROLE=$(tr '[:lower:]' '[:upper:]' <<<"$role")
  port=19000; [[ $role == taker ]] && port=19500
  cat <<ENV
LEZ_${ROLE}_SIGNER_SOCKET=$H/wallet/$role.sock
LEZ_${ROLE}_SIGNER_IDENTITY=$H/identity/$role
LEZ_${ROLE}_NODE_LAUNCHER=package
LEZ_${ROLE}_NODE_LOG=$H/$role.out.log
LEZ_${ROLE}_NODE_HOLD=$H/$role.hold
LEZ_${ROLE}_RPC_SOCKET=$H/run/$role/node.sock
LEZ_${ROLE}_WALLET_SIGNER_SOCKET=$H/wallet/$role.sock
LEZ_${ROLE}_SIDECAR_PORT_BASE=$port
LEZ_${ROLE}_BTC_WALLET=lez-$role
LEZ_${ROLE}_NODE_LOG_FILE=$H/$role.node.log
LEZ_${ROLE}_NODE_STATE_DIR=$H/$role
LEZ_${ROLE}_NODE_IDENTITY_DIR=$H/identity/$role
LEZ_${ROLE}_ZEC_SIDECAR_DIR=$H/zec/sidecar-$role
ENV
done
