#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  printf '%s\n' "M6 ZEC service runner contract failed: $*" >&2
  exit 1
}

readonly runner="scripts/run-m2-taker-sells-lez-poc.sh"
readonly wrapper="scripts/run-m6-zec-taker-service-poc.sh"
readonly refund_wrapper="scripts/run-m6-zec-taker-service-refund-poc.sh"

[[ -x "$wrapper" && ! -L "$wrapper" ]] || fail 'wrapper is not a regular executable'
[[ -x "$refund_wrapper" && ! -L "$refund_wrapper" ]] ||
  fail 'refund wrapper is not a regular executable'

bridge_timeout_ms="$(sed -nE '
  s/^const DEFAULT_LOCAL_BRIDGE_REQUEST_TIMEOUT_MILLIS: u64 = ([0-9_]+);$/\1/p
' crates/zec-reference-actor/src/config.rs)"
service_action_timeout_ms="$(sed -nE '
  s/^readonly M6_SERVICE_ACTION_TIMEOUT_MS=([0-9]+)$/\1/p
' "$runner")"
refund_supervisor_timeout_ms="$(sed -nE '
  s/^readonly M6_REFUND_SUPERVISOR_ATTEMPT_TIMEOUT_MS=([0-9]+)$/\1/p
' "$runner")"
corridor_seconds="$(sed -nE '
  /^  refund\)/,/^    ;;/ s/^    MAX_CORRIDOR_SECONDS=([0-9]+)$/\1/p
' "$runner")"
bridge_timeout_ms="$(tr -d '_' <<<"$bridge_timeout_ms")"
[[ "$bridge_timeout_ms" =~ ^[0-9]+$
  && "$service_action_timeout_ms" =~ ^[0-9]+$
  && "$refund_supervisor_timeout_ms" =~ ^[0-9]+$
  && "$corridor_seconds" =~ ^[0-9]+$ ]] ||
  fail 'M6 Refund timeout hierarchy is unavailable'
(( bridge_timeout_ms == 60000 )) ||
  fail 'local actor bridge does not cover the measured three-phase historical observation'
(( refund_supervisor_timeout_ms > bridge_timeout_ms )) ||
  fail 'Refund supervisor attempt does not dominate the actor bridge'
(( service_action_timeout_ms > refund_supervisor_timeout_ms )) ||
  fail 'service action caller does not dominate the Refund supervisor attempt'
(( service_action_timeout_ms < corridor_seconds * 1000 )) ||
  fail 'service action caller is not bounded by the unchanged corridor'
rg -Fq -- 'start_m5_supervisor_only_daemon "$M6_REFUND_SUPERVISOR_ATTEMPT_TIMEOUT_MS"' \
  "$runner" || fail 'Refund restart does not use its scoped supervisor budget'

bash -n "$runner" "$wrapper" "$refund_wrapper"

rg -Fq 'export M6_TAKER_SERVICE_MODE=1' "$wrapper" || fail 'wrapper does not select M6 service mode'
rg -Fq 'export M6_ZEC_JOURNEY=claim' "$wrapper" || fail 'claim wrapper does not fix the claim journey'
rg -Fq 'exec ./scripts/run-m2-taker-sells-lez-poc.sh "$@"' "$wrapper" || fail 'wrapper bypasses the proven local corridor'
rg -Fq 'export M6_TAKER_SERVICE_MODE=1' "$refund_wrapper" ||
  fail 'refund wrapper does not select M6 service mode'
rg -Fq 'export M6_ZEC_JOURNEY=refund' "$refund_wrapper" ||
  fail 'refund wrapper does not fix the refund journey'
rg -Fq 'exec ./scripts/run-m2-taker-sells-lez-poc.sh "$@"' "$refund_wrapper" ||
  fail 'refund wrapper bypasses the proven local corridor'

handler_source="$(sed -n '/^handle_zcash_submission() {$/,/^}$/p' "$runner")"
[[ -n "$handler_source" ]] || fail 'Zcash submission handler is missing'

