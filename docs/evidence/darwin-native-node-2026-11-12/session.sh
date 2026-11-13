#!/bin/bash
# session.sh <maker|taker> <logosctl args...> — one logosctl session per role
# (each role's environment, as each role's Basecamp container has its own).
# `session.sh maker daemon-start` starts the daemon with the role's environment.
set -euo pipefail
role="$1"; shift
L=$LOGOSCTL/bin/logosctl
S=$SESSIONS/$role
H=$LEZ_NODE_HOME
if [[ "${1:-}" == daemon-start ]]; then
  mkdir -p "$S" "$H/wallet/$role"
  set -a
  # shellcheck disable=SC1091
  eval "$(grep -E '^LEZ_(V02_CHANNEL_ID|BTC_|MAKER_ACTOR|TAKER_OBSERVER|NODE_FINALITY|LEZ_DISCOVERY)' $DEPLOY/runtime/runtime.env)"
  set +a
  ROLE=$(tr '[:lower:]' '[:upper:]' <<<"$role")
  port_base=19000; [[ $role == taker ]] && port_base=19500
  # A Finder-started app's PATH: /usr/bin/env bash is macOS's /bin/bash 3.2.
  exec env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin:/usr/sbin:/sbin \
    LEZ_NODE_HOME="$H" \
    LEZ_WALLET_DIR="$H/wallet/$role/store" LEZ_WALLET_SEQUENCER_URL=http://127.0.0.1:3040 \
    "LEZ_${ROLE}_SIGNER_SOCKET=$H/wallet/$role/signer.sock" "LEZ_${ROLE}_SIGNER_IDENTITY=$H/identity/$role" \
    "LEZ_${ROLE}_NODE_LAUNCHER=package" "LEZ_${ROLE}_NODE_LOG=$H/$role.out.log" "LEZ_${ROLE}_NODE_HOLD=$H/$role.hold" \
    LEZ_WALLET_SIGNER_SOCKET="$H/wallet/$role/signer.sock" \
    LEZ_SIDECAR_PORT_BASE="$port_base" LEZ_SEQUENCER_URL=http://127.0.0.1:3040 LEZ_INDEXER_URL=http://127.0.0.1:8779 \
    LEZ_V02_CHANNEL_ID="$LEZ_V02_CHANNEL_ID" LEZ_BTC_WALLET="lez-$role" LEZ_BTC_CLAIM_DESTINATION=wallet \
    LEZ_BTC_REFUND_CSV_BLOCKS="$LEZ_BTC_REFUND_CSV_BLOCKS" LEZ_BTC_SECOND_LOCK_REFUND_CSV_BLOCKS="$LEZ_BTC_SECOND_LOCK_REFUND_CSV_BLOCKS" \
    LEZ_BTC_BLOCK_SECONDS_FASTEST="$LEZ_BTC_BLOCK_SECONDS_FASTEST" LEZ_BTC_BLOCK_SECONDS_SLOWEST="$LEZ_BTC_BLOCK_SECONDS_SLOWEST" \
    LEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS="$LEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS" LEZ_BTC_EARLIER_REFUND_SECONDS="$LEZ_BTC_EARLIER_REFUND_SECONDS" \
    LEZ_BTC_LATER_REFUND_SECONDS="$LEZ_BTC_LATER_REFUND_SECONDS" LEZ_BTC_REFUND_MARGIN_SECONDS="$LEZ_BTC_REFUND_MARGIN_SECONDS" \
    LEZ_LEZ_DISCOVERY_MAX_BLOCKS="$LEZ_LEZ_DISCOVERY_MAX_BLOCKS" \
    LEZ_MAKER_ACTOR_REQUEUE_SECONDS="$LEZ_MAKER_ACTOR_REQUEUE_SECONDS" LEZ_MAKER_ACTOR_FAILURE_BACKOFF_SECONDS="$LEZ_MAKER_ACTOR_FAILURE_BACKOFF_SECONDS" \
    LEZ_TAKER_OBSERVER_SWEEP_SECONDS="$LEZ_TAKER_OBSERVER_SWEEP_SECONDS" LEZ_NODE_FINALITY_STALL_SECONDS="$LEZ_NODE_FINALITY_STALL_SECONDS" \
    LEZ_NODE_LOG_FILE="$H/$role.node.log" \
    "$L" --config-dir "$S" -D -d
fi
exec "$L" --config-dir "$S" "$@"
