#!/bin/bash
# Healthy when the LEZ network node's HTTP API accepts a connection on
# ${1:-18080} and the indexer module's JSON-RPC on ${2:-8779} answers
# getLastFinalizedBlockId with a result (null until the zone has a block).
exec 3<>"/dev/tcp/127.0.0.1/${1:-18080}" || exit 1
exec 3>&-
exec 4<>"/dev/tcp/127.0.0.1/${2:-8779}" || exit 1
body='{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}'
printf 'POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s' "${#body}" "$body" >&4
timeout 4 cat <&4 | grep -q '"result"'
