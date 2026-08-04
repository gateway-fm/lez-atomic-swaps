#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export LC_ALL=C

readonly wrapper="scripts/run-m5-xmr-application-poc.sh"
readonly runner="scripts/run-m4-actual-claim-poc.sh"
readonly sidecar_lock="compat/lez-v0_2-sidecar/Cargo.lock"
readonly release_lock="compat/lez-v0_2-xmr-release-service/Cargo.lock"
readonly taker_cli="crates/maker-node/src/bin/lez-taker.rs"
readonly xmr_receipt_loader="crates/maker-node/src/bin/support/taker_accept_xmr.rs"
readonly xmr_process_test="crates/maker-node/tests/xmr_chat_process.rs"
readonly xmr_application_provision="crates/xmr-reference-actor/src/application_provision.rs"

fail() {
  echo "M5 XMR application-to-chain contract failed: $*" >&2
  exit 1
}

[[ -x "$wrapper" && -f "$wrapper" && ! -L "$wrapper" ]] ||
  fail 'M5 XMR wrapper is absent, unsafe, or not executable'
bash -n "$wrapper" || fail 'M5 XMR wrapper has invalid shell syntax'

contract="$($wrapper contract)" || fail 'M5 XMR wrapper contract command failed'
jq -e '
  (keys | sort) == ([
    "application_mode", "certification", "delegation", "direction",
    "execution_performed", "hardening", "kind", "milestone", "pair",
    "planned_order", "runtime_external_resources", "schema_version", "scope"
  ] | sort) and
  .schema_version == 1 and
  .kind == "m5_xmr_application_to_chain_poc_contract" and
  .milestone == "M5" and
  .scope == "xmr_application_to_chain_local_poc" and
  .execution_performed == false and
  .application_mode == 1 and
  .pair == "monero" and
  .direction == "taker_sells_lez" and
  .certification == {
    status: "not_yet_executed",
    delegated_runner_splice_required: true,
    certifying_replay_performed: false
  } and
  .delegation == {
    runner: "scripts/run-m4-actual-claim-poc.sh",
    runner_mode: "execute",
    reuse: "exact_m4_actual_claim_runner",
    argument_policy: "forward_to_m4_fail_closed",
    opt_in_environment: {
      name: "M5_XMR_APPLICATION_MODE",
      value: "1"
    }
  } and
  .planned_order == [
    "delivery_only_daemon",
    "maker_offer_publication",
    "taker_delivery_plan",
    "authenticated_swap_id",
    "canonical_stage_a",
    "countersigned_stage_b",
    "maker_role_actor_installation",
    "authorized_maker_daemon",
    "real_taker_acceptance",
    "taker_role_actor_installation_and_receipt",
    "publisher_restart_reconciles_retryable_offer",
    "delivery_outage_after_authenticated_reconciliation",
    "exact_replay_without_delivery",
    "synchronous_application_cutoff",
    "m4_tag13_initialize_and_fund",
    "m4_monero_funding_and_verification",
    "m4_tag14_authorize_claim",
    "m4_tag15_claim",
    "m4_adaptor_extraction",
    "m4_monero_sweep",
    "m4_cross_chain_binding"
  ] and
  .runtime_external_resources == {
    public_rpc: false,
    faucet: false,
    public_funds: false,
    monero: {
      implementation: "official_monero",
      version: "0.18.5.1",
      network: "isolated_regtest",
      public_peers: false
    },
    lez: {
      version: "0.2",
      network: "isolated_local_devnet"
    },
    test_funds: "deterministic_local_genesis_and_regtest_outputs"
  } and
  .hardening == {
    status: "open",
    qa: "open",
    chaos_engineering: "open",
    infosec: "open",
    production_readiness: "open"
  }
' <<<"$contract" >/dev/null || fail 'M5 XMR wrapper contract shape or order is incomplete'

if env M5_XMR_APPLICATION_MODE=0 "$wrapper" contract >/dev/null 2>&1; then
  fail 'wrapper accepted a conflicting application-mode override'
