#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly runner="scripts/run-m4-actual-claim-poc.sh"
readonly driver="compat/lez-v0_2-sidecar/src/bin/lez-v02-xmr-tag17.rs"

fail() {
  echo "M7 actual Tag-17 PoC contract test failed: $*" >&2
  exit 1
}

function_source() {
  local function_name="$1"
  sed -n "/^${function_name}() {$/,/^}$/p" "$runner"
}

for command_name in bash jq rg sed; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done
[[ -x "$runner" ]] || fail "actual-local runner is missing or not executable"
[[ -f "$driver" && ! -L "$driver" ]] || fail "Tag-17 driver is missing or unsafe"
bash -n "$runner" || fail "runner shell syntax is invalid"

contract="$("$runner" contract)"
jq -e '
  .tag17_driver_implemented == true
  and .tag17_prepare_only_before_boundary == true
  and .tag17_transaction_id_bound_release == true
  and .tag17_actual_node_transition_reachable_in_execute == true
  and .tag17_actual_node_transition_executed_in_certifying_replay == false
  and .tag17_punish_delay_ms == {minimum:120000,maximum:600000,default:180000}
  and .tag17_finality_page_blocks == 8
  and .tag17_phases == ["tag17_prepare","tag17_wait","tag17","tag17_finality"]
  and (.required_future_binaries | index("lez-v02-xmr-tag17") != null)
' <<<"$contract" >/dev/null || fail "runner does not expose the Tag-17 safety contract"

environment_source="$(function_source environment_preflight)"
prepare_source="$(function_source prepare_tag17_punishment)"
publish_source="$(function_source publish_and_classify_tag17_punishment)"
execute_source="$(function_source execute_run)"
[[ -n "$environment_source" && -n "$prepare_source" && -n "$publish_source" &&
   -n "$execute_source" ]] || fail "Tag-17 runner functions are incomplete"

for required in   'M5_XMR_JOURNEY=punish is a protocol PoC and requires M5_XMR_APPLICATION_MODE=0'   'm7_xmr_punish_delay_ms >= 120000'   'm7_xmr_punish_delay_ms <= 600000'   'm7_xmr_punish_delay_ms % 1000 == 0'; do
  rg -Fq -- "$required" <<<"$environment_source" ||
    fail "punishment environment omits boundary: ${required}"
done

for required in   '--mode prepare'   '.submission.performed==false'   '.submission.request_id==null'   '--role maker --effect punish'   '--exact-transaction-file "$tag17_transaction"'   '--max-blocks 1'   '.outcome.status=="absent" or .outcome.status=="uncertain"'   '.outcome.finalized_clock.timestamp_ms < $punish'; do
  rg -Fq -- "$required" <<<"$prepare_source" ||
    fail "prepare phase omits boundary: ${required}"
done

for required in   '--mode release'   '.submission.request_id==.punish.transaction_id'   '.submission.automatic_retry==false'   '.resources.public_rpc_used==false'   'cmp -- "$tag17_transaction"'   '--role maker --effect punish'   '--role taker --effect punish'   '--max-blocks "$tag17_finality_page_blocks"'   '.outcome.status=="uncertain"'   '.outcome.scanned_window.start_height==$start'   'scan_start_height="$((scan_end_height + 1))"'   '.outcome.facts.containing_block.timestamp_ms >= $punish'   'Maker exact and Taker discovery Tag17 facts differ'; do
  rg -Fq -- "$required" <<<"$publish_source" ||
    fail "release/finality phase omits boundary: ${required}"
done

prepare_line="$(rg -n -m1 'prepare_tag17_punishment' <<<"$execute_source")"
publish_line="$(rg -n -m1 'publish_and_classify_tag17_punishment' <<<"$execute_source")"
prepare_line="${prepare_line%%:*}"
publish_line="${publish_line%%:*}"
[[ "$prepare_line" =~ ^[0-9]+$ && "$publish_line" =~ ^[0-9]+$ ]] ||
  fail "Tag-17 execute ordering is unavailable"
(( prepare_line < publish_line )) || fail "Tag-17 release is reachable before durable preparation"

for required in   'let request_id = prepared.punish.transaction_id.submission_request_id();'   'automatic_retry: false'   'performed'   '.create_new(true)'   '.mode(0o600)'   'metadata.nlink() == 1'   'metadata.permissions().mode().trailing_zeros() >= 6'; do
  rg -Fq -- "$required" "$driver" || fail "Tag-17 driver omits boundary: ${required}"
done

rg -Fq './scripts/test-m7-tag17-actual-poc-contract.sh' scripts/run-ci-quality-gates.sh ||
  fail "Tag-17 PoC contract is absent from the quality runner"
rg -Fq './scripts/test-m7-tag17-actual-poc-contract.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI hardening does not pin the Tag-17 PoC contract"

echo "M7 actual Tag-17 PoC contract test passed"
