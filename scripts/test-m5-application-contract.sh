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

if rg -Fq 'SIDECAR_STARTUP_WORST_CASE_SECONDS' "$runner"; then
  fail "fixed sidecar headroom must not reject a pre-effect run before the bounded readiness and pre-effect gates"
fi
for required in \
  'readonly MAX_PRE_EFFECT_SECONDS=25' \
  "remaining_budget_milliseconds 'sidecar-startup-before'" \
  "remaining_budget_milliseconds 'pre-effect-gate'" \
  'pre_effect_elapsed_ms <= MAX_PRE_EFFECT_SECONDS * 1000'; do
  rg -Fq -- "$required" "$runner" ||
    fail "M5 runner is missing bounded pre-effect timing contract: ${required}"
done

for required in \
  'install -m 0700 "$actor_bin" "$m5_actor_program"' \
  'strip --strip-debug "$m5_actor_program"' \
  'chmod 0500 "$m5_actor_program"' \
  'require_command strip' \
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

for required in \
  'taker_actor_root="$application_root/taker-actors"' \
  'acceptance_receipt="$application_root/taker-acceptance-receipt.json"' \
  '--zec-source-taker-config "$source_actors_root/taker/actor-config.json"' \
  '--zec-taker-actor-root "$taker_actor_root"' \
  '--zec-acceptance-receipt "$acceptance_receipt"' \
  '"$pair_inspector_bin" --maker-config "$queued_config"' \
  'm5-effect-actor-pair.json' \
  'effect_actor_pair_validated' \
  'acceptance_receipt_file' \
  'taker_actor_config' \
  'taker_actor_state'; do
  rg -Fq -- "$required" "$handoff" ||
    fail "M5 handoff is missing receipt-bound Taker contract: ${required}"
done

for required in \
  'drive_m5_taker' \
  'assert_m5_taker_receipt_unchanged' \
  '2>"$claim_stderr")"; then' \
  'raw_taker_drive_admitted' \
  'taker_sidecar_state_dir' \
  '--state-directory "$taker_sidecar_state_dir"' \
  '.phase == "claim_evidence_available" and .next_action == "claim_zcash"' \
  '.phase == "offered" and .next_action == "create_and_fund_lez"' \
  '"$taker_bin" claim --receipt "$m5_taker_acceptance_receipt"' \
  '.command == "claim"' \
  'm5-taker-receipt-claim.ndjson' \
  'm5-taker-receipt-monitor.ndjson' \
  'acceptance_receipt_sha256:$receipt_sha256' \
  'swap_id:$swap' \
  'taker_claim_authority:' \
  'then "receipt_bound_cli" else null end' \
  'direct_taker_claim_effects:'; do
  rg -Fq -- "$required" "$runner" ||
    fail "M5 runner is missing receipt-bound Taker claim contract: ${required}"
done

[[ "$(rg -Fc '"$taker_bin" claim --receipt "$m5_taker_acceptance_receipt"' "$runner")" == 1 ]] ||
  fail 'M5 runner must have exactly one receipt-bound Taker claim call site'
rg -UFq '2>"$claim_stderr")"; then
    assert_m5_taker_receipt_unchanged || return' "$runner" ||
  fail 'failed or timed-out Taker claim must re-pin the receipt after use'
if rg -Fq 'claim --actor-config' "$runner"; then
  fail 'M5 runner must not bypass the receipt for Taker claim'
fi
claim_guard_line="$(rg -n -F '.next_action == "claim_zcash"' "$runner" |
  sed -n '1s/:.*//p')"
receipt_claim_line="$(rg -n -F '"$taker_bin" claim --receipt "$m5_taker_acceptance_receipt"' \
  "$runner" | cut -d: -f1)"
[[ "$claim_guard_line" =~ ^[0-9]+$ && "$receipt_claim_line" =~ ^[0-9]+$ \
  && "$claim_guard_line" -lt "$receipt_claim_line" ]] ||
  fail 'receipt-bound Taker claim must follow exact next-action admission'

claimable_status='{"schema_version":1,"role":"taker","state":"active","phase":"claim_evidence_available","revision":3,"next_action":"claim_zcash"}'
waiting_status='{"schema_version":1,"role":"taker","state":"active","phase":"both_legs_locked","revision":2,"next_action":"wait"}'
jq -e '.schema_version == 1 and .role == "taker" and .state == "active"
  and .phase == "claim_evidence_available" and .next_action == "claim_zcash"' \
  <<<"$claimable_status" >/dev/null ||
  fail 'valid receipt-bound claim admission fixture was rejected'
