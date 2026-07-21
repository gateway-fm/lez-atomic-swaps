#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export LC_ALL=C
umask 077

readonly runner="scripts/run-m4-lez-actor-onboarding.sh"
readonly expected_program="4d6590332948743c2db88a183755815354ef92560550cd206ac27bddeea12c82"
channel="$(printf '6%.0s' {1..64})"
maker_tx="$(printf 'a%.0s' {1..64})"
taker_tx="$(printf 'b%.0s' {1..64})"
readonly channel maker_tx taker_tx
readonly maker_hex="d8f695e45e50c70bd6a99d2c39b9eca8dc81de5d6b5b9a51c5819cd71f35ff4f"
readonly maker_base58="Fbw9N5WSecWwV2dnD1LYorystRU8Wt7qvbYcwKRfUUBg"
readonly maker_vault_hex="59c301f630eadfea9df566ab54a8940e54df47a691d2171cf13362180ea0aa41"
readonly maker_vault_base58="73PkYv2kabhH1EWem8fHGzvq7w2KHJ48762bKNpusNor"
readonly taker_hex="15a53503615a30c8b7fcc48f6fa6857da4e2e234680a3482b13366e2d513d2c3"
readonly taker_base58="2TVfxNxZQyar34fZTxzeS2gTD2yEwGT5berurSnRKJPt"
readonly taker_vault_hex="02427b8411ca7a96f0d0d236638b38388fe0ad758b723771051546794a1f81bc"
readonly taker_vault_base58="9pcYbhvbTYg5CHtP837u8vtpqf2svFrrkJs2ZXN1vtj"

fail() { echo "M4 LEZ actor-onboarding contract test failed: $*" >&2; exit 1; }

for command_name in bash chmod jq mkdir mktemp rm sed sha256sum stat; do
  command -v "$command_name" >/dev/null || fail "missing dependency: ${command_name}"
done
[[ -x "$runner" ]] || fail "actor-onboarding runner is missing or not executable"
bash -n "$runner"

contract="$($runner contract)"
jq -e '
  .schema_version == 1
  and .kind == "m4_lez_actor_onboarding_contract"
  and .flow == "flow_0_fresh_vault_claims"
  and .roles == ["maker","taker"]
  and .submission_count_per_role == 1
  and .automatic_submission_retry == false
  and .finality_membership_variant == "Public"
  and .canonical_window_occurrences_required == 1
  and .indexer_account_read == "getAccountAtBlock_exact_containing_finalized_block"
  and .expected_effect.owner == {balance:"genesis_allocation",nonce:1}
  and .expected_effect.vault == {balance:0,nonce:0}
  and .requires_finalized_deployment == true
  and .monero_or_swap_effects_started == false
  and .runtime_external_resources == []
  and .public_rpc_used == false
  and .faucet_used == false
' <<<"$contract" >/dev/null || fail "contract omits the Flow 0 safety boundary"
"$runner" self-test-finality-selector >/dev/null

test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT
readonly test_root

