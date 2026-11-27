#!/usr/bin/env bash
# up-native.sh <maker|taker> <stack dir> <node home> <session dir> <lez_core.lgx> <lez_swap_node.lgx> <role.lgx>
# Brings ONE role up natively on this Mac against the stack's chains, the
# other role staying in its container:
#   1. the stack's funding wallet-rpc moved off 127.0.0.1:18083 (to 18183),
#      which the native role's own wallet-rpc takes, as its Node expects;
#   2. the role's container stopped (the native Node takes its identity);
#   3. its LEZ_NODE_HOME staged from the stack's runtime/ (stage-native-home.sh);
#   4. the official macOS monero-wallet-rpc (verify-monero-mac.sh) on
#      127.0.0.1:18083 with the role's login, against the stack's monerod;
#   5. its Logos host (one logosctl session) and the Node started from the
#      role's darwin-arm64 package (start-native.sh);
#   6. the Maker's Chat socket bridged across the Docker VM boundary
#      (chat-bridge.py + a socat forwarder container on the default bridge)
#      and the Delivery directory shared by a bind mount (the other role's
#      container recreated with *-with-native-*.yaml).
# Needs LOGOSCTL and MONERO_BIN (the directory of the verified macOS binaries).
set -euo pipefail
role="${1:?}"; STACK="${2:?}"; H="${3:?}"; SESSION="${4:?}"; shift 4
here="$(cd "$(dirname "$0")" && pwd)"
prefix="$(sed -n 's/^LEZ_STACK_PREFIX=//p' "$STACK/runtime/runtime.env" | head -1)"
image="$(cd "$STACK" && . ./release.env && echo "$LEZ_IMAGE_PREFIX-taker-node:$LEZ_IMAGE_TAG")"
other=maker; [[ $role == maker ]] && other=taker
log() { echo "[$(date -u +%H:%M:%S)Z up-native] $*"; }
export MIXED_NATIVE_HOME="$H"

log "funding wallet-rpc to 127.0.0.1:18183"
LEZ_XMR_FUNDING_WALLET_PORT=18183 "$here/stack-compose.sh" "$STACK" --profile monero up -d --no-deps --no-build --pull never xmr-funding-wallet
log "stopping $prefix-$role-node"
docker stop "$prefix-$role-node" >/dev/null
"$here/stage-native-home.sh" "$STACK" "$H" "$role"
log "monero-wallet-rpc (macOS) on 127.0.0.1:18083"
nohup "$MONERO_BIN/monero-wallet-rpc" --config-file "$H/xmr/wallet-rpc.conf" >"$H/xmr/log/wallet-rpc.stdout" 2>&1 </dev/null &
echo $! >"$H/xmr/wallet-rpc.pid"
for _ in $(seq 30); do lsof -nP -iTCP:18083 -sTCP:LISTEN >/dev/null 2>&1 && break; sleep 1; done
lsof -nP -iTCP:18083 -sTCP:LISTEN | grep -q monero-wa || { echo "wallet-rpc not listening" >&2; exit 1; }

if [[ $role == taker ]]; then
  # The Docker Maker writes its Delivery directory into this home; its Chat
  # socket reaches the native Taker at $H/run/maker/chat.sock.
  mkdir -p "$H/delivery" "$H/run/maker"; chmod 700 "$H/delivery" "$H/run" "$H/run/maker"
  log "recreating $prefix-maker-node with its Delivery directory in the native Taker's home"
  COMPOSE_EXTRA="-f $here/maker-with-native-taker.yaml" "$here/stack-compose.sh" "$STACK" --profile monero \
    up -d --no-deps --no-build --pull never --force-recreate maker-node
  until docker ps --format '{{.Names}} {{.Status}}' | grep -q "$prefix-maker-node.*(healthy)"; do sleep 3; done
  docker rm -f "$prefix-maker-chat-forwarder" >/dev/null 2>&1 || true
  docker run -d --name "$prefix-maker-chat-forwarder" --restart unless-stopped --user 4713:4713 \
    -p 127.0.0.1:18191:18191 -v "$prefix-maker-socket:/run/lez/maker" --entrypoint socat "$image" \
    TCP-LISTEN:18191,fork,reuseaddr UNIX-CONNECT:/run/lez/maker/chat.sock >/dev/null
  nohup python3 -I "$here/chat-bridge.py" unix-to-tcp "$H/run/maker/chat.sock" 18191 >"$H/chat-bridge.log" 2>&1 </dev/null &
  echo $! >"$H/chat-bridge.pid"
fi

log "Logos host (logosctl session) and the $role Node from its package"
export SESSION STACK LEZ_NODE_HOME="$H" ROLE="$role"
"$here/session.sh" daemon-start | sed "s#$HOME#~#g"
"$here/start-native.sh" "$@"

if [[ $role == maker ]]; then
  nohup python3 -I "$here/chat-bridge.py" tcp-to-unix 18190 "$H/run/maker/chat.sock" >"$H/chat-bridge.log" 2>&1 </dev/null &
  echo $! >"$H/chat-bridge.pid"
  docker rm -f "$prefix-maker-chat-forwarder" >/dev/null 2>&1 || true
  docker run -d --name "$prefix-maker-chat-forwarder" --restart unless-stopped --user 4713:4713 \
    -v "$prefix-maker-socket:/run/lez/maker" --entrypoint socat "$image" \
    UNIX-LISTEN:/run/lez/maker/chat.sock,fork,unlink-early,mode=600 TCP:host.docker.internal:18190 >/dev/null
  log "recreating $prefix-taker-node with the native Maker's Delivery directory"
  COMPOSE_EXTRA="-f $here/taker-with-native-maker.yaml" "$here/stack-compose.sh" "$STACK" --profile monero \
    up -d --no-deps --no-build --pull never --force-recreate taker-node
  until docker ps --format '{{.Names}} {{.Status}}' | grep -q "$prefix-taker-node.*(healthy)"; do sleep 3; done
fi
log "up: $role native, $other in $prefix-$other-node"
