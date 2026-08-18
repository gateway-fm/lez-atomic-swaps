#!/usr/bin/env bash
# One-time bootstrap of the long-standing settlement chains ("market").
#
# Runs INSIDE the lez-runner-arm container (host network). Deploys the M5
# escrow program to the standing LEZ chain, claims the four persistent wallet
# vaults into their owner accounts, and writes the attach manifests that
# attach-mode M3 swap runs consume. Idempotent: skips steps whose on-chain
# effect already exists.
set -euo pipefail
export LC_ALL=C
umask 077

readonly MARKET_ROOT="${MARKET_ROOT:-/Users/mandrigin/Desktop/las-logos/runner-work/market}"
readonly REPO_ROOT="${REPO_ROOT:-/Users/mandrigin/Desktop/las-logos/runner-work/repo}"
readonly SEQUENCER_URL="${SEQUENCER_URL:-http://127.0.0.1:3040}"
readonly INDEXER_URL="${INDEXER_URL:-http://127.0.0.1:8779}"
readonly CHANNEL_ID="b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"
readonly ESCROW_PROGRAM_ID="b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"
readonly AUTH_TRANSFER_PROGRAM_ID="dcbbfebcd59399961ed9973b8307dc475fd4c5ca5779aacfe7588f7dbc3f4a71"
readonly DEPLOYER="${DEPLOYER:-/tmp/lez-m3-artifact-arm/debug/lez-zec-escrow-v02-deployer}"
readonly VAULT_CLAIM_BIN="${VAULT_CLAIM_BIN:-$REPO_ROOT/compat/lez-v0_2-sidecar/target/debug/lez-v02-vault-claim-poc}"
readonly BTC_RPC_URL="${BTC_RPC_URL:-http://127.0.0.1:18443}"
readonly BTC_RPC_USER="${BTC_RPC_USER:-lezrpc}"
readonly BTC_RPC_PASSWORD="${BTC_RPC_PASSWORD:?BTC_RPC_PASSWORD is required}"
readonly BTC_ATTACH_RUN="market-btc-0001"
readonly LEZ_ATTACH_RUN="market-lez-0001"

fail() { echo "market bootstrap failed: $*" >&2; exit 2; }

indexer() {
  curl -sf -H 'content-type: application/json' \
    -d "$(jq -cn --arg m "$1" --argjson p "$2" '{jsonrpc:"2.0",id:1,method:$m,params:$p}')" \
    "$INDEXER_URL"
}

account_balance() { indexer getAccount "[\"$1\"]" | jq -r '.result.balance // empty'; }

# Wallet roster: name role owner_account vault_account allocation
WALLETS=(
  "maker-munich-01 maker BD6TpNTSLjeonDFmA3PXg6YtDy7xXt2LTm46266NpwJY 7v83atCzKMg4b7o6oS5AMxik1YrC6Kyx1g4HD7LenBmt 100000"
  "maker-basel-02 maker A81AE1KTGdZ5GCDfy4XdUe9XvgNmkFzfgZcRkkQXm8vm BgUm3srEYVNS7vATkByw3ZkDNppGBF1edd4CUEv2Zqx8 100000"
  "taker-zurich-01 taker 4vDRakzuvKqJFJZ6k4ig3ybzds6fTLv1xDpwU283SwBM 7bVx7C8fq8mdvnHJgTiRHMFezhoMnkancSGMhkNfNmqR 200000"
  "taker-limmat-02 taker 5A8bRmav5wjYQex6z7SpuuNNyhesqHwweAqjc3eWfchH 2G22MdePKTHXgQtWYUBfzoDHKxGgZCg8vFwkdX22x8Yr 200000"
)

mkdir -p "$MARKET_ROOT/bootstrap"
chmod 0700 "$MARKET_ROOT" "$MARKET_ROOT/bootstrap"

# ── 1. escrow program deployment (once per chain) ──────────────────────────
deployment_evidence="$MARKET_ROOT/bootstrap/deployment.json"
if [[ -s "$deployment_evidence" ]] \
   && jq -e '.transaction_hash | test("^[0-9a-f]{64}$")' "$deployment_evidence" >/dev/null 2>&1; then
  echo "escrow program already deployed: $(jq -r '.transaction_hash' "$deployment_evidence")"
