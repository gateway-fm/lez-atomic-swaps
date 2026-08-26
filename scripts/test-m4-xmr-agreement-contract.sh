#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly helper="scripts/run-m4-xmr-agreement.sh"

fail() {
  printf 'M4 XMR agreement helper contract test failed: %s\n' "$*" >&2
  exit 1
}

for command_name in bash cat chmod cmp cp cut diff find install jq ln mktemp rg rm sha256sum stat wc; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done
[[ -x "$helper" && ! -L "$helper" ]] || fail "agreement helper is missing or unsafe"
bash -n "$helper" || fail "agreement helper shell syntax is invalid"

contract="$("$helper" contract)"
jq -e '
  .schema_version == 1
  and .kind == "m4_xmr_agreement_helper_contract"
  and .milestone == "M4"
  and .network_effects == "stage_a_read_only_rpc_only"
  and .submission_performed == false
  and .receipt_terms_field == "requested_terms"
  and .terms_bound_to_stage_material_by_helper == false
  and .composer_receipt_validation_scope == "schema_shape_and_unsigned_wire_length_only"
  and .deterministic_swap_id == "sha256(run_id + \":stage-a:001\")"
  and .explicit_swap_id_override == "--swap-id"
  and .dynamic_literal_loopback_endpoints == true
  and .independent_role_roots == ["taker", "maker"]
  and .owner_private_view_key_handoff == true
  and .optional_shared_identity_inputs == {
    view_key:"--shared-view-key-source",
    maker_agreement_key:"--maker-agreement-key-source",
    owner_private_regular_files:true,
    copied_into_new_role_roots:true,
    per_swap_session_and_dleq_keys_remain_fresh:true
  }
  and .view_key_publication == "same_directory_temp_atomic_link_create_new"
  and .trusted_binaries.owner_owned == true
  and .trusted_binaries.single_link == true
  and .trusted_binaries.group_other_write_forbidden == true
  and .purpose_sessions == ["claim", "refund"]
  and .purpose_sessions_byte_compared_across_roles == true
  and .one_journal_per_role == true
  and .claim_boundary.taker_partial_private == true
  and .claim_boundary.taker_presignature_private == true
  and .claim_boundary.maker_stops_after_own_partial == true
  and .refund_boundary.both_role_presignatures_completed == true
  and .refund_boundary.presignatures_byte_compared == true
  and .stage_b_countersigned == true
  and .all_outputs_create_new == true
  and .output_files_owner_private_single_link == true
  and .automatic_retry == false
' <<<"$contract" >/dev/null || fail "helper safety contract drifted"

test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT
readonly test_root
chmod 0700 "$test_root"
readonly fake_bin="${test_root}/bin"
mkdir -m 0700 "$fake_bin"

readonly fake_actor="${fake_bin}/xmr-reference-actor"
readonly fake_runner="${fake_bin}/lez-adaptor-role-runner"
readonly fake_composer="${fake_bin}/lez-v02-xmr-stage-a-compose"

cat >"$fake_actor" <<'FAKE_ACTOR'
#!/usr/bin/env bash
set -euo pipefail
umask 077

