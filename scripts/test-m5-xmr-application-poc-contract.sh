#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export LC_ALL=C

readonly wrapper="scripts/run-m5-xmr-application-poc.sh"
readonly runner="scripts/run-m4-actual-claim-poc.sh"

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
    "exact_replay_without_delivery",
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
  [[ "$(wc -l <<<"$matches" | tr -d ' ')" == 1 ]] ||
    fail "delegated runner repeats or omits ${label}"
  printf '%s\n' "${matches%%:*}"
}

for required in \
  'readonly m5_xmr_application_mode="${M5_XMR_APPLICATION_MODE:-0}"' \
  'M5_XMR_APPLICATION_MODE must be unset, 0, or 1' \
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
  'if [[ "$m5_xmr_application_mode" == 1 && -n "${m5_application_daemon_pid:-}" ]]; then' \
  'stop_m5_xmr_application_daemon || cleanup_failed=1'; do
  require_runner_source "$required" "M5 source boundary: ${required}"
done

[[ "$(rg -c 'cleanup_failed=0' "$runner")" == 1 ]] ||
  fail 'cleanup failure state is reset after an earlier identity/removal error'
if rg -Fq '# Cleanup is judged by the final resource state' "$runner"; then
  fail 'legacy cleanup-error reset comment survived the fail-closed fix'
fi

plan_line="$(unique_line '^prepare_m5_xmr_delivery_plan$' 'M5 plan invocation')"
compose_line="$(unique_line '^  compose_xmr_agreement$' 'agreement invocation')"
handoff_line="$(unique_line '^    complete_m5_xmr_application_handoff$' 'M5 handoff invocation')"
cutoff_line="$(unique_line '^    verify_m5_xmr_application_cutoff$' 'M5 cutoff invocation')"
tag13_line="$(unique_line '^  submit_tag13$' 'Tag13 invocation')"
readonly plan_line compose_line handoff_line cutoff_line tag13_line
(( plan_line < compose_line && compose_line < handoff_line &&
   handoff_line < cutoff_line && cutoff_line < tag13_line )) ||
  fail 'M5 application plan/handoff/cutoff does not precede legacy Tag13 exactly'

cleanup_line="$(unique_line '^cleanup\(\) \{$' 'cleanup function')"
cleanup_hook_line="$(unique_line '^    stop_m5_xmr_application_daemon \|\| cleanup_failed=1$' 'M5 cleanup hook')"
readonly cleanup_line cleanup_hook_line
(( cleanup_line < cleanup_hook_line )) || fail 'M5 daemon cleanup hook is outside cleanup()'

echo 'M5 XMR application-to-chain contract passed'