# Execute the production function with inert chain effects. A service-owned
# claim must stop after its one Zcash effect and must not be reclassified as
# the earlier LEZ revealing claim.
eval "$handler_source"
M6_TAKER_SERVICE_MODE=1
lez_revealing_claim_seen=1
expected_zcash_claimant_role=taker
expected_zcash_funder_role=maker
zcash_claim_mined=0
m6_zcash_claim_txid="$(printf 'a%.0s' {1..64})"
zcash_claim_submitter=''
lez_revealing_claim_submitter=maker
mine_blocks() { [[ "$1" == followup-claim && "$2" == 1 ]]; }

claim='{"schema_version":1,"action":"claim","was_replay":false,"m6_first_claim":true}'
handle_zcash_submission taker "$claim" || fail 'service claim fell through into LEZ reveal validation'
[[ "$zcash_claim_mined" == 1 && "$zcash_claim_submitter" == taker ]] ||
  fail 'service claim did not record exactly one Taker Zcash effect'
[[ "$lez_revealing_claim_seen" == 1 && "$lez_revealing_claim_submitter" == maker ]] ||
  fail 'service claim mutated prior LEZ-reveal evidence'

handoff_source="$(sed -n '/^apply_m6_refund_parent_handoff() {$/,/^}$/p' "$runner")"
[[ -n "$handoff_source" ]] || fail 'Refund parent-handoff function is missing'
eval "$handoff_source"
m6_refund_admitted=0
m6_refund_generation=''
m6_lez_refund_txid=''
m6_lez_refund_finalized=0
m6_lez_refund_start_tip=''
m6_maker_supervisor_suppressed=1
m6_maker_supervisor_restarted=0
m6_test_supervisor_starts=0
start_m6_refund_maker_supervisor() {
  m6_test_supervisor_starts=$((m6_test_supervisor_starts + 1))
  m6_maker_supervisor_suppressed=0
  m6_maker_supervisor_restarted=1
}

if apply_m6_refund_parent_handoff 'not-json' >/dev/null 2>&1; then
  fail 'malformed child output was accepted'
fi
if apply_m6_refund_parent_handoff '{"m6_refund_parent_handoff":true,
  "m6_refund_admitted":true,"m6_refund_generation":7.5,
  "m6_lez_refund_txid":"","m6_lez_refund_finalized":false,
  "m6_lez_refund_start_tip":88}' >/dev/null 2>&1; then
  fail 'fractional Refund generation was accepted'
fi
if apply_m6_refund_parent_handoff '{"m6_refund_parent_handoff":true,
  "m6_refund_admitted":true,"m6_refund_generation":7,
  "m6_lez_refund_txid":"","m6_lez_refund_finalized":false,
  "m6_lez_refund_start_tip":88.5}' >/dev/null 2>&1; then
  fail 'fractional Refund start tip was accepted'
fi
apply_m6_refund_parent_handoff '{"state":"active"}'
[[ "$m6_refund_admitted" == 0 && "$m6_test_supervisor_starts" == 0 ]] ||
  fail 'non-Refund service output mutated parent state'

pending_handoff='{"m6_refund_parent_handoff":true,"m6_refund_admitted":true,
  "m6_refund_generation":7,"m6_lez_refund_txid":"",
  "m6_lez_refund_finalized":false,"m6_lez_refund_start_tip":88}'
apply_m6_refund_parent_handoff "$pending_handoff"
[[ "$m6_refund_admitted" == 1 && "$m6_refund_generation" == 7
  && "$m6_lez_refund_start_tip" == 88 && "$m6_lez_refund_finalized" == 0
  && "$m6_test_supervisor_starts" == 0 ]] ||
  fail 'pending Refund handoff was not restored in the parent'

refund_txid="$(printf 'b%.0s' {1..64})"
final_handoff="$(jq -nc --arg txid "$refund_txid" '
  {m6_refund_parent_handoff:true,m6_refund_admitted:true,
    m6_refund_generation:7,m6_lez_refund_txid:$txid,
    m6_lez_refund_finalized:true,m6_lez_refund_start_tip:88}
