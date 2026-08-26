#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly process_test="crates/maker-node/tests/daemon_actor_supervisor_process.rs"
readonly wrapper="scripts/run-m7-xmr-accepted-concurrency-poc.sh"
readonly delegated_runner="scripts/run-m4-actual-claim-poc.sh"
readonly funding_source="compat/lez-v0_2-sidecar/src/bin/lez-v02-xmr-regtest-fund.rs"
readonly taker_cli_source="crates/maker-node/src/bin/lez-taker.rs"

fail() {
  echo "M7 XMR accepted-concurrency contract failed: $*" >&2
  exit 1
}

for required in \
  'fn daemon_leases_two_accepted_xmr_applications_concurrently_across_restart()' \
  'fn wait_for_concurrent_leases(' \
  'two accepted XMR applications to hold concurrent leases' \
  'restarted accepted XMR applications to hold concurrent leases' \
  'distinct XMR actor configurations' \
  'distinct XMR actor state databases' \
  'accepted XMR restart must preserve terminal isolation'; do
  rg -Fq -- "$required" "$process_test" ||
    fail "process test is missing invariant: $required"
done

[[ "$(rg -Fc -- 'wait_for_concurrent_leases(' "$process_test")" == 3 ]] ||
  fail "bounded concurrent-lease helper must be called before and after restart"

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
  and .isolation.distinct_taker_lez_identities == true
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
  'for role in maker taker taker-b' \
  'LEZ_V02_TAKER_B_ACCOUNT_ID="$taker_b_account"' \
  'taker_b_identity="${evidence_root}/taker-b-lez-identity.json"' \
  'M4_ONBOARD_TAKER_B_IDENTITY="$taker_b_identity"' \
  'compose_m7_second_xmr_agreement() {' \
  'taker_owner="$(jq -er '\''.account_id_hex'\'' "${evidence_root}/taker-b-lez-identity.json")"' \
  '--shared-view-key-source "${agreement_root}/material/taker/monero-view.key"' \
  '--maker-agreement-key-source "${agreement_root}/material/maker/agreement.key"' \
  'run_m7_second_xmr_taker_acceptance() {' \
  'wait_m7_second_xmr_typed_blocked() {' \
  'submit_m7_second_xmr_tag13() {' \
  'fund_and_verify_m7_second_monero() {' \
  'provision_m7_second_taker_claim_effect_application() {' \
  'activate_and_run_m7_second_taker_tag14() {' \
  'activate_and_run_m7_second_taker_claim_sweep() {' \
  'verify_m7_xmr_accepted_concurrency_terminal_replay() {' \
  'accepted_swap_count:2,shared_daemon:true,shared_database:true,shared_chat:true' \
  'second_replay_acceptance_swap_id:$second_replay' \
  'M7 accepted XMR Maker actor authority aliases'; do
  rg -Fq -- "$required" "$delegated_runner" ||
    fail "delegated runner is missing accepted-concurrency invariant: $required"
done

[[ "$(rg -Fc -- '--private-key-file "${private_root}/lez-identities/taker-b/lez-signer.key"' \
  "$delegated_runner")" == 2 ]] ||
  fail "second Tag13 and second Taker sidecar do not share the distinct Taker B signer"
[[ "$(rg -Fc -- '--private-key-file "${private_root}/lez-identities/taker/lez-signer.key"' \
  "$delegated_runner")" == 2 ]] ||
  fail "primary Taker mutations no longer use only the primary Taker signer"

primary_agreement="$(sed -n '/^compose_xmr_agreement() {/,/^}/p' "$delegated_runner")"
secondary_agreement="$(sed -n '/^compose_m7_second_xmr_agreement() {/,/^}/p' "$delegated_runner")"
primary_tag13="$(sed -n '/^submit_tag13() {/,/^}/p' "$delegated_runner")"
secondary_tag13="$(sed -n '/^submit_m7_second_xmr_tag13() {/,/^}/p' "$delegated_runner")"
primary_sidecars="$(sed -n '/^start_role_sidecars() {/,/^}/p' "$delegated_runner")"
secondary_sidecars="$(sed -n '/^start_m7_second_xmr_role_sidecars() {/,/^}/p' "$delegated_runner")"
rg -Fq -- 'taker-lez-identity.json' <<<"$primary_agreement" &&
  ! rg -Fq -- 'taker-b-lez-identity.json' <<<"$primary_agreement" &&
  rg -Fq -- 'taker-b-lez-identity.json' <<<"$secondary_agreement" ||
  fail "agreement A/B Taker authority binding crossed"
