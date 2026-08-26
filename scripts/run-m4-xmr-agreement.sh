#!/usr/bin/env bash
set -euo pipefail
set -o noclobber

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

declare repo_root
repo_root="$(pwd)"
readonly repo_root

fail() {
  printf 'M4 XMR agreement helper failed: %s\n' "$*" >&2
  exit 1
}

emit_contract() {
  jq -n '
    {
      schema_version: 1,
      kind: "m4_xmr_agreement_helper_contract",
      milestone: "M4",
      network_effects: "stage_a_read_only_rpc_only",
      submission_performed: false,
      receipt_terms_field: "requested_terms",
      terms_bound_to_stage_material_by_helper: false,
      composer_receipt_validation_scope: "schema_shape_and_unsigned_wire_length_only",
      deterministic_swap_id: "sha256(run_id + \":stage-a:001\")",
      explicit_swap_id_override: "--swap-id",
      dynamic_literal_loopback_endpoints: true,
      independent_role_roots: ["taker", "maker"],
      owner_private_view_key_handoff: true,
      optional_shared_identity_inputs: {
        view_key: "--shared-view-key-source",
        maker_agreement_key: "--maker-agreement-key-source",
        owner_private_regular_files: true,
        copied_into_new_role_roots: true,
        per_swap_session_and_dleq_keys_remain_fresh: true
      },
      view_key_publication: "same_directory_temp_atomic_link_create_new",
      purpose_sessions: ["claim", "refund"],
      trusted_binaries: {
        owner_owned: true,
        single_link: true,
        group_other_write_forbidden: true
      },
      purpose_sessions_byte_compared_across_roles: true,
      one_journal_per_role: true,
      claim_boundary: {
        taker_partial_private: true,
        taker_presignature_private: true,
        maker_stops_after_own_partial: true
      },
      refund_boundary: {
        both_role_presignatures_completed: true,
        presignatures_byte_compared: true
      },
      stage_b_countersigned: true,
      all_outputs_create_new: true,
      output_files_owner_private_single_link: true,
      automatic_retry: false,
      modes: ["contract", "execute"],
      execute_required_options: [
        "--run-id", "--output-root", "--taker-lez-owner",
        "--maker-lez-owner", "--sequencer-url", "--indexer-url",
        "--monero-daemon-url", "--monero-rpc-username-file",
        "--monero-rpc-password-file", "--monero-amount-piconero",
        "--lez-amount", "--maker-xmr-funding-cutoff-ms",
        "--refund-at-ms", "--punish-at-ms"
      ],
      overridable_binaries: ["--actor-bin", "--role-runner-bin", "--composer-bin"]
    }
  '
}

require_command() {
  command -v "$1" >/dev/null || fail "missing required command: $1"
}

require_new_path() {
  local path="$1" label="$2"
  [[ ! -e "$path" && ! -L "$path" ]] || fail "${label} already exists"
}

require_private_file() {
  local path="$1" label="$2" canonical uid mode links
  [[ -f "$path" && ! -L "$path" ]] || fail "${label} is not a regular non-symlink file"
  canonical="$(readlink -f -- "$path")"
  [[ "$canonical" == "$path" ]] || fail "${label} path is not canonical"
  uid="$(stat -c '%u' -- "$path")"
  mode="$(stat -c '%a' -- "$path")"
  links="$(stat -c '%h' -- "$path")"
  [[ "$uid" == "$(id -u)" && "$mode" == 600 && "$links" == 1 ]] ||
    fail "${label} must be owner-owned, mode 0600, and single-link"
}

require_private_directory() {
  local path="$1" label="$2" canonical uid mode
  [[ -d "$path" && ! -L "$path" ]] || fail "${label} is not a regular directory"
  canonical="$(readlink -f -- "$path")"
  [[ "$canonical" == "$path" ]] || fail "${label} path is not canonical"
  uid="$(stat -c '%u' -- "$path")"
  mode="$(stat -c '%a' -- "$path")"
  [[ "$uid" == "$(id -u)" && "$mode" == 700 ]] ||
    fail "${label} must be owner-owned mode 0700"
}