')"
apply_m6_refund_parent_handoff "$final_handoff"
[[ "$m6_lez_refund_txid" == "$refund_txid" && "$m6_lez_refund_finalized" == 1
  && "$m6_maker_supervisor_restarted" == 1 && "$m6_test_supervisor_starts" == 1 ]] ||
  fail 'finalized Refund handoff did not restart parent-owned Maker authority once'
apply_m6_refund_parent_handoff "$final_handoff"
[[ "$m6_test_supervisor_starts" == 1 ]] ||
  fail 'exact finalized Refund handoff replay restarted Maker authority twice'

regressive_handoff="$(jq -nc --argjson pending "$pending_handoff" '$pending')"
if apply_m6_refund_parent_handoff "$regressive_handoff" >/dev/null 2>&1; then
  fail 'finalized Refund parent state accepted a regression'
fi
replacement_handoff="$(jq -nc --argjson final "$final_handoff" '$final + {m6_refund_generation:8}')"
if apply_m6_refund_parent_handoff "$replacement_handoff" >/dev/null 2>&1; then
  fail 'admitted Refund parent state accepted a replacement generation'
fi
replacement_txid="$(printf 'c%.0s' {1..64})"
replacement_handoff="$(jq -nc --argjson final "$final_handoff" \
  --arg txid "$replacement_txid" '
  $final + {m6_lez_refund_txid:$txid}
')"
if apply_m6_refund_parent_handoff "$replacement_handoff" >/dev/null 2>&1; then
  fail 'finalized Refund parent state accepted a replacement transaction'
fi

transient_source="$(sed -n '/^m6_refund_replay_is_transient() {$/,/^}$/p' "$runner")"
[[ -n "$transient_source" ]] || fail 'Refund replay transient classifier is missing'
eval "$transient_source"
valid_transient='{"jsonrpc":"2.0","id":"m6-refund-replay","error":{"code":-32010,
  "message":"Taker dependency unavailable",
  "data":{"category":"taker_action_execution_unavailable"}}}'
m6_refund_replay_is_transient "$valid_transient" ||
  fail 'documented Refund reconciliation transient was rejected'
if m6_refund_replay_is_transient '{"jsonrpc":"2.0","id":"m6-refund-replay",
  "error":{"code":-32010,
  "message":"Taker dependency unavailable",
  "data":"taker_action_execution_unavailable"}}'; then
  fail 'scalar Refund reconciliation error envelope was accepted'
fi
if m6_refund_replay_is_transient '{"jsonrpc":"2.0","id":"m6-refund-replay",
  "error":{"code":-32010,
  "message":"Taker dependency unavailable",
  "data":{"category":"different"}}}'; then
  fail 'wrong Refund reconciliation transient category was accepted'
fi
if m6_refund_replay_is_transient '{"jsonrpc":"1.0","id":"wrong",
  "error":{"code":-32010,"message":"Taker dependency unavailable",
  "data":{"category":"taker_action_execution_unavailable"}}}'; then
  fail 'wrong Refund replay version and ID were accepted'
fi
if m6_refund_replay_is_transient '{"jsonrpc":"2.0","id":"m6-refund-replay",
  "error":{"code":-32010,"message":"Taker dependency unavailable",
  "data":{"category":"taker_action_execution_unavailable","extra":true}},
  "extra":true}'; then
  fail 'Refund replay envelope with extra fields was accepted'
fi

quiesce_source="$(sed -n '/^m6_refund_waits_for_maker_recovery() {$/,/^}$/p' "$runner")"
[[ -n "$quiesce_source" ]] ||
  fail 'post-finality Taker reconciliation does not yield to Maker recovery'
eval "$quiesce_source"
emit_handoff_source="$(sed -n '/^emit_m6_refund_parent_handoff() {$/,/^}$/p' "$runner")"
[[ -n "$emit_handoff_source" ]] || fail 'Refund parent handoff emitter is missing'
eval "$emit_handoff_source"

