#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
readonly runner="scripts/run-m2-taker-sells-lez-poc.sh"
readonly handoff="scripts/run-m5-zec-chat-handoff.sh"

fail() {
  echo "M5 application contract failed: $*" >&2
  exit 1
}

[[ -x scripts/run-m5-zec-application-poc.sh ]] || fail "M5 wrapper is not executable"
rg -Fq 'export M5_APPLICATION_MODE=1' scripts/run-m5-zec-application-poc.sh ||
  fail "M5 wrapper does not force application mode"

for required in \
  'install -m 0500 "$actor_bin" "$m5_actor_program"' \
  'stat -c %h -- "$m5_actor_program"' \
  'sha256sum "$m5_actor_program"' \
  '--actor-program "$m5_actor_program"' \
  '--actor-program-sha256 "$m5_actor_program_sha256"'; do
  rg -Fq -- "$required" "$runner" ||
    fail "M5 runner is missing private actor deployment contract: ${required}"
done

for required in \
  '--zec-source-maker-config "$source_actors_root/maker/actor-config.json"' \
  '--zec-maker-actor-root "$actor_root"' \
  '--zec-actor-program "$actor_program"' \
  '--zec-actor-program-sha256 "$actor_program_sha256"' \
  'm5-queued-maker-actor.json' \
  'schedule_state' \
  'config_sha256' \
  'actor_program_sha256'; do
  rg -Fq -- "$required" "$handoff" ||
    fail "M5 handoff is missing queued actor contract: ${required}"
done

for required in 'actor_supervisor_enabled: false'; do
  rg -Fq -- "$required" "$handoff" ||
    fail "M5 handoff must return a queued actor before supervision: ${required}"
done

for required in \
  'ESCROW_PROGRAM_ID:-4d6590332948743c2db88a183755815354ef92560550cd206ac27bddeea12c82' \
  'M5_LEZ_GUEST_SHA256:-dc370bc34b432317730c51b49342760dbc675fca700e300b30b5fadefe5b7292' \
  'M5_LEZ_DEPLOYMENT_EVIDENCE_FILE' \
  'M5_LEZ_FINALITY_EVIDENCE_FILE' \
  'm5-lez-deployment.json' \
  'm5-lez-deployment-finality.json' \
  '.preflight.image_id == $program' \
  '.preflight.elf_sha256 == $guest' \
  '.preflight.rpc_url == $rpc' \
  '.preflight.channel_id == $channel' \
  'lez_escrow_program_id' \
  'lez_escrow_guest_sha256' \
  'lez_deployment_receipt_sha256' \
  'lez_deployment_finality_sha256' \
  'lez_deployment_transaction_hash' \
  'lez_deployment_inclusion_block_id' \
  'lez_deployment_inclusion_block_hash'; do
  rg -Fq -- "$required" "$runner" ||
    fail "M5 runner does not bind the current escrow deployment: ${required}"
done

for required in \
  'start_m5_full_supervised_daemon' \
  'start_m5_supervisor_only_daemon' \
  '--actor-supervisor' \
  '--actor-requeue-delay-seconds' \
  '--actor-failure-backoff-seconds' \
  'observe_m5_supervised_maker' \
  'm5-maker-supervisor-status.ndjson' \
  'm5-maker-supervisor-final.json' \
  'm5-maker-lock-intent.json' \
  'expected_zebra_txid' \
  'maker_effect_authority: "daemon_supervisor"' \
  'maker_daemon_alive: true' \
  'M5 Maker supervisor exited before terminal evidence publication' \
  '.schedule_state == "terminal"' \
  'concurrent_direct_maker_effects: false' \
  'maker_lock_intent_sha256' \
  'exact_funding_mempool_sha256' \
  'maker_supervisor_trace_sha256' \
  'maker_supervisor_final_sha256' \
  'maker_daemon_owned_at_terminal_observation'; do
  rg -Fq -- "$required" "$runner" ||
    fail "M5 runner does not prove supervisor-owned Maker effects: ${required}"
done


projection_function="$(sed -n \
  '/^prove_m5_terminal_operator_projection() {$/,/^}$/p' "$runner")"
[[ -n "$projection_function" ]] || fail "terminal operator projection function is missing"

for required in \
  '--terminal-zec-maker-state-db' \
  '--terminal-zec-swap-id' \
  '--terminal-zec-claim-key-id' \
  '--terminal-zec-claim-key-file' \
  'm5-history-after-terminal-restart.json' \
  'm5-status-after-terminal-restart.json' \
  'm5-terminal-operator-projection.json' \
  'chain_rpc_used_during_import: false' \
  'private_material_disclosed: false'; do
  rg -Fq -- "$required" <<<"$projection_function" ||
    fail "terminal projection is missing contract: ${required}"
done

[[ "$(rg -Fc '.phase == "Completed"' <<<"$projection_function")" == 2 ]] ||
  fail "history and status must use the real maker RPC terminal enum spelling"
if rg -Fq '.phase == "completed"' <<<"$projection_function"; then
  fail "runner must not compare maker RPC phases to actor-status spelling"
fi

history_fixture='[{"id":"m5-contract-swap","phase":"Completed"}]'
status_fixture='{"id":"m5-contract-swap","phase":"Completed"}'
jq -e --arg swap m5-contract-swap '
  length == 1 and .[0].id == $swap and .[0].phase == "Completed"
' <<<"$history_fixture" >/dev/null || fail "valid history fixture was rejected"
jq -e --arg swap m5-contract-swap '
  .id == $swap and .phase == "Completed"
' <<<"$status_fixture" >/dev/null || fail "valid status fixture was rejected"

maker_terminal_line="$(rg -n -F '${evidence_dir}/maker-status-final.json' "$runner" | tail -1 | cut -d: -f1)"
taker_terminal_line="$(rg -n -F '${evidence_dir}/taker-status-final.json' "$runner" | tail -1 | cut -d: -f1)"
projection_call_line="$(rg -n '^  prove_m5_terminal_operator_projection$' "$runner" |
  cut -d: -f1)"
[[ "$maker_terminal_line" =~ ^[0-9]+$ && "$taker_terminal_line" =~ ^[0-9]+$ &&
   "$projection_call_line" =~ ^[0-9]+$ ]] || fail "terminal ordering anchors are missing"
(( maker_terminal_line < projection_call_line && taker_terminal_line < projection_call_line )) ||
  fail "operator projection must run only after both role actors are terminal"

echo "M5 application contract passed"
