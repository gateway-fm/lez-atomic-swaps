#!/usr/bin/env bash
# Generates the runtime config tree for the local LEZ v0.2 devnet + Bitcoin
# regtest stack. Mirrors scripts/run-lez-v02-stack.sh from the repo (channel,
# genesis allocations, deployment-settings rendering) but static, local-test
# oriented, and compose-first.
set -euo pipefail

DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly DEPLOY_ROOT
RUNTIME="${1:-$DEPLOY_ROOT/runtime}"
readonly RUNTIME
readonly LEZ_SOURCE="$DEPLOY_ROOT/assets/lez-source"

readonly channel_id="b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"
readonly bedrock_signing_key_hex="0ab865b8054be13810889714c1f1d82c3d8bb2e4510c26d0edc35cc653f306c2"
readonly maker_account_id="B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd"
readonly taker_account_id="34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib"
readonly maker_genesis_allocation=100000
readonly taker_genesis_allocation=200000
readonly slot_duration_seconds="1.0"
readonly upstream_genesis_time_hex="2c04626900000000"

umask 077

echo "runtime root: $RUNTIME"
mkdir -p "$RUNTIME"/{config,bedrock,indexer,sequencer,sockets,btc,secrets}

# --- LEZ: fresh genesis time + rendered deployment settings -----------------
chain_start_epoch="$(date -u +%s)"
genesis_time_hex="$(printf "%016x" "$chain_start_epoch" | sed -E 's/^(..)(..)(..)(..)(..)(..)(..)(..)$/\8\7\6\5\4\3\2\1/')"

sed -e "s/${upstream_genesis_time_hex}/${genesis_time_hex}/" \
    "$LEZ_SOURCE/bedrock/deployment-settings.yaml" \
    >"$RUNTIME/config/deployment-settings.yaml"

jq --arg channel "$channel_id" \
  '.bedrock_config.addr = "http://bedrock:18080" | .channel_id = $channel' \
  "$LEZ_SOURCE/indexer-config/indexer_config.json" \
  >"$RUNTIME/config/indexer_config.json"

jq --arg channel "$channel_id" \
  --arg maker "$maker_account_id" --argjson maker_amount "$maker_genesis_allocation" \
  --arg taker "$taker_account_id" --argjson taker_amount "$taker_genesis_allocation" \
  '.home = "/var/lib/sequencer_service"
   | .bedrock_config.node_url = "http://bedrock:18080"
   | .bedrock_config.channel_id = $channel
   | del(.bedrock_config.backoff)
   | .genesis = [
       {"supply_account": {"account_id": $maker, "balance": $maker_amount}},
       {"supply_account": {"account_id": $taker, "balance": $taker_amount}}
     ]' \
  "$LEZ_SOURCE/sequencer-config/sequencer_config.json" \
  >"$RUNTIME/config/sequencer_config.json"

jq -e --arg channel "$channel_id" '.channel_id == $channel' "$RUNTIME/config/indexer_config.json" >/dev/null
jq -e --arg channel "$channel_id" '.bedrock_config.channel_id == $channel' "$RUNTIME/config/sequencer_config.json" >/dev/null
[[ "$(jq -r '.genesis | length' "$RUNTIME/config/sequencer_config.json")" == 2 ]]

# The sequencer generates its own bedrock signing key under its state dir
# (load_or_create_signing_key); no key file is mounted for the local stack.

# --- Bitcoin regtest --------------------------------------------------------
# NOTE: rewrite files IN PLACE (never rm+recreate): docker bind-mounts pin
# the inode, so replacing the file would leave already-running containers
# reading a stale password forever.
if [[ -s "$RUNTIME/btc/bitcoin.conf" ]] && grep -q '^rpcpassword=' "$RUNTIME/btc/bitcoin.conf"; then
  btc_rpc_password="$(grep -E '^rpcpassword=' "$RUNTIME/btc/bitcoin.conf" | head -1 | cut -d= -f2)"
  echo "reusing existing bitcoin RPC password"
else
  btc_rpc_password="$(openssl rand -hex 16)"
fi
cat >"$RUNTIME/btc/bitcoin.conf" <<EOF
regtest=1
proxy=0
networkactive=0
dnsseed=0
listen=0

txindex=1

[regtest]
rpcuser=lezrpc
rpcpassword=${btc_rpc_password}
rpcbind=0.0.0.0:18443
rpcallowip=0.0.0.0/0
server=1
fallbackfee=0.0002
EOF
chmod 0644 "$RUNTIME/btc/bitcoin.conf" || exit 1
# NOTE: 0644 not 0600 — the distroless bitcoin-core container reads this file
# as uid 65532 while the bind mount preserves host ownership. The RPC password
# inside is local-regtest-only.

# deterministic regtest mining key (well-known test key, local-only)
rm -f "$RUNTIME/secrets/mining.key"
cat >"$RUNTIME/secrets/mining.key" <<'EOF'
0000000000000000000000000000000000000000000000000000000000000001
EOF
chmod 0600 "$RUNTIME/secrets/mining.key" || exit 1

# --- maker/taker runtime ----------------------------------------------------
mkdir -p "$RUNTIME/sockets"
rm -f "$RUNTIME/runtime.env"
printf '%s\n' \
  "LEZ_V02_CHANNEL_ID=$channel_id" \
  "LEZ_V02_GENESIS_TIME_EPOCH=$chain_start_epoch" \
  "LEZ_V02_MAKER_ACCOUNT_ID=$maker_account_id" \
  "LEZ_V02_TAKER_ACCOUNT_ID=$taker_account_id" \
  "BTC_RPC_USER=lezrpc" \
  "BTC_RPC_PASSWORD=$btc_rpc_password" \
  >"$RUNTIME/runtime.env"
chmod 0600 "$RUNTIME/runtime.env"

echo "runtime generated at $RUNTIME"