m6_refund_admitted=1
m6_lez_refund_finalized=1
m6_maker_supervisor_restarted=1
m6_zcash_refund_mined=0
m6_refund_generation=2
m6_lez_refund_txid=05d1f9cf1abb7a25594a0da5044a5c6bf25e8b35093d7c6a38290cbb33f79f6a
m6_lez_refund_start_tip=151
m6_refund_waits_for_maker_recovery ||
  fail 'finalized LEZ Refund did not yield Taker reconciliation to Maker recovery'
for released in admitted finalized restarted zcash_mined; do
  m6_refund_admitted=1
  m6_lez_refund_finalized=1
  m6_maker_supervisor_restarted=1
  m6_zcash_refund_mined=0
  case "$released" in
    admitted) m6_refund_admitted=0 ;;
    finalized) m6_lez_refund_finalized=0 ;;
    restarted) m6_maker_supervisor_restarted=0 ;;
    zcash_mined) m6_zcash_refund_mined=1 ;;
  esac
  if m6_refund_waits_for_maker_recovery; then
    fail "Maker-recovery quiescence survived changed state: ${released}"
  fi
done
m6_refund_admitted=1
m6_lez_refund_finalized=1
m6_maker_supervisor_restarted=1
m6_zcash_refund_mined=0
handoff="$(emit_m6_refund_parent_handoff \
  '{"jsonrpc":"2.0","id":"m6-monitor","result":{"state":"refund_in_progress"}}')"
jq -e --arg tx "$m6_lez_refund_txid" '
  .state == "refund_in_progress" and .m6_refund_parent_handoff == true
  and .m6_refund_admitted == true and .m6_refund_generation == 2
  and .m6_lez_refund_txid == $tx and .m6_lez_refund_finalized == true
  and .m6_lez_refund_start_tip == 151
' <<<"$handoff" >/dev/null || fail 'quiescent Refund handoff changed parent state'

drive_refund_source="$(sed -n '/^drive_m6_taker_refund() {$/,/^}$/p' "$runner")"
[[ -n "$drive_refund_source" ]] || fail 'Refund drive function is missing'
eval "$drive_refund_source"
quiescence_root="$(mktemp -d)"
quiescence_calls="$quiescence_root/calls"
evidence_dir="$quiescence_root/evidence"
taker_config="$quiescence_root/taker.json"
m5_delivery_offline="$quiescence_root/offline"
m5_maker_socket="$quiescence_root/maker.sock"
m5_chat_socket="$quiescence_root/chat.sock"
m5_delivery_directory="$quiescence_root/delivery"
mkdir -m 0700 "$evidence_dir" "$m5_delivery_offline"
printf '%s\n' '{}' >"$taker_config"
: >"$quiescence_calls"
actor_bin="$quiescence_root/actor"
printf '%s\n' '#!/usr/bin/env bash' \
  'printf '\''actor:%s\n'\'' "$*" >>"$M6_QUIESCENCE_CALLS"' \
  'exit 97' >"$actor_bin"
chmod 0500 "$actor_bin"
export M6_QUIESCENCE_CALLS="$quiescence_calls"
m6_service_rpc() {
  printf '%s\n' "rpc:$*" >>"$quiescence_calls"
  return 97
}
m5_transport_cutover_complete=1
quiescent_output="$(drive_m6_taker_refund 77 \
  '{"jsonrpc":"2.0","id":"m6-monitor","result":{"state":"refund_in_progress"}}' \
  refund_in_progress)" || fail 'post-finality quiescence did not return successfully'
[[ ! -s "$quiescence_calls" ]] ||
  fail 'post-finality quiescence invoked the Taker actor or service RPC'
jq -e --arg tx "$m6_lez_refund_txid" '
  .state == "refund_in_progress" and .m6_refund_parent_handoff == true
  and .m6_refund_admitted == true and .m6_refund_generation == 2
  and .m6_lez_refund_txid == $tx and .m6_lez_refund_finalized == true
  and .m6_lez_refund_start_tip == 151
' <<<"$quiescent_output" >/dev/null ||
  fail 'quiescent Refund drive did not preserve the monitor handoff envelope'
rm -rf "$quiescence_root"