fi
if "$wrapper" contract unexpected >/dev/null 2>&1; then
  fail 'contract mode accepted an unexpected argument'
fi
if "$wrapper" unsupported >/dev/null 2>&1; then
  fail 'wrapper accepted an unsupported mode'
fi

for required in \
  'readonly m4_runner="scripts/run-m4-actual-claim-poc.sh"' \
  'M5_XMR_APPLICATION_MODE is fixed to 1' \
  'RUN_ID must be 8..48 lowercase letters, numbers, underscores, or hyphens' \
  'M4_EXPECTED_COMMIT must be one lowercase 40-character Git object ID' \
  'actual_commit="$(git rev-parse --verify HEAD)"' \
  'dirty="$(git status --porcelain=v1 --untracked-files=normal)"' \
  'git diff --quiet --exit-code' \
  'git diff --cached --quiet --exit-code' \
  "'prepare_m5_xmr_delivery_plan() {'" \
  "'complete_m5_xmr_application_handoff() {'" \
  'exec env M5_XMR_APPLICATION_MODE=1 "$m4_runner" execute "$@"'; do
  rg -Fq -- "$required" "$wrapper" ||
    fail "wrapper is missing a fail-closed source/delegation boundary: ${required}"
done

if rg -n '(^|[[:space:]])(cargo|docker)([[:space:]]|$)' "$wrapper" >/dev/null; then
  fail 'thin wrapper unexpectedly owns Cargo or Docker execution'
fi

[[ -x "$runner" && -f "$runner" && ! -L "$runner" ]] ||
  fail 'delegated M4 runner is absent, unsafe, or not executable'
bash -n "$runner" || fail 'delegated M4 runner has invalid shell syntax'

require_runner_source() {
  local needle="$1" label="$2"
  rg -Fq -- "$needle" "$runner" || fail "delegated runner omits ${label}"
}

unique_line() {
  local pattern="$1" label="$2" matches
  matches="$(rg -n -- "$pattern" "$runner")"
  [[ -n "$matches" ]] || fail "delegated runner omits ${label}"
  [[ "$(wc -l <<<"$matches" | tr -d ' ')" == 1 ]] ||
    fail "delegated runner repeats or omits ${label}"
  printf '%s\n' "${matches%%:*}"
}

for required in \
  'readonly m5_xmr_application_mode="${M5_XMR_APPLICATION_MODE:-0}"' \
  'M5_XMR_APPLICATION_MODE must be unset, 0, or 1' \
  'RUN_ID="$artifact_run_id" "$artifact_runner" verify-source' \
  'cargo +1.96.0 build --locked --offline -p lez-maker-node' \
  '--bin lez-maker --bin lez-maker-daemon --bin lez-taker --bin xmr-maker-actor' \
  'stage_executable "${workspace_target}/debug/lez-maker" "$m5_lez_maker_binary"' \
  'stage_executable "${workspace_target}/debug/lez-maker-daemon"' \
  'stage_executable "${workspace_target}/debug/lez-taker" "$m5_lez_taker_binary"' \
  'stage_executable "${workspace_target}/debug/xmr-maker-actor"' \
  '.binary_sha256 += {m5_lez_maker:$maker,m5_lez_maker_daemon:$daemon,m5_lez_taker:$taker,m5_xmr_maker_actor:$xmr_actor}' \
  'prepare_m5_xmr_delivery_plan() {' \
  'delivery-identity --signing-key-file "$m5_xmr_delivery_key"' \
  '--lez-units-per-lot 7 --foreign-units-per-lot 10000000000' \
  '--plan-xmr-offer "$m5_xmr_offer_id"' \
  'readonly m5_xmr_foreign_units=1000000000000' \
  'readonly m5_xmr_lez_units=700' \
  'm5_swap_id_argument=(--swap-id "$m5_xmr_planned_swap_id")' \
  "'.swap_id==\$swap' \"\$agreement_receipt\"" \
  'LEZ_V02_R0VM="$RISC0_SERVER_PATH"' \
  'complete_m5_xmr_application_handoff() {' \
  'provision-application maker' \
  '--xmr-actor-manifest-registry-file "$m5_xmr_actor_registry"' \
  '--accept-xmr-offer "$m5_xmr_offer_id"' \
  '--actor-requeue-delay-seconds 3600' \
  'next_action:"xmr_chain_effects_not_yet_composed"' \
  'mv "$m5_xmr_delivery_root" "$m5_xmr_removed_delivery_root"' \
  'm5_delivery_offer_files_absent "$m5_xmr_delivery_root"' \
  'cmp -- "$m5_xmr_artifacts_before" "$m5_xmr_artifacts_after"' \
  'cmp -- "$m5_xmr_journals_before" "$m5_xmr_journals_after"' \
  'stop_m5_xmr_application_daemon || fail "M5 XMR replay daemon did not stop before legacy Tag 13"' \
  'verify_m5_xmr_application_cutoff() {' \
  'configured_reobservation_seconds:3600' \
  'ps -eo pgid=,stat=' \
  '$2 !~ /^Z/' \
  'readonly m5_xmr_reconciled_delivery_root=' \
  'require_owner_file "${m5_xmr_delivery_root}/${m5_xmr_offer_id}.offer.json"' \
  'mv "$m5_xmr_delivery_root" "$m5_xmr_reconciled_delivery_root"' \
  'run_m5_xmr_taker_acceptance "$m5_xmr_replay_acceptance" 0' \
  'if [[ "$m5_xmr_application_mode" == 1 && -n "${m5_application_daemon_pid:-}" ]]; then' \
  'stop_m5_xmr_application_daemon || {'; do
  require_runner_source "$required" "M5 source boundary: ${required}"
