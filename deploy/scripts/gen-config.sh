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
# 2 000 000 LEZ each: a whole scenario matrix on one chain with margin. A ZEC
# corridor's lot is 50 000 LEZ and it leaves the depositor for good on a
# completed swap (the Taker in TakerSellsLez, the Maker in TakerSellsForeign);
# the old 200 000 covered four swaps, and the fifth failed its LEZ funding
# (the escrow program reports an insufficient depositor balance as
# "native funding account binding mismatch", error 6003).
readonly maker_genesis_allocation=2000000
readonly taker_genesis_allocation=2000000
# Persistent settlement wallets (owner account ids of the identities held in
# the market root, LEZ_MARKET_ROOT). Swaps run on this long-standing chain and
# these accounts accumulate real balances across swaps — the local analog of
# funded wallets on a real network.
# LEZ_WALLET_IDENTITIES (the market root's identities directory) overrides
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
readonly wallet_maker_allocation=2000000
readonly wallet_taker_allocation=2000000
# The finality heartbeat's two accounts (sender, recipient): funded only when
# their identities exist in the market root (from-scratch.sh / start.sh mint
# them); 100 000 units is 23 days of one unit per 20 s. LEZ_HEARTBEAT_SECONDS
# (0 disables the service) and LEZ_HEARTBEAT_STALL_SECONDS reach up.sh and the
# service through runtime.env.
heartbeat_pulse_account_id="$(wallet_account_id heartbeat-pulse-01 "")"
heartbeat_echo_account_id="$(wallet_account_id heartbeat-echo-02 "")"
heartbeat_allocation=0
if [[ -n "$heartbeat_pulse_account_id" && -n "$heartbeat_echo_account_id" ]]; then heartbeat_allocation=100000; fi
readonly heartbeat_pulse_account_id heartbeat_echo_account_id heartbeat_allocation
heartbeat_seconds="${LEZ_HEARTBEAT_SECONDS:-20}"
heartbeat_stall_seconds="${LEZ_HEARTBEAT_STALL_SECONDS:-120}"
[[ "$heartbeat_seconds" =~ ^[0-9]+$ && "$heartbeat_stall_seconds" =~ ^[0-9]+$ ]] || { echo "LEZ_HEARTBEAT_SECONDS and LEZ_HEARTBEAT_STALL_SECONDS must be integers" >&2; exit 1; }

# Preserve the selected volume namespace on ordinary restarts too.
volume_prefix="${LEZ_VOLUME_PREFIX:-}"
if [[ -z "$volume_prefix" && -f "$RUNTIME/runtime.env" ]]; then
  volume_prefix="$(sed -n 's/^LEZ_VOLUME_PREFIX=//p' "$RUNTIME/runtime.env" | head -1)"
fi
volume_prefix="${volume_prefix:-lez}"
[[ "$volume_prefix" =~ ^[a-zA-Z0-9][a-zA-Z0-9_.-]*$ ]] || { echo "invalid LEZ_VOLUME_PREFIX" >&2; exit 1; }

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

# --- LEZ: deployment settings ------------------------------------------------
# The timing profile is kept across reruns (read here, before the old file is
# replaced): a rerun without LEZ_TIMING_PROFILE must not silently move the
# Nodes back to `local`.
previous_timing_profile=""
previous_actor_trace=""
if [[ -s "$RUNTIME/runtime.env" ]]; then
  previous_timing_profile="$(sed -n 's/^LEZ_TIMING_PROFILE=//p' "$RUNTIME/runtime.env" | head -1)"
  previous_actor_trace="$(sed -n 's/^LEZ_BTC_ACTOR_TRACE=//p' "$RUNTIME/runtime.env" | head -1)"
fi

# Bedrock 0.2.1 starts from upstream's genesis as it is, however old; the
# genesis block commits to its time, so it is no longer ours to rewrite.
cp "$LEZ_SOURCE/bedrock/deployment-settings.yaml" "$RUNTIME/config/deployment-settings.yaml"

jq --arg channel "$channel_id" \
  '.bedrock_config.addr = "http://bedrock:18080" | .channel_id = $channel' \
  "$LEZ_SOURCE/indexer-config/indexer_config.json" \
  >"$RUNTIME/config/indexer_config.json"

