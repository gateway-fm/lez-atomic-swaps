#!/usr/bin/env bash
# node-hold.sh — hold a role's Node down inside its running Basecamp, or let it
# run again. Each Node runs in its party's Basecamp, supervised by
# lez_swap_node, which does not start it again while the role's hold file
# ($LEZ_<ROLE>_NODE_HOLD, compose.yaml) exists. Holding writes that file, then
# ends the Node's process group (the Node, its loopback forwarders and
# sidecars); Basecamp, the role's desk and its LEZ signer keep running, so a
# desk can be watched with its Node gone. Releasing removes the file and the
# supervisor starts the Node again.
#
#   scripts/node-hold.sh hold <maker|taker>...
#   scripts/node-hold.sh release <maker|taker>...
#
# LEZ_CONTAINER_PREFIX selects the pair's containers (default: lane 1 of the
# stack the deploy directory runs, scripts/lane.sh; lez in the default stack).
set -euo pipefail
action="${1:-}"; shift || true
# shellcheck source=lane.sh
source "$(dirname "${BASH_SOURCE[0]}")/lane.sh"
prefix="$LEZ_CONTAINER_PREFIX"
[[ "$action" == hold || "$action" == release ]] && [[ $# -gt 0 ]] ||
  { echo "usage: node-hold.sh hold|release <maker|taker>..." >&2; exit 64; }
for role in "$@"; do
  case "$role" in maker) var=LEZ_MAKER_NODE_HOLD ;; taker) var=LEZ_TAKER_NODE_HOLD ;;
    *) echo "role must be maker or taker: $role" >&2; exit 64 ;; esac
  if [[ "$action" == release ]]; then
    docker exec "$prefix-$role-node" sh -c 'rm -f "$'"$var"'"'
    continue
  fi
  docker exec "$prefix-$role-node" sh -c '
    touch "$'"$var"'"
    group="$(pgrep -o -x "lez-'"$role"'-node" || pgrep -o -f node-entrypoint.sh || true)"
    [ -n "$group" ] || exit 0
    pkill -TERM -g "$group"
    for _ in $(seq 75); do pgrep -g "$group" >/dev/null || exit 0; sleep 0.2; done
    pkill -KILL -g "$group"; sleep 1; ! pgrep -g "$group" >/dev/null'
done
