#!/bin/bash
# env-both.sh — (NAME=value per line) the environment of the one Logos host
# that runs both roles from the catalog-installed packages (as in the earlier
# native smoke): the devnet's timing settings from DEPLOY's runtime.env, the
# chains on localhost, and per role the signer, the Node launcher
# (LEZ_<ROLE>_NODE_LAUNCHER=package), its log, hold file, socket and sidecar
# ports. SSL_CERT_FILE: the system roots plus the local catalog's CA.
set -euo pipefail
H=${LEZ_NODE_HOME:?}
grep -E '^LEZ_(V02_CHANNEL_ID|BTC_(REFUND|SECOND|BLOCK|MAKER_LOCK|EARLIER|LATER)|MAKER_ACTOR|TAKER_OBSERVER|NODE_FINALITY|LEZ_DISCOVERY)' "${DEPLOY:?}/runtime/runtime.env"
cat <<ENV
SSL_CERT_FILE=${CA_BUNDLE:?}
LEZ_NODE_HOME=$H
LEZ_WALLET_DIR=$H/wallet/shared
LEZ_WALLET_SEQUENCER_URL=http://127.0.0.1:3040
LEZ_SEQUENCER_URL=http://127.0.0.1:3040
LEZ_INDEXER_URL=http://127.0.0.1:8779
LEZ_BTC_CLAIM_DESTINATION=wallet
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
ENV
done
