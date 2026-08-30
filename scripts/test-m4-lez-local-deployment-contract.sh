#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly runner="scripts/run-m4-lez-local-deployment.sh"
readonly expected_source_commit="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
readonly expected_elf="237037e1a54187697e7e67a9bf589dfb3eb88c475c7f9b62eb2396144e87c6d0"
readonly expected_program="431ab9aec4b21d66e88ecbf8bb83301d5ef4cc0cec0ba0fb76baaa0ac7f9a10b"
readonly expected_program_words='[2931366467,1713222340,4174089960,489718715,214758494,4221570028,178961014,195164615]'
channel="$(printf '6%.0s' {1..64})"
transaction="$(printf 'd%.0s' {1..64})"
genesis_hash="$(printf 'e%.0s' {1..64})"
block_80_hash="$(printf '8%.0s' {1..64})"
block_81_hash="$(printf 'a%.0s' {1..64})"
block_82_hash="$(printf 'b%.0s' {1..64})"
block_83_hash="$(printf 'c%.0s' {1..64})"
readonly channel transaction genesis_hash block_80_hash block_81_hash block_82_hash block_83_hash

fail() {
  echo "M4 LEZ local-deployment contract test failed: $*" >&2
  exit 1
}

[[ -x "$runner" ]] || fail "runner is missing or not executable"
for command_name in jq mktemp sed sha256sum stat; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

contract="$($runner contract)"
jq -e --arg source "$expected_source_commit" --arg elf "$expected_elf" \
  --arg program "$expected_program" --argjson words "$expected_program_words" '
  .schema_version == 1
  and .kind == "m4_lez_local_deployment_contract"
  and .lez_source_commit == $source
  and .embedded_guest_sha256 == $elf
  and .escrow_program_id == $program
  and .escrow_program_id_words == $words
  and .local_loopback_only == true
  and .single_send_code_path == true
  and .durable_cross_process_submission_counter == false
  and .deployment_retry_allowed == false
  and .finality_membership_variant == "ProgramDeployment"
  and .finality_scan == "sequential_finalized_indexer_blocks"
  and .exact_elf_pre_window_occurrences_required == 0
  and .exact_elf_post_window_occurrences_required == 1
  and .id_hash_id_lookups_equal == true
  and .sequencer_indexer_inclusion_equal == true
  and .no_clobber == true
  and .runtime_external_resources == []
  and .public_rpc_used == false
  and .faucet_used == false
  and .private_material_disclosed == false
' <<<"$contract" >/dev/null || fail "contract does not expose the required safety boundary"
"$runner" self-test-finality-selector >/dev/null

test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT
readonly test_root

make_fixture() {
  local name="$1" scenario="$2" fixture_root
  fixture_root="${test_root}/${name}"
  mkdir -m 0700 "$fixture_root" "$fixture_root/bin"
  cat >"${fixture_root}/run.env" <<EOF
RUN_ID=${name}
LEZ_V02_SOURCE_COMMIT=${expected_source_commit}
LEZ_V02_CHANNEL_PUBLIC_KEY=${channel}
LEZ_SEQUENCER_RPC_URL=http://127.0.0.1:39101
LEZ_INDEXER_RPC_URL=http://127.0.0.1:39102
DO_NOT_COPY_TEST_SECRET=fixture-private-material
EOF
  chmod 0600 "${fixture_root}/run.env"
  printf 'contract-fixture-guest-%s' "$name" >"${fixture_root}/guest.bin"
  chmod 0600 "${fixture_root}/guest.bin"
  fixture_elf_sha="$(sha256sum "${fixture_root}/guest.bin" | sed 's/ .*//')"
  cat >"${fixture_root}/artifact.toml" <<EOF
format_version = 1
milestone = "M4"
run_id = "${name}-artifact"
elf_path = "${fixture_root}/guest.bin"
elf_sha256 = "${fixture_elf_sha}"
image_id = "${expected_program}"
result = "passed"
runtime_external_resources = []
EOF
  chmod 0600 "${fixture_root}/artifact.toml"
  cat >"${fixture_root}/deployer" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == "deploy-m4-local" ]] || exit 41
