#!/bin/bash
# env-native.sh <stack dir> <node home> <role> — prints (NAME=value per line)
# the environment of the Logos host (one logosctl session) that runs ONE
# native role Node on this Mac: what the role's Compose service gives its
# container (compose.yaml, <role>-node: environment), with the macOS layout
# under LEZ_NODE_HOME and the chains on this host's loopback, plus the desk's
# signer settings that `start-native.sh` uses. No secret is in it.
set -euo pipefail
S="${1:?stack dir}"; H="${2:?node home}"; role="${3:?maker|taker}"
ROLE=$(tr '[:lower:]' '[:upper:]' <<<"$role")
R="$S/runtime/runtime.env"
grep -E '^LEZ_(V02_CHANNEL_ID|BTC_(REFUND|SECOND|BLOCK_SECONDS_(FASTEST|SLOWEST)|MAKER_LOCK|EARLIER|LATER|ACTOR_TRACE)|MAKER_ACTOR|TAKER_OBSERVER|NODE_FINALITY|LEZ_DISCOVERY|XMR_)' "$R" | grep -v '=$' || true
network="$(sed -n 's/^LEZ_MONERO_NETWORK=//p' "$R" | head -1)"
port=19000; [[ $role == taker ]] && port=19500
cat <<EOF
LEZ_XMR_NETWORK=${network:-regtest}
LEZ_NODE_HOME=$H
LEZ_WALLET_DIR=$H/wallet/lez
LEZ_WALLET_SEQUENCER_URL=http://127.0.0.1:3040
LEZ_SEQUENCER_URL=http://127.0.0.1:3040
LEZ_INDEXER_URL=http://127.0.0.1:8779
LEZ_BTC_CLAIM_DESTINATION=wallet
LEZ_BTC_WALLET=lez-$role
LEZ_SIDECAR_PORT_BASE=$port
LEZ_ZEC_PROFILE=deterministic-local-devnet-v5
LEZ_ZEBRA_COOKIE_FILE=$H/zebra-cookie
LEZ_WALLET_SIGNER_SOCKET=$H/wallet/$role.sock
LEZ_NODE_LOG_FILE=$H/$role.node.log
LEZ_${ROLE}_SIGNER_SOCKET=$H/wallet/$role.sock
LEZ_${ROLE}_SIGNER_IDENTITY=$H/identity/$role
LEZ_${ROLE}_NODE_LAUNCHER=package
LEZ_${ROLE}_NODE_LOG=$H/$role.out.log
LEZ_${ROLE}_NODE_HOLD=$H/$role.hold
LEZ_${ROLE}_RPC_SOCKET=$H/run/$role/node.sock
EOF