# The sequencer pays for its inscriptions from the genesis notes, which it finds
# by their public key: the genesis is the one place that names it.
funding_key="$(sed -n "s/^ *pk: '\{0,1\}\([0-9a-f]\{64\}\)'\{0,1\} *$/\1/p" "$LEZ_SOURCE/bedrock/deployment-settings.yaml" | head -1)"
[[ "$funding_key" =~ ^[0-9a-f]{64}$ ]] || { echo "no genesis funding key in deployment-settings.yaml" >&2; exit 1; }

jq --arg channel "$channel_id" --arg funding "$funding_key" \
  --arg maker "$maker_account_id" --argjson maker_amount "$maker_genesis_allocation" \
  --arg taker "$taker_account_id" --argjson taker_amount "$taker_genesis_allocation" \
  --arg munich "$wallet_munich_account_id" --arg basel "$wallet_basel_account_id" \
  --arg zurich "$wallet_zurich_account_id" --arg limmat "$wallet_limmat_account_id" \
  --argjson wallet_maker_amount "$wallet_maker_allocation" \
  --argjson wallet_taker_amount "$wallet_taker_allocation" \
  --arg pulse "$heartbeat_pulse_account_id" --arg echo "$heartbeat_echo_account_id" \
  --argjson heartbeat_amount "$heartbeat_allocation" \
  '.home = "/var/lib/sequencer_service"
   | .bedrock_config.node_url = "http://bedrock:18080"
   | .bedrock_config.channel_id = $channel
   | .bedrock_config.funding_key = $funding
   | del(.bedrock_config.backoff)
   | .genesis = [
       {"supply_account": {"account_id": $maker, "balance": $maker_amount}},
       {"supply_account": {"account_id": $taker, "balance": $taker_amount}},
       {"supply_account": {"account_id": $munich, "balance": $wallet_maker_amount}},
       {"supply_account": {"account_id": $basel, "balance": $wallet_maker_amount}},
       {"supply_account": {"account_id": $zurich, "balance": $wallet_taker_amount}},
       {"supply_account": {"account_id": $limmat, "balance": $wallet_taker_amount}}
     ] + (if $heartbeat_amount > 0 then [
       {"supply_account": {"account_id": $pulse, "balance": $heartbeat_amount}},
       {"supply_account": {"account_id": $echo, "balance": $heartbeat_amount}}
     ] else [] end)' \
  "$LEZ_SOURCE/sequencer-config/sequencer_config.json" \
  >"$RUNTIME/config/sequencer_config.json"

jq -e --arg channel "$channel_id" '.channel_id == $channel' "$RUNTIME/config/indexer_config.json" >/dev/null
jq -e --arg channel "$channel_id" '.bedrock_config.channel_id == $channel' "$RUNTIME/config/sequencer_config.json" >/dev/null
expected_genesis_accounts=6; (( heartbeat_allocation > 0 )) && expected_genesis_accounts=8
[[ "$(jq -r '.genesis | length' "$RUNTIME/config/sequencer_config.json")" == "$expected_genesis_accounts" ]]
[[ "$(jq -r '[.genesis[].supply_account.account_id] | unique | length' \
  "$RUNTIME/config/sequencer_config.json")" == "$expected_genesis_accounts" ]]

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
  for candidate in "$DEPLOY_ROOT/../../market" "$DEPLOY_ROOT/../market" "$DEPLOY_ROOT/../../runner-work/market" "$DEPLOY_ROOT/../runner-work/market"; do
    [[ -d "$candidate" ]] && { market_root="$(cd "$candidate" && pwd -P)"; break; }
  done
fi
[[ -n "$market_root" && -d "$market_root" ]] || {
  echo "LEZ_MARKET_ROOT must select the market root (wallet identities, market-bootstrap.env)" >&2
  exit 1
}
# Only public deployment IDs are mounted from the market. Its private root
# is mode 0700 and cannot be traversed by the Nodes' uid on Linux.
if [[ -s "$market_root/market-bootstrap.env" ]]; then
  cat "$market_root/market-bootstrap.env" > "$RUNTIME/market-bootstrap.env"