make_fixture() {
  local name="$1" scenario="$2" root
  root="${test_root}/${name}"
  mkdir -m 0700 "$root" "$root/bin" "$root/private"
  cat >"$root/stack.env" <<EOF
RUN_ID=${name}
LEZ_V02_SOURCE_COMMIT=a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a
LEZ_V02_CHANNEL_PUBLIC_KEY=${channel}
LEZ_SEQUENCER_RPC_URL=http://127.0.0.1:39111
LEZ_INDEXER_RPC_URL=http://127.0.0.1:39112
LEZ_V02_MAKER_ACCOUNT_ID=${maker_base58}
LEZ_V02_MAKER_VAULT_ACCOUNT_ID=${maker_vault_base58}
LEZ_V02_MAKER_GENESIS_ALLOCATION=100000
LEZ_V02_TAKER_ACCOUNT_ID=${taker_base58}
LEZ_V02_TAKER_VAULT_ACCOUNT_ID=${taker_vault_base58}
LEZ_V02_TAKER_GENESIS_ALLOCATION=200000
EOF
  chmod 0600 "$root/stack.env"
  jq -n --arg account "$maker_base58" --arg hex "$maker_hex" \
    --arg vault "$maker_vault_base58" --arg vault_hex "$maker_vault_hex" \
    '{schema:"lez-v0.2-local-actor-identity",version:2,account_id:$account,
      account_id_hex:$hex,vault_account_id:$vault,vault_account_id_hex:$vault_hex}' \
    >"$root/maker.json"
  jq -n --arg account "$taker_base58" --arg hex "$taker_hex" \
    --arg vault "$taker_vault_base58" --arg vault_hex "$taker_vault_hex" \
    '{schema:"lez-v0.2-local-actor-identity",version:2,account_id:$account,
      account_id_hex:$hex,vault_account_id:$vault,vault_account_id_hex:$vault_hex}' \
    >"$root/taker.json"
  printf '%064d\n' 1 >"$root/maker.key"
  printf '%064d\n' 2 >"$root/taker.key"
  chmod 0600 "$root/maker.json" "$root/taker.json" "$root/maker.key" "$root/taker.key"
  jq -n --arg run "$name" --arg channel "$channel" --arg program "$expected_program" '
    {schema_version:1,kind:"m4_lez_local_deployment",result:"passed",run_id:$run,
     stack:{channel_id:$channel,sequencer_rpc_url:"http://127.0.0.1:39111",
       indexer_rpc_url:"http://127.0.0.1:39112",isolated_loopback_only:true},
     artifact:{image_id:$program},canonical_window_occurrences:1,
     bedrock_status:"Finalized",id_hash_id_lookups_equal:true,
     sequencer_indexer_inclusion_equal:true,runtime_external_resources:[],
     public_rpc_used:false,faucet_used:false}' >"$root/deployment.json"
  chmod 0600 "$root/deployment.json"
  printf '%s\n' "$scenario" >"$root/scenario"

  cat >"$root/vault-claim" <<'CLAIM'
#!/usr/bin/env bash
set -euo pipefail
role="" run="" request="" state="" key="" sequencer="" chain="" program="" allocation=""
while (( $# > 0 )); do
  case "$1" in
    --role) role="$2"; shift 2 ;;
    --run-id) run="$2"; shift 2 ;;
    --request-id) request="$2"; shift 2 ;;
    --state-directory) state="$2"; shift 2 ;;
    --private-key-file) key="$2"; shift 2 ;;
    --sequencer-url) sequencer="$2"; shift 2 ;;
    --chain-id) chain="$2"; shift 2 ;;
    --escrow-program-id) program="$2"; shift 2 ;;
    --allocation) allocation="$2"; shift 2 ;;
    --max-scan-blocks) shift 2 ;;
    *) exit 41 ;;
  esac
done
[[ "$run" == "$FIXTURE_RUN" && "$request" == "${role}-flow0-vault-claim-0001" ]] || exit 42
[[ "$sequencer" == "$FIXTURE_SEQUENCER" && "$chain" == "$FIXTURE_CHANNEL" ]] || exit 43
[[ "$program" == "$FIXTURE_PROGRAM" && -d "$state" && -f "$key" ]] || exit 44
case "$role" in
  maker)
    [[ "$allocation" == 100000 ]] || exit 45
    owner="$FIXTURE_MAKER_HEX"; vault="$FIXTURE_MAKER_VAULT_HEX"; tx="$FIXTURE_MAKER_TX" ;;
  taker)
    [[ "$allocation" == 200000 ]] || exit 46
    owner="$FIXTURE_TAKER_HEX"; vault="$FIXTURE_TAKER_VAULT_HEX"; tx="$FIXTURE_TAKER_TX" ;;
  *) exit 47 ;;