else
  [[ -x "$DEPLOYER" ]] || fail "deployer missing: $DEPLOYER"
  "$DEPLOYER" deploy-m4-local --rpc-url "$SEQUENCER_URL" \
    --channel-id "$CHANNEL_ID" --timeout-seconds 300 >"$deployment_evidence.partial"
  jq -e --arg program "$ESCROW_PROGRAM_ID" '
    .preflight.image_id == $program
    and (.transaction_hash | test("^[0-9a-f]{64}$"))
    and .inclusion_block_id > .preflight.last_block_id
  ' "$deployment_evidence.partial" >/dev/null || fail "deployment evidence invalid"
  mv "$deployment_evidence.partial" "$deployment_evidence"
  echo "escrow program deployed: $(jq -r '.transaction_hash' "$deployment_evidence")"
fi
deployment_tx="$(jq -r '.transaction_hash' "$deployment_evidence")"

# ── 2. vault claims (once per wallet) ──────────────────────────────────────
for entry in "${WALLETS[@]}"; do
  read -r wallet role owner vault allocation <<<"$entry"
  # The vault, not the owner, says whether the one-time claim already ran:
  # owner balances drift as the wallet trades on the standing chain.
  vault_balance="$(account_balance "$vault")"
  if [[ "$vault_balance" == 0 || -z "$vault_balance" ]]; then
    echo "$wallet: vault already claimed (owner holds $(account_balance "$owner"))"
    continue
  fi
  [[ "$vault_balance" == "$allocation" ]] ||
    fail "$wallet vault balance unexpected: $vault_balance"
  balance="$(account_balance "$owner")"
  [[ "$balance" == 0 || -z "$balance" ]] || fail "$wallet owner balance unexpected: $balance"
  key="$MARKET_ROOT/identities/$wallet/lez-signer.key"
  [[ -f "$key" ]] || fail "$wallet signing key missing"
  state_dir="$MARKET_ROOT/bootstrap/$wallet"
  rm -rf "$state_dir"; mkdir -m 0700 "$state_dir"
  "$VAULT_CLAIM_BIN" --role "$role" --run-id "$LEZ_ATTACH_RUN-$wallet" \
    --request-id "${role}-vault-claim-0001" --state-directory "$state_dir" \
    --private-key-file "$key" --sequencer-url "$SEQUENCER_URL" \
    --chain-id "$CHANNEL_ID" --escrow-program-id "$ESCROW_PROGRAM_ID" \
    --allocation "$allocation" >"$MARKET_ROOT/bootstrap/$wallet-claim.json"
  jq -e --argjson allocation "$allocation" '
    .submission.decision == "admitted" and .allocation == $allocation
    and (.transaction_id | test("^[0-9a-f]{64}$"))
  ' "$MARKET_ROOT/bootstrap/$wallet-claim.json" >/dev/null || fail "$wallet claim evidence invalid"
  for _ in $(seq 1 60); do
    [[ "$(account_balance "$vault")" == 0 ]] && break
    sleep 1
  done
  [[ "$(account_balance "$vault")" == 0 ]] || fail "$wallet vault not swept after claim"
  echo "$wallet: claimed $allocation into $owner ($(jq -r '.transaction_id' "$MARKET_ROOT/bootstrap/$wallet-claim.json"))"
done

# ── 3. attach manifests ────────────────────────────────────────────────────
genesis_block_hash="$(jq -r '.preflight.genesis_block_hash' "$deployment_evidence")"
[[ "$genesis_block_hash" =~ ^[0-9a-f]{64}$ ]] || fail "genesis block hash unavailable"

