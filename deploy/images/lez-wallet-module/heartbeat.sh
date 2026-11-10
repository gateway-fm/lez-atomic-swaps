#!/bin/bash
# lez-wallet-heartbeat: the devnet finality heartbeat through the LEZ wallet
# module. The flags, the tick and the status file are lez-v02-heartbeat's:
# every --interval-seconds one transfer of one LEZ unit from the sender (the
# key file's account, imported into lez_core) to --recipient-account-id, then
# the sequencer tip and the indexer's finalized height; a stall is finality
# flat for --stall-seconds while the tip moves (one loud line, repeated every
# five minutes while it lasts, and `stalled: true` in --status-file, which the
# devnet's runners read). lez_core signs and submits the transfer
# (transfer_public, the authenticated transfer program); the heartbeat checks
# that program against the bootstrap's before it starts.
set -uo pipefail
# shellcheck source=wallet-lib.sh
source /usr/local/lib/lez-wallet-lib.sh

sequencer="" indexer="" key="" recipient="" program="" interval=20 stall=120 status=""
while (( $# )); do
  case "$1" in
    --sequencer-url) sequencer="$2" ;;
    --indexer-url) indexer="$2" ;;
    --private-key-file) key="$2" ;;
    --recipient-account-id) recipient="$2" ;;
    --authenticated-transfer-program-id) program="$2" ;;
    --interval-seconds) interval="$2" ;;
    --stall-seconds) stall="$2" ;;
    --status-file) status="$2" ;;
    *) wallet_fail "unknown argument: $1" ;;
  esac
  shift 2
done
for v in sequencer indexer key recipient program status; do
  [[ -n "${!v}" ]] || wallet_fail "--${v//_/-} is required"
done
(( interval >= 20 && interval <= 600 )) || wallet_fail "interval must be 20..=600 seconds (the inclusion wait is 15 s)"
(( stall >= 30 && stall <= 3600 )) || wallet_fail "stall threshold must be 30..=3600 seconds"
recipient="${recipient,,}"; program="${program,,}"
[[ "$recipient" =~ ^[0-9a-f]{64}$ ]] || wallet_fail "invalid recipient account id"
wallet_key_file "$key"
[[ "$(program_id_hex "$sequencer" authenticated_transfer)" == "$program" ]] ||
  wallet_fail "the sequencer's authenticated transfer program is not $program"

wallet_start "$sequencer" "$key"
sender="$WALLET_ACCOUNT"
[[ "$sender" != "$recipient" ]] || wallet_fail "the recipient must differ from the sender"
echo "heartbeat: one unit from $sender to $recipient every $interval s; a stall is finality flat for $stall s while the tip moves (lez_core wallet module)" >&2
trap 'wallet_stop; exit 0' INT TERM

tick=0 last_finalized="" moved_at="$(date +%s)" tip_when_moved="" stalled_since="" last_stall_line=0
one_unit="$(le16 1)"
while :; do
  tick=$((tick + 1))
  # The transaction: balance first (a sender with nothing samples without one),
  # one submission through the wallet, a bounded wait for inclusion.
  tx="" included=null inclusion_ms=null
  balance="$(wallet_result "$(wallet_call get_balance "str:$sender" true)")"
  if [[ ! "$balance" =~ ^[0-9]+$ ]]; then
    echo "heartbeat: sender account unavailable" >&2; balance=null
  elif [[ "$balance" == 0 ]]; then
    echo "heartbeat: the sender holds no LEZ; sampling without a transaction" >&2
  else
    submitted="$(date +%s%3N)"
    result="$(wallet_result "$(wallet_call transfer_public "str:$sender" "str:$recipient" "str:$one_unit")")"
    if [[ "$(jq -r '.success' <<<"$result" 2>/dev/null)" == true ]]; then
      tx="$(jq -r '.tx_hash' <<<"$result" | tr 'A-F' 'a-f')"; included=false
      while (( $(date +%s%3N) - submitted < 15000 )); do
        found="$(rpc "$sequencer" getTransaction "[\"$tx\"]" 2>/dev/null)"
        if [[ -n "$found" && "$found" != null ]]; then included=true; inclusion_ms=$(( $(date +%s%3N) - submitted )); break; fi
        sleep 0.5
      done
    else
      echo "heartbeat: submit refused: $(jq -r '.error // empty' <<<"$result" 2>/dev/null); a fresh nonce next tick" >&2
      included=false
    fi
  fi
  tip="$(rpc "$sequencer" getLastBlockId '[]' 2>/dev/null)"; [[ "$tip" =~ ^[0-9]+$ ]] || tip=""
  finalized="$(rpc "$indexer" getLastFinalizedBlockId '[]' 2>/dev/null)"; [[ "$finalized" =~ ^[0-9]+$ ]] || finalized=""
  now="$(date +%s)"
  if [[ -n "$finalized" && "$finalized" != "$last_finalized" ]]; then
    last_finalized="$finalized"; moved_at="$now"; tip_when_moved="$tip"
    if [[ -n "$stalled_since" ]]; then
      echo "heartbeat: finality moves again at $finalized after $(( now - stalled_since )) s" >&2
      stalled_since=""; last_stall_line=0
    fi
  fi
  flat=$(( now - moved_at ))
  if (( flat >= stall )) && [[ -n "$tip" && -n "$tip_when_moved" ]] && (( tip > tip_when_moved )); then
    [[ -n "$stalled_since" ]] || stalled_since="$now"
    if (( last_stall_line == 0 || now - last_stall_line >= 300 )); then
      echo "heartbeat: STALL finalized ${last_finalized:-0} flat $flat s, tip $tip (was $tip_when_moved when finality last moved)" >&2
      last_stall_line="$now"
    fi
  fi
  case "$included" in
    true) outcome="included in $inclusion_ms ms" ;;
    false) [[ -n "$tx" ]] && outcome="not included within 15 s" || outcome="not sent" ;;
    *) outcome="not sent" ;;
  esac
  echo "heartbeat: tick $tick tx ${tx:--} ($outcome) tip ${tip:--} finalized ${finalized:--} (flat $flat s)${stalled_since:+ STALLED}" >&2
  jq -n --argjson tick "$tick" --argjson at "$now" --arg tip "$tip" --arg finalized "$finalized" \
    --argjson moved "$moved_at" --argjson flat "$flat" --arg since "$stalled_since" --arg tx "$tx" \
    --argjson included "$included" --argjson ms "$inclusion_ms" --arg balance "$balance" '{
      schema_version: 1, tick: $tick, at_unix: $at,
      tip: ($tip | if . == "" then null else tonumber end),
      finalized: ($finalized | if . == "" then null else tonumber end),
      finality_moved_at_unix: $moved, finality_flat_seconds: $flat,
      stalled: ($since != ""), stalled_since_unix: ($since | if . == "" then null else tonumber end),
      last_transaction_id: ($tx | if . == "" then null else . end),
      last_transaction_included: (if $tx == "" and $included == false then false else $included end),
      last_inclusion_ms: $ms,
      sender_balance: ($balance | if . == "null" or . == "" then null else tonumber end)
    }' >"$status.tmp" && chmod 0644 "$status.tmp" && mv "$status.tmp" "$status" ||
    echo "heartbeat: status file not written" >&2
  sleep "$interval"
done