done

for required in \
  'local -a ledger_rows=() cleanup_failure_reasons=()' \
  'schema_version:2' \
  'failure_reasons:$reasons' \
  'cleanup_failure_reasons+=("unclassified_cleanup_failure")' \
  'cleanup_failure_reasons+=("image_label_mismatch")' \
  'cleanup_failure_reasons+=("ephemeral_path_boundary_failed")'; do
  require_runner_source "$required" "fail-closed cleanup diagnostics: ${required}"
done

cleanup_reason_fallback_line="$(unique_line '^  if \[\[ "\$cleanup_failed" != 0 && \$\{#cleanup_failure_reasons\[@\]\} == 0 \]\]; then$' 'cleanup reason fallback')"
cleanup_final_probe_line="$(unique_line '^  if \[\[ -n "\$\{sentinel_name:-\}" \]\] && docker network inspect ' 'cleanup final sentinel probe')"
cleanup_result_line="$(unique_line '^  local cleanup_result=passed$' 'cleanup result publication')"
readonly cleanup_reason_fallback_line cleanup_final_probe_line cleanup_result_line
(( cleanup_final_probe_line < cleanup_reason_fallback_line &&
   cleanup_reason_fallback_line < cleanup_result_line )) ||
  fail 'cleanup failure fallback is not after all final probes and before result publication'

[[ "$(rg -c 'cleanup_failed=0' "$runner")" == 1 ]] ||
  fail 'cleanup failure state is reset after an earlier identity/removal error'
if rg -Fq '# Cleanup is judged by the final resource state' "$runner"; then
  fail 'legacy cleanup-error reset comment survived the fail-closed fix'
fi
if rg -Fq 'fail "M5 XMR replay daemon reconstructed a Delivery offer"' "$runner"; then
  fail 'runner still rejects intentional durable Delivery reconciliation'
fi

