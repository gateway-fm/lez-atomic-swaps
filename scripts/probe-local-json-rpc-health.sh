#!/bin/bash
set -euo pipefail

readonly curl=/usr/bin/curl
readonly jq=/usr/bin/jq
readonly zcash_genesis=029f11d80ef9765602235e1bc9727e3eb6ba20839319f761fee920d63401e327
readonly bitcoin_genesis=0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206

rpc() {
  local endpoint="$1"
  local payload="$2"
  "$curl" --fail --silent --show-error --noproxy '*' --connect-timeout 1 \
    --max-time 2 --header 'content-type: application/json' --data "$payload" "$endpoint"
}

case "${1:-}" in
  zcash)
    [[ "${2:-}" =~ ^http://127\.0\.0\.1:[1-9][0-9]{0,4}/?$ ]] || exit 2
    height="$(rpc "$2" '{"jsonrpc":"2.0","id":"health-height","method":"getblockcount","params":[]}')"
    genesis="$(rpc "$2" '{"jsonrpc":"2.0","id":"health-genesis","method":"getblockhash","params":[0]}')"
    "$jq" -e '.error == null and (.result | numbers) >= 0' <<<"$height" >/dev/null
    "$jq" -e --arg expected "$zcash_genesis" \
      '.error == null and .result == $expected' <<<"$genesis" >/dev/null
    ;;
  bitcoin)
    [[ "${2:-}" == /* && -f "${2:-}" && ! -L "${2:-}" ]] || exit 2
    chain="$($curl --config "$2" \
      --data '{"jsonrpc":"2.0","id":"health-chain","method":"getblockchaininfo","params":[]}')"
    genesis="$($curl --config "$2" \
      --data '{"jsonrpc":"2.0","id":"health-genesis","method":"getblockhash","params":[0]}')"
    "$jq" -e '.error == null and .result.chain == "regtest"
      and .result.blocks >= 0 and .result.headers >= .result.blocks' <<<"$chain" >/dev/null
    "$jq" -e --arg expected "$bitcoin_genesis" \
      '.error == null and .result == $expected' <<<"$genesis" >/dev/null
    ;;
  *) exit 2 ;;
esac
