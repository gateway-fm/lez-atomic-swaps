#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly process_test="crates/maker-node/tests/daemon_actor_supervisor_process.rs"
readonly wrapper="scripts/run-m7-xmr-accepted-concurrency-poc.sh"
readonly delegated_runner="scripts/run-m4-actual-claim-poc.sh"

fail() {
  echo "M7 XMR accepted-concurrency contract failed: $*" >&2
  exit 1
}

for required in \
  'fn daemon_leases_two_accepted_xmr_applications_concurrently_across_restart()' \
  'two accepted XMR applications must hold concurrent leases' \
  'distinct XMR actor configurations' \
  'distinct XMR actor state databases' \
  'accepted XMR restart must preserve terminal isolation'; do
  rg -Fq -- "$required" "$process_test" ||
    fail "process test is missing invariant: $required"
done

[[ -x "$wrapper" ]] || fail "actual-node concurrency wrapper is absent or not executable"
contract="$($wrapper contract)" || fail "actual-node concurrency contract command failed"
jq -e '
  .schema_version == 1
  and .kind == "m7_xmr_accepted_concurrency_poc_contract"
  and .execution_performed == false
  and .pair == "monero"
  and .direction == "taker_sells_lez"
  and .journey == "claim"
  and .accepted_swap_count == 2
  and .shared_application_boundary == {
    maker_daemon_count:1,maker_database_count:1,delivery_directory_count:1,
    chat_socket_count:1,actor_worker_count:2
  }
  and .shared_chain_boundary == {
    lez_v0_2_stack_count:1,monerod_regtest_count:1,program_deployment_count:1
  }
  and all(.isolation[]; . == true)
  and .ordering.both_applications_accepted_before_actor_activation == true
  and .ordering.both_swaps_in_flight_before_settlement == true
  and .replay.one_daemon_restart_before_activation == true
  and .replay.terminal_resubmission_count == 0
  and ([.runtime_external_resources.public_rpc,
        .runtime_external_resources.public_peer,
        .runtime_external_resources.faucet,
        .runtime_external_resources.public_funds,
        .runtime_external_resources.public_deployment] | all(. == false))
' <<<"$contract" >/dev/null || fail "actual-node concurrency contract is incomplete"

for variable in M5_XMR_APPLICATION_MODE M7_XMR_ACCEPTED_CONCURRENCY \
  M5_XMR_JOURNEY M7_XMR_SEMANTIC_CLAIM; do
  case "$variable" in
    M5_XMR_APPLICATION_MODE|M7_XMR_ACCEPTED_CONCURRENCY|M7_XMR_SEMANTIC_CLAIM) value=0 ;;
    M5_XMR_JOURNEY) value=refund ;;
  esac
  if env "$variable=$value" "$wrapper" contract >/dev/null 2>&1; then
    fail "actual-node concurrency wrapper accepted conflicting $variable override"
  fi
done

for required in \
  'export M5_XMR_APPLICATION_MODE=1' \
  'export M7_XMR_ACCEPTED_CONCURRENCY=1' \
  'export M5_XMR_JOURNEY=claim' \
  'export M7_XMR_SEMANTIC_CLAIM=1' \
  'exec ./scripts/run-m5-xmr-application-poc.sh "$@"'; do
  rg -Fq -- "$required" "$wrapper" ||
    fail "actual-node concurrency wrapper is missing delegation: $required"
done

for required in \
  'M7_XMR_ACCEPTED_CONCURRENCY' \
  'M7_XMR_ACCEPTED_CONCURRENCY=1 requires application semantic-claim mode' \
  'accepted_swap_count: 2' \
  'shared_daemon: true' \
  'shared_lez_stack: true' \
  'shared_monerod: true' \
  'both_accepted_before_activation: true' \
  '--actor-worker-count "$((m7_xmr_accepted_concurrency == 1 ? 2 : 1))"' \
  'm7_xmr_second_offer_id="m7-xmr-application-offer-002"' \
  'compose_m7_second_xmr_agreement() {' \
  '--shared-view-key-source "${agreement_root}/material/taker/monero-view.key"' \
  '--maker-agreement-key-source "${agreement_root}/material/maker/agreement.key"' \
  'run_m7_second_xmr_taker_acceptance() {' \
  'wait_m7_second_xmr_typed_blocked() {' \
  'accepted_swap_count:2,shared_daemon:true,shared_database:true,shared_chat:true' \
  'second_replay_acceptance_swap_id:$second_replay' \
  'M7 accepted XMR Maker actor authority aliases'; do
  rg -Fq -- "$required" "$delegated_runner" ||
    fail "delegated runner is missing accepted-concurrency invariant: $required"
done

cargo +1.96.0 test --locked --offline -p lez-maker-node \
  --test daemon_actor_supervisor_process \
  daemon_leases_two_accepted_xmr_applications_concurrently_across_restart \
  -- --exact

rg -Fq './scripts/test-m7-xmr-accepted-concurrency-contract.sh' scripts/run-ci-quality-gates.sh ||
  fail "contract is absent from the quality runner"
rg -Fq './scripts/test-m7-xmr-accepted-concurrency-contract.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI policy does not pin the functional concurrency contract"

echo "M7 XMR accepted-concurrency contract passed"
