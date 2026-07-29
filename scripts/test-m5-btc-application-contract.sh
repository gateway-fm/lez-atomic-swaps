#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly wrapper="scripts/run-m5-btc-application-poc.sh"

fail() {
  echo "M5 BTC application contract failed: $*" >&2
  exit 1
}

[[ -x "$wrapper" ]] || fail "BTC wrapper is absent or not executable"

contract="$($wrapper contract)" || fail "BTC wrapper contract command failed"
jq -e '
  .schema_version == 1 and
  .kind == "m5_btc_application_poc_contract" and
  .execution_performed == false and
  .application_mode == 1 and
  .pair == "bitcoin" and
  .direction == "taker_sells_foreign" and
  .asset_mode == "native" and
  .schedule == "sequential" and
  .journey == "claim" and
  .application_order == [
    "delivery_only_daemon",
    "maker_publish",
    "taker_plan",
    "derived_swap_id",
    "stage_two",
    "canonical_draft_export",
    "authorized_daemon_and_taker",
    "role_fixed_provisioning",
    "actual_role_actors"
  ] and
  .runtime_external_resources.public_rpc == false and
  .runtime_external_resources.faucet == false and
  .runtime_external_resources.public_funds == false and
  .runtime_external_resources.bitcoin_core == "isolated_regtest" and
  .runtime_external_resources.lez == "isolated_v0_2"
' <<<"$contract" >/dev/null || fail "BTC wrapper contract is incomplete"

for case_name in application_mode asset_mode schedule journey; do
  case "$case_name" in
    application_mode) command=(env M5_BTC_APPLICATION_MODE=0 "$wrapper" contract) ;;
    asset_mode) command=(env M3_ACTOR_POC_ASSET_MODE=custom_token "$wrapper" contract) ;;
    schedule) command=(env M3_ACTOR_POC_SCHEDULE=overlap "$wrapper" contract) ;;
    journey) command=(env M3_ACTOR_POC_JOURNEY=refund "$wrapper" contract) ;;
  esac
  if "${command[@]}" >/dev/null 2>&1; then
    fail "BTC wrapper accepted conflicting ${case_name} override"
  fi
done

for required in \
  'export M5_BTC_APPLICATION_MODE=1' \
  'export M3_ACTOR_POC_ASSET_MODE=native' \
  'export M3_ACTOR_POC_SCHEDULE=sequential' \
  'export M3_ACTOR_POC_JOURNEY=claim' \
  'exec ./scripts/run-m3-actor-local-poc.sh "$@"'; do
  rg -Fq -- "$required" "$wrapper" ||
    fail "BTC wrapper is missing fixed delegation: ${required}"
done

echo "M5 BTC application contract passed"