maker_lock_source="$(sed -n '/^reconcile_m6_suppressed_maker_lock() {$/,/^}$/p' "$runner")"
[[ -n "$maker_lock_source" ]] || fail 'suppressed Maker-lock reconciliation is missing'
eval "$maker_lock_source"
maker_lock_root="$(mktemp -d)"
trap 'rm -rf "$maker_lock_root"' EXIT
actor_bin="$maker_lock_root/actor"
actor_calls="$maker_lock_root/actor-calls"
rpc_calls="$maker_lock_root/rpc-calls"
timeout_calls="$maker_lock_root/timeout-calls"
budget_calls="$maker_lock_root/budget-calls"
maker_config="$maker_lock_root/maker.json"
evidence_dir="$maker_lock_root/evidence"
mkdir -m 0700 "$evidence_dir"
printf '%s\n' '{}' >"$maker_config"
printf '%s\n' '#!/usr/bin/env bash' \
  'printf '\''%s\n'\'' '\''{"schema_version":1,"role":"maker","command":"drive","outcome":"projected","operation":"maker_lock","phase":"both_legs_locked","revision":2,"next_action":"claim_lez"}'\''' \
  >"$actor_bin"
chmod 0500 "$actor_bin"
mv "$actor_bin" "$maker_lock_root/actor-output"
printf '%s\n' '#!/usr/bin/env bash' \
  'set -euo pipefail' \
  '[[ "$#" -eq 3 && "$1" == "--config" && "$2" == "$EXPECTED_MAKER_CONFIG" && "$3" == "drive" ]]' \
  'echo "$*" >>"$ACTOR_CALLS"' \
  'exec "$ACTOR_OUTPUT" "$@"' >"$actor_bin"
chmod 0500 "$actor_bin"
export ACTOR_CALLS="$actor_calls"
export ACTOR_OUTPUT="$maker_lock_root/actor-output"
export EXPECTED_MAKER_CONFIG="$maker_config"
ZEBRA_RPC_URL=http://127.0.0.1:1
m5_daemon_pid=''
m5_daemon_start_ticks=''
m5_transport_cutover_complete=1
m5_maker_socket="$maker_lock_root/maker.sock"
m5_chat_socket="$maker_lock_root/chat.sock"
m6_maker_supervisor_suppressed=0
zcash_fund_mined=2
rpc_mode=normal
bounded_actor_timeout() {
  [[ "$1" == m6-maker-lock-reconciliation ]] || return 1
  printf '%s\n' '1.000'
}
timeout() {
  [[ "$#" -eq 6 && "$1" == '--signal=KILL' && "$2" == '1.000s'
    && "$3" == "$actor_bin" && "$4" == '--config'
    && "$5" == "$maker_config" && "$6" == drive ]] || return 1
  printf '%s\n' "$*" >>"$timeout_calls"
  shift 2
  "$@"
}
remaining_budget_milliseconds() {
  [[ "$1" == m6-maker-lock-reconciliation-after ]] || return 1
  printf '%s\n' "$1" >>"$budget_calls"
  printf '%s\n' '1000'
}
rpc() {
  [[ "$1" == "$ZEBRA_RPC_URL" ]] || return 1
  local id method
  id="$(jq -er '.id' <<<"$2")" || return
  method="$(jq -er '.method' <<<"$2")" || return
  printf '%s:%s\n' "$id" "$method" >>"$rpc_calls"
  case "$id:$method" in
    m6-maker-lock-before-tip:getblockcount)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":106}'
      ;;
    m6-maker-lock-before-mempool:getrawmempool)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":[]}'
      ;;
    m6-maker-lock-after-tip:getblockcount)
      if [[ "$rpc_mode" == changed_tip ]]; then
        printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":107}'
      else
        printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":106}'
      fi
      ;;
    m6-maker-lock-after-mempool:getrawmempool)
      if [[ "$rpc_mode" == dirty_mempool ]]; then
        printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":["unexpected"]}'
      else
        printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":[]}'
      fi
      ;;
    *)
      return 1
      ;;
  esac
}

: >"$actor_calls"
: >"$rpc_calls"
: >"$timeout_calls"
: >"$budget_calls"
if reconcile_m6_suppressed_maker_lock >/dev/null 2>&1; then
  fail 'Maker-lock reconciliation ran without suppressed authority'
