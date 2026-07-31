#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly runner="scripts/run-m3-actor-local-poc.sh"
readonly direction_driver="scripts/run-m3-actor-direction.sh"
readonly wrapper="scripts/run-m5-btc-application-poc.sh"
readonly bootstrap_driver="scripts/run-m3-lez-bootstrap.sh"

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
  'M5_LEZ_DEPLOYER_SHA256' \
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
  .evidence_packet_kind == "m5_btc_application_local_poc" and
  .service_configuration.lez_v0_2.deployment_profile == "m4_checked_local" and
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
  'M5_LEZ_DEPLOYER_SHA256 is required' \
  'M5_LEZ_DEPLOYER_SHA256 must be a lowercase SHA-256 digest'; do
  rg -Fq -- "$required" "$wrapper" ||
    fail "M5 wrapper is missing explicit deployer identity validation: ${required}"
done

for required in \
  'ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee' \
  'b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433' \
  'deployment_profile="m4_checked_local"' \
  'deployment_command="deploy-m4-local"' \
  'selected_deployer_sha256="${M5_LEZ_DEPLOYER_SHA256:-}"'; do
  rg -Fq -- "$required" "$bootstrap_driver" ||
    fail "LEZ bootstrap is missing M5 deployment identity: ${required}"
done
m5_bootstrap_contract="$(M5_BTC_APPLICATION_MODE=1 "$bootstrap_driver" contract)" ||
  fail "LEZ bootstrap rejected the M5 checked deployment profile"
jq -e '
  .embedded_guest_sha256 ==
    "ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee" and
  .escrow_program_id ==
    "b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433" and
  .deployment_profile == "m4_checked_local"
' <<<"$m5_bootstrap_contract" >/dev/null ||
  fail "LEZ bootstrap emitted the wrong M5 checked deployment profile"

legacy_bootstrap_contract="$("$bootstrap_driver" contract)" ||
  fail "LEZ bootstrap rejected the legacy M3 checked deployment profile"
jq -e '
  .embedded_guest_sha256 ==
    "bc2ea18eaacb917727934fcf0366dd54c1f9a2b69b61ea53080c926850967fd7" and
  .escrow_program_id ==
    "f3ead24b95d316ce91980cb3531a70b83a27fd1640f47c1b857757aef26c244e" and
  .deployment_profile == "m3_f7_checked_local"
' <<<"$legacy_bootstrap_contract" >/dev/null ||
  fail "LEZ bootstrap no longer preserves the legacy checked deployment profile"

for required in 'm5_btc_application_local_poc' \
  'if [[ "$m5_btc_application_mode" != 1 ]]; then' \
  'if $m5_btc_application_mode == "1" then .[0:1] else . end' \
  'if $m5_btc_application_mode == "1" then 1 else 2 end'; do
  rg -Fq -- "$required" "$runner" ||
    fail "M5 final evidence remains two-direction-only: ${required}"
done


for required in \
  'M5_BTC_APPLICATION_MODE="$m5_btc_application_mode"' \
  'M5_LEZ_DEPLOYER_SHA256="$expected_lez_deployer_sha256"' \
  '--arg deployment_profile "$expected_lez_deployment_profile"'; do
  rg -Fq -- "$required" "$runner" ||
    fail "outer runner is missing M5 bootstrap handoff: ${required}"
done

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

for required in \
  'terminal_replay_actor_config() {' \
  'local application_root="${direction_root}/application"' \
  'local owner_root="${application_root}/owner"' \
  'local database="${application_root}/maker.sqlite3"' \
  'local receipt="${application_root}/owner/acceptance-receipt.json"' \
  'local agreement_file="${owner_root}/agreement-v1.borsh"' \
  'swap_id="${m5_btc_swap_ids[$direction]:-}"' \
  'SELECT manifest_path, lower(hex(manifest_sha256)), state_db_path' \
  "WHERE swap_id = '\${swap_id}' AND actor_kind = 'bitcoin';" \
  '.actor_config_file | strings' \
  '.actor_config_sha256 | strings' \
  'config="${direction_root}/actors/${role}/actor-config.json"' \
  '[[ "$config" == /* && -f "$config" && ! -L "$config"' \
  'agreement_sha="$(sha256sum "$agreement_file"' \
  '"$receipt_agreement_sha" == "$agreement_sha"' \
  'sha256sum "$config"' \
  '== "$config_sha"' \
  '"$(readlink -f "$state_db")" == "$state_db"' \
  '.schema_version == 6 and .role == $role' \
  '.agreement_sha256 == $agreement_sha' \
  '.state_db == $state_db' \
  'config="$(terminal_replay_actor_config "$direction" "$role")"'; do
  rg -Fq -- "$required" "$runner" ||
    fail "terminal replay is missing role-provisioned authority: ${required}"
done

echo "M5 BTC runner splice contract passed"
