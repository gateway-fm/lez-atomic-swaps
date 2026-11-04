#!/bin/bash
# Healthy when the module's JSON-RPC answers getLastFinalizedBlockId with a result.
exec 3<>"/dev/tcp/127.0.0.1/${1:-8779}" || exit 1
body='{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}'
printf 'POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s' "${#body}" "$body" >&3
timeout 4 cat <&3 | grep -q '"result"'