lez_dir="$REPO_ROOT/.e2e/$LEZ_ATTACH_RUN/lez-v02"
mkdir -p "$lez_dir"
cat >"$lez_dir/run.env" <<EOF
RUN_ID=$LEZ_ATTACH_RUN
LEZ_V02_SLOT_DURATION_SECONDS=1.0
LEZ_V02_CHANNEL_PUBLIC_KEY=$CHANNEL_ID
LEZ_SEQUENCER_RPC_URL=$SEQUENCER_URL
LEZ_INDEXER_RPC_URL=$INDEXER_URL
LEZ_V02_MAKER_ACCOUNT_ID=BD6TpNTSLjeonDFmA3PXg6YtDy7xXt2LTm46266NpwJY
LEZ_V02_MAKER_VAULT_ACCOUNT_ID=7v83atCzKMg4b7o6oS5AMxik1YrC6Kyx1g4HD7LenBmt
LEZ_V02_MAKER_GENESIS_ALLOCATION=100000
LEZ_V02_TAKER_ACCOUNT_ID=4vDRakzuvKqJFJZ6k4ig3ybzds6fTLv1xDpwU283SwBM
LEZ_V02_TAKER_VAULT_ACCOUNT_ID=7bVx7C8fq8mdvnHJgTiRHMFezhoMnkancSGMhkNfNmqR
LEZ_V02_TAKER_GENESIS_ALLOCATION=200000
EOF
chmod 0600 "$lez_dir/run.env"
mkdir -p "$lez_dir/bedrock/logs"
touch "$lez_dir/bedrock/logs/logos-blockchain.log"

market_bootstrap_manifest="$MARKET_ROOT/market-bootstrap.env"
cat >"$market_bootstrap_manifest" <<EOF
M3_POC_LEZ_ESCROW_PROGRAM_ID=$ESCROW_PROGRAM_ID
M3_POC_LEZ_AUTH_TRANSFER_PROGRAM_ID=$AUTH_TRANSFER_PROGRAM_ID
M3_POC_LEZ_GENESIS_BLOCK_HASH=$genesis_block_hash
M3_POC_LEZ_DEPLOYMENT_TRANSACTION_ID=$deployment_tx
EOF
chmod 0600 "$market_bootstrap_manifest"

btc_dir="$REPO_ROOT/.e2e/$BTC_ATTACH_RUN/bitcoin-core"
cred_dir="$btc_dir/credentials"
mkdir -p "$cred_dir"
chmod 0700 "$btc_dir" "$cred_dir"
for actor in maker taker; do
  cat >"$cred_dir/$actor.curlrc" <<EOF
user = "$BTC_RPC_USER:$BTC_RPC_PASSWORD"
url = "$BTC_RPC_URL"
connect-timeout = 2
max-time = 10
silent
show-error
EOF
  printf '%s:%s\n' "$BTC_RPC_USER" "$BTC_RPC_PASSWORD" >"$cred_dir/$actor.basic"
  chmod 0600 "$cred_dir/$actor.curlrc" "$cred_dir/$actor.basic"
done
cat >"$cred_dir/funding.env" <<EOF
BITCOIN_CORE_NETWORK=regtest
BITCOIN_CORE_FUNDING_SECRET_KEY_HEX=0000000000000000000000000000000000000000000000000000000000000001
BITCOIN_CORE_FUNDING_DESCRIPTOR=rawtr(79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798)#xsjqcczm
BITCOIN_CORE_FUNDING_ADDRESS=bcrt1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqc8gma6
BITCOIN_CORE_FUNDING_TXID=0000000000000000000000000000000000000000000000000000000000000000
BITCOIN_CORE_FUNDING_VOUT=0
BITCOIN_CORE_FUNDING_VALUE_SAT=5000000000
EOF
chmod 0600 "$cred_dir/funding.env"
cat >"$btc_dir/run.env" <<EOF
RUN_ID=$BTC_ATTACH_RUN
BITCOIN_CORE_E2E_MODE=service
COMPOSE_PROJECT_NAME=lez-swap-stack
BITCOIN_CORE_RPC_URL=$BTC_RPC_URL
BITCOIN_CORE_MAKER_CURL_CONFIG=$cred_dir/maker.curlrc
BITCOIN_CORE_TAKER_CURL_CONFIG=$cred_dir/taker.curlrc
BITCOIN_CORE_MAKER_BASIC_CREDENTIALS=$cred_dir/maker.basic
BITCOIN_CORE_TAKER_BASIC_CREDENTIALS=$cred_dir/taker.basic
BITCOIN_CORE_FUNDING_CREDENTIALS=$cred_dir/funding.env
EOF
chmod 0600 "$btc_dir/run.env"

echo "market bootstrap complete"
echo "  LEZ manifest:  $lez_dir/run.env"
echo "  BTC manifest:  $btc_dir/run.env"
echo "  bootstrap:     $market_bootstrap_manifest"