else
  : > "$RUNTIME/market-bootstrap.env"
fi
chmod 0644 "$RUNTIME/market-bootstrap.env"
identities_root="${LEZ_WALLET_IDENTITIES:-$market_root/identities}"
for pair in maker:maker-munich-01 taker:taker-zurich-01; do
  role="${pair%%:*}"; wallet="${pair##*:}"
  mkdir -p "$RUNTIME/lez/$role"
  chmod 0755 "$RUNTIME/lez/$role"
  if [[ -n "$identities_root" && -f "$identities_root/$wallet/lez-signer.key" ]]; then
    cp "$identities_root/$wallet/lez-signer.key" "$RUNTIME/lez/$role/lez-signer.key"
    cp "$identities_root/$wallet/identity.json" "$RUNTIME/lez/$role/identity.json"
  else
    echo "warning: $wallet identity unavailable; $role Node runs without the Bitcoin lifecycle" >&2
  fi
  # Bind mounts keep host ownership; the entrypoint copies these owner-private.
  chmod 0644 "$RUNTIME/lez/$role"/* 2>/dev/null || true
done
# Each role's Zcash key, minted once beside its LEZ identity and kept across
# reruns: the Maker's is also its Delivery signing key (the entrypoint installs
# it; the ZEC ceremony binds Delivery identity, Chat key and Zcash key into one
# key) and zebra mines to its transparent address, so the Maker owns the
# coinbase the ZEC provisioner spends; the Taker's key funds Zcash in
# TakerSellsForeign (scripts/zec-provision.sh pays it once from the Maker's).
# Nothing here is a constant: a fresh runtime mints fresh keys. The address
# comes from zec-local-poc-identity, staged on the host by from-scratch.sh
# --only rust or, once the stage is gone, run from the Taker image.
zcash_transparent_address() { # zcash_transparent_address <owner-private key file>
  local key="$1" tool="$DEPLOY_ROOT/images/taker-node/zec-local-poc-identity" identity
  if [[ -x "$tool" ]]; then
    identity="$("$tool" --zcash-key-file "$key")"
  else
    identity="$(docker run --rm --user "$(id -u):$(id -g)" -v "$key:/in/zcash.key:ro" \
      --entrypoint zec-local-poc-identity "${LEZ_IMAGE_PREFIX:-lez}-taker-node:${LEZ_IMAGE_TAG:-local}" \
      --zcash-key-file /in/zcash.key </dev/null)"
  fi || { echo "zec-local-poc-identity failed: stage it (from-scratch.sh --only rust) or build the Taker image first" >&2; return 1; }
  python3 -c 'import json, sys; print(json.load(sys.stdin)["transparent_address"])' <<<"$identity"
}
for role in maker taker; do
  if [[ ! -s "$RUNTIME/lez/$role/zcash.key" ]]; then
    (umask 077; openssl rand -hex 32 >"$RUNTIME/lez/$role/zcash.key")
    echo "minted the $role Zcash key: runtime/lez/$role/zcash.key"
  fi
done
# The identity tool reads owner-private files only; the mounted copy is 0644
# like lez-signer.key (the entrypoint re-installs it 0600), so derive from a
# private copy under runtime/secrets.
maker_zcash_private="$(mktemp "$RUNTIME/secrets/zcash-key.XXXXXX")"
trap 'rm -f "$maker_zcash_private"' EXIT
install -m 0600 "$RUNTIME/lez/maker/zcash.key" "$maker_zcash_private"
zebra_miner_address="$(zcash_transparent_address "$maker_zcash_private")" || exit 1
rm -f "$maker_zcash_private"
trap - EXIT
chmod 0644 "$RUNTIME/lez/maker/zcash.key" "$RUNTIME/lez/taker/zcash.key"
mkdir -p "$RUNTIME/zebra"
sed "s|^miner_address = .*|miner_address = \"$zebra_miner_address\"|" "$DEPLOY_ROOT/assets/zebra/zebrad.toml" >"$RUNTIME/zebra/zebrad.toml"
chmod 0644 "$RUNTIME/zebra/zebrad.toml"
echo "zebra mines to the Maker's Zcash key ($zebra_miner_address): runtime/zebra/zebrad.toml"
mkdir -p "$RUNTIME/sockets"
rm -f "$RUNTIME/runtime.env"
# Recovery timing profile for the Node-owned Bitcoin lifecycle. `local` keeps
# network-like deadlines; `fast` shortens them so refund and abandonment paths
# can be exercised in minutes (Bitcoin CSV maturity is a block count, mined on
# demand on regtest; the LEZ deadlines are wall-clock). The Maker's second-lock
# cutoff must outlast a Bitcoin confirmation (one block every two minutes), and
# a Node refuses a profile whose refund delays, in blocks, break the order its
# times promise. The first value is the delay when Bitcoin is the first lock,
# the seventh when it is the second; the last two are seconds per block at the
# fastest and slowest pace (the regtest miner keeps 120).
# The last value is the LEZ discovery window in blocks: a Maker lock can only
# appear before the cutoff, and a refund asserts its absence only once that
# whole window is finalized, so the window must cover the cutoff (10 s slots on
# the local devnet: 600 s = 60 blocks) and nothing more than needed.
timing_profile="${LEZ_TIMING_PROFILE:-$previous_timing_profile}"
[[ -n "${LEZ_TIMING_PROFILE:-}" || -z "$timing_profile" ]] || echo "reusing existing timing profile ${timing_profile}"
timing_profile="${timing_profile:-local}"
case "$timing_profile" in
  local) timing=(60 1800 3600 7200 300 360 33 120 120) ;;
  fast)  timing=(15 600 900 1800 60 120 8 120 120) ;;
  *) echo "LEZ_TIMING_PROFILE must be local or fast" >&2; exit 1 ;;
esac
printf '%s\n' \
  "LEZ_VOLUME_PREFIX=$volume_prefix" \
  "LEZ_TIMING_PROFILE=$timing_profile" \
  "LEZ_BTC_REFUND_CSV_BLOCKS=${timing[0]}" \
  "LEZ_BTC_SECOND_LOCK_REFUND_CSV_BLOCKS=${timing[6]}" \
  "LEZ_BTC_BLOCK_SECONDS_FASTEST=${timing[7]}" \
  "LEZ_BTC_BLOCK_SECONDS_SLOWEST=${timing[8]}" \
  "LEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS=${timing[1]}" \
  "LEZ_BTC_EARLIER_REFUND_SECONDS=${timing[2]}" \
  "LEZ_BTC_LATER_REFUND_SECONDS=${timing[3]}" \
  "LEZ_BTC_REFUND_MARGIN_SECONDS=${timing[4]}" \
  "LEZ_LEZ_DISCOVERY_MAX_BLOCKS=${timing[5]}" \
  "LEZ_BTC_ACTOR_TRACE=${LEZ_BTC_ACTOR_TRACE:-$previous_actor_trace}" \
  "LEZ_V02_CHANNEL_ID=$channel_id" \
  "LEZ_V02_MAKER_ACCOUNT_ID=$maker_account_id" \
  "LEZ_V02_TAKER_ACCOUNT_ID=$taker_account_id" \
  "BTC_RPC_USER=lezrpc" \
  "BTC_RPC_PASSWORD=$btc_rpc_password" \
  "LEZ_MARKET_ROOT=$market_root" \
  "LEZ_WALLET_MAKER_ALLOCATION=$wallet_maker_allocation" \
  "LEZ_WALLET_TAKER_ALLOCATION=$wallet_taker_allocation" \
  "LEZ_HEARTBEAT_ALLOCATION=$heartbeat_allocation" \
  "LEZ_HEARTBEAT_SECONDS=$heartbeat_seconds" \
  "LEZ_HEARTBEAT_STALL_SECONDS=$heartbeat_stall_seconds" \
  >"$RUNTIME/runtime.env"
chmod 0600 "$RUNTIME/runtime.env"

# Configs contain public chain settings and are mounted as individual files.
chmod 0644 "$RUNTIME/config/"*.json "$RUNTIME/config/"*.yaml

echo "runtime generated at $RUNTIME"
