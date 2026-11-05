#!/bin/bash
# lez-wallet-signer --socket <path> --private-key-file <key> --sequencer-url <url>
# A role's LEZ signer as the Logos lez_core wallet module: the role's key is
# imported into a throwaway wallet, and an owner-only unix socket answers one
# signing request per connection for that one account (the sidecar's
# WalletModuleSigner): a JSON line {"account_id", "message_hash"} gets the
# wallet's {"signature", "public_key"} over that hash (lez_core
# sign_public_message_hash), or {"error"}. The wallet builds and submits
# nothing here; the sidecar plans, records and submits every transaction.
set -euo pipefail
# shellcheck source=wallet-lib.sh
source /usr/local/lib/lez-wallet-lib.sh

if [[ "${1:-}" == --serve-one ]]; then
  # One connection (socat EXEC): one request line in, one answer line out.
  account="$2"
  reply() { jq -cn "$@"; }
  IFS= read -r -t 10 line || { reply '{error: "no request"}'; exit 0; }
  requested="$(jq -er '.account_id | select(type == "string")' <<<"$line" 2>/dev/null | tr 'A-F' 'a-f')" || requested=""
  hash="$(jq -er '.message_hash | select(type == "string")' <<<"$line" 2>/dev/null | tr 'A-F' 'a-f')" || hash=""
  [[ "$requested" == "$account" ]] || { reply '{error: "this signer holds another account"}'; exit 0; }
  [[ "$hash" =~ ^[0-9a-f]{64}$ ]] || { reply '{error: "message_hash must be 64 hex characters"}'; exit 0; }
  result="$(wallet_result "$(wallet_call sign_public_message_hash "str:$account" "str:$hash")")"
  if jq -e '(.signature | test("^[0-9a-f]{128}$")) and (.public_key | test("^[0-9a-f]{64}$"))' <<<"$result" >/dev/null 2>&1; then
    jq -c '{signature, public_key}' <<<"$result"
    echo "lez-wallet-signer: signed ${hash:0:16}… for ${account:0:16}…" >&2
  else
    reply --arg e "$(jq -r '.error // "the wallet did not sign"' <<<"$result" 2>/dev/null || echo "the wallet did not sign")" '{error: $e}'
  fi
  exit 0
fi

socket="" key="" sequencer=""
while (( $# )); do
  case "$1" in
    --socket) socket="$2" ;;
    --private-key-file) key="$2" ;;
    --sequencer-url) sequencer="$2" ;;
    *) wallet_fail "unknown argument: $1" ;;
  esac
  shift 2
done
for v in socket key sequencer; do [[ -n "${!v}" ]] || wallet_fail "--${v//_/-} is required"; done
wallet_key_file "$key"
wallet_start "$sequencer" "$key"
umask 077
rm -f "$socket"
echo "lez-wallet-signer: lez_core ${WALLET_VERSION:-?} signs for $WALLET_ACCOUNT on $socket" >&2
trap 'rm -f "$socket"; wallet_stop; exit 0' INT TERM
socat "UNIX-LISTEN:$socket,fork,mode=0600,unlink-early" "EXEC:/usr/local/bin/lez-wallet-signer --serve-one $WALLET_ACCOUNT" &
wait $!
