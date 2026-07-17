#!/usr/bin/env bash
set -euo pipefail

export LC_ALL=C
umask 077

readonly mode="${M3_F7_TOKEN_FIXTURE_MODE:-execute}"
readonly pinned_commit="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
readonly total_supply=1000
readonly funded_balance=250

emit_contract() {
  jq -n --arg commit "$pinned_commit" \
    --argjson supply "$total_supply" --argjson funded "$funded_balance" '
    {
      schema_version:1,kind:"m3_f7_official_token_fixture_contract",
      execution_performed:false,
      upstream:{source_commit:$commit,wallet:"official_lez_v0_2_wallet"},
      assets:[
        {name:"M3F7A",depositor:"maker",initial_depositor_balance:$funded,
         initial_counterparty_balance:0,total_supply:$supply},
        {name:"M3F7B",depositor:"taker",initial_depositor_balance:$funded,
         initial_counterparty_balance:0,total_supply:$supply}],
      accounts:{definitions:2,supply_accounts:2,actor_atas:4},
      transactions:{token_definitions:2,ata_creations:4,initial_funding:2},
      isolation:{loopback_only:true,refuses_state_reuse:true,
        wallet_directory_mode:"0700",wallet_file_mode:"0600",broad_cleanup_used:false},
      external_resources:{public_rpc:false,faucet:false,public_funds:false,
        test_funds:"official_local_token_definitions_and_transfers"},
      security:{private_key_import_transport:"argv_local_poc_only",
        upstream_wallet_storage:"plaintext_json_unencrypted",
        mnemonic_persisted_by_fixture:false,production_ready:false}
    }'
}

case "$mode" in
  contract)
    command -v jq >/dev/null || exit 2
    emit_contract
    exit 0
    ;;
  execute) ;;
  *) echo "M3_F7_TOKEN_FIXTURE_MODE must be execute or contract" >&2; exit 2 ;;
esac

fail() {
  echo "M3 F7 official-token fixture failed: $*" >&2
  exit 2
}

for command_name in chmod dirname env git jq mkdir mv readlink rg sed sha256sum sort stat \
  timeout tr wc; do
  command -v "$command_name" >/dev/null || fail "missing required tool: ${command_name}"
done
for name in RUN_ID OUTPUT_ROOT WALLET_BIN SOURCE_DIR LEZ_MANIFEST \
  MAKER_IDENTITY MAKER_KEY TAKER_IDENTITY TAKER_KEY; do
  variable="M3_F7_TOKEN_FIXTURE_${name}"
  [[ -n "${!variable:-}" ]] || fail "set ${variable}"
done

readonly run_id="$M3_F7_TOKEN_FIXTURE_RUN_ID"
readonly output_root="$M3_F7_TOKEN_FIXTURE_OUTPUT_ROOT"
readonly wallet_bin="$M3_F7_TOKEN_FIXTURE_WALLET_BIN"
readonly source_dir="$M3_F7_TOKEN_FIXTURE_SOURCE_DIR"
readonly lez_manifest="$M3_F7_TOKEN_FIXTURE_LEZ_MANIFEST"
readonly maker_identity="$M3_F7_TOKEN_FIXTURE_MAKER_IDENTITY"
readonly maker_key_file="$M3_F7_TOKEN_FIXTURE_MAKER_KEY"
readonly taker_identity="$M3_F7_TOKEN_FIXTURE_TAKER_IDENTITY"
readonly taker_key_file="$M3_F7_TOKEN_FIXTURE_TAKER_KEY"
readonly testing="${M3_F7_TOKEN_FIXTURE_TESTING:-0}"

