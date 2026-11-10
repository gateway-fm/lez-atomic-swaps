#!/bin/bash
# lez-wallet-vault-claim: the market bootstrap's one-time vault claim through
# the LEZ wallet module (lez_core vault_claim). Takes the flags and writes the
# JSON fields of lez-v02-vault-claim-poc that market-bootstrap.sh and
# node-e2e.py read (allocation, transaction_id, submission.decision), so the
# bootstrap is unchanged. The claim is the official vault program's Claim of
# the allocation from the owner's vault, signed by the owner: the transaction
# the proof-of-concept tool built, now built and signed by the wallet.
set -euo pipefail
# shellcheck source=wallet-lib.sh
source /usr/local/lib/lez-wallet-lib.sh

role="" run_id="" request_id="" state_dir="" key="" sequencer="" chain_id="" escrow="" allocation=""
while (( $# )); do
  case "$1" in
    --role) role="$2" ;;
    --run-id) run_id="$2" ;;
    --request-id) request_id="$2" ;;
    --state-directory) state_dir="$2" ;;
    --private-key-file) key="$2" ;;
    --sequencer-url) sequencer="$2" ;;
    --chain-id) chain_id="$2" ;;
    --escrow-program-id) escrow="$2" ;;
    --allocation) allocation="$2" ;;
    *) wallet_fail "unknown argument: $1" ;;
  esac
  shift 2
done
for v in role run_id request_id state_dir key sequencer chain_id escrow allocation; do
  [[ -n "${!v}" ]] || wallet_fail "--${v//_/-} is required"
done
[[ "$allocation" =~ ^[1-9][0-9]*$ ]] || wallet_fail "allocation must be a positive integer: $allocation"
[[ -d "$state_dir" ]] || wallet_fail "state directory missing: $state_dir"
wallet_key_file "$key"

# The chain the caller means, before anything is signed.
channel="$(rpc "$sequencer" getChannelId '[]' | jq -r '.')"
[[ "$channel" == "$chain_id" ]] || wallet_fail "sequencer channel $channel is not the expected $chain_id"
tip_before="$(rpc "$sequencer" getLastBlockId '[]')"

wallet_start "$sequencer" "$key"
owner_before="$(wallet_result "$(wallet_call get_account_public "str:$WALLET_ACCOUNT")")"
jq -e 'type == "object"' <<<"$owner_before" >/dev/null 2>&1 || owner_before='{}'
result="$(wallet_result "$(wallet_call vault_claim "str:$WALLET_ACCOUNT" "str:$(le16 "$allocation")")")"
[[ "$(jq -r '.success' <<<"$result" 2>/dev/null)" == true ]] ||
  wallet_fail "vault_claim refused: $(jq -r '.error // empty' <<<"$result" 2>/dev/null || echo "$result")"
transaction="$(jq -r '.tx_hash' <<<"$result" | tr 'A-F' 'a-f')"
[[ "$transaction" =~ ^[0-9a-f]{64}$ ]] || wallet_fail "vault_claim returned no transaction hash"
version="$WALLET_VERSION"

jq -n --arg role "$role" --arg run_id "$run_id" --arg request_id "$request_id" \
  --arg chain_id "$chain_id" --arg escrow "$escrow" --argjson allocation "$allocation" \
  --arg transaction "$transaction" --argjson tip "$tip_before" --arg owner "$WALLET_ACCOUNT" \
  --argjson owner_before "$owner_before" --arg version "$version" '{
    schema: "lez-wallet-module-vault-claim-v1",
    role: $role, run_id: $run_id, request_id: $request_id,
    runtime: {chain_id: $chain_id, chain_id_source: "sequencer getChannelId",
              escrow_program_id: $escrow, escrow_program_id_source: "caller"},
    allocation: $allocation,
    transaction_id: $transaction,
    submission: {decision: "admitted"},
    signer: {kind: "lez_core wallet module", version: $version},
    before: {sequencer_tip: $tip, owner: ($owner_before + {account_id: $owner})}
  }'
