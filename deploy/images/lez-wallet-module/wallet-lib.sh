#!/bin/bash
# Shared by lez-wallet-vault-claim and lez-wallet-heartbeat: the LEZ wallet as
# the Logos lez_core module in a headless logoscore. Each run gets a throwaway
# wallet (fresh storage and password under $TMPDIR) holding one imported
# account, so no wallet state outlives the container and the key file stays
# the only copy of the key.
L=/opt/logoscore/AppRun

wallet_fail() { echo "lez-wallet: $*" >&2; exit 2; }

# wallet_key_file <path>: the owner-only regular file with 64 hex characters
# that the tools it replaces accept.
wallet_key_file() {
  local path="$1" mode
  [[ -f "$path" && ! -L "$path" ]] || wallet_fail "private key must be a regular file: $path"
  mode="$(stat -c '%a' "$path")"
  (( (8#$mode & 8#077) == 0 )) || wallet_fail "private key file must not be accessible by group or others: $path"
  [[ "$(stat -c '%h' "$path")" == 1 ]] || wallet_fail "private key file must have one link: $path"
  [[ "$(tr -d '[:space:]' < "$path")" =~ ^[0-9a-fA-F]{64}$ ]] || wallet_fail "private key file must contain exactly 64 hex characters: $path"
}

# wallet_call <method> [args…]: one lez_core call; prints the call's JSON.
wallet_call() { "$L" call lez_core "$@" 2>/dev/null; }

# wallet_result <json>: the call's result, or empty when the call failed.
wallet_result() { jq -r 'select(.status == "ok") | .result | if type == "string" then . else tojson end' <<<"$1"; }

# wallet_start <sequencer url> <key file>: the daemon, the module, a new wallet
# on that sequencer and the key imported; sets WALLET_ACCOUNT (lower-case hex)
# and WALLET_VERSION.
wallet_start() {
  local sequencer="$1" key="$2" out before after
  WALLET_DIR="$(mktemp -d "${TMPDIR:-/tmp}/lez-wallet.XXXXXX")"
  "$L" daemon --modules-dir /opt/lez-modules --persistence-path "$WALLET_DIR/logoscore" >"$WALLET_DIR/daemon.log" 2>&1 &
  WALLET_DAEMON=$!
  trap wallet_stop EXIT
  for _ in $(seq 1 60); do
    out="$("$L" load-module lez_core 2>&1 || true)"
    [[ "$out" == *'"status":"ok"'* ]] && break
    sleep 1
  done
  [[ "$out" == *'"status":"ok"'* ]] || wallet_fail "lez_core did not load: $out"
  # The module's own version (its manifest, e.g. 0.4.1-import.1); lez_core's
  # version() call answers a constant that does not follow the releases.
  # shellcheck disable=SC2034 # read by the tools that source this library
  WALLET_VERSION="$(jq -r '.version // empty' <<<"$out" 2>/dev/null)"
  jq -n --arg s "$sequencer" '{sequencers: [{sequencer_addr: $s, basic_auth: null}],
    seq_poll_timeout: "12s", seq_tx_poll_max_blocks: 5, seq_poll_max_retries: 5,
    seq_block_poll_max_amount: 100}' >"$WALLET_DIR/wallet_config.json"
  out="$(wallet_call create_new "str:$WALLET_DIR/wallet_config.json" "str:$WALLET_DIR/storage.json" \
    "str:$WALLET_DIR/statistics.json" "str:$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')")"
  [[ -n "$(wallet_result "$out")" ]] || wallet_fail "create_new failed"
  before="$(wallet_result "$(wallet_call list_accounts)" | jq -r '.[] | select(.is_public) | .account_id' | sort)"
  out="$(wallet_result "$(wallet_call import_public_account "str:$(tr -d '[:space:]' < "$key")")")"
  [[ "$out" == 0 ]] || wallet_fail "import_public_account failed (wallet FFI error ${out:-none})"
  after="$(wallet_result "$(wallet_call list_accounts)" | jq -r '.[] | select(.is_public) | .account_id' | sort)"
  WALLET_ACCOUNT="$(comm -13 <(echo "$before") <(echo "$after") | tr 'A-F' 'a-f')"
  [[ "$WALLET_ACCOUNT" =~ ^[0-9a-f]{64}$ ]] || wallet_fail "the imported account is not in the wallet"
}

wallet_stop() {
  [[ -n "${WALLET_DAEMON:-}" ]] && { kill -INT "$WALLET_DAEMON" 2>/dev/null; wait "$WALLET_DAEMON" 2>/dev/null; }
  [[ -n "${WALLET_DIR:-}" ]] && rm -rf "$WALLET_DIR"
  WALLET_DAEMON=""; WALLET_DIR=""
}

# le16 <decimal>: the 16-byte little-endian hex lez_core takes for an amount.
le16() {
  local n="$1" out=""
  [[ "$n" =~ ^[0-9]{1,18}$ ]] || wallet_fail "amount out of range: $n"
  for _ in $(seq 1 16); do out+="$(printf '%02x' $(( n & 255 )))"; n=$(( n >> 8 )); done
  printf '%s' "$out"
}

# rpc <url> <method> <params json>: one JSON-RPC call; prints the result.
rpc() {
  curl -sf -m "${RPC_TIMEOUT:-10}" -H 'content-type: application/json' \
    -d "$(jq -cn --arg m "$2" --argjson p "$3" '{jsonrpc: "2.0", id: 1, method: $m, params: $p}')" "$1" |
    jq -c '.result'
}

# program_id_hex <sequencer url> <name>: the sequencer's builtin program id as
# the 64 hex characters the bootstrap manifest records (u32 words, little-endian).
program_id_hex() {
  local word out=""
  for word in $(rpc "$1" getProgramIds '[]' | jq -r --arg n "$2" '.[$n][]'); do
    out+="$(printf '%08x' "$word" | sed -E 's/(..)(..)(..)(..)/\4\3\2\1/')"
  done
  printf '%s' "$out"
}