[[ "$run_id" =~ ^[a-z0-9][a-z0-9_-]{7,47}$ ]] || fail "invalid run ID"
[[ "$output_root" == /* && ! -e "$output_root" && ! -L "$output_root" ]] ||
  fail "output root must be one new absolute path"
[[ -d "$(dirname "$output_root")" && ! -L "$(dirname "$output_root")" ]] ||
  fail "output-root parent must be one existing non-symlink directory"

case "$testing" in
  0)
    [[ -z "${M3_F7_TOKEN_FIXTURE_EXPECTED_SOURCE_COMMIT+x}" ]] ||
      fail "source-commit override is forbidden"
    expected_commit="$pinned_commit"
    ;;
  1)
    expected_commit="${M3_F7_TOKEN_FIXTURE_EXPECTED_SOURCE_COMMIT:-}"
    [[ "$expected_commit" =~ ^[0-9a-f]{40}$ ]] || fail "invalid testing commit"
    ;;
  *) fail "testing mode must be exactly 0 or 1" ;;
esac
readonly expected_commit

[[ -d "$source_dir/.git" && ! -L "$source_dir" &&
   "$(readlink -f "$source_dir")" == "$source_dir" ]] || fail "unsafe source checkout"
[[ -z "$(git -C "$source_dir" status --porcelain --untracked-files=all)" ]] ||
  fail "source checkout is dirty"
actual_commit="$(git -C "$source_dir" rev-parse HEAD)"
readonly actual_commit
[[ "$actual_commit" == "$expected_commit" ]] || fail "source commit mismatch"
if [[ "$testing" == 0 ]]; then
  [[ "$(git -C "$source_dir" rev-parse 'refs/tags/v0.2.0^{}')" == "$pinned_commit" ]] ||
    fail "v0.2.0 tag mismatch"
fi
[[ "$wallet_bin" == /* && -x "$wallet_bin" && -f "$wallet_bin" &&
   ! -L "$wallet_bin" && "$(readlink -f "$wallet_bin")" == "$wallet_bin" ]] ||
  fail "unsafe official wallet binary"
wallet_sha256="$(sha256sum "$wallet_bin" | sed 's| .*||')"
readonly wallet_sha256
[[ "$wallet_sha256" =~ ^[0-9a-f]{64}$ ]] || fail "wallet SHA-256 is invalid"

validate_private_file() {
  local path="$1" label="$2"
  [[ "$path" == /* && -f "$path" && ! -L "$path" &&
     "$(readlink -f "$path")" == "$path" && "$(stat -c '%a' "$path")" == 600 ]] ||
    fail "${label} must be a canonical mode-0600 regular file"
}
for entry in \
  "$lez_manifest:LEZ manifest" "$maker_identity:maker identity" \
  "$maker_key_file:maker key" "$taker_identity:taker identity" \
  "$taker_key_file:taker key"; do
  validate_private_file "${entry%%:*}" "${entry#*:}"
done

manifest_value() {
  local key="$1"
  local -a values=()
  mapfile -t values < <(sed -n "s/^${key}=//p" "$lez_manifest")
  [[ "${#values[@]}" == 1 && -n "${values[0]}" ]] ||
    fail "manifest must contain exactly one ${key}"
  printf '%s\n' "${values[0]}"
}
validate_loopback() {
  local value="$1" label="$2" port
  [[ "$value" =~ ^http://127\.0\.0\.1:([1-9][0-9]{0,4})$ ]] ||
    fail "${label} must be literal IPv4 loopback HTTP"
  port="${BASH_REMATCH[1]}"
  (( 10#$port <= 65535 )) || fail "${label} port is invalid"
}
sequencer_url="$(manifest_value LEZ_SEQUENCER_RPC_URL)"
indexer_url="$(manifest_value LEZ_INDEXER_RPC_URL)"
readonly sequencer_url indexer_url
validate_loopback "$sequencer_url" sequencer
validate_loopback "$indexer_url" indexer
[[ "$sequencer_url" != "$indexer_url" ]] || fail "node endpoints collided"

read_actor() {
  local file="$1" role="$2"
  jq -e '.schema == "lez-v0.2-local-actor-identity" and .version == 2
    and (.account_id | strings | test("^[1-9A-HJ-NP-Za-km-z]{43,44}$"))' \
    "$file" >/dev/null || fail "invalid ${role} identity"
  jq -er '.account_id' "$file"
}
read_key() {
  local file="$1" role="$2" key
  [[ "$(wc -l <"$file" | tr -d '[:space:]')" == 1 ]] ||
    fail "${role} key must contain one line"
  key="$(sed -n '1p' "$file")"
  [[ "$key" =~ ^[0-9a-f]{64}$ ]] || fail "invalid ${role} key"
  printf '%s\n' "$key"
}
maker="$(read_actor "$maker_identity" maker)"
taker="$(read_actor "$taker_identity" taker)"
readonly maker taker
[[ "$maker" != "$taker" ]] || fail "actor identities collided"

readonly evidence_dir="${output_root}/evidence"
readonly private_dir="${output_root}/private"
readonly wallet_root="${private_dir}/wallets"
readonly logs_dir="${private_dir}/wallet-logs"
readonly maker_home="${wallet_root}/maker"
readonly taker_home="${wallet_root}/taker"
readonly tx_records="${private_dir}/transactions.ndjson"
readonly evidence_file="${evidence_dir}/f7-token-fixture.json"
mkdir -m 0700 "$output_root"
mkdir -m 0700 "$evidence_dir" "$private_dir" "$wallet_root" "$logs_dir" \
  "$maker_home" "$taker_home"
: >"$tx_records"
chmod 0600 "$tx_records"

write_config() {
  local path="$1/wallet_config.json"
  jq -n --arg endpoint "$sequencer_url" \
    '{sequencer_addr:$endpoint,seq_poll_timeout:"1s",
      seq_tx_poll_max_blocks:120,seq_poll_max_retries:5,
      seq_block_poll_max_amount:100}' >"$path"
  chmod 0600 "$path"
}
write_config "$maker_home"
write_config "$taker_home"

wallet_home() {
  case "$1" in
    maker) printf '%s\n' "$maker_home" ;;
    taker) printf '%s\n' "$taker_home" ;;
    *) fail "invalid wallet role" ;;
  esac
}
WALLET_OUTPUT=""
wallet_call() {
  local role="$1" label="$2" home
  shift 2
  home="$(wallet_home "$role")"
  WALLET_OUTPUT="$(
    printf '%s\n' 'local-poc-password-unused-upstream' |
      timeout --preserve-status 180s env LEE_WALLET_HOME_DIR="$home" "$wallet_bin" "$@"
  )" || fail "wallet ${role}/${label} failed"
  chmod 0600 "${home}/wallet_config.json"
  if [[ -f "${home}/storage.json" ]]; then
    [[ ! -L "${home}/storage.json" ]] || fail "wallet storage became a symlink"
    chmod 0600 "${home}/storage.json"
  fi
  if [[ "$label" != secret-import ]]; then
    printf '%s\n' "$WALLET_OUTPUT" >"${logs_dir}/${role}-${label}.log"
    chmod 0600 "${logs_dir}/${role}-${label}.log"
  fi
}

maker_key="$(read_key "$maker_key_file" maker)"
taker_key="$(read_key "$taker_key_file" taker)"
wallet_call maker secret-import account import public --private-key "$maker_key"
[[ "$(sed -n 's|^Imported public account Public/||p' <<<"$WALLET_OUTPUT")" == "$maker" ]] ||
  fail "maker import identity mismatch"
WALLET_OUTPUT=""
wallet_call taker secret-import account import public --private-key "$taker_key"
[[ "$(sed -n 's|^Imported public account Public/||p' <<<"$WALLET_OUTPUT")" == "$taker" ]] ||
  fail "taker import identity mismatch"
WALLET_OUTPUT=""
unset maker_key taker_key
for role in maker taker; do
  home="$(wallet_home "$role")"
  [[ -f "${home}/storage.json" && ! -L "${home}/storage.json" &&
     "$(stat -c '%a' "$home")" == 700 &&
     "$(stat -c '%a' "${home}/wallet_config.json")" == 600 &&
     "$(stat -c '%a' "${home}/storage.json")" == 600 ]] ||
    fail "${role} wallet state is not owner-private"
done

parse_account() {
  local output="$1" label="$2"
  local -a found=()
  mapfile -t found < <(sed -n \
    's|^Generated new account with account_id Public/\([1-9A-HJ-NP-Za-km-z]\{43,44\}\) at path .*|\1|p' \
    <<<"$output")
  [[ "${#found[@]}" == 1 ]] || fail "${label} account output is ambiguous"
  printf '%s\n' "${found[0]}"
}
parse_tx() {
  local output="$1" label="$2"
  local -a found=()
  mapfile -t found < <(sed -n 's|^Transaction hash is \([0-9a-f]\{64\}\)$|\1|p' \
    <<<"$output")
  [[ "${#found[@]}" == 1 ]] || fail "${label} transaction output is ambiguous"
  printf '%s\n' "${found[0]}"
}
parse_ata() {
  local output="$1" label="$2"
  local -a found=()
  mapfile -t found < <(sed -n '/^[1-9A-HJ-NP-Za-km-z]\{43,44\}$/p' <<<"$output")
  [[ "${#found[@]}" == 1 ]] || fail "${label} ATA output is ambiguous"
  printf '%s\n' "${found[0]}"
}
record_tx() {
  jq -nc --arg kind "$1" --arg asset "$2" --arg role "$3" --arg transaction_id "$4" \
    '{kind:$kind,asset:$asset,submitted_by:$role,transaction_id:$transaction_id}' \
    >>"$tx_records"
}

wallet_call maker new-a-definition account new public --label f7-a-definition
definition_a="$(parse_account "$WALLET_OUTPUT" "M3F7A definition")"
wallet_call maker new-a-supply account new public --label f7-a-supply
supply_a="$(parse_account "$WALLET_OUTPUT" "M3F7A supply")"
wallet_call taker new-b-definition account new public --label f7-b-definition
definition_b="$(parse_account "$WALLET_OUTPUT" "M3F7B definition")"
wallet_call taker new-b-supply account new public --label f7-b-supply
supply_b="$(parse_account "$WALLET_OUTPUT" "M3F7B supply")"
readonly definition_a supply_a definition_b supply_b
[[ "$(printf '%s\n' "$definition_a" "$supply_a" "$definition_b" "$supply_b" |
  sort -u | wc -l | tr -d '[:space:]')" == 4 ]] || fail "definition/supply IDs collided"

wallet_call maker create-token-a token new --definition-account-id f7-a-definition \
  --supply-account-id f7-a-supply --name M3F7A --total-supply "$total_supply"
tx="$(parse_tx "$WALLET_OUTPUT" "M3F7A definition")"
record_tx token_definition M3F7A maker "$tx"
wallet_call taker create-token-b token new --definition-account-id f7-b-definition \
  --supply-account-id f7-b-supply --name M3F7B --total-supply "$total_supply"
tx="$(parse_tx "$WALLET_OUTPUT" "M3F7B definition")"
record_tx token_definition M3F7B taker "$tx"

wallet_call maker derive-maker-a ata address --owner "$maker" --token-definition "$definition_a"
maker_ata_a="$(parse_ata "$WALLET_OUTPUT" "maker M3F7A")"
wallet_call maker derive-taker-a ata address --owner "$taker" --token-definition "$definition_a"
taker_ata_a="$(parse_ata "$WALLET_OUTPUT" "taker M3F7A")"
wallet_call maker derive-maker-b ata address --owner "$maker" --token-definition "$definition_b"
maker_ata_b="$(parse_ata "$WALLET_OUTPUT" "maker M3F7B")"
wallet_call maker derive-taker-b ata address --owner "$taker" --token-definition "$definition_b"
taker_ata_b="$(parse_ata "$WALLET_OUTPUT" "taker M3F7B")"
readonly maker_ata_a taker_ata_a maker_ata_b taker_ata_b
[[ "$(printf '%s\n' "$maker_ata_a" "$taker_ata_a" "$maker_ata_b" "$taker_ata_b" |
  sort -u | wc -l | tr -d '[:space:]')" == 4 ]] || fail "actor ATAs collided"

for spec in \
  "maker:M3F7A:$maker:$definition_a:create-maker-a" \
  "taker:M3F7A:$taker:$definition_a:create-taker-a" \
  "maker:M3F7B:$maker:$definition_b:create-maker-b" \
  "taker:M3F7B:$taker:$definition_b:create-taker-b"; do
  IFS=: read -r role asset owner definition label <<<"$spec"
  wallet_call "$role" "$label" ata create --owner "Public/${owner}" \
    --token-definition "$definition"
  tx="$(parse_tx "$WALLET_OUTPUT" "${role} ${asset} ATA")"
  record_tx ata_creation "$asset" "$role" "$tx"
done

wallet_call maker fund-maker-a token send --from f7-a-supply \
  --to "Public/${maker_ata_a}" --amount "$funded_balance"
tx="$(parse_tx "$WALLET_OUTPUT" "M3F7A funding")"
record_tx initial_funding M3F7A maker "$tx"
wallet_call taker fund-taker-b token send --from f7-b-supply \
  --to "Public/${taker_ata_b}" --amount "$funded_balance"
tx="$(parse_tx "$WALLET_OUTPUT" "M3F7B funding")"
record_tx initial_funding M3F7B taker "$tx"

wallet_call maker balances-maker ata list --owner "$maker" \
  --token-definition "$definition_a" "$definition_b"
maker_balances="$WALLET_OUTPUT"
wallet_call maker balances-taker ata list --owner "$taker" \
  --token-definition "$definition_a" "$definition_b"
taker_balances="$WALLET_OUTPUT"
rg -Fqx "ATA ${maker_ata_a} (definition ${definition_a}): balance 250" \
  <<<"$maker_balances" || fail "maker M3F7A balance mismatch"
rg -Fqx "ATA ${maker_ata_b} (definition ${definition_b}): balance 0" \
  <<<"$maker_balances" || fail "maker M3F7B balance mismatch"
rg -Fqx "ATA ${taker_ata_a} (definition ${definition_a}): balance 0" \
  <<<"$taker_balances" || fail "taker M3F7A balance mismatch"
rg -Fqx "ATA ${taker_ata_b} (definition ${definition_b}): balance 250" \
  <<<"$taker_balances" || fail "taker M3F7B balance mismatch"
[[ "$(wc -l <"$tx_records" | tr -d '[:space:]')" == 8 &&
   "$(jq -s '[.[].transaction_id] | unique | length' "$tx_records")" == 8 ]] ||
  fail "transaction evidence is incomplete or aliased"

jq -n --arg run_id "$run_id" --arg source_commit "$actual_commit" \
  --arg wallet_sha "$wallet_sha256" --arg sequencer "$sequencer_url" \
  --arg indexer "$indexer_url" --arg maker "$maker" --arg taker "$taker" \
  --arg da "$definition_a" --arg sa "$supply_a" \
  --arg db "$definition_b" --arg sb "$supply_b" \
  --arg maa "$maker_ata_a" --arg taa "$taker_ata_a" \
  --arg mab "$maker_ata_b" --arg tab "$taker_ata_b" \
  --slurpfile transactions "$tx_records" '
  {
    schema_version:1,kind:"m3_f7_official_token_fixture",result:"passed",run_id:$run_id,
    upstream:{source_commit:$source_commit,wallet:"official_lez_v0_2_wallet",
      wallet_binary_sha256:$wallet_sha},
    actors:{maker:$maker,taker:$taker},
    assets:{
      M3F7A:{definition:$da,supply_account:$sa,depositor:"maker",total_supply:1000,
        atas:{maker:$maa,taker:$taa},initial_balances:{maker:250,taker:0}},
      M3F7B:{definition:$db,supply_account:$sb,depositor:"taker",total_supply:1000,
        atas:{maker:$mab,taker:$tab},initial_balances:{maker:0,taker:250}}},
    transactions:$transactions,
    external_resources:{public_rpc:false,faucet:false,public_funds:false,
      sequencer:$sequencer,indexer:$indexer},
    finality:{initial_balance_read:"official_wallet_sequencer_account_read",
      swap_certification_requires_finalized_indexer_evidence:true},
    isolation:{output_root_new:true,wallet_roles_separate:true,loopback_only:true,
      wallet_directory_mode:"0700",wallet_file_mode:"0600",broad_cleanup_used:false},
    security:{private_key_import_transport:"argv_local_poc_only",
      upstream_wallet_storage:"plaintext_json_unencrypted",
      mnemonic_persisted_by_fixture:false,production_ready:false}
  }' >"${evidence_file}.partial"
chmod 0600 "${evidence_file}.partial"
mv "${evidence_file}.partial" "$evidence_file"
jq -e '.result == "passed" and (.transactions | length) == 8
  and ([.transactions[].transaction_id] | unique | length) == 8
  and .external_resources.public_rpc == false and .external_resources.faucet == false
  and .external_resources.public_funds == false
  and .finality.swap_certification_requires_finalized_indexer_evidence == true
  and .security.mnemonic_persisted_by_fixture == false
  and .security.production_ready == false' "$evidence_file" >/dev/null ||
  fail "final evidence is inconsistent"

echo "M3 F7 official-token fixture passed: ${evidence_file}"
