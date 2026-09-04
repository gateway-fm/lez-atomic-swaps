#!/usr/bin/env bash
# One-time bootstrap of the long-standing settlement chains ("market").
#
# Runs in one throwaway builder container on the stack's network (see
# from-scratch.sh, market_bootstrap). Deploys the escrow program to the standing
# LEZ chain, claims the four persistent wallet vaults into their owner accounts,
# and writes market-bootstrap.env, the manifest the Nodes' entrypoints render
# into btc-role.json. Idempotent: skips steps whose on-chain effect exists.
set -euo pipefail
export LC_ALL=C
umask 077

readonly MARKET_ROOT="${MARKET_ROOT:?MARKET_ROOT must select the market root (identities, bootstrap)}"
readonly SEQUENCER_URL="${SEQUENCER_URL:-http://127.0.0.1:3040}"
readonly INDEXER_URL="${INDEXER_URL:-http://127.0.0.1:8779}"
readonly CHANNEL_ID="b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"
# LEZ program identity of the pinned escrow guest (its Risc0 image ID differs).
readonly ESCROW_PROGRAM_ID="${ESCROW_PROGRAM_ID:-b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433}"
readonly AUTH_TRANSFER_PROGRAM_ID="dcbbfebcd59399961ed9973b8307dc475fd4c5ca5779aacfe7588f7dbc3f4a71"
readonly DEPLOYER="${DEPLOYER:-/provision/escrow-artifact/debug/lez-zec-escrow-v02-deployer}"
readonly VAULT_CLAIM_BIN="${VAULT_CLAIM_BIN:-/provision/sidecar/lez-v02-vault-claim-poc}"
readonly LEZ_ATTACH_RUN="market-lez-0001"

fail() { echo "market bootstrap failed: $*" >&2; exit 2; }

indexer() {
  curl -sf -H 'content-type: application/json' \
    -d "$(jq -cn --arg m "$1" --argjson p "$2" '{jsonrpc:"2.0",id:1,method:$m,params:$p}')" \
    "$INDEXER_URL"
}

account_balance() { indexer getAccount "[\"$1\"]" | jq -r '.result.balance // empty'; }

# Wallet roster: name role owner_account vault_account allocation, read from
# the identities the genesis funded (deploy/scripts/gen-config.sh reads the
# same files), so a fresh workspace with fresh identities stays consistent.
WALLETS=()
for wallet in maker-munich-01 maker-basel-02 taker-zurich-01 taker-limmat-02; do
  identity="$MARKET_ROOT/identities/$wallet/identity.json"
  [[ -f "$identity" ]] || fail "$wallet identity missing: $identity"
  role="${wallet%%-*}"
  allocation=100000; [[ "$role" == taker ]] && allocation=200000
  WALLETS+=("$wallet $role $(jq -er '.account_id' "$identity") $(jq -er '.vault_account_id' "$identity") $allocation")
done

mkdir -p "$MARKET_ROOT/bootstrap"
chmod 0700 "$MARKET_ROOT" "$MARKET_ROOT/bootstrap"

# ── 1. escrow program deployment (once per chain) ──────────────────────────
deployment_evidence="$MARKET_ROOT/bootstrap/deployment.json"
# Evidence for another program (a chain recreated since, or another pinned
# guest) does not count: deploy again and record the new transaction.
if [[ -s "$deployment_evidence" ]] \
   && jq -e --arg program "$ESCROW_PROGRAM_ID" '.preflight.image_id == $program
        and (.transaction_hash | test("^[0-9a-f]{64}$"))' "$deployment_evidence" >/dev/null 2>&1; then
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

# ── 3. bootstrap manifest (what the Nodes read) ────────────────────────────
genesis_block_hash="$(jq -r '.preflight.genesis_block_hash' "$deployment_evidence")"
[[ "$genesis_block_hash" =~ ^[0-9a-f]{64}$ ]] || fail "genesis block hash unavailable"
market_bootstrap_manifest="$MARKET_ROOT/market-bootstrap.env"
cat >"$market_bootstrap_manifest" <<EOF
M3_POC_LEZ_ESCROW_PROGRAM_ID=$ESCROW_PROGRAM_ID
M3_POC_LEZ_AUTH_TRANSFER_PROGRAM_ID=$AUTH_TRANSFER_PROGRAM_ID
M3_POC_LEZ_GENESIS_BLOCK_HASH=$genesis_block_hash
M3_POC_LEZ_DEPLOYMENT_TRANSACTION_ID=$deployment_tx
EOF
chmod 0600 "$market_bootstrap_manifest"

echo "market bootstrap complete"
echo "  bootstrap manifest: $market_bootstrap_manifest"
