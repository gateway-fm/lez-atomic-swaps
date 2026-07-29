#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly runner="scripts/run-m3-actor-local-poc.sh"
readonly direction_driver="scripts/run-m3-actor-direction.sh"

fail() {
  echo "M5 BTC runner splice contract failed: $*" >&2
  exit 1
}

for required in \
  'readonly m5_btc_application_mode="${M5_BTC_APPLICATION_MODE:-0}"' \
  'M5_BTC_APPLICATION_MODE must be 0 or 1' \
  'M5_BTC_APPLICATION_MODE=1 requires M3_ACTOR_POC_ASSET_MODE=native' \
  'M5_BTC_APPLICATION_MODE=1 requires M3_ACTOR_POC_SCHEDULE=sequential' \
  'M5_BTC_APPLICATION_MODE=1 requires M3_ACTOR_POC_JOURNEY=claim' \
  'directions=(taker_sells_foreign)' \
  'm5_btc_application_mode: ($m5_btc_application_mode == "1")'; do
  rg -Fq -- "$required" "$runner" ||
    fail "outer runner is missing opt-in contract: ${required}"
done

contract="$(
  RUN_ID=m5btc-contract \
  M5_BTC_APPLICATION_MODE=1 \
  M3_ACTOR_POC_MODE=contract \
  M3_ACTOR_POC_ASSET_MODE=native \
  M3_ACTOR_POC_SCHEDULE=sequential \
  M3_ACTOR_POC_JOURNEY=claim \
    "$runner"
)" || fail "outer runner rejected the supported M5 BTC contract"

jq -e '
  .execution_performed == false and
  .m5_btc_application_mode == true and
  .asset_mode == "native" and
  .schedule == "sequential" and
  .journey == "claim" and
  .directions == ["taker_sells_foreign"] and
  .application_route == {
    pair: "bitcoin",
    direction: "taker_sells_foreign",
    delivery_before_stage_two: true,
    authenticated_swap_id: true,
    real_maker_cli: true,
    real_taker_cli: true,
    schema_6_role_provisioning: true
  }
' <<<"$contract" >/dev/null || fail "outer runner emitted the wrong M5 BTC contract"

for required in \
  'readonly m5_btc_application_mode="${M5_BTC_APPLICATION_MODE:-0}"' \
  'required M5 BTC environment is missing: M3_POC_SWAP_ID' \
  'M3_POC_SWAP_ID must be a canonical 32-byte lowercase hex value' \
  'swap_id="$M3_POC_SWAP_ID"' \
  'swap_id="$(openssl rand -hex 32)"'; do
  rg -Fq -- "$required" "$direction_driver" ||
    fail "direction driver is missing authenticated swap-ID contract: ${required}"
done

direction_contract="$(
  M5_BTC_APPLICATION_MODE=1 \
  M3_POC_ASSET_MODE=native \
    "$direction_driver" contract
)" || fail "direction driver rejected the supported M5 BTC contract"
jq -e '
  .m5_btc_application_mode == true and
  .stage_two_swap_id_source == "authenticated_delivery_reservation" and
  .actor_config_schema_version == 6 and
  .application_route == {
    pair: "bitcoin",
    direction: "taker_sells_foreign",
    asset_mode: "native",
    journey: "claim"
  }
' <<<"$direction_contract" >/dev/null ||
  fail "direction driver emitted the wrong authenticated swap-ID contract"

for required in \
  'prepare_m5_btc_delivery_plan() {' \
  'setsid "$maker_daemon_bin"' \
  'register_owned_process m5-btc-delivery planning' \
  'configure-pair --request-id' \
  'set-local-price --request-id' \
  'publish-offer --request-id' \
  '--plan-btc-offer "$offer_id"' \
  '--reservation-id "$reservation_id"' \
  '.private_material_disclosed == false' \
  'm5_btc_swap_ids["$direction"]="$swap_id"' \
  'M3_POC_SWAP_ID="$m5_swap_id"' \
  'stop_provisional_owned_process "$daemon_pid"'; do
  rg -Fq -- "$required" "$runner" ||
    fail "outer runner is missing real Delivery planning: ${required}"
done

for required in \
  'schema=6' \
  '--arg agreement_sha256 "$agreement_sha256"' \
  '{agreement_sha256:$agreement_sha256}' \
  'write_actor_configs "$initial_tip" 4096'; do
  rg -Fq -- "$required" "$direction_driver" ||
    fail "direction driver is missing schema-6 agreement binding: ${required}"
done

for required in \
  'complete_m5_btc_application_handoff() {' \
  'export-draft' \
  '--btc-source-maker-config "$maker_source_config"' \
  '--btc-maker-actor-root "$maker_actor_root"' \
  '--accept-btc-offer "$offer_id"' \
  '--btc-source-taker-config "$taker_source_config"' \
  '--btc-taker-actor-root "$taker_actor_root"' \
  '--btc-acceptance-receipt "$receipt_file"' \
  'm5_btc_actor_configs[maker]="$maker_config"' \
  'm5_btc_actor_configs[taker]="$taker_config"' \
  'actor_runtime_config() {' \
  'register_m5_application_process' \
  'stop_m5_application_process'; do
  rg -Fq -- "$required" "$direction_driver" ||
    fail "direction driver is missing full BTC application handoff: ${required}"
done

runtime_config_consumers="$(
  rg -Fc 'config="$(actor_runtime_config "$role")"' "$direction_driver"
)"
[[ "$runtime_config_consumers" == 8 ]] ||
  fail "all nine actor invocations must resolve one of eight role config declarations"
source_config_assignments="$(
  rg -Fc 'config="${M3_POC_DIRECTION_ROOT}/actors/${role}/actor-config.json"' \
    "$direction_driver"
)"
[[ "$source_config_assignments" == 2 ]] || fail "source config assignments drifted"

initial_config_line="$(
  rg -n -F 'write_actor_configs "$initial_tip" 4096' "$direction_driver" |
    tail -n1 | cut -d: -f1
)"
handoff_line="$(
  rg -n -F 'complete_m5_btc_application_handoff' "$direction_driver" |
    tail -n1 | cut -d: -f1
)"
activate_line="$(rg -n -F 'activate_actors' "$direction_driver" | tail -n1 | cut -d: -f1)"
[[ "$initial_config_line" =~ ^[0-9]+$ && "$handoff_line" =~ ^[0-9]+$ &&
   "$activate_line" =~ ^[0-9]+$ && "$initial_config_line" -lt "$handoff_line" &&

   "$handoff_line" -lt "$activate_line" ]] ||
  fail "BTC application handoff must publish actor bundles before activation"
plan_line="$(rg -n -F 'prepare_m5_btc_delivery_plan "$direction"' "$runner" |
  cut -d: -f1)"
stage_two_line="$(rg -n -F 'run_stage_two "$direction"' "$runner" | tail -n1 |
  cut -d: -f1)"
[[ "$plan_line" =~ ^[0-9]+$ && "$stage_two_line" =~ ^[0-9]+$ &&
   "$plan_line" -lt "$stage_two_line" ]] ||
  fail "real Delivery planning must precede stage two"
echo "M5 BTC runner splice contract passed"