if jq -e '.phase == "claim_evidence_available" and .next_action == "claim_zcash"' \
  <<<"$waiting_status" >/dev/null; then
  fail 'waiting Taker status incorrectly admitted a claim'
fi
wrong_phase_claim_status='{"schema_version":1,"role":"taker","state":"active","phase":"both_legs_locked","revision":2,"next_action":"claim_zcash"}'
if jq -e '.phase == "claim_evidence_available" and .next_action == "claim_zcash"' \
  <<<"$wrong_phase_claim_status" >/dev/null; then
  fail 'wrong-phase Taker status incorrectly admitted a claim'
fi
raw_drive_status='{"schema_version":1,"role":"taker","state":"active","phase":"offered","revision":0,"next_action":"create_and_fund_lez"}'
jq -e '(.phase == "offered" and .next_action == "create_and_fund_lez")' \
  <<<"$raw_drive_status" >/dev/null ||
  fail 'valid raw Taker drive admission fixture was rejected'
unknown_drive_status='{"schema_version":1,"role":"taker","state":"active","phase":"offered","revision":0,"next_action":"wait"}'
if jq -e '(.phase == "offered" and .next_action == "create_and_fund_lez")' \
  <<<"$unknown_drive_status" >/dev/null; then
  fail 'unknown raw Taker drive status was admitted'
fi

required='actor_supervisor_enabled: false'
rg -Fq -- "$required" "$handoff" ||
  fail "M5 handoff must return a queued actor before supervision: ${required}"

for required in \
  'ESCROW_PROGRAM_ID:-4d6590332948743c2db88a183755815354ef92560550cd206ac27bddeea12c82' \
  'M5_LEZ_GUEST_SHA256:-dc370bc34b432317730c51b49342760dbc675fca700e300b30b5fadefe5b7292' \
  'M5_LEZ_DEPLOYMENT_EVIDENCE_FILE' \
  'M5_LEZ_FINALITY_EVIDENCE_FILE' \
  'M5_LEZ_ONBOARDING_EVIDENCE_FILE' \
  'M5_LEZ_MAKER_SIGNER_KEY_FILE' \
  'M5_LEZ_TAKER_SIGNER_KEY_FILE' \
  'm5-lez-deployment.json' \
  'm5-lez-deployment-finality.json' \
  'm5-lez-actor-onboarding.json' \
  '.preflight.image_id == $program' \
  '.preflight.elf_sha256 == $guest' \
  '.preflight.rpc_url == $rpc' \
  '.preflight.channel_id == $channel' \
  '.actors.maker.account_id == $maker' \
  '.actors.taker.account_id == $taker' \
  '.deployment.finalized_evidence_sha256 == $deployment_sha' \
  'lez_escrow_program_id' \
  'lez_escrow_guest_sha256' \
  'lez_deployment_receipt_sha256' \
  'lez_deployment_finality_sha256' \
  'lez_actor_onboarding_sha256' \
  'lez_maker_vault_claim_transaction_hash' \
  'lez_taker_vault_claim_transaction_hash' \
  'lez_deployment_transaction_hash' \
  'lez_deployment_inclusion_block_id' \
  'lez_deployment_inclusion_block_hash'; do
  rg -Fq -- "$required" "$runner" ||
    fail "M5 runner does not bind the current escrow deployment: ${required}"
done

for required in \
  '--maker-lez-signer-key-file "$M5_LEZ_MAKER_SIGNER_KEY_FILE"' \
  '--taker-lez-signer-key-file "$M5_LEZ_TAKER_SIGNER_KEY_FILE"' \
  '"$M5_LEZ_MAKER_SIGNER_KEY_FILE" "${provision_actors_root}/maker/lez-signer.key"' \
  '"$M5_LEZ_TAKER_SIGNER_KEY_FILE" "${provision_actors_root}/taker/lez-signer.key"' \
  '"$MAKER_ACCOUNT_BASE58" == 34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib' \
  '"$TAKER_ACCOUNT_BASE58" == B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd' \
  'M5 requires fresh LEZ identities rather than deterministic fixture defaults'; do
  rg -Fq -- "$required" "$runner" ||
    fail "M5 runner does not preserve a fresh LEZ signer: ${required}"
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
