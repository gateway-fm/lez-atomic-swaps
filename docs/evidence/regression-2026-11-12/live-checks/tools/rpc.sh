#!/usr/bin/env bash
# rpc.sh <maker|taker> <method> <params-json>: one owner-RPC call on lane 1.
role="$1"; method="$2"; params="${3:-{\}}"
docker exec "lez-$role-node" curl -sS --max-time 60 --unix-socket "/run/lez/$role/node.sock" \
  -H 'content-type: application/json' --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":[$params]}" http://localhost/ </dev/null
