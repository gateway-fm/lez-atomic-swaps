#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export LC_ALL=C
umask 077

readonly repo_root="$(pwd)"
readonly m4_runner="scripts/run-m4-actual-claim-poc.sh"

fail() {
  echo "M5 XMR application-to-chain PoC failed: $*" >&2
  exit 2
}

[[ -z "${M5_XMR_APPLICATION_MODE:-}" || "$M5_XMR_APPLICATION_MODE" == 1 ]] ||
  fail 'M5_XMR_APPLICATION_MODE is fixed to 1'

emit_contract() {
  command -v jq >/dev/null || fail 'jq is required to emit the M5 XMR contract'
  jq -n '
    {
      schema_version: 1,
      kind: "m5_xmr_application_to_chain_poc_contract",
      milestone: "M5",
      scope: "xmr_application_to_chain_local_poc",
      execution_performed: false,
      application_mode: 1,
      pair: "monero",
      direction: "taker_sells_lez",
      certification: {
        status: "not_yet_executed",
        delegated_runner_splice_required: true,
        certifying_replay_performed: false
      },
      delegation: {
        runner: "scripts/run-m4-actual-claim-poc.sh",
        runner_mode: "execute",
        reuse: "exact_m4_actual_claim_runner",
        argument_policy: "forward_to_m4_fail_closed",
        opt_in_environment: {
          name: "M5_XMR_APPLICATION_MODE",
          value: "1"
        }
      },
      planned_order: [
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
      ],
      runtime_external_resources: {
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
      },
      hardening: {
        status: "open",
        qa: "open",
        chaos_engineering: "open",
        infosec: "open",
        production_readiness: "open"
      }
    }'
}

validate_execute_inputs() {
  command -v git >/dev/null || fail 'git is required for execute source validation'
  command -v rg >/dev/null || fail 'rg is required for execute runner validation'
  [[ -x "$m4_runner" && -f "$m4_runner" && ! -L "$m4_runner" ]] ||
    fail 'exact M4 actual-claim runner is unavailable or unsafe'
  [[ "${RUN_ID:-}" =~ ^[a-z0-9][a-z0-9_-]{7,47}$ ]] ||
    fail 'RUN_ID must be 8..48 lowercase letters, numbers, underscores, or hyphens'
  [[ "${M4_EXPECTED_COMMIT:-}" =~ ^[0-9a-f]{40}$ ]] ||
    fail 'M4_EXPECTED_COMMIT must be one lowercase 40-character Git object ID'

  local actual_commit dirty marker
  actual_commit="$(git rev-parse --verify HEAD)"
  [[ "$actual_commit" == "$M4_EXPECTED_COMMIT" ]] ||
    fail 'HEAD differs from M4_EXPECTED_COMMIT'
  [[ "$(git rev-parse --show-toplevel)" == "$repo_root" ]] ||
    fail 'repository root identity drift'
  dirty="$(git status --porcelain=v1 --untracked-files=normal)"
  [[ -z "$dirty" ]] || fail 'exact-commit replay requires a clean worktree'
  git diff --quiet --exit-code || fail 'unstaged tracked source differs'
  git diff --cached --quiet --exit-code || fail 'staged source differs'

  for marker in \
    'M5_XMR_APPLICATION_MODE' \
    'prepare_m5_xmr_delivery_plan() {' \
    'complete_m5_xmr_application_handoff() {'; do
    rg -Fq -- "$marker" "$m4_runner" ||
      fail "delegated M4 runner lacks the M5 XMR application splice: ${marker}"
  done
}

mode="${1:-}"
[[ -n "$mode" ]] || fail 'expected contract or execute'
shift
case "$mode" in
  contract)
    [[ "$#" == 0 ]] || fail 'contract accepts no arguments'
    emit_contract
    ;;
  execute)
    validate_execute_inputs
    exec env M5_XMR_APPLICATION_MODE=1 "$m4_runner" execute "$@"
    ;;
  *) fail 'expected contract or execute' ;;
esac