shift
rpc="" channel="" timeout=""
while (( $# > 0 )); do
  case "$1" in
    --rpc-url) rpc="$2"; shift 2 ;;
    --channel-id) channel="$2"; shift 2 ;;
    --timeout-seconds) timeout="$2"; shift 2 ;;
    *) exit 42 ;;
  esac
done
[[ "$rpc" == "$FIXTURE_SEQUENCER_URL" && "$channel" == "$FIXTURE_CHANNEL" && "$timeout" == "7" ]] || exit 43
count=0
[[ ! -f "$FIXTURE_DEPLOY_COUNT" ]] || read -r count <"$FIXTURE_DEPLOY_COUNT"
printf '%s\n' "$((count + 1))" >"$FIXTURE_DEPLOY_COUNT"
inclusion_id=82
[[ "$FIXTURE_SCENARIO" != "sequencer_mismatch" ]] || inclusion_id=83
inclusion_hash="$FIXTURE_BLOCK_82_HASH"
[[ "$inclusion_id" == 82 ]] || inclusion_hash="$FIXTURE_BLOCK_83_HASH"
jq -n --arg rpc "$rpc" --arg channel "$channel" --arg elf "$FIXTURE_ELF" \
  --arg image "$FIXTURE_PROGRAM" --arg tx "$FIXTURE_TRANSACTION" \
  --arg genesis "$FIXTURE_GENESIS_HASH" --arg hash "$inclusion_hash" \
  --argjson inclusion "$inclusion_id" '
  {schema_version:1,
   preflight:{rpc_url:$rpc,channel_id:$channel,genesis_block_id:1,
     genesis_block_hash:$genesis,elf_sha256:$elf,image_id:$image,
     program_id_words:[2931366467,1713222340,4174089960,489718715,214758494,4221570028,178961014,195164615],
     authenticated_transfer_program_id:[3170810844,2526647253,999807262,1205602179,3401962591,3484055895,2106546407,1900691388],
     token_program_id:[2282739141,348907455,1046946228,3735699860,585462133,3426087150,772528164,2090518099],
     associated_token_account_program_id:[3357312149,3615960253,3351583505,2234166003,4153433811,2743238177,2886052503,4160755157],
     associated_token_account_identity_source:"lez-v0.2.0-checked-elf-rpc-map-omits-key",
     rpc_program_names:["amm","authenticated_transfer","pinata","privacy_preserving_circuit","token"],
     last_block_id:81},
   transaction_hash:$tx,inclusion_block_id:$inclusion,inclusion_block_hash:$hash}'
EOF
  chmod 0700 "${fixture_root}/deployer"
  cat >"${fixture_root}/bin/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
request=""
while (( $# > 0 )); do
  case "$1" in
    --data) request="$2"; shift 2 ;;
    *) shift ;;
  esac
