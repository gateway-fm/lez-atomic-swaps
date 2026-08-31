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
  'strip --strip-all "$m5_actor_program"' \
  'chmod 0500 "$m5_actor_program"' \
  'require_command strip' \
  'stat -c %h -- "$m5_actor_program"' \
  'sha256sum "$m5_actor_program"' \
  '--actor-program "$m5_actor_program"' \
  '--actor-program-sha256 "$m5_actor_program_sha256"'; do
  rg -Fq -- "$required" "$runner" ||
    fail "M5 runner is missing private actor deployment contract: ${required}"
done
if rg -Fq 'strip --strip-debug "$m5_actor_program"' "$runner"; then
  fail "M5 runner must not retain debug symbols in the staged actor"
fi

for required in \
  'cargo +1.96.0 build --locked --offline --release -p zec-reference-actor --bins' \
  'cargo +1.96.0 build --locked --offline --release -p lez-maker-node --bins' \
  'cargo +1.96.0 build --locked --offline --release -p lez-taker-node --bins' \
  'cargo +1.96.0 build --locked --offline --release -p lez-maker-node --example maker-actor-inspect' \
  'cargo +1.96.0 build --locked --offline --release -p lez-maker-node --example maker-zec-lock-intent-inspect' \
  '--locked --offline --release --bin lez-v02-bridge-poc' \
  'target/release/lez-zec-maker-actor' \
  'target/release/zec-local-poc-provision' \
  'compat/lez-v0_2-sidecar/target/release/lez-v02-bridge-poc' \
  'target/release/lez-maker-node' \
  'target/release/lez-maker-cli' \
  'target/release/lez-taker-cli' \
  'target/release/zec-local-poc-chat-draft' \
  'target/release/zec-local-poc-chat-finalize' \
  'target/release/examples/maker-actor-inspect' \
  'target/release/zec-actor-pair-inspect' \
  'target/release/examples/maker-zec-lock-intent-inspect'; do
  rg -Fq -- "$required" "$runner" ||
    fail "M5 runner is missing release build/path contract: ${required}"
done

for required in \
  'target/debug/lez-zec-maker-actor' \
  'target/debug/zec-local-poc-provision' \
  'compat/lez-v0_2-sidecar/target/debug/lez-v02-bridge-poc'; do
  rg -Fq -- "$required" "$runner" ||
    fail "M2 runner must retain its debug build path: ${required}"
done

[[ "$(rg -Fc 'target/debug' "$runner")" == 3 ]] ||
  fail "M5 runner must not select target/debug binaries"


cleanup_function="$(sed -n '/^cleanup() {$/,/^}$/p' "$runner")"
[[ -n "$cleanup_function" ]] || fail "M5 runner cleanup function is missing"
daemon_stop_line="$(rg -n -F 'stop_owned_m5_daemon' <<<"$cleanup_function" | cut -d: -f1)"
maker_stop_line="$(rg -n -F 'stop_owned_process "$maker_pid"' <<<"$cleanup_function" | cut -d: -f1)"
taker_stop_line="$(rg -n -F 'stop_owned_process "$taker_pid"' <<<"$cleanup_function" | cut -d: -f1)"
for line in "$daemon_stop_line" "$maker_stop_line" "$taker_stop_line"; do
  [[ "$line" =~ ^[0-9]+$ ]] ||
    fail "M5 cleanup is missing an owned-process stop anchor"
done
(( daemon_stop_line < maker_stop_line && daemon_stop_line < taker_stop_line )) ||
  fail "M5 cleanup must stop the effect-bearing Maker daemon before sidecars"

[[ "$(rg -Fc -- '--actor-effect-cutoff-boottime-milliseconds "$corridor_deadline_monotonic_ms"' "$runner")" == 2 ]] ||
  fail "both M5 daemon incarnations must share the absolute corridor effect cutoff"



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
  'elif $m5_application_mode == 1 then' \
  '"receipt_bound_cli"' \
  'direct_taker_claim_effects:'; do
  rg -Fq -- "$required" "$runner" ||
    fail "M5 runner is missing receipt-bound Taker claim contract: ${required}"
done
[[ "$(rg -Fc '"receipt_bound_cli"' "$runner")" == 1 ]] ||
  fail 'M5 runner must project receipt-bound CLI authority exactly once'

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
  'ESCROW_PROGRAM_ID:-431ab9aec4b21d66e88ecbf8bb83301d5ef4cc0cec0ba0fb76baaa0ac7f9a10b' \
  'M5_LEZ_GUEST_SHA256:-237037e1a54187697e7e67a9bf589dfb3eb88c475c7f9b62eb2396144e87c6d0' \
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
  'capture_m5_supervised_maker_status' \
  'm5-maker-status-retries.ndjson' \
  'actor_configuration_unavailable' \
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
status_capture_function="$(sed -n \
  '/^capture_m5_supervised_maker_status() {$/,/^}$/p' "$runner")"
[[ -n "$status_capture_function" ]] ||
  fail "supervised Maker status helper is missing"
[[ "$(rg -Fc 'capture_m5_supervised_maker_status' "$runner")" == 3 ]] ||
  fail "supervised Maker status helper must have exactly two call sites"
[[ "$(rg -Fc '"$actor_bin" --config "$maker_config" status' "$runner")" == 5 ]] ||
  fail "only pre/final status, the supervised helper, and the constrained M6/M7 observation-only branches may invoke direct Maker status"

status_retry_test_root="$(mktemp -d "${TMPDIR:-/tmp}/lez-m5-status-contract.XXXXXX")" ||
  fail "could not create supervised status contract root"
