#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly wrapper="scripts/run-m7-zec-accepted-process-kill-poc.sh"
readonly runner="scripts/run-m2-taker-sells-lez-poc.sh"

fail() {
  echo "M7 accepted-ZEC process-kill contract failed: $*" >&2
  exit 1
}

[[ -x "$wrapper" ]] || fail "wrapper is absent or not executable"

contract="$($wrapper contract)" || fail "wrapper contract command failed"
jq -e '
  .schema_version == 1
  and .kind == "m7_zec_accepted_process_kill_poc_contract"
  and .execution_performed == false
  and .application_mode == true
  and .pair == "zcash"
  and .direction == "taker_sells_lez"
  and .journey == "claim"
  and .crash_boundary == "zcash_fund_submitted_before_actor_stdout"
  and .processes_killed == ["maker_daemon", "maker_zcash_actor_process_group"]
  and .kill_order == "daemon_then_actor"
  and .accepted_submission.exact_singleton_mempool_transaction == true
  and .accepted_submission.confirmations_before_restart == 0
  and .accepted_submission.tip_unchanged_through_restart == true
  and .restart.same_database == true
  and .restart.abandoned_generation_transfer_required == true
  and .restart.old_process_identities_absent_required == true
  and .restart.observe_before_resend == true
  and .restart.automatic_resubmission_allowed == false
  and .terminal.both_roles_complete == true
  and .terminal.scheduler_state == "terminal"
  and .test_seam.compile_time_feature_only == true
  and .test_seam.production_binary_exposes_hook == false
  and .isolation.literal_loopback_only == true
  and .isolation.owner_private_build_cache == true
  and .runtime_external_resources == []
  and .public_rpc_used == false
  and .faucet_used == false
  and .public_funds_used == false
  and .public_deployment == false
' <<<"$contract" >/dev/null || fail "wrapper contract is incomplete"

for variable in M5_APPLICATION_MODE M6_TAKER_SERVICE_MODE M6_ZEC_JOURNEY \
  POC_DIRECTION M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION; do
  case "$variable" in
    M5_APPLICATION_MODE) value=0 ;;
    M6_TAKER_SERVICE_MODE) value=1 ;;
    M6_ZEC_JOURNEY) value=refund ;;
    POC_DIRECTION) value=taker_sells_foreign ;;
    M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION) value=0 ;;
  esac
  if env "$variable=$value" "$wrapper" contract >/dev/null 2>&1; then
    fail "wrapper accepted conflicting ${variable} override"
  fi
done

for required in \
  'export M5_APPLICATION_MODE=1' \
  'export M6_TAKER_SERVICE_MODE=0' \
  'export M6_ZEC_JOURNEY=claim' \
  'export POC_DIRECTION=taker_sells_lez' \
  'export M7_ZEC_ACCEPTED_PROCESS_KILL_AFTER_SUBMISSION=1' \
  'exec ./scripts/run-m2-taker-sells-lez-poc.sh "$@"'; do
  rg -Fq -- "$required" "$wrapper" ||
    fail "wrapper is missing fixed delegation: $required"
done

for required in \
  'M7_ZEC_CRASH_BUILD_CACHE_ROOT' \
  '--features test-crash-hooks' \
  'actor_attempt_timeout_ms=120000' \
  'process_start_identity_matches' \
  'inject_m7_zec_accepted_process_kill_if_ready' \
  'M7 accepted-ZEC scheduler does not bind the exact leased actor identity' \
  'M7 accepted-ZEC crash marker does not bind the leased Maker actor' \
  'M7 accepted-ZEC crash boundary lacks the exact singleton funding transaction' \
  'kill -KILL "$crashed_daemon_pid"' \
  'kill -KILL -- "-${crashed_actor_pid}"' \
  'start_m5_full_supervised_daemon recovery' \
  'M7 accepted-ZEC restart did not transfer the abandoned actor lease' \
  'confirmations_mined_before_restart:0' \
  'old_process_identities_absent:true' \
  'automatic_resubmission_observed:false' \
  'production_binary_exposes_crash_hook:false' \
  'accepted_zec_process_kill_recovery'; do
  rg -Fq -- "$required" "$runner" ||
    fail "delegated runner is missing invariant: $required"
done

[[ "$(rg -Fc 'inject_m7_zec_accepted_process_kill_if_ready' "$runner")" -ge 3 ]] ||
  fail "runner lacks definition plus both pre-observation and pre-mining gates"
[[ "$(rg -Fc 'process_start_identity_matches' "$runner")" -ge 7 ]] ||
  fail "runner lacks exact pre-kill binding plus post-kill identity-absence checks"

rg -Fq './scripts/test-m7-zec-accepted-process-kill-contract.sh' \
  scripts/run-ci-quality-gates.sh || fail "contract is absent from the quality runner"
rg -Fq './scripts/test-m7-zec-accepted-process-kill-contract.sh' \
  scripts/test-ci-hardening-policy.sh || fail "CI hardening does not pin the contract"

echo "M7 accepted-ZEC process-kill contract passed"