artifact_preflight_line="$(unique_line '^  RUN_ID="\$artifact_run_id" "\$artifact_runner" verify-source$' 'artifact fast-preflight invocation')"
build_line="$(unique_line '^  build_identity_and_artifact$' 'heavy build invocation')"
plan_line="$(unique_line '^    prepare_m5_xmr_delivery_plan$' 'M5 plan invocation')"
compose_line="$(unique_line '^  compose_xmr_agreement$' 'agreement invocation')"
handoff_line="$(unique_line '^    complete_m5_xmr_application_handoff$' 'M5 handoff invocation')"
cutoff_line="$(unique_line '^    verify_m5_xmr_application_cutoff$' 'M5 cutoff invocation')"
tag13_line="$(unique_line '^  submit_tag13$' 'Tag13 invocation')"
readonly artifact_preflight_line build_line plan_line compose_line handoff_line cutoff_line tag13_line
(( artifact_preflight_line < build_line && build_line < plan_line &&
   plan_line < compose_line && compose_line < handoff_line &&
   handoff_line < cutoff_line && cutoff_line < tag13_line )) ||
  fail 'M5 application plan/handoff/cutoff does not precede legacy Tag13 exactly'

replay_daemon_line="$(unique_line '^  start_m5_xmr_application_daemon replay 1$' 'M5 replay daemon invocation')"
reconciled_offer_line="$(unique_line '^  require_owner_file "\$\{m5_xmr_delivery_root\}/\$\{m5_xmr_offer_id\}\.offer\.json" \\$' 'M5 reconciled offer check')"
reconciled_archive_line="$(unique_line '^  mv "\$m5_xmr_delivery_root" "\$m5_xmr_reconciled_delivery_root"$' 'M5 reconciled offer archive')"
delivery_free_replay_line="$(unique_line '^  run_m5_xmr_taker_acceptance "\$m5_xmr_replay_acceptance" 0$' 'M5 Delivery-free replay')"
readonly replay_daemon_line reconciled_offer_line reconciled_archive_line delivery_free_replay_line
(( replay_daemon_line < reconciled_offer_line &&
   reconciled_offer_line < reconciled_archive_line &&
   reconciled_archive_line < delivery_free_replay_line &&
   delivery_free_replay_line < cutoff_line &&
   cutoff_line < tag13_line )) ||
  fail 'M5 retry reconciliation/authentication/outage/replay does not precede cutoff and Tag13'


cleanup_line="$(unique_line '^cleanup\(\) \{$' 'cleanup function')"
cleanup_hook_line="$(unique_line '^    stop_m5_xmr_application_daemon \|\| \{$' 'M5 cleanup hook')"
readonly cleanup_line cleanup_hook_line
(( cleanup_line < cleanup_hook_line )) || fail 'M5 daemon cleanup hook is outside cleanup()'

rg -Uq 'name = "lez-swap-core"
version = "0.1.0"
dependencies = \[
 "serde",
 "sha2",
 "thiserror 2.0.18",
\]' "$sidecar_lock" ||
  fail 'sidecar lock omits the reachable lez-swap-core package'
for xmr_lock in "$sidecar_lock" "$release_lock"; do
  rg -Uq 'name = "lez-xmr-swap-sdk"