done
method="$(jq -er '.method' <<<"$request")"
case "$method" in
  getLastFinalizedBlockId)
    if [[ -s "$FIXTURE_DEPLOY_COUNT" ]]; then
      if [[ "$FIXTURE_SCENARIO" == "tip_regression" ]]; then tip=79; else tip=83; fi
    else tip=80
    fi
    jq -cn --argjson tip "$tip" '{jsonrpc:"2.0",id:1,result:$tip}' ;;
  getBlockById)
    height="$(jq -er '.params[0]' <<<"$request")"; printf '%s\n' "$height" >>"$FIXTURE_ID_LOG"
    printf -v hash '%064x' "$height"; transactions='[]'; status="Finalized"; header_id="$height"
    case "$height" in
      1)
        hash="$FIXTURE_GENESIS_HASH"
        [[ "$FIXTURE_SCENARIO" != "genesis_mismatch" ]] || hash="$(printf 'f%.0s' {1..64})"
        ;;
      80) hash="$FIXTURE_BLOCK_80_HASH" ;;
      81)
        hash="$FIXTURE_BLOCK_81_HASH"
        [[ "$FIXTURE_SCENARIO" != "nonfinalized" ]] || status="Pending"
        [[ "$FIXTURE_SCENARIO" != "header_mismatch" ]] || header_id=80
        ;;
      82)
        hash="$FIXTURE_BLOCK_82_HASH"
        if [[ "$FIXTURE_SCENARIO" == "missing_transaction" ]]; then transactions='[]'
        elif [[ "$FIXTURE_SCENARIO" == "wrong_variant" ]]; then
          transactions="$(jq -cn --arg tx "$FIXTURE_TRANSACTION" --arg bytecode "$FIXTURE_BYTECODE" \
            '[{Public:{hash:$tx,message:{bytecode:$bytecode}}}]')"
        else
          transactions="$(jq -cn --arg tx "$FIXTURE_TRANSACTION" --arg bytecode "$FIXTURE_BYTECODE" \
            '[{ProgramDeployment:{hash:$tx,message:{bytecode:$bytecode}}}]')"
        fi
        ;;
      83)
        hash="$FIXTURE_BLOCK_83_HASH"
        if [[ "$FIXTURE_SCENARIO" == "duplicate" ]]; then
          transactions="$(jq -cn --arg tx "$FIXTURE_TRANSACTION" --arg bytecode "$FIXTURE_BYTECODE" \
            '[{ProgramDeployment:{hash:$tx,message:{bytecode:$bytecode}}}]')"
        fi
        ;;
    esac
    jq -cn --argjson height "$header_id" --arg hash "$hash" --arg status "$status" \
      --argjson transactions "$transactions" \
      '{jsonrpc:"2.0",id:1,result:{header:{block_id:$height,hash:$hash},bedrock_status:$status,body:{transactions:$transactions},comparison_marker:"canonical"}}' ;;
  getBlockByHash)
    printf 'hash\n' >>"$FIXTURE_HASH_LOG"; extra="canonical"
    [[ "$FIXTURE_SCENARIO" != "id_hash_mismatch" ]] || extra="different"
    transactions="$(jq -cn --arg tx "$FIXTURE_TRANSACTION" --arg bytecode "$FIXTURE_BYTECODE" \
      '[{ProgramDeployment:{hash:$tx,message:{bytecode:$bytecode}}}]')"
    jq -cn --arg hash "$FIXTURE_BLOCK_82_HASH" --arg extra "$extra" --argjson transactions "$transactions" \
      '{jsonrpc:"2.0",id:1,result:{header:{block_id:82,hash:$hash},bedrock_status:"Finalized",body:{transactions:$transactions},comparison_marker:$extra}}' ;;
  *) exit 52 ;;
esac
EOF
  chmod 0700 "${fixture_root}/bin/curl"
  printf '%s\n' "$scenario" >"${fixture_root}/scenario"
}