require_safe_parent() {
  local path="$1" canonical uid mode
  [[ -d "$path" && ! -L "$path" ]] || fail "output parent is not a regular directory"
  canonical="$(readlink -f -- "$path")"
  [[ "$canonical" == "$path" ]] || fail "output parent path is not canonical"
  uid="$(stat -c '%u' -- "$path")"
  mode="$(stat -c '%a' -- "$path")"
  [[ "$uid" == "$(id -u)" && $((8#$mode & 022)) == 0 ]] ||
    fail "output parent is not safely owner-controlled"
}

require_binary() {
  local path="$1" label="$2" canonical uid mode links
  [[ "$path" == /* && -f "$path" && ! -L "$path" && -x "$path" ]] ||
    fail "${label} must be an absolute executable regular file"
  canonical="$(readlink -f -- "$path")"
  [[ "$canonical" == "$path" ]] || fail "${label} path is not canonical"
  uid="$(stat -c '%u' -- "$path")"
  mode="$(stat -c '%a' -- "$path")"
  links="$(stat -c '%h' -- "$path")"
  [[ "$uid" == "$(id -u)" && "$links" == 1 && $((8#$mode & 022)) == 0 ]] ||
    fail "${label} must be owner-owned, single-link, and not group/other-writable"
}

publish_private_copy_new() {
  local source="$1" destination="$2" label="$3"
  local source_sha256_before source_sha256_after temporary
  require_private_file "$source" "${label} source"
  require_new_path "$destination" "$label"
  source_sha256_before="$(sha256_file "$source")"

  temporary="$(mktemp "${destination}.tmp.XXXXXX")" ||
    fail "could not stage ${label} in its destination directory"
  chmod 0600 -- "$temporary"
  require_private_file "$temporary" "${label} staging file"
  if ! cp --reflink=never -- "$source" "$temporary"; then
    unlink -- "$temporary" || true
    fail "could not copy ${label} into its staging file"
  fi
  require_private_file "$temporary" "${label} staging file"
  source_sha256_after="$(sha256_file "$source")"
  [[ "$source_sha256_before" == "$source_sha256_after" ]] || {
    unlink -- "$temporary" || true
    fail "${label} source changed while staging"
  }
  if ! cmp -- "$source" "$temporary"; then
    unlink -- "$temporary" || true
    fail "${label} staging bytes differ from the source"
  fi

  if ! ln -- "$temporary" "$destination"; then
    unlink -- "$temporary" || true
    fail "${label} create-new publication collided"
  fi
  unlink -- "$temporary" || fail "could not remove ${label} staging link"
  require_private_file "$destination" "$label"
  [[ "$(sha256_file "$source")" == "$source_sha256_before" ]] ||
    fail "${label} source changed during publication"
  cmp -- "$source" "$destination" || fail "${label} publication changed bytes"
}

require_exact_entries() {
  local directory="$1" expected="$2" label="$3" actual
  actual="$(find "$directory" -mindepth 1 -maxdepth 1 -printf '%f\n' | sort)"
  [[ "$actual" == "$expected" ]] || fail "${label} has unexpected entries"
}

require_hex32() {
  local value="$1" label="$2"
  [[ "$value" =~ ^[0-9a-f]{64}$ && ! "$value" =~ ^0{64}$ ]] ||
    fail "${label} must be one nonzero lowercase-hex 32-byte value"
}

decimal_lte() {
  local left="$1" right="$2"
  (( ${#left} < ${#right} )) && return 0
  (( ${#left} > ${#right} )) && return 1
  [[ "$left" == "$right" || "$left" < "$right" ]]
}

decimal_lt() {
  local left="$1" right="$2"
  (( ${#left} < ${#right} )) && return 0
  (( ${#left} > ${#right} )) && return 1
  [[ "$left" < "$right" ]]
}

require_bounded_decimal() {
  local value="$1" maximum="$2" label="$3"
  [[ "$value" =~ ^[1-9][0-9]*$ ]] || fail "${label} must be a canonical positive decimal"
  decimal_lte "$value" "$maximum" || fail "${label} exceeds its protocol integer range"
}

require_deadline() {
  local value="$1" label="$2"
  require_bounded_decimal "$value" 18446744073709551615 "$label"
  [[ "$value" == *000 ]] || fail "${label} must be an exact whole-second millisecond timestamp"
}

require_loopback_url() {
  local value="$1" label="$2" port
  [[ "$value" =~ ^http://127\.0\.0\.1:([1-9][0-9]{0,4})$ ]] ||
    fail "${label} must be an http://127.0.0.1:PORT literal-loopback root URL"
  port="${BASH_REMATCH[1]}"
  (( 10#$port <= 65535 )) || fail "${label} port is outside 1..65535"
}

sha256_file() {
  sha256sum -- "$1" | cut -d' ' -f1
}

parse_execute_arguments() {
  run_id=""
  explicit_swap_id=""
  output_root=""
  taker_lez_owner=""
  maker_lez_owner=""
  sequencer_url=""
  indexer_url=""
  monero_daemon_url=""
  monero_rpc_username_file=""
  monero_rpc_password_file=""
  monero_amount_piconero=""
  lez_amount=""
  maker_xmr_funding_cutoff_ms=""
  refund_at_ms=""
  punish_at_ms=""
  shared_view_key_source=""
  maker_agreement_key_source=""
  actor_bin="${repo_root}/target/debug/xmr-reference-actor"
  role_runner_bin="${repo_root}/target/debug/lez-adaptor-role-runner"
  composer_bin="${repo_root}/compat/lez-v0_2-sidecar/target/debug/lez-v02-xmr-stage-a-compose"
  declare -A seen=()

  while (( $# > 0 )); do
    local option="$1"
    shift
    case "$option" in
      --run-id|--swap-id|--output-root|--taker-lez-owner|--maker-lez-owner|--sequencer-url|--indexer-url|\
      --monero-daemon-url|--monero-rpc-username-file|--monero-rpc-password-file|\
      --monero-amount-piconero|--lez-amount|--maker-xmr-funding-cutoff-ms|\
      --refund-at-ms|--punish-at-ms|--shared-view-key-source|\
      --maker-agreement-key-source|--actor-bin|--role-runner-bin|--composer-bin)
        (( $# > 0 )) || fail "missing value for ${option}"
        [[ -z "${seen[$option]+present}" ]] || fail "duplicate option: ${option}"
        seen[$option]=1
        ;;
      *) fail "unknown execute option: ${option}" ;;
    esac
    case "$option" in
      --run-id) run_id="$1" ;;
      --swap-id) explicit_swap_id="$1" ;;
      --output-root) output_root="$1" ;;
      --taker-lez-owner) taker_lez_owner="$1" ;;
      --maker-lez-owner) maker_lez_owner="$1" ;;
      --sequencer-url) sequencer_url="$1" ;;
      --indexer-url) indexer_url="$1" ;;
      --monero-daemon-url) monero_daemon_url="$1" ;;
      --monero-rpc-username-file) monero_rpc_username_file="$1" ;;
      --monero-rpc-password-file) monero_rpc_password_file="$1" ;;
      --monero-amount-piconero) monero_amount_piconero="$1" ;;
      --lez-amount) lez_amount="$1" ;;
      --maker-xmr-funding-cutoff-ms) maker_xmr_funding_cutoff_ms="$1" ;;
      --refund-at-ms) refund_at_ms="$1" ;;
      --punish-at-ms) punish_at_ms="$1" ;;
      --shared-view-key-source) shared_view_key_source="$1" ;;
      --maker-agreement-key-source) maker_agreement_key_source="$1" ;;
      --actor-bin) actor_bin="$1" ;;
      --role-runner-bin) role_runner_bin="$1" ;;
      --composer-bin) composer_bin="$1" ;;
    esac
    shift
  done

  local required
  for required in run_id output_root taker_lez_owner maker_lez_owner sequencer_url \
      indexer_url monero_daemon_url monero_rpc_username_file monero_rpc_password_file \
      monero_amount_piconero lez_amount maker_xmr_funding_cutoff_ms refund_at_ms punish_at_ms; do
    [[ -n "${!required}" ]] || fail "missing required execute option for ${required}"
  done
  [[ -z "$explicit_swap_id" ]] || require_hex32 "$explicit_swap_id" "explicit swap ID"
  [[ -n "$shared_view_key_source" && -n "$maker_agreement_key_source" ]] ||
    [[ -z "$shared_view_key_source" && -z "$maker_agreement_key_source" ]] ||
    fail "shared view and Maker agreement key sources must be supplied together"
}

validate_execute_arguments() {
  local output_parent
  [[ "$run_id" =~ ^[a-z0-9][a-z0-9_-]{7,47}$ ]] ||
    fail "run ID must be 8..48 lowercase letters, numbers, underscores, or hyphens"
  [[ "$output_root" == /* && "$(dirname -- "$output_root")" != / ]] ||
    fail "output root must be a non-root absolute path"
  output_parent="$(dirname -- "$output_root")"
  require_safe_parent "$output_parent"
  require_new_path "$output_root" "output root"

  require_hex32 "$taker_lez_owner" "Taker LEZ owner"
  require_hex32 "$maker_lez_owner" "Maker LEZ owner"
  [[ "$taker_lez_owner" != "$maker_lez_owner" ]] || fail "role LEZ owners must differ"

  require_loopback_url "$sequencer_url" "sequencer URL"
  require_loopback_url "$indexer_url" "indexer URL"
  require_loopback_url "$monero_daemon_url" "Monero daemon URL"
  [[ "$sequencer_url" != "$indexer_url" && "$sequencer_url" != "$monero_daemon_url" &&
     "$indexer_url" != "$monero_daemon_url" ]] || fail "the three RPC roots must be distinct"

  require_private_file "$monero_rpc_username_file" "Monero RPC username file"
  require_private_file "$monero_rpc_password_file" "Monero RPC password file"
  [[ "$monero_rpc_username_file" != "$monero_rpc_password_file" ]] ||
    fail "Monero RPC username and password files must differ"
  if [[ -n "$shared_view_key_source" ]]; then
    require_private_file "$shared_view_key_source" "shared view-key source"
    require_private_file "$maker_agreement_key_source" "Maker agreement-key source"
    [[ "$shared_view_key_source" != "$maker_agreement_key_source" ]] ||
      fail "shared identity source files must differ"
  fi

  require_bounded_decimal "$monero_amount_piconero" 18446744073709551615 \
    "Monero amount"
  require_bounded_decimal "$lez_amount" 340282366920938463463374607431768211455 \
    "LEZ amount"
  require_deadline "$maker_xmr_funding_cutoff_ms" "Maker funding cutoff"
  require_deadline "$refund_at_ms" "refund deadline"
  require_deadline "$punish_at_ms" "punishment deadline"
  decimal_lt "$maker_xmr_funding_cutoff_ms" "$refund_at_ms" ||
    fail "Maker funding cutoff must precede refund deadline"
  decimal_lt "$refund_at_ms" "$punish_at_ms" ||
    fail "refund deadline must precede punishment deadline"

  require_binary "$actor_bin" "XMR reference actor"
  require_binary "$role_runner_bin" "adaptor role runner"
  require_binary "$composer_bin" "Stage-A composer"
}

verify_role_bundle() {
  local role="$1" role_root="$2" public_packet="$3" owner="$4" file
  require_private_directory "$role_root" "${role} private root"
  require_exact_entries "$role_root" \
    $'agreement.key\nclaim.key\nmanifest.json\nmonero-view.key\nrefund.key\nxmr-share.key' \
    "${role} private root"
  for file in agreement.key claim.key manifest.json monero-view.key refund.key xmr-share.key; do
    require_private_file "${role_root}/${file}" "${role} ${file}"
  done
  require_private_file "$public_packet" "${role} public packet"
  jq -e --arg role "$role" --arg owner "$owner" \
    '.role == $role and .lez_owner_account == $owner' "$public_packet" >/dev/null ||
    fail "${role} public packet is not bound to the supplied LEZ owner"
}

verify_public_role_pair() {
  jq -e -s '
    length == 2 and
    .[0].role == "taker" and .[1].role == "maker" and
    .[0].public_view_key == .[1].public_view_key and
    .[0].lez_owner_account != .[1].lez_owner_account and
    .[0].agreement_public_key != .[1].agreement_public_key and
    .[0].claim_session_public_key != .[1].claim_session_public_key and
    .[0].refund_session_public_key != .[1].refund_session_public_key
  ' "$taker_public_packet" "$maker_public_packet" >/dev/null ||
    fail "public role packets violate independent-role or shared-view invariants"
}

verify_session_root() {
  local role="$1" root="$2"
  require_private_directory "$root" "${role} session root"
  require_exact_entries "$root" $'claim.json\nrefund.json' "${role} session root"
  require_private_file "${root}/claim.json" "${role} claim session"
  require_private_file "${root}/refund.json" "${role} refund session"
  [[ ! "${root}/claim.json" -ef "${root}/refund.json" ]] ||
    fail "${role} claim and refund sessions alias one inode"
}

run_m4_round() {
  local purpose="$1" round maker_session taker_session maker_key taker_key
  local taker_partial taker_presignature
  round="${stage_b_exchange}/${purpose}"
  maker_session="${maker_session_root}/${purpose}.json"
  taker_session="${taker_session_root}/${purpose}.json"
  maker_key="${maker_private_root}/${purpose}.key"
  taker_key="${taker_private_root}/${purpose}.key"
  mkdir -m 0700 "$round"
  require_private_directory "$round" "${purpose} exchange directory"

  require_new_path "${round}/maker-commitment.json" "Maker ${purpose} commitment"
  "$role_runner_bin" maker --journal "$maker_journal" --session "$maker_session" \
    reserve --secret-key-file "$maker_key" --output "${round}/maker-commitment.json"
  require_private_file "${round}/maker-commitment.json" "Maker ${purpose} commitment"
  require_private_file "$maker_journal" "Maker role journal"

  require_new_path "${round}/taker-commitment.json" "Taker ${purpose} commitment"
  "$role_runner_bin" taker --journal "$taker_journal" --session "$taker_session" \
    reserve --secret-key-file "$taker_key" --output "${round}/taker-commitment.json"
  require_private_file "${round}/taker-commitment.json" "Taker ${purpose} commitment"
  require_private_file "$taker_journal" "Taker role journal"

  "$role_runner_bin" maker --journal "$maker_journal" --session "$maker_session" \
    accept-commitment --input "${round}/taker-commitment.json"
  "$role_runner_bin" taker --journal "$taker_journal" --session "$taker_session" \
    accept-commitment --input "${round}/maker-commitment.json"

  require_new_path "${round}/maker-nonce.json" "Maker ${purpose} nonce"
  "$role_runner_bin" maker --journal "$maker_journal" --session "$maker_session" \
    reveal-nonce --output "${round}/maker-nonce.json"
  require_private_file "${round}/maker-nonce.json" "Maker ${purpose} nonce"
  require_new_path "${round}/taker-nonce.json" "Taker ${purpose} nonce"
  "$role_runner_bin" taker --journal "$taker_journal" --session "$taker_session" \
    reveal-nonce --output "${round}/taker-nonce.json"
  require_private_file "${round}/taker-nonce.json" "Taker ${purpose} nonce"

  require_new_path "${round}/maker-partial.json" "Maker ${purpose} partial"
  "$role_runner_bin" maker --journal "$maker_journal" --session "$maker_session" \
    accept-nonce-sign --input "${round}/taker-nonce.json" \
    --secret-key-file "$maker_key" --output "${round}/maker-partial.json"
  require_private_file "${round}/maker-partial.json" "Maker ${purpose} partial"

  if [[ "$purpose" == claim ]]; then
    taker_partial="${taker_outbox}/claim-partial.json"
    taker_presignature="${taker_outbox}/claim-presignature.json"
  else
    taker_partial="${round}/taker-partial.json"
    taker_presignature="${round}/taker-presignature.json"
  fi
  require_new_path "$taker_partial" "Taker ${purpose} partial"
  "$role_runner_bin" taker --journal "$taker_journal" --session "$taker_session" \
    accept-nonce-sign --input "${round}/maker-nonce.json" \
    --secret-key-file "$taker_key" --output "$taker_partial"
  require_private_file "$taker_partial" "Taker ${purpose} partial"

  require_new_path "$taker_presignature" "Taker ${purpose} presignature"
  "$role_runner_bin" taker --journal "$taker_journal" --session "$taker_session" \
    accept-peer-partial --input "${round}/maker-partial.json" --output "$taker_presignature"
  require_private_file "$taker_presignature" "Taker ${purpose} presignature"

  if [[ "$purpose" == refund ]]; then
    require_new_path "${round}/maker-presignature.json" "Maker refund presignature"
    "$role_runner_bin" maker --journal "$maker_journal" --session "$maker_session" \
      accept-peer-partial --input "${round}/taker-partial.json" \
      --output "${round}/maker-presignature.json"
    require_private_file "${round}/maker-presignature.json" "Maker refund presignature"
    cmp -- "${round}/maker-presignature.json" "${round}/taker-presignature.json" ||
      fail "role-local refund presignatures differ"
  fi

  require_private_file "$maker_journal" "Maker role journal"
  require_private_file "$taker_journal" "Taker role journal"
}

run_execute() {
  parse_execute_arguments "$@"
  validate_execute_arguments

  mkdir -m 0700 "$output_root"
  require_private_directory "$output_root" "agreement output root"

  readonly material_root="${output_root}/material"
  readonly exchange_root="${output_root}/exchange"
  readonly handoff_root="${output_root}/handoff"
  readonly stage_b_root="${output_root}/stage-b"
  readonly stage_b_exchange="${stage_b_root}/exchange"
  readonly stage_b_private="${stage_b_root}/private"
  readonly taker_outbox="${stage_b_private}/taker-outbox"
  readonly stage_b_signatures="${stage_b_root}/signatures"
  mkdir -m 0700 "$material_root" "$exchange_root" "$handoff_root" "$stage_b_root" \
    "$stage_b_exchange" "$stage_b_private" "$taker_outbox" "$stage_b_signatures"

  readonly taker_private_root="${material_root}/taker"
  readonly maker_private_root="${material_root}/maker"
  readonly taker_public_packet="${exchange_root}/taker.json"
  readonly maker_public_packet="${exchange_root}/maker.json"
  readonly shared_view_handoff="${handoff_root}/monero-view.key"

  local -a taker_identity_args=() maker_identity_args=()
  if [[ -n "$shared_view_key_source" ]]; then
    taker_identity_args=(--shared-view-key-file "$shared_view_key_source")
    maker_identity_args=(--agreement-key-file "$maker_agreement_key_source")
  fi
  "$actor_bin" provision taker --private-root "$taker_private_root" \
    --lez-owner-account "$taker_lez_owner" "${taker_identity_args[@]}" \
    --public-packet "$taker_public_packet"
  verify_role_bundle taker "$taker_private_root" "$taker_public_packet" "$taker_lez_owner"

  publish_private_copy_new "${taker_private_root}/monero-view.key" \
    "$shared_view_handoff" "shared Monero view-key handoff"

  "$actor_bin" provision maker --private-root "$maker_private_root" \
    --lez-owner-account "$maker_lez_owner" --shared-view-key-file "$shared_view_handoff" \
    "${maker_identity_args[@]}" \
    --public-packet "$maker_public_packet"
  verify_role_bundle maker "$maker_private_root" "$maker_public_packet" "$maker_lez_owner"
  cmp -- "${maker_private_root}/monero-view.key" "$shared_view_handoff" ||
    fail "Maker role did not retain the exact private view-key handoff"
  verify_public_role_pair

  local swap_id
  if [[ -n "$explicit_swap_id" ]]; then
    swap_id="$explicit_swap_id"
  else
    swap_id="$(printf '%s' "${run_id}:stage-a:001" | sha256sum | cut -d' ' -f1)"
  fi
  readonly swap_id
  readonly unsigned_stage_a="${exchange_root}/unsigned-stage-a.bin"
  readonly composer_receipt="${exchange_root}/stage-a-composer-receipt.json"
  local unsigned_stage_a_bytes
  require_new_path "$unsigned_stage_a" "unsigned Stage A"
  require_new_path "$composer_receipt" "Stage-A composer receipt"
  "$composer_bin" --sequencer-url "$sequencer_url" --indexer-url "$indexer_url" \
    --monero-daemon-url "$monero_daemon_url" \
    --monero-rpc-username-file "$monero_rpc_username_file" \
    --monero-rpc-password-file "$monero_rpc_password_file" \
    --maker-public-packet "$maker_public_packet" --taker-public-packet "$taker_public_packet" \
    --output-unsigned-stage-a "$unsigned_stage_a" --swap-id "$swap_id" \
    --monero-amount-piconero "$monero_amount_piconero" --lez-amount "$lez_amount" \
    --maker-xmr-funding-cutoff-ms "$maker_xmr_funding_cutoff_ms" \
    --refund-at-ms "$refund_at_ms" --punish-at-ms "$punish_at_ms" >"$composer_receipt"
  require_private_file "$unsigned_stage_a" "unsigned Stage A"
  require_private_file "$composer_receipt" "Stage-A composer receipt"
  unsigned_stage_a_bytes="$(stat -c '%s' -- "$unsigned_stage_a")"
  jq -e --argjson wire_bytes "$unsigned_stage_a_bytes" '
    def hex32:
      type == "string"
      and test("^[0-9a-f]{64}$")
      and (test("^0{64}$") == false);
    type == "object"
    and keys == [
      "agreement_commitment", "lez_channel_id", "lez_finalized_block_hash",
      "lez_finalized_height", "lez_genesis_hash", "monero_genesis_hash", "wire_bytes"
    ]
    and (.agreement_commitment | hex32)
    and (.monero_genesis_hash | hex32)
    and (.lez_genesis_hash | hex32)
    and (.lez_channel_id | hex32)
    and (.lez_finalized_block_hash | hex32)
    and (.lez_finalized_height | type == "number" and . >= 0 and floor == .)
    and (.wire_bytes == $wire_bytes and .wire_bytes > 0)
  ' "$composer_receipt" >/dev/null ||
    fail "Stage-A composer receipt identity/wire metadata is invalid"

  readonly taker_stage_a_signature="${exchange_root}/taker-stage-a.sig"
  readonly maker_stage_a_signature="${exchange_root}/maker-stage-a.sig"
  readonly agreement_stage_a="${exchange_root}/agreement-stage-a.bin"
  "$actor_bin" sign-stage-a taker --private-root "$taker_private_root" \
    --own-public-packet "$taker_public_packet" --peer-public-packet "$maker_public_packet" \
    --unsigned-stage-a "$unsigned_stage_a" --output-signature "$taker_stage_a_signature"
  require_private_file "$taker_stage_a_signature" "Taker Stage-A signature"
  "$actor_bin" sign-stage-a maker --private-root "$maker_private_root" \
    --own-public-packet "$maker_public_packet" --peer-public-packet "$taker_public_packet" \
    --unsigned-stage-a "$unsigned_stage_a" --output-signature "$maker_stage_a_signature"
  require_private_file "$maker_stage_a_signature" "Maker Stage-A signature"
  "$actor_bin" assemble-stage-a --maker-public-packet "$maker_public_packet" \
    --taker-public-packet "$taker_public_packet" --unsigned-stage-a "$unsigned_stage_a" \
    --maker-signature "$maker_stage_a_signature" --taker-signature "$taker_stage_a_signature" \
    --output-stage-a "$agreement_stage_a"
  require_private_file "$agreement_stage_a" "countersigned Stage A"

  readonly taker_session_root="${material_root}/taker-sessions"
  readonly maker_session_root="${material_root}/maker-sessions"
  "$actor_bin" initialize-sessions taker --private-root "$taker_private_root" \
    --own-public-packet "$taker_public_packet" --peer-public-packet "$maker_public_packet" \
    --agreement-stage-a "$agreement_stage_a" --session-root "$taker_session_root"
  verify_session_root taker "$taker_session_root"
  "$actor_bin" initialize-sessions maker --private-root "$maker_private_root" \
    --own-public-packet "$maker_public_packet" --peer-public-packet "$taker_public_packet" \
    --agreement-stage-a "$agreement_stage_a" --session-root "$maker_session_root"
  verify_session_root maker "$maker_session_root"
  cmp -- "${taker_session_root}/claim.json" "${maker_session_root}/claim.json" ||
    fail "claim session bytes differ across roles"
  cmp -- "${taker_session_root}/refund.json" "${maker_session_root}/refund.json" ||
    fail "refund session bytes differ across roles"

  readonly maker_journal="${stage_b_private}/maker.sqlite"
  readonly taker_journal="${stage_b_private}/taker.sqlite"
  run_m4_round claim
  [[ ! -e "${stage_b_exchange}/claim/taker-partial.json" &&
     ! -L "${stage_b_exchange}/claim/taker-partial.json" ]] ||
    fail "Taker claim partial crossed into the exchange"
  [[ ! -e "${stage_b_exchange}/claim/taker-presignature.json" &&
     ! -L "${stage_b_exchange}/claim/taker-presignature.json" ]] ||
    fail "Taker claim presignature crossed into the exchange"
  run_m4_round refund

  readonly unsigned_stage_b="${stage_b_root}/unsigned-stage-b.bin"
  readonly maker_stage_b_signature="${stage_b_signatures}/maker.sig"
  readonly taker_stage_b_signature="${stage_b_signatures}/taker.sig"
  readonly agreement_stage_b="${stage_b_root}/stage-b.bin"
  "$actor_bin" compose-stage-b --private-root "$taker_private_root" \
    --own-public-packet "$taker_public_packet" --peer-public-packet "$maker_public_packet" \
    --agreement-stage-a "$agreement_stage_a" --journal "$taker_journal" \
    --output-unsigned-stage-b "$unsigned_stage_b"
  require_private_file "$unsigned_stage_b" "unsigned Stage B"
  "$actor_bin" sign-stage-b maker --private-root "$maker_private_root" \
    --own-public-packet "$maker_public_packet" --peer-public-packet "$taker_public_packet" \
    --agreement-stage-a "$agreement_stage_a" --unsigned-stage-b "$unsigned_stage_b" \
    --output-signature "$maker_stage_b_signature"
  require_private_file "$maker_stage_b_signature" "Maker Stage-B signature"
  "$actor_bin" sign-stage-b taker --private-root "$taker_private_root" \
    --own-public-packet "$taker_public_packet" --peer-public-packet "$maker_public_packet" \
    --agreement-stage-a "$agreement_stage_a" --unsigned-stage-b "$unsigned_stage_b" \
    --output-signature "$taker_stage_b_signature"
  require_private_file "$taker_stage_b_signature" "Taker Stage-B signature"
  "$actor_bin" assemble-stage-b taker --private-root "$taker_private_root" \
    --own-public-packet "$taker_public_packet" --peer-public-packet "$maker_public_packet" \
    --agreement-stage-a "$agreement_stage_a" --unsigned-stage-b "$unsigned_stage_b" \
    --maker-signature "$maker_stage_b_signature" --taker-signature "$taker_stage_b_signature" \
    --output-stage-b "$agreement_stage_b"
  require_private_file "$agreement_stage_b" "countersigned Stage B"

  readonly final_receipt="${output_root}/agreement-receipt.json"
  require_new_path "$final_receipt" "agreement receipt"
  jq -n --arg run_id "$run_id" --arg swap_id "$swap_id" \
    --arg stage_a_sha256 "$(sha256_file "$agreement_stage_a")" \
    --arg stage_b_sha256 "$(sha256_file "$agreement_stage_b")" \
    --arg monero_amount_piconero "$monero_amount_piconero" --arg lez_amount "$lez_amount" \
    --arg maker_xmr_funding_cutoff_ms "$maker_xmr_funding_cutoff_ms" \
    --arg refund_at_ms "$refund_at_ms" --arg punish_at_ms "$punish_at_ms" '
      {schema_version:1,kind:"m4_xmr_agreement_receipt",result:"passed",run_id:$run_id,
       swap_id:$swap_id,stage_a_sha256:$stage_a_sha256,stage_b_sha256:$stage_b_sha256,
       requested_terms:{monero_amount_piconero:$monero_amount_piconero,lez_amount:$lez_amount,
         maker_xmr_funding_cutoff_ms:$maker_xmr_funding_cutoff_ms,
         refund_at_ms:$refund_at_ms,punish_at_ms:$punish_at_ms},
       terms_bound_to_stage_material_by_helper:false,
       composer_receipt_validation_scope:"schema_shape_and_unsigned_wire_length_only",
       composer_receipt_wire_bytes_matched_output:true,
       submission_performed:false,stage_a_rpc_scope:"read_only",
       sessions_equal_across_roles:true,taker_claim_material_private:true,
       refund_presignatures_equal:true,stage_b_countersigned:true}
    ' >"$final_receipt"
  require_private_file "$final_receipt" "agreement receipt"
  cat -- "$final_receipt"
}

main() {
  local mode="${1:-}"
  [[ -n "$mode" ]] || fail "expected mode: contract or execute"
  shift
  case "$mode" in
    contract)
      (( $# == 0 )) || fail "contract mode accepts no arguments"
      require_command jq
      emit_contract
      ;;
    execute)
      local command_name
      for command_name in bash cat chmod cmp cp cut find id jq ln mkdir mktemp readlink \
          sha256sum sort stat unlink; do
        require_command "$command_name"
      done
      run_execute "$@"
      ;;
    *) fail "unknown mode: ${mode}" ;;
  esac
}

main "$@"
