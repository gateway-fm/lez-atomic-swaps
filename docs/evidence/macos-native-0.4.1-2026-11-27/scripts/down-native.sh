#!/usr/bin/env bash
# down-native.sh <maker|taker> <stack dir> <node home> <session dir>
# Ends what up-native.sh started for the role (its Node held down by its
# hold file, its logosctl daemon, its wallet-rpc, the chat bridge and the
# forwarder container) and gives the stack back its own containers for both
# roles (the other role's container recreated without the override). The
# native home and session stay on disk.
set -euo pipefail
role="${1:?}"; STACK="${2:?}"; H="${3:?}"; SESSION="${4:?}"
here="$(cd "$(dirname "$0")" && pwd)"
prefix="$(sed -n 's/^LEZ_STACK_PREFIX=//p' "$STACK/runtime/runtime.env" | head -1)"
touch "$H/$role.hold"
export SESSION STACK LEZ_NODE_HOME="$H" ROLE="$role" LOGOSCTL
"$here/session.sh" daemon stop || true
for _ in $(seq 30); do pgrep -f -- "--socket $H/run/$role/node.sock" >/dev/null || break; sleep 1; done
pkill -f -- "--socket $H/run/$role/node.sock" 2>/dev/null || true
for pid in "$H/xmr/wallet-rpc.pid" "$H/chat-bridge.pid"; do [[ -f $pid ]] && kill "$(cat "$pid")" 2>/dev/null || true; done
docker rm -f "$prefix-maker-chat-forwarder" >/dev/null 2>&1 || true
other=maker; [[ $role == maker ]] && other=taker
"$here/stack-compose.sh" "$STACK" --profile monero up -d --no-deps --no-build --pull never --force-recreate "$other-node"
docker start "$prefix-$role-node" >/dev/null
echo "down: $role native ended; both roles in their containers"