esac
count_file="${FIXTURE_ROOT}/${role}.count"; count=0
[[ ! -f "$count_file" ]] || read -r count <"$count_file"
printf '%s\n' "$((count + 1))" >"$count_file"
jq -n --arg role "$role" --arg run "$run" --arg request "$request" \
  --arg owner "$owner" --arg vault "$vault" --arg tx "$tx" --argjson allocation "$allocation" \
  --arg channel "$FIXTURE_CHANNEL" --arg program "$FIXTURE_PROGRAM" '
  {schema:"lez_v02_vault_claim_poc_v1",role:$role,run_id:$run,request_id:$request,
   runtime:{chain_id:$channel,channel_id:$channel,escrow_program_id:$program},
   allocation:$allocation,transaction_id:$tx,submission:{decision:"admitted"},
   durable_state:"admitted",durable_attempt_count:1,durable_revision:2,
   before:{sequencer_tip:80,owner:{account_id:$owner,balance:0,nonce:0},
     vault:{account_id:$vault,balance:$allocation,nonce:0}},
   post:null,post_observation:"unavailable_or_non_atomic",finality:"not_observed_in_this_poc_slice"}'
CLAIM
  chmod 0755 "$root/vault-claim"

  cat >"$root/bin/curl" <<'CURL'
#!/usr/bin/env bash
set -euo pipefail
request=""
while (( $# > 0 )); do
  case "$1" in --data) request="$2"; shift 2 ;; *) shift ;; esac
done
method="$(jq -er '.method' <<<"$request")"
case "$method" in
  getLastFinalizedBlockId)
    if [[ -f "$FIXTURE_ROOT/taker.count" ]]; then tip=84
    elif [[ -f "$FIXTURE_ROOT/maker.count" ]]; then tip=82
    else tip=80
    fi
    jq -cn --argjson tip "$tip" '{jsonrpc:"2.0",id:1,result:$tip}' ;;
  getBlockById)
    height="$(jq -er '.params[0]' <<<"$request")"; printf -v hash '%064x' "$height"
    transactions='[]'
    if [[ "$height" == 82 ]]; then
      transactions="$(jq -cn --arg tx "$FIXTURE_MAKER_TX" '[{Public:{hash:$tx}}]')"
      if [[ "$FIXTURE_SCENARIO" == duplicate ]]; then
        transactions="$(jq -cn --arg tx "$FIXTURE_MAKER_TX" '[{Public:{hash:$tx}},{Public:{hash:$tx}}]')"
      fi
    elif [[ "$height" == 84 ]]; then
      transactions="$(jq -cn --arg tx "$FIXTURE_TAKER_TX" '[{Public:{hash:$tx}}]')"
    fi
    jq -cn --argjson height "$height" --arg hash "$hash" --argjson txs "$transactions" \
      '{jsonrpc:"2.0",id:1,result:{header:{block_id:$height,hash:$hash},
        bedrock_status:"Finalized",body:{transactions:$txs}}}' ;;
  getBlockByHash)
    hash="$(jq -er '.params[0]' <<<"$request")"; height=$((16#${hash: -2}))
    if [[ "$height" == 82 ]]; then tx="$FIXTURE_MAKER_TX"; else tx="$FIXTURE_TAKER_TX"; fi
    transactions="$(jq -cn --arg tx "$tx" '[{Public:{hash:$tx}}]')"
    jq -cn --argjson height "$height" --arg hash "$hash" --argjson txs "$transactions" \
      '{jsonrpc:"2.0",id:1,result:{header:{block_id:$height,hash:$hash},
        bedrock_status:"Finalized",body:{transactions:$txs}}}' ;;
  getAccountAtBlock)
    account="$(jq -er '.params[0]' <<<"$request")"; block="$(jq -er '.params[1]' <<<"$request")"
    case "$account" in
      "$FIXTURE_MAKER_BASE58") balance=100000; nonce=1; expected=82 ;;
      "$FIXTURE_MAKER_VAULT_BASE58") balance=0; nonce=0; expected=82 ;;
      "$FIXTURE_TAKER_BASE58") balance=200000; nonce=1; expected=84 ;;
      "$FIXTURE_TAKER_VAULT_BASE58") balance=0; nonce=0; expected=84 ;;
      *) exit 52 ;;
    esac
    [[ "$block" == "$expected" ]] || exit 53
    if [[ "$FIXTURE_SCENARIO" == wrong_state && "$account" == "$FIXTURE_MAKER_BASE58" ]]; then balance=999; fi
    printf '%s\t%s\n' "$account" "$block" >>"$FIXTURE_ROOT/account-reads"
    jq -cn --argjson balance "$balance" --argjson nonce "$nonce" \
      '{jsonrpc:"2.0",id:1,result:{balance:$balance,nonce:$nonce}}' ;;
  *) exit 54 ;;
