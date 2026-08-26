#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly runner="scripts/run-m3-f7-token-fixture.sh"

fail() {
  echo "M3 F7 token-fixture contract failed: $*" >&2
  exit 1
}

[[ -x "$runner" && ! -L "$runner" ]] || fail "runner is missing, non-executable, or a symlink"
bash -n "$runner"

contract="$(M3_F7_TOKEN_FIXTURE_MODE=contract "$runner")" ||
  fail "contract mode failed"
jq -e '
  .schema_version == 1
  and .kind == "m3_f7_official_token_fixture_contract"
  and .execution_performed == false
  and .upstream.source_commit == "a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
  and .upstream.wallet == "official_lez_v0_2_wallet"
  and .assets == [
    {name:"M3F7A",depositor:"maker",initial_depositor_balance:250,
     initial_counterparty_balance:0,total_supply:1000},
    {name:"M3F7B",depositor:"taker",initial_depositor_balance:250,
     initial_counterparty_balance:0,total_supply:1000}
  ]
  and .accounts.definitions == 2
  and .accounts.supply_accounts == 2
  and .accounts.actor_atas == 4
  and .transactions.token_definitions == 2
  and .transactions.ata_creations == 4
  and .transactions.initial_funding == 2
  and .isolation.loopback_only == true
  and .isolation.refuses_state_reuse == true
  and .isolation.wallet_directory_mode == "0700"
  and .isolation.wallet_file_mode == "0600"
  and .external_resources.public_rpc == false
  and .external_resources.faucet == false
  and .external_resources.public_funds == false
  and .security.private_key_import_transport == "argv_local_poc_only"
  and .security.upstream_wallet_storage == "plaintext_json_unencrypted"
  and .security.production_ready == false
  and .security.mnemonic_persisted_by_fixture == false
' <<<"$contract" >/dev/null || fail "contract is incomplete"

scratch="$(mktemp -d /tmp/m3-f7-token-fixture-contract.XXXXXX)"
readonly scratch
cleanup() {
  rm -rf --one-file-system -- "$scratch"
}
trap cleanup EXIT

readonly source_dir="${scratch}/source"
mkdir -m 0700 "$source_dir"
git -C "$source_dir" init -q
git -C "$source_dir" config user.name fixture
git -C "$source_dir" config user.email fixture@example.invalid
printf '%s\n' pinned >"${source_dir}/PINNED"
git -C "$source_dir" add PINNED
git -C "$source_dir" commit -q -m pinned
source_commit="$(git -C "$source_dir" rev-parse HEAD)"
readonly source_commit

readonly maker="6RM5ZNkJTJq956xSqMaN3aWY2m1XBihTen6YGMbVCLHp"
readonly taker="6RaTRpBrQ5oLHMggooPAWzGDn4KhwuT3whZQx8cPNk27"
readonly def_a="5qQHUjQQ6QGwD2JbEN8X4Kk38wuyJUV2GHRBAno8yPqi"
readonly supply_a="5HvgQszSuB6wR6kMEyhzCQ1YwA7cR9hBt3Jvcqbj6DPL"
readonly def_b="9rQxQmAvk5GvJpyXDvPUnQ44D1AZYrBJGrgbSvPgwD36"
readonly supply_b="8jRWwixv6hV8Y48AeRmp2UPhSZmG1zuzzUtGQQwGfSYw"
readonly maker_a="9Wvwhkioh1qypgk6wPtbi7MkdwdWiL6zrrTWF4wBNNNb"
readonly taker_a="GyM2vyCmBckny7XBC9CoYzRNTqrQL9EDprbTX3ywnnra"
readonly maker_b="ZDdUh8HMjRQPii38TEVFrN8mMAkRDNgRNG6WMo78Wog"
readonly taker_b="EBJ8qVSXEC4Ur4DAGQKCQLhTcFZyqw8LysHKAxzdU7xk"

readonly identities="${scratch}/identities"
mkdir -p "${identities}/maker" "${identities}/taker"
chmod 0700 "$identities" "${identities}/maker" "${identities}/taker"
jq -n --arg account "$maker" \
  '{schema:"lez-v0.2-local-actor-identity",version:2,account_id:$account}' \
  >"${identities}/maker/identity.json"
jq -n --arg account "$taker" \
  '{schema:"lez-v0.2-local-actor-identity",version:2,account_id:$account}' \
  >"${identities}/taker/identity.json"
printf '%064d\n' 1 >"${identities}/maker/lez-signer.key"
printf '%064d\n' 2 >"${identities}/taker/lez-signer.key"
chmod 0600 "${identities}/maker/identity.json" \
  "${identities}/maker/lez-signer.key" \
  "${identities}/taker/identity.json" \
  "${identities}/taker/lez-signer.key"