execute_fixture() {
  local name="$1" fixture_root fixture_elf_sha deployer_sha bytecode
  fixture_root="${test_root}/${name}"
  fixture_elf_sha="$(sha256sum "${fixture_root}/guest.bin" | sed 's/ .*//')"
  deployer_sha="$(sha256sum "${fixture_root}/deployer" | sed 's/ .*//')"
  [[ ! -f "${fixture_root}/expected-deployer-sha" ]] || deployer_sha="$(sed -n '1p' "${fixture_root}/expected-deployer-sha")"
  bytecode="$(base64 -w0 "${fixture_root}/guest.bin")"
  FIXTURE_SCENARIO="$(sed -n '1p' "${fixture_root}/scenario")" \
  FIXTURE_SEQUENCER_URL="http://127.0.0.1:39101" \
  FIXTURE_CHANNEL="$channel" FIXTURE_ELF="$fixture_elf_sha" FIXTURE_PROGRAM="$expected_program" \
  FIXTURE_BYTECODE="$bytecode" \
  FIXTURE_TRANSACTION="$transaction" FIXTURE_GENESIS_HASH="$genesis_hash" \
  FIXTURE_BLOCK_80_HASH="$block_80_hash" FIXTURE_BLOCK_81_HASH="$block_81_hash" \
  FIXTURE_BLOCK_82_HASH="$block_82_hash" FIXTURE_BLOCK_83_HASH="$block_83_hash" \
  FIXTURE_DEPLOY_COUNT="${fixture_root}/deploy-count" \
  FIXTURE_ID_LOG="${fixture_root}/id-log" FIXTURE_HASH_LOG="${fixture_root}/hash-log" \
  PATH="${fixture_root}/bin:${PATH}" M4_LEZ_RUN_ID="$name" \
  M4_LEZ_STACK_MANIFEST="${fixture_root}/run.env" \
  M4_LEZ_ARTIFACT_EVIDENCE="${fixture_root}/artifact.toml" \
  M4_LEZ_DEPLOYER="${fixture_root}/deployer" M4_LEZ_EXPECTED_DEPLOYER_SHA256="$deployer_sha" \
  M4_LEZ_CONTRACT_TEST_ONLY=1 M4_LEZ_CONTRACT_TEST_EXPECTED_GUEST_SHA256="$fixture_elf_sha" \
  M4_LEZ_CONTRACT_TEST_FINALITY_POLLS=2 \
  M4_LEZ_EVIDENCE_ROOT="${fixture_root}/evidence" M4_LEZ_TIMEOUT_SECONDS=7 \
    "$runner" execute
}

make_fixture success success
execute_fixture success >/dev/null || fail "valid isolated deployment fixture was rejected"
success_root="${test_root}/success"
[[ "$(sed -n '1p' "${success_root}/deploy-count")" == 1 ]] || fail "deployment was not submitted exactly once"
jq -s -e '[.[].block_id] == [range(1;84)] and
  ([.[] | select(.phase=="pre") | .exact_elf_occurrences] | add) == 0 and
  ([.[] | select(.phase=="post") | .exact_elf_occurrences] | add) == 1' \
  "${success_root}/evidence/finalized-chain-scan.jsonl" >/dev/null ||
  fail "finalized genesis-through-tip scan is incomplete or not artifact-exact"
[[ "$(wc -l <"${success_root}/hash-log")" == 1 ]] || fail "containing block was not looked up by hash exactly once"
jq -e --arg tx "$transaction" --arg hash "$block_82_hash" '
  .result == "passed"
  and .transaction_id == $tx
  and .send_attempts_this_process == 1
  and .durable_cross_process_submission_counter == false
  and .canonical_window_occurrences == 1
  and .artifact.exact_elf_pre_window_occurrences == 0
  and .artifact.exact_elf_post_window_occurrences == 1
  and .artifact.finalized_wire_bytecode_equal == true
  and .containing_block_id == 82
  and .containing_block_hash == $hash
  and .bedrock_status == "Finalized"
  and .id_hash_id_lookups_equal == true
  and .sequencer_indexer_inclusion_equal == true
  and .runtime_external_resources == []
  and .public_rpc_used == false
  and .faucet_used == false
  and .private_material_disclosed == false
' "${success_root}/evidence/finality.json" >/dev/null || fail "retained finality evidence is incomplete"
jq -e '.result=="passed" and (.raw_evidence|length)>=8 and .secret_safety.allowlist_enforced==true' \
  "${success_root}/evidence/bundle.json" >/dev/null || fail "hash-joined secret-safe bundle is incomplete"
while IFS=$'\t' read -r evidence_name evidence_sha; do
  [[ "$(sha256sum "${success_root}/evidence/${evidence_name}" | sed 's/ .*//')" == "$evidence_sha" ]] ||
    fail "bundle hash differs for ${evidence_name}"
done < <(jq -r '.raw_evidence[] | [.path,.sha256] | @tsv' "${success_root}/evidence/bundle.json")

