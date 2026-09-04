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
readonly maker_account_id="B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd"
readonly taker_account_id="34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib"
readonly maker_genesis_allocation=100000
readonly taker_genesis_allocation=200000
# Persistent settlement wallets (owner account ids of the identities held in
# runner-work/market/identities). Swaps run on this long-standing chain and
# these accounts accumulate real balances across swaps — the local analog of
# funded wallets on a real network.
# LEZ_WALLET_IDENTITIES (a runner-work/market/identities directory) overrides
# the recorded ids with the ids of the identities that directory holds.
wallet_account_id() { # wallet_account_id <wallet> <recorded-id>
  local identity="${LEZ_WALLET_IDENTITIES:-}/$1.json"
  if [[ -n "${LEZ_WALLET_IDENTITIES:-}" && -f "$identity" ]]; then jq -er '.account_id' "$identity"; else echo "$2"; fi
}
wallet_munich_account_id="$(wallet_account_id maker-munich-01 BD6TpNTSLjeonDFmA3PXg6YtDy7xXt2LTm46266NpwJY)"
wallet_basel_account_id="$(wallet_account_id maker-basel-02 A81AE1KTGdZ5GCDfy4XdUe9XvgNmkFzfgZcRkkQXm8vm)"
wallet_zurich_account_id="$(wallet_account_id taker-zurich-01 4vDRakzuvKqJFJZ6k4ig3ybzds6fTLv1xDpwU283SwBM)"
wallet_limmat_account_id="$(wallet_account_id taker-limmat-02 5A8bRmav5wjYQex6z7SpuuNNyhesqHwweAqjc3eWfchH)"
readonly wallet_munich_account_id wallet_basel_account_id wallet_zurich_account_id wallet_limmat_account_id
readonly wallet_maker_allocation=100000
readonly wallet_taker_allocation=200000
readonly upstream_genesis_time_hex="2c04626900000000"

umask 077

echo "runtime root: $RUNTIME"
mkdir -p "$RUNTIME"/{config,bedrock,indexer,sequencer,sockets,btc,secrets}

# The proof view and the explorer's evidence index start from the certified
# sample until export-node-evidence.py publishes a swap the Nodes settled.
mkdir -p "$RUNTIME/evidence"
if [[ ! -s "$RUNTIME/m3-btc-ui-evidence.json" ]]; then
  cp "$DEPLOY_ROOT/assets/certified-evidence-m5arm-08180005-ui.json" \
    "$RUNTIME/m3-btc-ui-evidence.json"
  chmod 0666 "$RUNTIME/m3-btc-ui-evidence.json"
fi
[[ -s "$RUNTIME/evidence/certified-m5arm-08180005.json" ]] \
  || cp "$DEPLOY_ROOT/assets/certified-evidence-m5arm-08180005-ui.json" "$RUNTIME/evidence/certified-m5arm-08180005.json"
chmod 0644 "$RUNTIME/evidence/"*.json  # the explorer reads them as another uid
chmod 0666 "$RUNTIME/m3-btc-ui-evidence.json"
jq -e '
  .kind == "m3_btc_ui_evidence"
  and .result == "passed"
  and .terminal == {phase:"completed",revision:4}
  and (.effects | length) == 5
  and .private_material_disclosed == false
' "$RUNTIME/m3-btc-ui-evidence.json" >/dev/null

# --- LEZ: genesis time + rendered deployment settings -----------------------
# The standing LEZ chain keeps its genesis across restarts: reuse the epoch a
# previous run recorded, and mint a fresh one only for a new runtime root.
chain_start_epoch=""
if [[ -s "$RUNTIME/runtime.env" ]]; then
  chain_start_epoch="$(sed -n 's/^LEZ_V02_GENESIS_TIME_EPOCH=//p' "$RUNTIME/runtime.env" | head -1)"
fi
if [[ "$chain_start_epoch" =~ ^[0-9]+$ ]]; then
  echo "reusing existing LEZ genesis time ${chain_start_epoch}"
else
  chain_start_epoch="$(date -u +%s)"