version = "0.1.0"
dependencies = \[
 "hex",
 "lez-adaptor-signature",
 "lez-swap-core",' "$xmr_lock" ||
    fail "locked graph omits XMR SDK runtime dependency edges: ${xmr_lock}"
done

rg -Uq 'name = "command-fds"
version = "0.3.3"
source = "registry\+https://github.com/rust-lang/crates.io-index"
checksum = "1b60b5124979fccd9addd89d8b97a1d6eebb4950694520c75ddd722535ea443f"' "$release_lock" ||
  fail 'release lock omits the checksum-pinned command-fds package'
release_store_block="$(sed -n '/^name = "lez-swap-store"$/,/^$/p' "$release_lock")"
readonly release_store_block
for required_edge in command-fds lez-btc-swap-sdk lez-xmr-swap-sdk rustix; do
  rg -Fqx -- " \"${required_edge}\"," <<<"$release_store_block" ||
    fail "release lock omits swap-store runtime edge: ${required_edge}"
done

[[ "$(rg -Fo -- '--shared-wallet-url "${monero_env[MONERO_FUNDING_WALLET_ENDPOINT]}"' "$runner" | wc -l)" == 3 ]] ||
  fail 'runner must restore the shared XMR wallet only on the neutral provisioner RPC for funding and both role-correct sweeps'
require_runner_source '--funding-wallet-url "${monero_env[MONERO_MAKER_WALLET_ENDPOINT]}"' 'Maker funding and claim-mining wallet role'
rg -Fq -- '--taker-wallet-url "${monero_env[MONERO_TAKER_WALLET_ENDPOINT]}"' "$runner" ||
  fail 'runner must sweep reconstructed XMR only to the Taker wallet RPC'
rg -Fq -- '--target-wallet-url "${monero_env[MONERO_TAKER_WALLET_ENDPOINT]}"' "$runner" ||
  fail 'post-sweep verification must target the Taker wallet RPC'
rg -Fq -- '--foreign-wallet-url "${monero_env[MONERO_MAKER_WALLET_ENDPOINT]}"' "$runner" ||
  fail 'post-sweep verification must reject the Maker wallet as destination'
require_runner_source '--target-wallet-url "${monero_env[MONERO_FUNDING_WALLET_ENDPOINT]}"' 'neutral shared-wallet pre-sweep verification target'
if rg -Fq -- '--taker-wallet-url "${monero_env[MONERO_MAKER_WALLET_ENDPOINT]}"' "$runner"; then
  fail 'runner retains the role-inverted Maker destination for the Taker sweep'
fi
if rg -Fq -- '--shared-wallet-url "${monero_env[MONERO_TAKER_WALLET_ENDPOINT]}"' "$runner"; then
  fail 'runner reuses the Taker RPC as the reconstructed shared-wallet process'
fi
if rg -Fq -- '--shared-wallet-url "${monero_env[MONERO_MAKER_WALLET_ENDPOINT]}"' "$runner"; then
  fail 'runner reuses the Maker RPC as the reconstructed shared-wallet process'
fi

for source in "$taker_cli" "$xmr_receipt_loader" "$xmr_process_test" \
  "$xmr_application_provision"; do
  [[ -f "$source" && ! -L "$source" ]] ||
    fail "receipt-only XMR Taker monitor source is absent or unsafe: ${source}"
done

for required in \
  'XmrMonitor(Box<XmrTakerReceiptSelector>)' \
  'XmrEffect(Box<XmrTakerEffectReceiptSelector>)' \
  'load_xmr_taker_receipt_selector(path).ok(),' \
  'load_xmr_taker_effect_receipt_selector(path).ok(),' \
  'MakerActorHeldLock::acquire_for(selector.swap_id(), selector.state_database())' \
  'MakerActorHeldLock::acquire_for(selector.swap_id(), selector.workflow_journal())' \
  'load_validated_xmr_taker_authority_bytes(selector.manifest_bytes())' \
  'selector.receipt_matches(&authority)' \
  'XMR Taker claim and refund are not yet composed' \
  'phase: "application_activated"' \
  'claim_session: "presignature_verified"' \
  'refund_session: "presignature_verified"'; do
  rg -Fq -- "$required" "$taker_cli" ||
    fail "Taker CLI omits receipt-only XMR monitor boundary: ${required}"
done

for required in \
  'pub fn load_validated_xmr_taker_authority_bytes(' \
  'load_validated_xmr_role_authority_bytes(bytes, ActorRole::Taker)?;'; do
  rg -Fq -- "$required" "$xmr_application_provision" ||
    fail "typed XMR Taker authority loader omits semantic validation: ${required}"
done

for required in \
  'MAX_TAKER_RECEIPT_BYTES' \
  'serde_json::to_vec(&receipt)? == bytes.as_slice()' \
  'normalized_absolute(path)' \
  'Sha256::digest(&manifest_bytes).as_slice() == expected_manifest.as_slice()' \
  'validate_taker_manifest_config_bytes(' \
  'pub(crate) fn receipt_matches('; do
  rg -Fq -- "$required" "$xmr_receipt_loader" ||
    fail "XMR receipt selector omits strict binding: ${required}"
done

for required in \
  'let monitor = run_taker_monitor(&fixture.receipt);' \
  '"phase": "application_activated"' \
  'before_monitor.assert_unchanged(&fixture);' \
  'for action in ["claim", "refund"]' \
  'write_receipt_with_unknown_field(&fixture.receipt, &unknown_receipt);' \
  '"actor_manifest_sha256",' \
  '"agreement_commitment",' \
  'assert!(!lock_path(&unbound_state).exists());' \
  'receipt-bound XMR Taker actor semantics changed' \
  'XMR Taker actor is already running or unsafe'; do
  rg -Fq -- "$required" "$xmr_process_test" ||
    fail "real XMR process test omits monitor proof: ${required}"
done

for required in \
  'readonly m5_xmr_journey="${M5_XMR_JOURNEY:-claim}"' \
  'readonly m5_xmr_refund_delay_ms="${M5_XMR_REFUND_DELAY_MS:-900000}"' \
  'M5_XMR_JOURNEY=refund requires M5_XMR_APPLICATION_MODE=1' \
  'M5_XMR_REFUND_DELAY_MS must be 600000..3600000 milliseconds' \
  'readonly m5_xmr_refund_window_ms=600000' \
  '--bin xmr-reference-tag16' \
  'readonly tag16_binary="${staged_binary_root}/xmr-reference-tag16"' \
  'wait_for_m5_xmr_refund_window() {' \
  '--max-blocks 1' \
  '.outcome.status=="absent" or .outcome.status=="uncertain"' \
  '.outcome.finalized_clock.timestamp_ms' \
  'tag16_scan_start_height="$((finalized_height + 1))"' \
  '(( finalized_timestamp_ms >= refund_at_ms && finalized_timestamp_ms < punish_at_ms ))' \
  'prepare_tag16_refund_signature() {' \
  '"${agreement_root}/stage-b/exchange/refund/taker-presignature.json"' \
  'publish_tag16_refund() {' \
  '"$tag16_binary" --sidecar-endpoint "$taker_endpoint"' \
  '--runtime-file "$tag13_handoff_root/taker-runtime.json"' \
  '--prepare-request-id "${run_id}-tag16-prepare-001"' \
  'classify_tag16_refund_finality() {' \
  '--role maker --effect refund' \
  'ingest-finalized-refund-signature' \
  '--private-root "${agreement_root}/material/maker"' \
  '--journal "${agreement_root}/stage-b/private/maker.sqlite"' \
  'extract_refund_adaptor_scalar() {' \
  '"$agreement_role_runner_binary" maker' \
  '--session "${agreement_root}/material/maker-sessions/refund.json"' \
  '--presignature "${agreement_root}/stage-b/exchange/refund/maker-presignature.json"' \
  'sweep_monero_refund() {' \
  '"$monero_sweep_binary" --journey refund' \
  '--maker-share-file "${agreement_root}/material/maker/xmr-share.key"' \
  '--extracted-taker-adaptor-scalar-file "$extracted_taker_scalar"' \
  '--shared-wallet-url "${monero_env[MONERO_FUNDING_WALLET_ENDPOINT]}"' \
  '--taker-wallet-url "${monero_env[MONERO_TAKER_WALLET_ENDPOINT]}"' \
  '--funding-wallet-url "${monero_env[MONERO_MAKER_WALLET_ENDPOINT]}"' \
  '.schema=="lez_v02_m5_actual_local_monero_refund_sweep_v3"' \
  '.journey=="refund" and .revealed_role=="taker_refund_signature" and .sweeping_role=="maker"' \
  '--target-wallet-url "${monero_env[MONERO_MAKER_WALLET_ENDPOINT]}"' \
  '--foreign-wallet-url "${monero_env[MONERO_TAKER_WALLET_ENDPOINT]}"' \
  'bind_refund_sweep() {' \
  'bind-finalized-refund-sweep' \
  '--refund-run-id "$run_id"' \
  '--monero-sweep-evidence "$monero_refund_sweep_evidence"' \
  '.schema=="lez_v02_m5_refund_cross_chain_binding_v1"' \
  '.atomicity_scope=="successful_refund_path_conditional_atomicity"'; do
  require_runner_source "$required" "M5 refund journey boundary: ${required}"
done

for required in \
  'readonly m5_xmr_refund_clock_stall_samples=2' \
  'readonly m5_xmr_refund_clock_max_ticks=1' \
  'readonly m5_xmr_refund_clock_punish_guard_ms=60000' \
  'readonly m5_xmr_refund_clock_progress_timeout_seconds=60' \
  '--bin lez-v02-local-clock-driver' \
  'readonly local_clock_driver_binary="${staged_binary_root}/lez-v02-local-clock-driver"' \
  'stage_executable "${sidecar_target}/debug/lez-v02-local-clock-driver"' \
  'drive_m5_xmr_local_finality_clock() {' \
  'local finalized-clock driving is restricted to the M5 refund journey' \
  '[[ "$tick_number" == 1 ]]' \
  'taker_owner="$(jq -er '\''.account_id_hex'\'' "${evidence_root}/taker-lez-identity.json")"' \
  'jq -e --arg run "$run_id" --arg sender "$taker_owner" --arg recipient "$maker_owner"' \
  'finalized_identity="${finalized_height}:${finalized_hash}:${finalized_timestamp_ms}"' \
  'if [[ "$finalized_identity" == "$previous_finalized_identity" ]]; then' \
  'identical_clock_samples >= m5_xmr_refund_clock_stall_samples' \
  'host_timestamp_ms >= refund_at_ms' \
  'host_timestamp_ms < punish_at_ms - m5_xmr_refund_clock_punish_guard_ms' \
  'clock_tick_count < m5_xmr_refund_clock_max_ticks' \
  'probe_height="$(drive_m5_xmr_local_finality_clock "$clock_tick_count")"' \
  '--finality-timeout-seconds "$m5_xmr_refund_clock_progress_timeout_seconds"' \
  'jq -er '\''.finalized_clock_after.height'\'' "$tick_evidence"' \
  '"$local_clock_driver_binary" \' \
  '--sidecar-endpoint "$taker_endpoint"' \
  '--capability-file "$taker_sidecar_root/capability"' \
  '--runtime-file "$tag13_handoff_root/taker-runtime.json"' \
  '--terms-file "$tag13_handoff_root/terms.json"' \
  '--recipient-account-id "$maker_owner"' \
  '--exclusive-punish-at-ms "$punish_at_ms"' \
  '.schema=="lez_v02_m5_local_clock_driver_v1"' \
  '.finalized_clock_before.height < .finalized_clock_after.height' \
  '.finalized_clock_after.height >= .clock_after.height' \
  '.finalized_clock_after.timestamp_ms >= $refund' \
  '.finalized_clock_after.timestamp_ms < $punish' \
  '.finalized_observation_attempts_before >= 1' \
  '.finalized_observation_attempts_after >= 1' \
  '.finality_source=="authenticated_genesis_bound_official_indexer"' \
  '.submission_request_id==.transaction_id' \
  '.node_submission_attempts==1' \
  '.transfer_amount==1' \
  '.sender_before.account_id==$sender and .sender_after.account_id==$sender' \
  '.recipient_before.account_id==$recipient and .recipient_after.account_id==$recipient' \
  '.sender_before.program_owner==.terms.authenticated_transfer_program_id' \
  '.sender_after.program_owner==.terms.authenticated_transfer_program_id' \
  '.recipient_before.program_owner==.terms.authenticated_transfer_program_id' \
  '.recipient_after.program_owner==.terms.authenticated_transfer_program_id' \
  '.sender_after.balance == (.sender_before.balance - 1)' \
  '.sender_after.nonce == (.sender_before.nonce + 1)' \
  '.recipient_after.balance == (.recipient_before.balance + 1)' \
  '.recipient_after.nonce == .recipient_before.nonce' \
  '.metadata_account_sha256_after==.metadata_account_sha256_before' \
  '.custody_account_sha256_after==.custody_account_sha256_before' \
  '.escrow_accounts_byte_identical==true' \
  '.accounting_verified==true' \
  '.local_only==true' \
  '.retry_policy=="one_node_submission_attempt_no_retry_poll_only"'; do
  require_runner_source "$required" "bounded authenticated local clock driver: ${required}"