fi
[[ ! -s "$actor_calls" && ! -s "$rpc_calls"
  && ! -s "$timeout_calls" && ! -s "$budget_calls" ]] ||
  fail 'unsuppressed Maker-lock reconciliation performed I/O'

m6_maker_supervisor_suppressed=1
m5_daemon_pid=123
m5_daemon_start_ticks=456
if reconcile_m6_suppressed_maker_lock >/dev/null 2>&1; then
  fail 'Maker-lock reconciliation ran with live daemon authority'
fi
[[ ! -s "$actor_calls" && ! -s "$rpc_calls"
  && ! -s "$timeout_calls" && ! -s "$budget_calls" ]] ||
  fail 'live-daemon Maker-lock reconciliation performed I/O'
m5_daemon_pid=''
m5_daemon_start_ticks=''

assert_reconciliation_calls() {
  mapfile -t actual_rpc_calls <"$rpc_calls"
  expected_rpc_calls=(
    'm6-maker-lock-before-tip:getblockcount'
    'm6-maker-lock-before-mempool:getrawmempool'
    'm6-maker-lock-after-tip:getblockcount'
    'm6-maker-lock-after-mempool:getrawmempool'
  )
  [[ "${actual_rpc_calls[*]}" == "${expected_rpc_calls[*]}" ]] ||
    fail 'Maker-lock reconciliation made unexpected or reordered RPC calls'
  (( $(wc -l <"$actor_calls") == 1 )) ||
    fail 'Maker-lock reconciliation did not invoke exactly one actor command'
  [[ "$(sed -n '1p' "$actor_calls")" == "--config $maker_config drive" ]] ||
    fail 'Maker-lock reconciliation actor argv changed'
  [[ "$(sed -n '1p' "$timeout_calls")" == "--signal=KILL 1.000s $actor_bin --config $maker_config drive"
    && "$(wc -l <"$timeout_calls")" -eq 1 ]] ||
    fail 'Maker-lock reconciliation did not use one exact hard timeout'
  [[ "$(sed -n '1p' "$budget_calls")" == m6-maker-lock-reconciliation-after
    && "$(wc -l <"$budget_calls")" -eq 1 ]] ||
    fail 'Maker-lock reconciliation did not recheck its corridor budget'
}

rpc_mode=changed_tip
: >"$actor_calls"
: >"$rpc_calls"
: >"$timeout_calls"
: >"$budget_calls"
if reconcile_m6_suppressed_maker_lock >/dev/null 2>&1; then
  fail 'Maker-lock reconciliation accepted a changed Zebra tip'
fi
assert_reconciliation_calls

rpc_mode=dirty_mempool
: >"$actor_calls"
: >"$rpc_calls"
: >"$timeout_calls"
: >"$budget_calls"
if reconcile_m6_suppressed_maker_lock >/dev/null 2>&1; then
  fail 'Maker-lock reconciliation accepted a nonempty Zebra mempool'
fi
assert_reconciliation_calls

rpc_mode=normal
: >"$actor_calls"
: >"$rpc_calls"
: >"$timeout_calls"
: >"$budget_calls"
reconcile_m6_suppressed_maker_lock || fail 'suppressed Maker lock did not reconcile'
assert_reconciliation_calls
jq -e '
  .schema_version == 1 and .result == "passed"
  and .authority == "direct_observation_only"
  and .actor.role == "maker" and .actor.command == "drive"
  and .actor.operation == "maker_lock" and .actor.outcome == "projected"
  and .actor.phase == "both_legs_locked" and .actor.next_action == "claim_lez"
  and .actor_timeout_seconds == "1.000"
  and .before.tip.error == null and .before.tip.result == 106
  and .after.tip.error == null and .after.tip.result == 106
  and .before.tip.result == .after.tip.result
  and .before.mempool.error == null and .before.mempool.result == []
  and .after.mempool.error == null and .after.mempool.result == []
  and .before.mempool.result == .after.mempool.result
  and .zebra_tip_unchanged == true and .zebra_mempool_unchanged_empty == true
  and .new_chain_effect == false