value_after() {
  local wanted="$1"
  shift
  while (( $# > 0 )); do
    if [[ "$1" == "$wanted" ]]; then
      (( $# > 1 )) || exit 90
      printf '%s\n' "$2"
      return 0
    fi
    shift
  done
  return 1
}

write_new() {
  local path="$1" value="$2"
  [[ ! -e "$path" && ! -L "$path" ]] || exit 91
  printf '%s\n' "$value" >"$path"
  chmod 0600 "$path"
}

{
  printf 'actor_full'
  printf '|%s' "$@"
  printf '\n'
} >>"$FAKE_LOG"

action="$1"
role="-"
case "$action" in
  provision|sign-stage-a|initialize-sessions|sign-stage-b|assemble-stage-b)
    role="$2"
    ;;
esac
printf 'actor|%s|%s\n' "$action" "$role" >>"$FAKE_SUMMARY"

case "$action" in
  provision)
    private_root="$(value_after --private-root "$@")"
    owner="$(value_after --lez-owner-account "$@")"
    public_packet="$(value_after --public-packet "$@")"
    mkdir -m 0700 "$private_root"
    agreement_source="$(value_after --agreement-key-file "$@" 2>/dev/null || true)"
    if [[ -n "$agreement_source" ]]; then
      install -m 0600 -- "$agreement_source" "${private_root}/agreement.key"
    else
      write_new "${private_root}/agreement.key" "${role}-agreement-secret"
    fi
    write_new "${private_root}/claim.key" "${role}-claim-secret"
    write_new "${private_root}/refund.key" "${role}-refund-secret"
    write_new "${private_root}/xmr-share.key" "${role}-share-secret"
    write_new "${private_root}/manifest.json" '{"fake":"manifest"}'
    if [[ "$role" == taker ]] && ! shared="$(value_after --shared-view-key-file "$@" 2>/dev/null)"; then
      write_new "${private_root}/monero-view.key" "shared-view-secret"
    else
      shared="${shared:-$(value_after --shared-view-key-file "$@")}"
      install -m 0600 -- "$shared" "${private_root}/monero-view.key"
    fi
    agreement_key="${role}-agreement-public"
    claim_key="${role}-claim-public"
    refund_key="${role}-refund-public"
    jq -cn --arg role "$role" --arg owner "$owner" --arg agreement "$agreement_key" \
      --arg claim "$claim_key" --arg refund "$refund_key" \
      '{role:$role,lez_owner_account:$owner,public_view_key:"shared-view-public",
        agreement_public_key:$agreement,claim_session_public_key:$claim,
        refund_session_public_key:$refund}' >"$public_packet"
    chmod 0600 "$public_packet"
    ;;
  initialize-sessions)
    session_root="$(value_after --session-root "$@")"
    mkdir -m 0700 "$session_root"
    claim_value='{"purpose":"claim","session":"same"}'
    if [[ "${FAKE_SESSION_MISMATCH:-0}" == 1 && "$role" == maker ]]; then
      claim_value='{"purpose":"claim","session":"mismatch"}'
    fi
    write_new "${session_root}/claim.json" "$claim_value"
    write_new "${session_root}/refund.json" '{"purpose":"refund","session":"same"}'
    ;;
  sign-stage-a|sign-stage-b)
    output="$(value_after --output-signature "$@")"
    write_new "$output" "${action}-${role}"
    ;;
  assemble-stage-a)
    output="$(value_after --output-stage-a "$@")"
    write_new "$output" "countersigned-stage-a"
    ;;
  compose-stage-b)
    output="$(value_after --output-unsigned-stage-b "$@")"
    write_new "$output" "unsigned-stage-b"
    ;;
  assemble-stage-b)
    output="$(value_after --output-stage-b "$@")"
    write_new "$output" "countersigned-stage-b"
    ;;
  *) exit 92 ;;
esac
FAKE_ACTOR

cat >"$fake_runner" <<'FAKE_RUNNER'
#!/usr/bin/env bash
set -euo pipefail
umask 077

value_after() {
  local wanted="$1"
  shift
  while (( $# > 0 )); do
    if [[ "$1" == "$wanted" ]]; then
      (( $# > 1 )) || exit 90
      printf '%s\n' "$2"
      return 0
    fi
    shift
  done
  return 1
}

role="$1"
journal="$(value_after --journal "$@")"
session="$(value_after --session "$@")"
case "$session" in
  *"/claim.json") purpose=claim ;;
  *"/refund.json") purpose=refund ;;
  *) exit 91 ;;
esac
action=""
for argument in "$@"; do
  case "$argument" in
    reserve|accept-commitment|reveal-nonce|accept-nonce-sign|accept-peer-partial)
      action="$argument"
      break
      ;;
  esac
done
[[ -n "$action" ]] || exit 92

{
  printf 'runner_full'
  printf '|%s' "$@"
  printf '\n'
} >>"$FAKE_LOG"
printf 'runner|%s|%s|%s\n' "$purpose" "$role" "$action" >>"$FAKE_SUMMARY"

if [[ ! -e "$journal" ]]; then
  printf 'fake journal\n' >"$journal"
  chmod 0600 "$journal"
fi