chmod 0700 "$status_retry_test_root"
trap 'rm -rf -- "$status_retry_test_root"' EXIT
(
  eval "$status_capture_function"
  # shellcheck disable=SC2034 # Consumed by the eval-extracted production helper.
  actor_bin=fake_actor
  # shellcheck disable=SC2034 # Consumed by the eval-extracted production helper.
  maker_config=/tmp/m5-contract-maker-config
  # shellcheck disable=SC2034 # Consumed by the eval-extracted production helper.
  m5_daemon_pid=91
  # shellcheck disable=SC2034 # Consumed by the eval-extracted production helper.
  m5_daemon_start_ticks=92
  # shellcheck disable=SC2034 # Consumed by the eval-extracted production helper.
  m5_daemon_bin=/tmp/m5-contract-daemon
  # shellcheck disable=SC2034 # Consumed by the eval-extracted production helper.
  evidence_dir="$status_retry_test_root"
  MAX_SUPERVISED_STATUS_RETRIES=8
  # shellcheck disable=SC2034 # Consumed by the eval-extracted production helper.
  SUPERVISED_STATUS_RETRY_DELAY_SECONDS=0
  process_is_owned() { return 0; }
  remaining_budget_milliseconds() { return 0; }
  sleep() { :; }

  fake_actor_calls=0
  fake_actor() {
    fake_actor_calls=$((fake_actor_calls + 1))
    if (( fake_actor_calls == 1 )); then
      printf '%s\n' 'actor configuration is unavailable' >&2
      return 2
    fi
    printf '%s\n' \
      '{"schema_version":1,"role":"maker","state":"active","phase":"offered","revision":0}'
  }
  capture_m5_supervised_maker_status \
    "$status_retry_test_root/status.json" "$status_retry_test_root/status.stderr" activation
  [[ "$fake_actor_calls" == 2 && ! -s "$status_retry_test_root/status.stderr" ]] ||
    exit 1
  jq -e '.role == "maker" and .state == "active" and .revision == 0' \
    "$status_retry_test_root/status.json" >/dev/null
  jq -s -e '
    length == 1 and .[0].event == "supervised_maker_status_retry"
    and .[0].label == "activation" and .[0].attempt == 1
    and .[0].error_class == "actor_configuration_unavailable"
  ' "$status_retry_test_root/m5-maker-status-retries.ndjson" >/dev/null

  fake_actor_calls=0
  fake_actor() {
    fake_actor_calls=$((fake_actor_calls + 1))
    if (( fake_actor_calls == 1 )); then
      printf '%s\n' 'actor status material is unavailable' >&2
      return 2
    fi
    printf '%s\n' \
      '{"schema_version":1,"role":"maker","state":"active","phase":"offered","revision":0}'
  }
  capture_m5_supervised_maker_status \
    "$status_retry_test_root/material.json" \
    "$status_retry_test_root/material.stderr" material
  [[ "$fake_actor_calls" == 2 && ! -s "$status_retry_test_root/material.stderr" ]] ||
    exit 1
  jq -e '.role == "maker" and .state == "active" and .revision == 0' \
    "$status_retry_test_root/material.json" >/dev/null
  jq -s -e '
    length == 2 and .[1].event == "supervised_maker_status_retry"
    and .[1].label == "material" and .[1].attempt == 1
    and .[1].error_class == "actor_status_material_unavailable"
  ' "$status_retry_test_root/m5-maker-status-retries.ndjson" >/dev/null

  fake_actor_calls=0
  fake_actor() {
    fake_actor_calls=$((fake_actor_calls + 1))
    printf '%s\n' 'actor status is unavailable' >&2
    return 2
  }
  if capture_m5_supervised_maker_status \
    "$status_retry_test_root/wrong.json" "$status_retry_test_root/wrong.stderr" wrong \
    2>"$status_retry_test_root/wrong-helper.stderr"; then
    exit 1
  fi
  [[ "$fake_actor_calls" == 1 ]] || exit 1

  fake_actor_calls=0
  fake_actor() {
    fake_actor_calls=$((fake_actor_calls + 1))
    printf '%s\n' 'actor configuration is unavailable' >&2
    return 2
  }
  process_is_owned() { return 1; }
  if capture_m5_supervised_maker_status \
    "$status_retry_test_root/dead.json" "$status_retry_test_root/dead.stderr" dead \
    2>"$status_retry_test_root/dead-helper.stderr"; then
    exit 1
  fi
  [[ "$fake_actor_calls" == 1 ]] || exit 1

  process_is_owned() { return 0; }
  # shellcheck disable=SC2034 # Consumed by the eval-extracted production helper.
  MAX_SUPERVISED_STATUS_RETRIES=1
  fake_actor_calls=0
  if capture_m5_supervised_maker_status \
    "$status_retry_test_root/bounded.json" "$status_retry_test_root/bounded.stderr" bounded \
    2>"$status_retry_test_root/bounded-helper.stderr"; then
    exit 1
  fi
  [[ "$fake_actor_calls" == 2 ]] || exit 1
) || fail "supervised Maker status retry behavior is unsafe"
rm -rf -- "$status_retry_test_root"
trap - EXIT


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
  'expected_phase=Completed' \
  'expected_phase=Refunded' \
  'chain_rpc_used_during_import: false' \
  'private_material_disclosed: false'; do
  rg -Fq -- "$required" <<<"$projection_function" ||
    fail "terminal projection is missing contract: ${required}"
done

[[ "$(rg -Fc '.phase == $phase' <<<"$projection_function")" == 2 ]] ||
  fail "history and status must compare against the selected real maker RPC terminal enum"
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