done

clock_driver_source="$(sed -n '/^drive_m5_xmr_local_finality_clock() {$/,/^prepare_tag16_refund_signature() {$/p' "$runner")"
readonly clock_driver_source
if rg -n '(^|[^[:alnum:]_])(curl|wget|nc|socat)([^[:alnum:]_]|$)' <<<"$clock_driver_source" >/dev/null; then
  fail 'M5 local clock driver bypasses the authenticated sidecar/CLI boundary'
fi

stall_trigger_line="$(unique_line '^            identical_clock_samples >= m5_xmr_refund_clock_stall_samples \)\); then$' 'repeated-clock stall trigger')"
punish_guard_line="$(unique_line '^        \(\( host_timestamp_ms < punish_at_ms - m5_xmr_refund_clock_punish_guard_ms \)\) \|\|$' 'clock-driver punish guard')"
clock_tick_bound_line="$(unique_line '^        \(\( clock_tick_count < m5_xmr_refund_clock_max_ticks \)\) \|\|$' 'clock-driver fixed tick bound')"
clock_driver_call_line="$(unique_line '^        probe_height=.*drive_m5_xmr_local_finality_clock "\$clock_tick_count".*$' 'clock-driver invocation')"
readonly stall_trigger_line punish_guard_line clock_tick_bound_line clock_driver_call_line
(( stall_trigger_line < punish_guard_line && punish_guard_line < clock_tick_bound_line &&
   clock_tick_bound_line < clock_driver_call_line )) ||
  fail 'M5 local clock drive is not gated by repeated identity, punish guard, and tick bound'