[[ "$(stat -c '%a' "${success_root}/evidence")" == 700 ]] || fail "evidence directory is not owner-only"
while IFS= read -r evidence_file; do
  [[ "$(stat -c '%a' "$evidence_file")" == 600 ]] || fail "evidence file is not owner-only: ${evidence_file}"
done < <(find "${success_root}/evidence" -type f -print)
! grep -R -Fq 'fixture-private-material' "${success_root}/evidence" || fail "stack secret leaked into evidence"
if execute_fixture success >"${success_root}/clobber.out" 2>"${success_root}/clobber.err"; then
  fail "runner reused an existing evidence root"
fi
[[ "$(sed -n '1p' "${success_root}/deploy-count")" == 1 ]] || fail "no-clobber rejection resubmitted deployment"

for scenario in duplicate id_hash_mismatch sequencer_mismatch nonfinalized header_mismatch genesis_mismatch tip_regression wrong_variant missing_transaction; do
  make_fixture "$scenario" "$scenario"
  if execute_fixture "$scenario" >"${test_root}/${scenario}.out" 2>"${test_root}/${scenario}.err"; then
    fail "unsafe ${scenario} fixture was accepted"
  fi
  [[ "$(sed -n '1p' "${test_root}/${scenario}/deploy-count")" == 1 ]] ||
    fail "${scenario} handling changed the single-submission rule"
done
assert_rejected_before_send() {
  local name="$1"
  if execute_fixture "$name" >"${test_root}/${name}.out" 2>"${test_root}/${name}.err"; then
    fail "unsafe pre-send ${name} fixture was accepted"
  fi
  [[ ! -e "${test_root}/${name}/deploy-count" ]] || fail "${name} rejection submitted deployment"
}

make_fixture wrong_source success
sed -i "s/${expected_source_commit}/$(printf 'f%.0s' {1..40})/" "${test_root}/wrong_source/run.env"
assert_rejected_before_send wrong_source

make_fixture wrong_artifact_hash success
sed -i "s/^elf_sha256 = .*/elf_sha256 = \"$(printf 'f%.0s' {1..64})\"/" \
  "${test_root}/wrong_artifact_hash/artifact.toml"
assert_rejected_before_send wrong_artifact_hash

make_fixture duplicate_artifact_claim success
printf 'result = "passed"\n' >>"${test_root}/duplicate_artifact_claim/artifact.toml"
assert_rejected_before_send duplicate_artifact_claim

make_fixture wrong_deployer_hash success
printf '%s\n' "$(printf 'f%.0s' {1..64})" >"${test_root}/wrong_deployer_hash/expected-deployer-sha"
assert_rejected_before_send wrong_deployer_hash

make_fixture artifact_symlink success
mv "${test_root}/artifact_symlink/artifact.toml" "${test_root}/artifact_symlink/artifact.real.toml"
ln -s artifact.real.toml "${test_root}/artifact_symlink/artifact.toml"
assert_rejected_before_send artifact_symlink

make_fixture stack_symlink success
mv "${test_root}/stack_symlink/run.env" "${test_root}/stack_symlink/run.real.env"
ln -s run.real.env "${test_root}/stack_symlink/run.env"
assert_rejected_before_send stack_symlink

make_fixture deployer_symlink success
mv "${test_root}/deployer_symlink/deployer" "${test_root}/deployer_symlink/deployer.real"
ln -s deployer.real "${test_root}/deployer_symlink/deployer"
assert_rejected_before_send deployer_symlink

make_fixture public_endpoint success
sed -i 's#http://127\.0\.0\.1:39102#https://public.example.invalid#' "${test_root}/public_endpoint/run.env"
if execute_fixture public_endpoint >"${test_root}/public.out" 2>"${test_root}/public.err"; then
  fail "public indexer endpoint was accepted"
fi
[[ ! -e "${test_root}/public_endpoint/deploy-count" ]] || fail "public endpoint rejection submitted a deployment"

echo "M4 isolated LEZ deployment runner contract passed"
