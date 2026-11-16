#!/bin/bash
# env-both.sh — prints (one per line, NAME=value) the environment of ONE Logos
# host that runs both roles: the desks' signer/Node settings (role-specific
# by design) and the launcher's settings, role-specific where they differ.
set -euo pipefail
H=$LEZ_NODE_HOME
R=$DEPLOY/runtime/runtime.env
grep -E '^LEZ_(V02_CHANNEL_ID|BTC_(REFUND|SECOND|BLOCK|MAKER_LOCK|EARLIER|LATER)|MAKER_ACTOR|TAKER_OBSERVER|NODE_FINALITY|LEZ_DISCOVERY)' "$R"
cat <<EOF
LEZ_NODE_HOME=$H
LEZ_WALLET_DIR=$H/wallet/shared
LEZ_WALLET_SEQUENCER_URL=http://127.0.0.1:3040
LEZ_SEQUENCER_URL=http://127.0.0.1:3040
LEZ_INDEXER_URL=http://127.0.0.1:8779
LEZ_BTC_CLAIM_DESTINATION=wallet
EOF
for role in maker taker; do
  ROLE=$(tr '[:lower:]' '[:upper:]' <<<"$role")
  port=19000; [[ $role == taker ]] && port=19500
  cat <<EOF
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
EOF
done