rg -Fq -- 'lez-identities/taker/lez-signer.key' <<<"$primary_tag13" &&
  ! rg -Fq -- 'lez-identities/taker-b/lez-signer.key' <<<"$primary_tag13" &&
  rg -Fq -- 'lez-identities/taker-b/lez-signer.key' <<<"$secondary_tag13" ||
  fail "Tag13 A/B signer binding crossed"
rg -Fq -- 'lez-identities/taker/lez-signer.key' <<<"$primary_sidecars" &&
  ! rg -Fq -- 'lez-identities/taker-b/lez-signer.key' <<<"$primary_sidecars" &&
  rg -Fq -- 'lez-identities/taker-b/lez-signer.key' <<<"$secondary_sidecars" ||
  fail "sidecar A/B signer binding crossed"

first_funding_line="$(rg -n '^[[:space:]]+fund_and_verify_monero$' "$delegated_runner" | tail -n 1 | cut -d: -f1)"
second_funding_line="$(rg -n '^[[:space:]]+fund_and_verify_m7_second_monero$' "$delegated_runner" | cut -d: -f1)"
first_preparation_line="$(rg -n '^[[:space:]]+prepare_tag14_release$' "$delegated_runner" | head -n 1 | cut -d: -f1)"
first_settlement_line="$(rg -n '^[[:space:]]+activate_and_run_m7_taker_tag14$' "$delegated_runner" | tail -n 1 | cut -d: -f1)"
[[ "$first_funding_line" =~ ^[1-9][0-9]*$ &&
   "$second_funding_line" =~ ^[1-9][0-9]*$ &&
   "$first_preparation_line" =~ ^[1-9][0-9]*$ &&
   "$first_settlement_line" =~ ^[1-9][0-9]*$ &&
   "$first_funding_line" -lt "$first_preparation_line" &&
   "$first_preparation_line" -lt "$second_funding_line" &&
   "$first_funding_line" -lt "$first_settlement_line" &&
   "$second_funding_line" -lt "$first_settlement_line" ]] ||
  fail "XMR output preparation or both-in-flight settlement ordering drifted"

refresh_line="$(rg -n '\.refresh_from_height\(arguments\.restore_height\)' "$funding_source" | head -n 1 | cut -d: -f1)"
fund_line="$(rg -n '\.fund_shared_exact_and_confirm\(' "$funding_source" | head -n 1 | cut -d: -f1)"
[[ "$refresh_line" =~ ^[1-9][0-9]*$ && "$fund_line" =~ ^[1-9][0-9]*$ &&
   "$refresh_line" -lt "$fund_line" ]] ||
  fail "sequential XMR funding does not refresh the Maker wallet before transfer"

rg -Fq 'const XMR_EFFECT_OBSERVATION_TIMEOUT: Duration = Duration::from_mins(2);' \
  "$taker_cli_source" ||
  fail "exact LEZ finality observation lacks the measured 120-second completion bound"
[[ "$(rg -Fc 'child.wait_timeout(XMR_EFFECT_OBSERVATION_TIMEOUT)' "$taker_cli_source")" == 1 ]] ||
  fail "the measured XMR observation bound is not limited to the read-only observer"
observer_function_line="$(rg -n '^fn observe_xmr_taker_effect\(' "$taker_cli_source" | cut -d: -f1)"
observer_timeout_line="$(rg -n 'child\.wait_timeout\(XMR_EFFECT_OBSERVATION_TIMEOUT\)' \
  "$taker_cli_source" | cut -d: -f1)"
[[ "$observer_function_line" =~ ^[1-9][0-9]*$ &&
   "$observer_timeout_line" =~ ^[1-9][0-9]*$ &&
   "$observer_function_line" -lt "$observer_timeout_line" ]] ||
  fail "the measured completion bound does not belong to read-only observation"

cargo +1.96.0 test --locked --offline -p lez-maker-node \
  --test daemon_actor_supervisor_process \
  daemon_leases_two_accepted_xmr_applications_concurrently_across_restart \
  -- --exact

rg -Fq './scripts/test-m7-xmr-accepted-concurrency-contract.sh' scripts/run-ci-quality-gates.sh ||
  fail "contract is absent from the quality runner"
rg -Fq './scripts/test-m7-xmr-accepted-concurrency-contract.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI policy does not pin the functional concurrency contract"

echo "M7 XMR accepted-concurrency contract passed"
