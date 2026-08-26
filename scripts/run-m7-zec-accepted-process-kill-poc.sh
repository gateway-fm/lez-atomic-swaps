#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
umask 077

fail() {
  printf 'M7 accepted-ZEC process-kill PoC failed: %s\n' "$*" >&2
  exit 2
}

[[ -z "${M5_APPLICATION_MODE:-}" || "$M5_APPLICATION_MODE" == 1 ]] ||
  fail 'M5_APPLICATION_MODE is fixed to 1'
[[ -z "${M6_TAKER_SERVICE_MODE:-}" || "$M6_TAKER_SERVICE_MODE" == 0 ]] ||
  fail 'M6_TAKER_SERVICE_MODE is fixed to 0'
[[ -z "${M6_ZEC_JOURNEY:-}" || "$M6_ZEC_JOURNEY" == claim ]] ||
  fail 'M6_ZEC_JOURNEY is fixed to claim'
[[ -z "${POC_DIRECTION:-}" || "$POC_DIRECTION" == taker_sells_lez ]] ||
  fail 'POC_DIRECTION is fixed to taker_sells_lez'
[[ -z "${M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION:-}" \
  || "$M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION" == 1 ]] ||
  fail 'M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION is fixed to 1'

if [[ "${1:-}" == contract ]]; then
  [[ "$#" == 1 ]] || fail 'contract accepts no additional arguments'
  jq -n '
    {
      schema_version: 1,
      kind: "m7_zec_accepted_process_kill_poc_contract",
      execution_performed: false,
      application_mode: true,
      pair: "zcash",
      direction: "taker_sells_lez",
      journey: "claim",
      crash_boundary: "zcash_fund_submitted_before_actor_stdout",
      processes_killed: ["maker_daemon", "maker_zcash_actor_process_group"],
      kill_order: "daemon_then_actor",
      accepted_submission: {
        exact_singleton_mempool_transaction: true,
        confirmations_before_restart: 0,
        tip_unchanged_through_restart: true
      },
      restart: {
        same_database: true,
        abandoned_generation_transfer_required: true,
        old_process_identities_absent_required: true,
        observe_before_resend: true,
        automatic_resubmission_allowed: false
      },
      terminal: {both_roles_complete: true, scheduler_state: "terminal"},
      test_seam: {
        compile_time_feature_only: true,
        production_binary_exposes_hook: false
      },
      isolation: {literal_loopback_only: true, owner_private_build_cache: true},
      runtime_external_resources: [],
      public_rpc_used: false,
      faucet_used: false,
      public_funds_used: false,
      public_deployment: false
    }
  '
  exit 0
fi

export M5_APPLICATION_MODE=1
export M6_TAKER_SERVICE_MODE=0
export M6_ZEC_JOURNEY=claim
export POC_DIRECTION=taker_sells_lez
export M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION=1

# The delegated application runner retains the endpoint-tuple lock, exact
# actor/daemon identity checks, bounded corridor clock, real local chain
# effects, and owner-scoped cleanup. Crash-feature builds use the unique
# private run root by default; an explicitly configured canonical 0700 cache
# can be reused across retries without touching the repository's shared target.
exec ./scripts/run-m2-taker-sells-lez-poc.sh "$@"