if rg -n 'awaiting_tick_finality|tick_finality_identity|tick_finality_deadline_seconds' \
    <<<"$clock_driver_source" >/dev/null; then
  fail 'M5 runner still conflates fixed-window classification with current finalized progress'
fi
finalized_height_output_line="$(unique_line '^  jq -er '\''.finalized_clock_after.height'\'' "\$tick_evidence"$' 'driver finalized-height output')"
readonly finalized_height_output_line
(( finalized_height_output_line < clock_driver_call_line )) ||
  fail 'M5 runner invokes the clock driver before its finalized-height contract exists'

refund_wait_line="$(unique_line '^    wait_for_m5_xmr_refund_window$' 'refund-window wait invocation')"
refund_prepare_line="$(unique_line '^    prepare_tag16_refund_signature$' 'tag16 signature invocation')"
refund_publish_line="$(unique_line '^    publish_tag16_refund$' 'tag16 publication invocation')"
refund_classify_line="$(unique_line '^    classify_tag16_refund_finality$' 'tag16 classification invocation')"
refund_ingest_line="$(unique_line '^    ingest_refund_signature$' 'refund ingestion invocation')"
refund_extract_line="$(unique_line '^    extract_refund_adaptor_scalar$' 'refund extraction invocation')"
refund_sweep_line="$(unique_line '^    sweep_monero_refund$' 'refund sweep invocation')"
refund_bind_line="$(unique_line '^    bind_refund_sweep$' 'refund binding invocation')"
readonly refund_wait_line refund_prepare_line refund_publish_line refund_classify_line
readonly refund_ingest_line refund_extract_line refund_sweep_line refund_bind_line
(( refund_wait_line < refund_prepare_line && refund_prepare_line < refund_publish_line &&
   refund_publish_line < refund_classify_line && refund_classify_line < refund_ingest_line &&
   refund_ingest_line < refund_extract_line && refund_extract_line < refund_sweep_line &&
   refund_sweep_line < refund_bind_line )) ||
  fail 'M5 refund journey order is not wait/adapt/tag16/classify/ingest/extract/sweep/bind'

echo 'M5 XMR application-to-chain contract passed'