' "$evidence_dir/m6-maker-lock-reconciliation.json" >/dev/null ||
  fail 'Maker-lock reconciliation evidence is incomplete'

required_markers=(
  'readonly M6_ZEC_JOURNEY="${M6_ZEC_JOURNEY:-claim}"'
  'readonly M6_SERVICE_QUERY_TIMEOUT_MS=15000'
  'readonly M6_SERVICE_ACTION_TIMEOUT_MS=90000'
  'readonly M6_REFUND_SUPERVISOR_ATTEMPT_TIMEOUT_MS=75000'
  'M6_ZEC_JOURNEY must be claim or refund'
  'MAX_CORRIDOR_SECONDS=300'
  'm6_claim_generation:$generation'
  'm6_zcash_claim_txid:$txid'
  'm6-zebra-mempool-before-claim.json'
  'm6-zebra-mempool-after-first-claim.json'
  'm6-zebra-mempool-after-claim-replay.json'
  'm6_claim_generation="$(jq -er'
  '.m6_claim_generation | numbers'
  '--argjson m6_taker_service_mode "$M6_TAKER_SERVICE_MODE"'
  'm6_taker_service_mode: ($m6_taker_service_mode == 1)'
  '"owner_taker_service"'
  'drive_m6_taker_refund()'
  'apply_m6_refund_parent_handoff()'
  'm6_refund_parent_handoff:true'
  'm6_lez_refund_start_tip:$start_tip'
  'apply_m6_refund_parent_handoff "$taker_output"'
  'm6_refund_replay_is_transient'
  'phase:"reconcile"'
  'taker_swap_refund_v1'
  'action:"refund"'
  'taker_action_conflict'
  '.error.data.category == "taker_action_conflict"'
  'm6-taker-service-refund-first.json'
  'm6-taker-service-refund-transients.ndjson'
  'm6-taker-service-refund-terminal-replay.json'
  'm6-taker-lez-submission-trace-before-terminal-replay.json'
  'm6-taker-lez-submission-trace-after-terminal-replay.json'
  'm6-zebra-zcash-refund-inclusion.json'
  'm6-zebra-terminal-replay-before.json'
  'm6-zebra-terminal-replay-after.json'
  'getblock'
  'getblockhash'
  'terminal:true'
  'exact_terminal_replay_has_no_new_chain_effect: true'
  'zcash_refund_inclusion_sha256'
  'taker_refund_terminal_replay_sha256'
  'taker_refund_terminal_no_effect_sha256'
  'm6-taker-service-refund-commit.json'
  'm6-taker-service-refund-replay.json'
  'm6-taker-service-refund-claim-exclusion.json'
  '"m6-refund-admission-${admission_attempt}" \'
  '"$refund_request" "$M6_SERVICE_ACTION_TIMEOUT_MS")"'
  '"m6-refund-replay-${round}" "$refund_request" "$M6_SERVICE_ACTION_TIMEOUT_MS"'
  'm6_taker_lez_refund_deadline_ms()'
  'wait_for_m6_lez_refund_window'
  'm6-taker-lez-refund-window.json'
  'm6-taker-lez-refund-finality.json'
  'm6-refund-maker-manual-action.json'
  'm6-zebra-mempool-zcash-refund.json'
  'm6_maker_supervisor_suppressed=1'
  'start_m6_refund_maker_supervisor'
  'reconcile_m6_suppressed_maker_lock'
  'm6-maker-lock-reconciliation.json'
  'maker_lock_reconciliation_sha256'
  'direct Taker drive crossed the M6 service terminal-action boundary'
  '--arg journey "$M6_ZEC_JOURNEY"'
  'journey: $journey'
  '"lez_refund_finalized"'
  '"zcash_refund_submitted_and_confirmed"'
)
for required in "${required_markers[@]}"; do
  rg -Fq -- "$required" "$runner" || fail "runner is missing replay evidence propagation: ${required}"
done

printf '%s\n' 'M6 ZEC service runner contract passed'