fi
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
  --arg munich "$wallet_munich_account_id" --arg basel "$wallet_basel_account_id" \
  --arg zurich "$wallet_zurich_account_id" --arg limmat "$wallet_limmat_account_id" \
  --argjson wallet_maker_amount "$wallet_maker_allocation" \
  --argjson wallet_taker_amount "$wallet_taker_allocation" \
  '.home = "/var/lib/sequencer_service"
   | .bedrock_config.node_url = "http://bedrock:18080"
   | .bedrock_config.channel_id = $channel
   | del(.bedrock_config.backoff)
   | .genesis = [
       {"supply_account": {"account_id": $maker, "balance": $maker_amount}},
       {"supply_account": {"account_id": $taker, "balance": $taker_amount}},
       {"supply_account": {"account_id": $munich, "balance": $wallet_maker_amount}},
       {"supply_account": {"account_id": $basel, "balance": $wallet_maker_amount}},
       {"supply_account": {"account_id": $zurich, "balance": $wallet_taker_amount}},
       {"supply_account": {"account_id": $limmat, "balance": $wallet_taker_amount}}
     ]' \
  "$LEZ_SOURCE/sequencer-config/sequencer_config.json" \
  >"$RUNTIME/config/sequencer_config.json"

jq -e --arg channel "$channel_id" '.channel_id == $channel' "$RUNTIME/config/indexer_config.json" >/dev/null
jq -e --arg channel "$channel_id" '.bedrock_config.channel_id == $channel' "$RUNTIME/config/sequencer_config.json" >/dev/null
[[ "$(jq -r '.genesis | length' "$RUNTIME/config/sequencer_config.json")" == 6 ]]
[[ "$(jq -r '[.genesis[].supply_account.account_id] | unique | length' \
  "$RUNTIME/config/sequencer_config.json")" == 6 ]]

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
txospenderindex=1

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
# Bitcoin Core credentials in cookie form for the Nodes and their actors
# (rewritten in place: bind mounts pin the inode).
printf 'lezrpc:%s\n' "$btc_rpc_password" >"$RUNTIME/secrets/btc-rpc-cookie"
chmod 0644 "$RUNTIME/secrets/btc-rpc-cookie"

# --- maker/taker runtime ----------------------------------------------------
# Per-role LEZ identities for the Node-owned Bitcoin lifecycle (ADR 0213): the
# Maker settles as maker-munich-01 and the Taker as taker-zurich-01, the two
# funded market wallets, so their balances move on the standing chain.
# The market root holds the wallet identities and market-bootstrap.env (the
# escrow deployment the Nodes read); market-bootstrap.sh writes it once.
market_root="${LEZ_MARKET_ROOT:-}"
if [[ -z "$market_root" ]]; then
  for candidate in "$DEPLOY_ROOT/../runner-work/market" "$DEPLOY_ROOT/../../runner-work/market"; do
    [[ -d "$candidate" ]] && { market_root="$(cd "$candidate" && pwd -P)"; break; }
  done
fi
[[ -n "$market_root" && -d "$market_root" ]] || {
  echo "LEZ_MARKET_ROOT must select the market root (wallet identities, market-bootstrap.env)" >&2
  exit 1
}
identities_root="${LEZ_WALLET_IDENTITIES:-$market_root/identities}"
for pair in maker:maker-munich-01 taker:taker-zurich-01; do
  role="${pair%%:*}"; wallet="${pair##*:}"
  mkdir -p "$RUNTIME/lez/$role"
  if [[ -n "$identities_root" && -f "$identities_root/$wallet/lez-signer.key" ]]; then
    cp "$identities_root/$wallet/lez-signer.key" "$RUNTIME/lez/$role/lez-signer.key"
    cp "$identities_root/$wallet/identity.json" "$RUNTIME/lez/$role/identity.json"
  else
    echo "warning: $wallet identity unavailable; $role Node runs without the Bitcoin lifecycle" >&2
  fi
  # Bind mounts keep host ownership; the entrypoint copies these owner-private.
  chmod 0644 "$RUNTIME/lez/$role"/* 2>/dev/null || true
done
mkdir -p "$RUNTIME/sockets"
rm -f "$RUNTIME/runtime.env"
printf '%s\n' \
  "LEZ_V02_CHANNEL_ID=$channel_id" \
  "LEZ_V02_GENESIS_TIME_EPOCH=$chain_start_epoch" \
  "LEZ_V02_MAKER_ACCOUNT_ID=$maker_account_id" \
  "LEZ_V02_TAKER_ACCOUNT_ID=$taker_account_id" \
  "BTC_RPC_USER=lezrpc" \
  "BTC_RPC_PASSWORD=$btc_rpc_password" \
  "LEZ_MARKET_ROOT=$market_root" \
  >"$RUNTIME/runtime.env"
chmod 0600 "$RUNTIME/runtime.env"

echo "runtime generated at $RUNTIME"