readonly manifest="${scratch}/lez.env"
printf '%s\n' \
  'LEZ_SEQUENCER_RPC_URL=http://127.0.0.1:31001' \
  'LEZ_INDEXER_RPC_URL=http://127.0.0.1:31002' >"$manifest"
chmod 0600 "$manifest"

readonly calls="${scratch}/wallet-calls"
: >"$calls"
chmod 0600 "$calls"
readonly fake_wallet="${scratch}/wallet"
sed \
  -e "s|@MAKER@|${maker}|g" \
  -e "s|@TAKER@|${taker}|g" \
  -e "s|@DEF_A@|${def_a}|g" \
  -e "s|@SUPPLY_A@|${supply_a}|g" \
  -e "s|@DEF_B@|${def_b}|g" \
  -e "s|@SUPPLY_B@|${supply_b}|g" \
  -e "s|@MAKER_A@|${maker_a}|g" \
  -e "s|@TAKER_A@|${taker_a}|g" \
  -e "s|@MAKER_B@|${maker_b}|g" \
  -e "s|@TAKER_B@|${taker_b}|g" \
  scripts/fixtures/m3-f7-fake-wallet.sh >"$fake_wallet"
chmod 0500 "$fake_wallet"

readonly output_root="${scratch}/fixture"
FAKE_WALLET_CALLS="$calls" \
M3_F7_TOKEN_FIXTURE_MODE=execute \
M3_F7_TOKEN_FIXTURE_TESTING=1 \
M3_F7_TOKEN_FIXTURE_EXPECTED_SOURCE_COMMIT="$source_commit" \
M3_F7_TOKEN_FIXTURE_RUN_ID=m3f7contract \
M3_F7_TOKEN_FIXTURE_OUTPUT_ROOT="$output_root" \
M3_F7_TOKEN_FIXTURE_WALLET_BIN="$fake_wallet" \
M3_F7_TOKEN_FIXTURE_SOURCE_DIR="$source_dir" \
M3_F7_TOKEN_FIXTURE_LEZ_MANIFEST="$manifest" \
M3_F7_TOKEN_FIXTURE_MAKER_IDENTITY="${identities}/maker/identity.json" \
M3_F7_TOKEN_FIXTURE_MAKER_KEY="${identities}/maker/lez-signer.key" \
M3_F7_TOKEN_FIXTURE_TAKER_IDENTITY="${identities}/taker/identity.json" \
M3_F7_TOKEN_FIXTURE_TAKER_KEY="${identities}/taker/lez-signer.key" \
  "$runner" >"${scratch}/runner.out"

readonly evidence="${output_root}/evidence/f7-token-fixture.json"
[[ -f "$evidence" && ! -L "$evidence" ]] || fail "fixture evidence is missing"
jq -e \
  --arg maker "$maker" --arg taker "$taker" \
  --arg def_a "$def_a" --arg def_b "$def_b" \
  --arg supply_a "$supply_a" --arg supply_b "$supply_b" \
  --arg maker_a "$maker_a" --arg taker_a "$taker_a" \
  --arg maker_b "$maker_b" --arg taker_b "$taker_b" '
  .schema_version == 1
  and .kind == "m3_f7_official_token_fixture"
  and .result == "passed"
  and .actors == {maker:$maker,taker:$taker}
  and .assets.M3F7A.definition == $def_a
  and .assets.M3F7A.supply_account == $supply_a
  and .assets.M3F7A.depositor == "maker"
  and .assets.M3F7A.atas == {maker:$maker_a,taker:$taker_a}
  and .assets.M3F7A.initial_balances == {maker:250,taker:0}
  and .assets.M3F7B.definition == $def_b
  and .assets.M3F7B.supply_account == $supply_b
  and .assets.M3F7B.depositor == "taker"
  and .assets.M3F7B.atas == {maker:$maker_b,taker:$taker_b}
  and .assets.M3F7B.initial_balances == {maker:0,taker:250}
  and ([.transactions[]] | length) == 8
  and ([.transactions[].transaction_id] | unique | length) == 8
  and all(.transactions[]; .transaction_id | test("^[0-9a-f]{64}$"))
  and .external_resources == {
    public_rpc:false,faucet:false,public_funds:false,
    sequencer:"http://127.0.0.1:31001",
    indexer:"http://127.0.0.1:31002"
  }
  and .security.mnemonic_persisted_by_fixture == false
  and .security.private_key_import_transport == "argv_local_poc_only"
  and .security.upstream_wallet_storage == "plaintext_json_unencrypted"
  and .security.production_ready == false
