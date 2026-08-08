#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly wrapper="scripts/run-m7-btc-accepted-concurrency-poc.sh"

fail() {
  echo "M7 BTC accepted-concurrency contract failed: $*" >&2
  exit 1
}

[[ -x "$wrapper" ]] || fail "M7 accepted-concurrency wrapper is absent or not executable"

contract="$($wrapper contract)" || fail "M7 wrapper contract command failed"
jq -e '
  .schema_version == 1 and
  .kind == "m7_btc_accepted_concurrency_poc_contract" and
  .execution_performed == false and
  .application_mode == 1 and
  .accepted_application_concurrency == true and
  .pair == "bitcoin" and
  .asset_mode == "native" and
  .schedule == "overlap" and
  .journey == "claim" and
  .accepted_swap_count == 2 and
  .directions == ["taker_sells_foreign", "taker_sells_lez"] and
  .shared_application_boundary == {
    maker_daemon_count: 1,
    maker_database_count: 1,
    delivery_directory_count: 1,
    chat_socket_count: 1,
    actor_worker_count: 2
  } and
  .isolation.distinct_swap_ids == true and
  .isolation.distinct_agreements == true and
  .isolation.distinct_actor_stores == true and
  .isolation.distinct_signing_journals == true and
  .isolation.distinct_escrows == true and
  .isolation.distinct_deadlines == true and
  .ordering.both_applications_accepted_before_actor_activation == true and
  .ordering.both_swaps_locked_before_settlement == true and
  .restart.one_daemon_restart_before_activation == true and
  .restart.no_acceptance_or_actor_replay == true and
  .runtime_external_resources.public_rpc == false and
  .runtime_external_resources.faucet == false and
  .runtime_external_resources.public_funds == false and
  .runtime_external_resources.bitcoin_core == "isolated_regtest" and
  .runtime_external_resources.lez == "isolated_v0_2"
' <<<"$contract" >/dev/null || fail "M7 wrapper contract is incomplete"

for variable in M5_BTC_APPLICATION_MODE M7_BTC_ACCEPTED_CONCURRENCY \
  M3_ACTOR_POC_ASSET_MODE M3_ACTOR_POC_SCHEDULE M3_ACTOR_POC_JOURNEY; do
  case "$variable" in
    M5_BTC_APPLICATION_MODE) value=0 ;;
    M7_BTC_ACCEPTED_CONCURRENCY) value=0 ;;
    M3_ACTOR_POC_ASSET_MODE) value=custom_token ;;
    M3_ACTOR_POC_SCHEDULE) value=sequential ;;
    M3_ACTOR_POC_JOURNEY) value=refund ;;
  esac
  if env "$variable=$value" "$wrapper" contract >/dev/null 2>&1; then
    fail "M7 wrapper accepted conflicting $variable override"
  fi
done

for required in \
  'export M5_BTC_APPLICATION_MODE=1' \
  'export M7_BTC_ACCEPTED_CONCURRENCY=1' \
  'export M3_ACTOR_POC_ASSET_MODE=native' \
  'export M3_ACTOR_POC_SCHEDULE=overlap' \
  'export M3_ACTOR_POC_JOURNEY=claim' \
  'exec ./scripts/run-m3-actor-local-poc.sh "$@"'; do
  rg -Fq -- "$required" "$wrapper" ||
    fail "M7 wrapper is missing fixed delegation: $required"
done

readonly delegated_runner="scripts/run-m3-actor-local-poc.sh"
for required in \
  'M7_BTC_ACCEPTED_CONCURRENCY=1 requires M5_BTC_APPLICATION_MODE=1' \
  '--maker-signing-key-file' \
  'assert_m7_shared_maker_identity' \
  'kind:"m7_shared_maker_identity"' \
  'swap_specific_authority_distinct:true' \
  'prepare_m5_btc_delivery_plan "${directions[@]}"' \
  'overlap_actor_config()' \
  'config="$(overlap_actor_config "$direction" "$role")"' \
  'handoff_config="$(overlap_actor_config "$direction" "$role")"' \
  'M7 terminal replay acceptance escaped shared authority' \
  '(map(.swap_id) | unique | length) == 2' \
  '(map(.offer_id) | unique | length) == 2' \
  '(map(.reservation_id) | unique | length) == 2' \
  'M3_POC_M7_APPLICATION_ROOT="${private_dir}/m7-application"' \
  'M7_BTC_ACCEPTED_CONCURRENCY="$m7_btc_accepted_concurrency"'; do
  rg -Fq -- "$required" "$delegated_runner" ||
    fail "delegated runner is missing M7 stage-one invariant: $required"
done

readonly direction_runner="scripts/run-m3-actor-direction.sh"
for required in \
  'complete_m7_btc_application_handoff' \
  'M3_POC_M7_APPLICATION_ROOT' \
  '--btc-source-maker-config' \
  'm7-application-handoff.json' \
  'post_acceptance_restart:true' \
  'accepted_swap_count:2'; do
  rg -Fq -- "$required" "$direction_runner" ||
    fail "direction runner is missing shared M7 handoff invariant: $required"
done

echo "M7 BTC accepted-concurrency contract passed"