output="$(value_after --output "$@" || true)"
if [[ -n "$output" ]]; then
  [[ ! -e "$output" && ! -L "$output" ]] || exit 93
  if [[ "$purpose" == refund && "$action" == accept-peer-partial ]]; then
    value="role-neutral-refund-presignature"
  else
    value="${purpose}-${action}-${role}"
  fi
  printf '%s\n' "$value" >"$output"
  chmod 0600 "$output"
fi
FAKE_RUNNER

cat >"$fake_composer" <<'FAKE_COMPOSER'
#!/usr/bin/env bash
set -euo pipefail
umask 077

value_after() {
  local wanted="$1"
  shift
  while (( $# > 0 )); do
    if [[ "$1" == "$wanted" ]]; then
      (( $# > 1 )) || exit 90
      printf '%s\n' "$2"
      return 0
    fi
    shift
  done
  return 1
}

{
  printf 'composer_full'
  printf '|%s' "$@"
  printf '\n'
} >>"$FAKE_LOG"
printf 'composer|compose\n' >>"$FAKE_SUMMARY"
output="$(value_after --output-unsigned-stage-a "$@")"
[[ ! -e "$output" && ! -L "$output" ]] || exit 91
printf 'unsigned-stage-a\n' >"$output"
chmod 0600 "$output"
wire_bytes=17
if [[ "${FAKE_COMPOSER_BAD_WIRE_BYTES:-0}" == 1 ]]; then
  wire_bytes=18
fi
jq -cn --arg agreement_commitment "$(printf '1%.0s' {1..64})" \
  --arg monero_genesis_hash "$(printf '2%.0s' {1..64})" \
  --arg lez_genesis_hash "$(printf '3%.0s' {1..64})" \
  --arg lez_channel_id "$(printf '4%.0s' {1..64})" \
  --arg lez_finalized_block_hash "$(printf '5%.0s' {1..64})" \
  --argjson wire_bytes "$wire_bytes" \
  '{agreement_commitment:$agreement_commitment,monero_genesis_hash:$monero_genesis_hash,
    lez_genesis_hash:$lez_genesis_hash,lez_channel_id:$lez_channel_id,
    lez_finalized_block_hash:$lez_finalized_block_hash,lez_finalized_height:42,
    wire_bytes:$wire_bytes}'
FAKE_COMPOSER

chmod 0700 "$fake_actor" "$fake_runner" "$fake_composer"

readonly collision_bin="${test_root}/collision-bin"
declare real_ln
real_ln="$(command -v ln)"
readonly real_ln
mkdir -m 0700 "$collision_bin"
cat >"${collision_bin}/ln" <<'FAKE_LN'
#!/usr/bin/env bash
set -euo pipefail
destination="${@: -1}"
if [[ "${FAKE_HANDOFF_COLLISION_PATH:-}" == "$destination" ]]; then
  [[ ! -e "$destination" && ! -L "$destination" ]] || exit 95
  printf 'collision-sentinel\n' >"$destination"
  chmod 0600 "$destination"
fi
exec "$FAKE_REAL_LN" "$@"
FAKE_LN
chmod 0700 "${collision_bin}/ln"

readonly username_file="${test_root}/monero.username"
readonly password_file="${test_root}/monero.password"
printf 'rpc-user-secret\n' >"$username_file"
printf 'rpc-password-secret\n' >"$password_file"
chmod 0600 "$username_file" "$password_file"

readonly taker_owner="1111111111111111111111111111111111111111111111111111111111111111"
readonly maker_owner="2222222222222222222222222222222222222222222222222222222222222222"
readonly run_id="m4agreefixture"
declare expected_swap_id
expected_swap_id="$(
  printf '%s' "${run_id}:stage-a:001" | sha256sum | cut -d' ' -f1
)"
readonly expected_swap_id

run_fixture() {
  local destination="$1"
  local selected_actor="${2:-$fake_actor}"
  local explicit_swap_id="${3:-}"
  local shared_view_source="${4:-}" maker_agreement_source="${5:-}"
  local swap_id_args=() shared_identity_args=()
  [[ -z "$explicit_swap_id" ]] || swap_id_args=(--swap-id "$explicit_swap_id")
  if [[ -n "$shared_view_source" || -n "$maker_agreement_source" ]]; then
    shared_identity_args=(--shared-view-key-source "$shared_view_source"
      --maker-agreement-key-source "$maker_agreement_source")
  fi
  "$helper" execute \
    --run-id "$run_id" \
    "${swap_id_args[@]}" \
    --output-root "$destination" \
    --taker-lez-owner "$taker_owner" \
    --maker-lez-owner "$maker_owner" \
    --sequencer-url http://127.0.0.1:31001 \
    --indexer-url http://127.0.0.1:31002 \
    --monero-daemon-url http://127.0.0.1:31003 \
    --monero-rpc-username-file "$username_file" \
    --monero-rpc-password-file "$password_file" \
    --monero-amount-piconero 1000000000000 \
    --lez-amount 700 \
    --maker-xmr-funding-cutoff-ms 2000000000000 \
    --refund-at-ms 2000000010000 \
    --punish-at-ms 2000000020000 \
    "${shared_identity_args[@]}" \
    --actor-bin "$selected_actor" \
    --role-runner-bin "$fake_runner" \
    --composer-bin "$fake_composer"
}

export FAKE_LOG="${test_root}/happy-full.log"
export FAKE_SUMMARY="${test_root}/happy-summary.log"
: >"$FAKE_LOG"
: >"$FAKE_SUMMARY"
chmod 0600 "$FAKE_LOG" "$FAKE_SUMMARY"

readonly happy_root="${test_root}/happy"
readonly stdout_receipt="${test_root}/happy.stdout.json"
run_fixture "$happy_root" >"$stdout_receipt"

jq -e --arg run_id "$run_id" --arg swap_id "$expected_swap_id" '
  .schema_version == 1
  and .kind == "m4_xmr_agreement_receipt"
  and .result == "passed"
  and .run_id == $run_id
  and .swap_id == $swap_id
  and (has("terms") | not)
  and .requested_terms.monero_amount_piconero == "1000000000000"
  and .requested_terms.lez_amount == "700"
  and .requested_terms.maker_xmr_funding_cutoff_ms == "2000000000000"
  and .requested_terms.refund_at_ms == "2000000010000"
  and .requested_terms.punish_at_ms == "2000000020000"
  and .submission_performed == false
  and .stage_a_rpc_scope == "read_only"
  and .terms_bound_to_stage_material_by_helper == false
  and .composer_receipt_validation_scope == "schema_shape_and_unsigned_wire_length_only"
  and .composer_receipt_wire_bytes_matched_output == true
  and .sessions_equal_across_roles == true
  and .taker_claim_material_private == true
  and .refund_presignatures_equal == true
  and .stage_b_countersigned == true
' "$stdout_receipt" >/dev/null || fail "happy-path receipt is incomplete"

cmp -- "$stdout_receipt" "${happy_root}/agreement-receipt.json" ||
  fail "stdout receipt differs from the durable create-new receipt"
cmp -- "${happy_root}/material/taker-sessions/claim.json" \
  "${happy_root}/material/maker-sessions/claim.json" ||
  fail "happy-path claim sessions differ"
cmp -- "${happy_root}/material/taker-sessions/refund.json" \
  "${happy_root}/material/maker-sessions/refund.json" ||
  fail "happy-path refund sessions differ"
cmp -- "${happy_root}/stage-b/exchange/refund/maker-presignature.json" \
  "${happy_root}/stage-b/exchange/refund/taker-presignature.json" ||
  fail "happy-path refund presignatures differ"
[[ -f "${happy_root}/stage-b/private/taker-outbox/claim-partial.json" ]] ||
  fail "Taker private claim partial is missing"
[[ -f "${happy_root}/stage-b/private/taker-outbox/claim-presignature.json" ]] ||
  fail "Taker private claim presignature is missing"
[[ ! -e "${happy_root}/stage-b/exchange/claim/taker-partial.json" ]] ||
  fail "Taker claim partial leaked into the exchange"
[[ ! -e "${happy_root}/stage-b/exchange/claim/taker-presignature.json" ]] ||
  fail "Taker claim presignature leaked into the exchange"

[[ -z "$(find "$happy_root" -type f ! -perm 0600 -print -quit)" ]] ||
  fail "a happy-path output file is not mode 0600"
[[ -z "$(find "$happy_root" -type f -links +1 -print -quit)" ]] ||
  fail "a happy-path output file is not single-link"
[[ -z "$(find "$happy_root" -type d ! -perm 0700 -print -quit)" ]] ||
  fail "a happy-path output directory is not mode 0700"

readonly expected_summary="${test_root}/expected-summary.log"
cat >"$expected_summary" <<'EXPECTED_SUMMARY'
actor|provision|taker
actor|provision|maker
composer|compose
actor|sign-stage-a|taker
actor|sign-stage-a|maker
actor|assemble-stage-a|-
actor|initialize-sessions|taker
actor|initialize-sessions|maker
runner|claim|maker|reserve
runner|claim|taker|reserve
runner|claim|maker|accept-commitment
runner|claim|taker|accept-commitment
runner|claim|maker|reveal-nonce
runner|claim|taker|reveal-nonce
runner|claim|maker|accept-nonce-sign
runner|claim|taker|accept-nonce-sign
runner|claim|taker|accept-peer-partial
runner|refund|maker|reserve
runner|refund|taker|reserve
runner|refund|maker|accept-commitment
runner|refund|taker|accept-commitment
runner|refund|maker|reveal-nonce
runner|refund|taker|reveal-nonce
runner|refund|maker|accept-nonce-sign
runner|refund|taker|accept-nonce-sign
runner|refund|taker|accept-peer-partial
runner|refund|maker|accept-peer-partial
actor|compose-stage-b|-
actor|sign-stage-b|maker
actor|sign-stage-b|taker
actor|assemble-stage-b|taker
EXPECTED_SUMMARY

diff -u "$expected_summary" "$FAKE_SUMMARY" ||
  fail "actor/composer/journal process order drifted"

composer_line="$(rg '^composer_full\|' "$FAKE_LOG")"
for exact_argument in \
    "--sequencer-url|http://127.0.0.1:31001" \
    "--indexer-url|http://127.0.0.1:31002" \
    "--monero-daemon-url|http://127.0.0.1:31003" \
    "--swap-id|${expected_swap_id}" \
    "--monero-amount-piconero|1000000000000" \
    "--lez-amount|700" \
    "--maker-xmr-funding-cutoff-ms|2000000000000" \
    "--refund-at-ms|2000000010000" \
    "--punish-at-ms|2000000020000"; do
  [[ "$composer_line" == *"|${exact_argument}"* ]] ||
    fail "composer omitted exact argument: ${exact_argument}"
done

taker_provision="$(rg '^actor_full\|provision\|taker\|' "$FAKE_LOG")"
maker_provision="$(rg '^actor_full\|provision\|maker\|' "$FAKE_LOG")"
[[ "$taker_provision" != *"|--shared-view-key-file|"* ]] ||
  fail "Taker provisioning unexpectedly consumed a handed-off view key"
[[ "$maker_provision" == *"|--shared-view-key-file|${happy_root}/handoff/monero-view.key"* ]] ||
  fail "Maker provisioning did not consume the owner-private view-key handoff"
[[ "$taker_provision" == *"|--lez-owner-account|${taker_owner}"* ]] ||
  fail "Taker provisioning owner binding drifted"
[[ "$maker_provision" == *"|--lez-owner-account|${maker_owner}"* ]] ||
  fail "Maker provisioning owner binding drifted"

readonly imported_root="${test_root}/imported-identity"
readonly imported_receipt="${test_root}/imported-identity.stdout.json"
run_fixture "$imported_root" "$fake_actor" \
  bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
  "${happy_root}/material/taker/monero-view.key" \
  "${happy_root}/material/maker/agreement.key" >"$imported_receipt"
cmp -- "${happy_root}/material/taker/monero-view.key" \
  "${imported_root}/material/taker/monero-view.key" ||
  fail "imported Taker view key changed"
cmp -- "${happy_root}/material/maker/agreement.key" \
  "${imported_root}/material/maker/agreement.key" ||
  fail "imported Maker agreement key changed"
imported_taker_provision="$(rg '^actor_full\|provision\|taker\|' "$FAKE_LOG" | tail -n 1)"
imported_maker_provision="$(rg '^actor_full\|provision\|maker\|' "$FAKE_LOG" | tail -n 1)"
[[ "$imported_taker_provision" == *"|--shared-view-key-file|${happy_root}/material/taker/monero-view.key"* ]] ||
  fail "second Taker did not consume the shared view-key source"
[[ "$imported_maker_provision" == *"|--agreement-key-file|${happy_root}/material/maker/agreement.key"* ]] ||
  fail "second Maker did not consume the shared agreement-key source"

if "$helper" execute --run-id "$run_id" --output-root "${test_root}/unpaired-identity" \
    --taker-lez-owner "$taker_owner" --maker-lez-owner "$maker_owner" \
    --sequencer-url http://127.0.0.1:31001 --indexer-url http://127.0.0.1:31002 \
    --monero-daemon-url http://127.0.0.1:31003 \
    --monero-rpc-username-file "$username_file" --monero-rpc-password-file "$password_file" \
    --monero-amount-piconero 1000000000000 --lez-amount 700 \
    --maker-xmr-funding-cutoff-ms 2000000000000 --refund-at-ms 2000000010000 \
    --punish-at-ms 2000000020000 \
    --shared-view-key-source "${happy_root}/material/taker/monero-view.key" \
    --actor-bin "$fake_actor" --role-runner-bin "$fake_runner" --composer-bin "$fake_composer" \
    >"${test_root}/unpaired.stdout" 2>"${test_root}/unpaired.stderr"; then
  fail "unpaired shared-identity input unexpectedly succeeded"
fi
rg -F 'shared view and Maker agreement key sources must be supplied together' \
  "${test_root}/unpaired.stderr" >/dev/null ||
  fail "unpaired shared identity did not fail at argument validation"
[[ ! -e "${test_root}/unpaired-identity" ]] ||
  fail "unpaired shared identity created an output root"

if rg '^runner_full\|maker\|.*taker-sessions' "$FAKE_LOG" >/dev/null; then
  fail "Maker journal invocation consumed a Taker session path"
fi
if rg '^runner_full\|taker\|.*maker-sessions' "$FAKE_LOG" >/dev/null; then
  fail "Taker journal invocation consumed a Maker session path"
fi
[[ "$(rg -c "^runner_full\\|maker\\|--journal\\|${happy_root}/stage-b/private/maker.sqlite\\|" "$FAKE_LOG")" == 9 ]] ||
  fail "Maker did not use exactly one role journal for its nine transitions"
[[ "$(rg -c "^runner_full\\|taker\\|--journal\\|${happy_root}/stage-b/private/taker.sqlite\\|" "$FAKE_LOG")" == 10 ]] ||
  fail "Taker did not use exactly one role journal for its ten transitions"
if rg '^runner\|claim\|maker\|accept-peer-partial$' "$FAKE_SUMMARY" >/dev/null; then
  fail "Maker claim journal advanced past its own partial"
fi
rg -F "runner_full|taker|--journal|${happy_root}/stage-b/private/taker.sqlite|--session|${happy_root}/material/taker-sessions/claim.json|accept-nonce-sign" \
  "$FAKE_LOG" | rg -F "|--output|${happy_root}/stage-b/private/taker-outbox/claim-partial.json" >/dev/null ||
  fail "Taker claim partial was not routed to the private outbox"

readonly override_swap_id="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
readonly override_root="${test_root}/explicit-swap"
readonly override_receipt="${test_root}/explicit-swap.stdout.json"
run_fixture "$override_root" "$fake_actor" "$override_swap_id" >"$override_receipt"
jq -e --arg swap_id "$override_swap_id" '.swap_id == $swap_id' "$override_receipt" >/dev/null ||
  fail "explicit swap ID did not reach the durable receipt"
[[ "$(rg '^composer_full\|' "$FAKE_LOG" | tail -n 1)" == *"|--swap-id|${override_swap_id}"* ]] ||
  fail "explicit swap ID did not reach the composer"

readonly invalid_swap_root="${test_root}/invalid-explicit-swap"
if run_fixture "$invalid_swap_root" "$fake_actor" not-hex >"${test_root}/invalid-swap.stdout" 2>"${test_root}/invalid-swap.stderr"; then
  fail "malformed explicit swap ID unexpectedly succeeded"
fi
rg -F "explicit swap ID must be one nonzero lowercase-hex 32-byte value" "${test_root}/invalid-swap.stderr" >/dev/null ||
  fail "malformed explicit swap ID failure was not precise"
[[ ! -e "$invalid_swap_root" && ! -L "$invalid_swap_root" ]] ||
  fail "malformed explicit swap ID created output"

before_reuse="$(wc -l <"$FAKE_LOG")"
if run_fixture "$happy_root" >"${test_root}/reuse.stdout" 2>"${test_root}/reuse.stderr"; then
  fail "helper reused an existing output root"
fi
after_reuse="$(wc -l <"$FAKE_LOG")"
[[ "$before_reuse" == "$after_reuse" ]] ||
  fail "create-new rejection occurred after invoking a protocol binary"

readonly unsafe_writable_actor="${fake_bin}/unsafe-writable-actor"
cp --reflink=never -- "$fake_actor" "$unsafe_writable_actor"
chmod 0775 "$unsafe_writable_actor"
before_unsafe="$(wc -l <"$FAKE_LOG")"
if run_fixture "${test_root}/unsafe-writable" "$unsafe_writable_actor" \
    >"${test_root}/unsafe-writable.stdout" 2>"${test_root}/unsafe-writable.stderr"; then
  fail "group-writable protocol binary was accepted"
fi
after_unsafe="$(wc -l <"$FAKE_LOG")"
[[ "$before_unsafe" == "$after_unsafe" ]] ||
  fail "unsafe writable-binary rejection occurred after a protocol invocation"
[[ ! -e "${test_root}/unsafe-writable" ]] ||
  fail "unsafe writable binary reached output-root creation"
rg -F 'not group/other-writable' "${test_root}/unsafe-writable.stderr" >/dev/null ||
  fail "unsafe writable binary did not fail at the binary-trust boundary"

readonly unsafe_linked_actor="${fake_bin}/unsafe-linked-actor"
readonly unsafe_linked_peer="${fake_bin}/unsafe-linked-actor.peer"
cp --reflink=never -- "$fake_actor" "$unsafe_linked_actor"
chmod 0700 "$unsafe_linked_actor"
ln -- "$unsafe_linked_actor" "$unsafe_linked_peer"
[[ "$(stat -c '%h' "$unsafe_linked_actor")" == 2 ]] ||
  fail "linked-binary negative fixture does not have two links"
before_unsafe="$(wc -l <"$FAKE_LOG")"
if run_fixture "${test_root}/unsafe-linked" "$unsafe_linked_actor" \
    >"${test_root}/unsafe-linked.stdout" 2>"${test_root}/unsafe-linked.stderr"; then
  fail "multiply-linked protocol binary was accepted"
fi
after_unsafe="$(wc -l <"$FAKE_LOG")"
[[ "$before_unsafe" == "$after_unsafe" ]] ||
  fail "unsafe linked-binary rejection occurred after a protocol invocation"
[[ ! -e "${test_root}/unsafe-linked" ]] ||
  fail "unsafe linked binary reached output-root creation"
rg -F 'single-link' "${test_root}/unsafe-linked.stderr" >/dev/null ||
  fail "unsafe linked binary did not fail at the binary-trust boundary"

export FAKE_LOG="${test_root}/collision-full.log"
export FAKE_SUMMARY="${test_root}/collision-summary.log"
: >"$FAKE_LOG"
: >"$FAKE_SUMMARY"
chmod 0600 "$FAKE_LOG" "$FAKE_SUMMARY"
readonly collision_root="${test_root}/handoff-collision"
readonly collision_path="${collision_root}/handoff/monero-view.key"
if (
  export FAKE_REAL_LN="$real_ln"
  export FAKE_HANDOFF_COLLISION_PATH="$collision_path"
  export PATH="${collision_bin}:${PATH}"
  run_fixture "$collision_root"
) >"${test_root}/collision.stdout" 2>"${test_root}/collision.stderr"; then
  fail "handoff publication collision was accepted"
fi
rg -F 'create-new publication collided' "${test_root}/collision.stderr" >/dev/null ||
  fail "handoff collision did not fail at the atomic publication boundary"
[[ "$(cat "$collision_path")" == collision-sentinel ]] ||
  fail "handoff publication overwrote the adversarial collision sentinel"
[[ "$(stat -c '%a %h' "$collision_path")" == '600 1' ]] ||
  fail "collision sentinel identity changed during rejected publication"
[[ -z "$(find "${collision_root}/handoff" -maxdepth 1 -name '.monero-view.key.tmp.*' -print -quit)" ]] ||
  fail "rejected handoff publication retained a staging link"
[[ "$(rg -c '^actor\|provision\|taker$' "$FAKE_SUMMARY")" == 1 ]] ||
  fail "handoff collision did not occur after exactly one Taker provision"
if rg '^actor\|provision\|maker$' "$FAKE_SUMMARY" >/dev/null; then
  fail "handoff collision reached Maker provisioning"
fi

export FAKE_LOG="${test_root}/composer-mismatch-full.log"
export FAKE_SUMMARY="${test_root}/composer-mismatch-summary.log"
: >"$FAKE_LOG"
: >"$FAKE_SUMMARY"
chmod 0600 "$FAKE_LOG" "$FAKE_SUMMARY"
readonly composer_mismatch_root="${test_root}/composer-mismatch"
if (
  export FAKE_COMPOSER_BAD_WIRE_BYTES=1
  run_fixture "$composer_mismatch_root"
) >"${test_root}/composer-mismatch.stdout" 2>"${test_root}/composer-mismatch.stderr"; then
  fail "composer receipt with mismatched wire length was accepted"
fi
rg -F 'composer receipt identity/wire metadata is invalid' \
  "${test_root}/composer-mismatch.stderr" >/dev/null ||
  fail "composer wire-length mismatch did not fail at the receipt boundary"
[[ "$(rg -c '^composer\|compose$' "$FAKE_SUMMARY")" == 1 ]] ||
  fail "composer wire-length mismatch did not exercise exactly one composition"
if rg '^actor\|sign-stage-a\|' "$FAKE_SUMMARY" >/dev/null; then
  fail "invalid composer receipt reached Stage-A signing"
fi
if rg '^runner\|' "$FAKE_SUMMARY" >/dev/null; then
  fail "invalid composer receipt reached a journal transition"
fi

export FAKE_LOG="${test_root}/mismatch-full.log"
export FAKE_SUMMARY="${test_root}/mismatch-summary.log"
: >"$FAKE_LOG"
: >"$FAKE_SUMMARY"
chmod 0600 "$FAKE_LOG" "$FAKE_SUMMARY"
readonly mismatch_root="${test_root}/mismatch"
if (
  export FAKE_SESSION_MISMATCH=1
  run_fixture "$mismatch_root"
) >"${test_root}/mismatch.stdout" 2>"${test_root}/mismatch.stderr"; then
  fail "cross-role claim-session byte mismatch was accepted"
fi
if rg '^runner\|' "$FAKE_SUMMARY" >/dev/null; then
  fail "session mismatch reached a journal transition"
fi
rg -F 'claim session bytes differ across roles' "${test_root}/mismatch.stderr" >/dev/null ||
  fail "session mismatch did not fail at the intended boundary"
for secret in rpc-user-secret rpc-password-secret shared-view-secret taker-claim-secret maker-claim-secret; do
  if rg -F "$secret" "${test_root}/mismatch.stderr" "${test_root}/reuse.stderr" >/dev/null; then
    fail "failure output disclosed fixture secret material"
  fi
done

for forbidden in 'curl ' 'wget ' 'docker ' 'cargo ' 'git ' 'rm -rf ' 'pkill ' 'killall '; do
  if rg -F "$forbidden" "$helper" >/dev/null; then
    fail "helper contains forbidden side effect or broad cleanup: ${forbidden}"
  fi
done
for retained_example_port in 33145 33146 33147 39185 41189 46769 58393; do
  if rg -n "(^|[^0-9])${retained_example_port}([^0-9]|$)" "$helper" >/dev/null; then
    fail "helper hard-codes a retained example port: ${retained_example_port}"
  fi
done

echo "M4 XMR agreement helper contract passed"