' "$evidence" >/dev/null || fail "fixture evidence is inconsistent"

[[ "$(stat -c '%a' "$output_root")" == 700 ]] || fail "fixture root is not mode 0700"
for role in maker taker; do
  [[ "$(stat -c '%a' "${output_root}/private/wallets/${role}")" == 700 ]] ||
    fail "${role} wallet directory is not mode 0700"
  for file in wallet_config.json storage.json; do
    [[ "$(stat -c '%a' "${output_root}/private/wallets/${role}/${file}")" == 600 ]] ||
      fail "${role} ${file} is not mode 0600"
  done
done
[[ "$(stat -c '%a' "$evidence")" == 600 ]] || fail "evidence is not mode 0600"

[[ "$(wc -l <"$calls" | tr -d '[:space:]')" == 20 ]] ||
  fail "official wallet command count drifted"
rg -Fq $'maker\taccount import public --private-key <redacted>' "$calls" ||
  fail "maker import was not role-isolated and redacted"
rg -Fq $'taker\taccount import public --private-key <redacted>' "$calls" ||
  fail "taker import was not role-isolated and redacted"
if rg -Fq "$(sed -n '1p' "${identities}/maker/lez-signer.key")" "$calls" ||
   rg -Fq "$(sed -n '1p' "${identities}/taker/lez-signer.key")" "$calls"; then
  fail "private key leaked into the fixture command audit"
fi

if FAKE_WALLET_CALLS="$calls" \
  M3_F7_TOKEN_FIXTURE_MODE=execute \
  M3_F7_TOKEN_FIXTURE_TESTING=1 \
  M3_F7_TOKEN_FIXTURE_EXPECTED_SOURCE_COMMIT="$source_commit" \
  M3_F7_TOKEN_FIXTURE_RUN_ID=m3f7contract \
  M3_F7_TOKEN_FIXTURE_OUTPUT_ROOT="$output_root" \
  M3_F7_TOKEN_FIXTURE_WALLET_BIN="$fake_wallet" \
  M3_F7_TOKEN_FIXTURE_SOURCE_DIR="$source_dir" \
  M3_F7_TOKEN_FIXTURE_LEZ_MANIFEST="$manifest" \
  M3_F7_TOKEN_FIXTURE_MAKER_IDENTITY="${identities}/maker/identity.json" \
  M3_F7_TOKEN_FIXTURE_MAKER_KEY="${identities}/maker/lez-signer.key" \
  M3_F7_TOKEN_FIXTURE_TAKER_IDENTITY="${identities}/taker/identity.json" \
  M3_F7_TOKEN_FIXTURE_TAKER_KEY="${identities}/taker/lez-signer.key" \
    "$runner" >/dev/null 2>&1; then
  fail "runner reused existing fixture state"
fi

bad_manifest="${scratch}/bad-lez.env"
printf '%s\n' \
  'LEZ_SEQUENCER_RPC_URL=https://public.example.invalid' \
  'LEZ_INDEXER_RPC_URL=http://127.0.0.1:31002' >"$bad_manifest"
if FAKE_WALLET_CALLS="$calls" \
  M3_F7_TOKEN_FIXTURE_MODE=execute \
  M3_F7_TOKEN_FIXTURE_TESTING=1 \
  M3_F7_TOKEN_FIXTURE_EXPECTED_SOURCE_COMMIT="$source_commit" \
  M3_F7_TOKEN_FIXTURE_RUN_ID=m3f7badroute \
  M3_F7_TOKEN_FIXTURE_OUTPUT_ROOT="${scratch}/bad-route" \
  M3_F7_TOKEN_FIXTURE_WALLET_BIN="$fake_wallet" \
  M3_F7_TOKEN_FIXTURE_SOURCE_DIR="$source_dir" \
  M3_F7_TOKEN_FIXTURE_LEZ_MANIFEST="$bad_manifest" \
  M3_F7_TOKEN_FIXTURE_MAKER_IDENTITY="${identities}/maker/identity.json" \
  M3_F7_TOKEN_FIXTURE_MAKER_KEY="${identities}/maker/lez-signer.key" \
  M3_F7_TOKEN_FIXTURE_TAKER_IDENTITY="${identities}/taker/identity.json" \
  M3_F7_TOKEN_FIXTURE_TAKER_KEY="${identities}/taker/lez-signer.key" \
    "$runner" >/dev/null 2>&1; then
  fail "runner accepted a public sequencer endpoint"
fi

echo "M3 F7 official-token fixture contract passed"
