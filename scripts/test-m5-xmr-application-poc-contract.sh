#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export LC_ALL=C

readonly wrapper="scripts/run-m5-xmr-application-poc.sh"

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

echo 'M5 XMR application-to-chain contract passed'