esac
CURL
  chmod 0700 "$root/bin/curl"
}

run_fixture() {
  local name="$1" expect="$2" root binary_sha
  root="${test_root}/${name}"
  binary_sha="$(sha256sum "$root/vault-claim" | sed 's/ .*//')"
  set +e
  FIXTURE_ROOT="$root" FIXTURE_RUN="$name" FIXTURE_SCENARIO="$(sed -n '1p' "$root/scenario")" \
  FIXTURE_SEQUENCER=http://127.0.0.1:39111 FIXTURE_CHANNEL="$channel" \
  FIXTURE_PROGRAM="$expected_program" FIXTURE_MAKER_HEX="$maker_hex" \
  FIXTURE_MAKER_VAULT_HEX="$maker_vault_hex" FIXTURE_TAKER_HEX="$taker_hex" \
  FIXTURE_TAKER_VAULT_HEX="$taker_vault_hex" FIXTURE_MAKER_TX="$maker_tx" \
  FIXTURE_TAKER_TX="$taker_tx" FIXTURE_MAKER_BASE58="$maker_base58" \
  FIXTURE_MAKER_VAULT_BASE58="$maker_vault_base58" FIXTURE_TAKER_BASE58="$taker_base58" \
  FIXTURE_TAKER_VAULT_BASE58="$taker_vault_base58" PATH="$root/bin:$PATH" \
  M4_ONBOARD_RUN_ID="$name" M4_ONBOARD_STACK_MANIFEST="$root/stack.env" \
  M4_ONBOARD_DEPLOYMENT_FINALITY="$root/deployment.json" \
  M4_ONBOARD_EVIDENCE_ROOT="$root/evidence" M4_ONBOARD_PRIVATE_ROOT="$root/private" \
  M4_ONBOARD_MAKER_IDENTITY="$root/maker.json" M4_ONBOARD_TAKER_IDENTITY="$root/taker.json" \
  M4_ONBOARD_MAKER_PRIVATE_KEY="$root/maker.key" M4_ONBOARD_TAKER_PRIVATE_KEY="$root/taker.key" \
  M4_ONBOARD_VAULT_CLAIM_BIN="$root/vault-claim" \
  M4_ONBOARD_EXPECTED_VAULT_CLAIM_SHA256="$binary_sha" \
  M4_ONBOARD_CONTRACT_TEST_ONLY=1 M4_ONBOARD_CONTRACT_TEST_FINALITY_POLLS=2 \
    "$runner" execute >/dev/null 2>&1
  status=$?
  set -e
  if [[ "$expect" == pass ]]; then
    [[ "$status" == 0 ]] || fail "happy fixture failed"
    [[ "$(<"$root/maker.count")" == 1 && "$(<"$root/taker.count")" == 1 ]] ||
      fail "happy fixture did not submit each role exactly once"
    [[ "$(wc -l <"$root/account-reads" | tr -d ' ')" == 4 ]] || fail "expected four exact-block account reads"
    jq -e '.result=="passed" and .flow=="flow_0_fresh_vault_claims"
      and .actors.maker.submission_count==1 and .actors.taker.submission_count==1
      and .actors.maker.owner_after=={balance:100000,nonce:1}
      and .actors.taker.owner_after=={balance:200000,nonce:1}
      and .actors.maker.vault_after=={balance:0,nonce:0}
      and .actors.taker.vault_after=={balance:0,nonce:0}
      and .monero_or_swap_effects_started==false' "$root/evidence/summary.json" >/dev/null ||
      fail "happy summary is incomplete"
  else
    [[ "$status" != 0 ]] || fail "unsafe ${name} fixture passed"
  fi
}

make_fixture happy happy
run_fixture happy pass
make_fixture duplicate duplicate
run_fixture duplicate fail
make_fixture wrong-state wrong_state
run_fixture wrong-state fail
make_fixture group-write happy
chmod 0775 "$test_root/group-write/vault-claim"
run_fixture group-write fail

echo "M4 LEZ actor-onboarding contract passed"
