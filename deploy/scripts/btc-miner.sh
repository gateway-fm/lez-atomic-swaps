#!/usr/bin/env bash
# btc-miner.sh — mines one regtest block on a fixed cadence via JSON-RPC.
set -euo pipefail

: "${BTC_CONF:?BTC_CONF must point at bitcoin.conf}"

rpc_user="$(grep -E '^rpcuser=' "$BTC_CONF" | head -1 | cut -d= -f2)"
rpc_pass="$(grep -E '^rpcpassword=' "$BTC_CONF" | head -1 | cut -d= -f2)"
rpc_host="${BTC_RPC_HOST:-bitcoin-core}"
rpc_port="${BTC_RPC_PORT:-18443}"

rpc() {
  curl -fsS --max-time 5 --user "$rpc_user:$rpc_pass" \
    --header 'content-type: application/json' \
    --data "$1" "http://${rpc_host}:${rpc_port}" | jq -r '.result // empty'
}

# well-known deterministic key P2TR destination (matches repo fixtures)
address="bcrt1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqc8gma6"

echo "btc-miner: mining 1 block every ${MINE_INTERVAL:-15}s to $address"
while true; do
  if height="$(rpc '{"jsonrpc":"2.0","id":1,"method":"getblockcount","params":[]}')"; then
    if [[ "$height" =~ ^[0-9]+$ ]]; then
      if rpc "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"generatetoaddress\",\"params\":[1,\"$address\"]}" >/dev/null; then
        echo "btc-miner: block $((height + 1)) mined"
      fi
    fi
  fi
  sleep "${MINE_INTERVAL:-15}"
done
