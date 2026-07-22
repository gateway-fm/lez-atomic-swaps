#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export LC_ALL=C
umask 077

readonly pinned_source_commit="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
readonly pinned_guest_sha256="dc370bc34b432317730c51b49342760dbc675fca700e300b30b5fadefe5b7292"
readonly pinned_program_id="4d6590332948743c2db88a183755815354ef92560550cd206ac27bddeea12c82"
readonly pinned_program_words='[865101133,1014253609,411744301,1400984887,1452470100,550326277,3715875434,2183963118]'
readonly pinned_auth_program='[3170810844,2526647253,999807262,1205602179,3401962591,3484055895,2106546407,1900691388]'
readonly pinned_token_program='[2282739141,348907455,1046946228,3735699860,585462133,3426087150,772528164,2090518099]'
readonly pinned_ata_program='[3357312149,3615960253,3351583505,2234166003,4153433811,2743238177,2886052503,4160755157]'
readonly pinned_ata_source="lez-v0.2.0-checked-elf-rpc-map-omits-key"
readonly m4_manifest="compat/lez-v0.2-provisional/escrow/methods/guest/m4-deployment-manifest.toml"
readonly deployer_manifest="compat/lez-v0.2-provisional/escrow/deployer/Cargo.toml"
readonly deployer_source="compat/lez-v0.2-provisional/escrow/deployer/src/main.rs"
readonly artifact_runner="scripts/run-m4-lez-artifact-tests.sh"
readonly max_finality_blocks=4096

fail() { echo "M4 LEZ local deployment failed: $*" >&2; exit 2; }

transaction_occurrences() {
  jq -er --arg tx "$2" '[.result.body.transactions[]
    | select((keys|length)==1 and has("ProgramDeployment"))
    | .ProgramDeployment | select(.hash==$tx)] | length' "$1"
}

self_test_finality_selector() {
  local fixture tx; tx="$(printf 'a%.0s' {1..64})"
  fixture="$(jq -cn --arg tx "$tx" '{result:{body:{transactions:[
    {ProgramDeployment:{hash:$tx}},{Public:{hash:$tx}},
    {ProgramDeployment:{hash:$tx},Public:{hash:$tx}}]}}}')"
  [[ "$(transaction_occurrences - "$tx" <<<"$fixture")" == 1 ]] || fail "selector is not variant-exact"
}

emit_contract() {
  jq -n --arg source "$pinned_source_commit" --arg guest "$pinned_guest_sha256" --arg program "$pinned_program_id" '
    {schema_version:1,kind:"m4_lez_local_deployment_contract",lez_source_commit:$source,
     embedded_guest_sha256:$guest,escrow_program_id:$program,exact_fresh_artifact_proof_required:true,
     exact_deployer_hash_required:true,deployer_hash_checked_before_and_after_point_of_use:true,
     local_loopback_only:true,single_send_code_path:true,deployment_retry_allowed:false,
     durable_cross_process_submission_counter:false,finality_membership_variant:"ProgramDeployment",
     canonical_bounded_window_occurrences_required:1,exact_elf_pre_window_occurrences_required:0,
     exact_elf_post_window_occurrences_required:1,finality_scan:"sequential_finalized_indexer_blocks",
     pre_finalized_anchor_stable_by_id:true,finalized_genesis_identity_required:true,
     every_scanned_block_header_and_finality_validated:true,id_hash_id_lookups_equal:true,
     sequencer_indexer_inclusion_equal:true,no_clobber:true,runtime_external_resources:[],
     public_rpc_used:false,faucet_used:false,secret_scan_allowlist_enforced:true,
     private_material_disclosed:false}'
}

case "${1:-}" in
  contract) [[ "$#" == 1 ]] || fail "contract accepts no arguments"; command -v jq >/dev/null; emit_contract; exit ;;
  self-test-finality-selector) [[ "$#" == 1 ]] || fail "self-test accepts no arguments"; command -v jq >/dev/null; self_test_finality_selector; exit ;;
  execute) [[ "$#" == 1 ]] || fail "execute accepts no arguments" ;;
  *) fail "expected contract, self-test-finality-selector, or execute" ;;
esac

for name in base64 basename chmod cp curl dirname find jq mkdir mv readlink rg rm sed sha256sum sleep sort stat; do
  command -v "$name" >/dev/null || fail "missing required tool: $name"
done
required=(M4_LEZ_RUN_ID M4_LEZ_STACK_MANIFEST M4_LEZ_ARTIFACT_EVIDENCE M4_LEZ_DEPLOYER
  M4_LEZ_EXPECTED_DEPLOYER_SHA256 M4_LEZ_EVIDENCE_ROOT)
for name in "${required[@]}"; do [[ -n "${!name:-}" ]] || fail "missing environment: $name"; done
for name in M4_LEZ_STACK_MANIFEST M4_LEZ_ARTIFACT_EVIDENCE M4_LEZ_DEPLOYER M4_LEZ_EVIDENCE_ROOT; do
  [[ "${!name}" == /* ]] || fail "path must be absolute: $name"
done
[[ "$M4_LEZ_RUN_ID" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]] || fail "invalid run ID"
[[ "$M4_LEZ_EXPECTED_DEPLOYER_SHA256" =~ ^[0-9a-f]{64}$ ]] || fail "invalid expected deployer hash"
readonly timeout_seconds="${M4_LEZ_TIMEOUT_SECONDS:-300}"
[[ "$timeout_seconds" =~ ^[1-9][0-9]{0,3}$ ]] && (( 10#$timeout_seconds <= 3600 )) || fail "invalid timeout"

owner_file() {
  local mode
  [[ -f "$1" && ! -L "$1" && "$(readlink -f "$1")" == "$1" ]] || fail "$2 is missing or unsafe"
  mode="$(stat -c '%a' "$1")"; (( (8#$mode & 077) == 0 )) || fail "$2 must be owner-only"
}
owner_file "$M4_LEZ_STACK_MANIFEST" "stack manifest"
owner_file "$M4_LEZ_ARTIFACT_EVIDENCE" "artifact evidence"
[[ -x "$M4_LEZ_DEPLOYER" && -f "$M4_LEZ_DEPLOYER" && ! -L "$M4_LEZ_DEPLOYER" &&
  "$(readlink -f "$M4_LEZ_DEPLOYER")" == "$M4_LEZ_DEPLOYER" ]] || fail "unsafe deployer"
[[ ! -e "$M4_LEZ_EVIDENCE_ROOT" && ! -L "$M4_LEZ_EVIDENCE_ROOT" ]] || fail "evidence root exists"
parent="$(dirname "$M4_LEZ_EVIDENCE_ROOT")"
[[ -d "$parent" && ! -L "$parent" && "$(readlink -f "$parent")" == "$parent" ]] || fail "unsafe evidence parent"

env_value() {
  local -a values=(); mapfile -t values < <(sed -n "s/^${1}=//p" "$2")
  [[ "${#values[@]}" == 1 && -n "${values[0]}" ]] || fail "expected one $1 in manifest"
  printf '%s\n' "${values[0]}"
}
toml_value() {
  local -a values=(); mapfile -t values < <(sed -n "s/^${1} = \"\([^\"]*\)\"$/\1/p" "$2")
  [[ "${#values[@]}" == 1 && -n "${values[0]}" ]] || fail "expected one $1 in TOML"
  printf '%s\n' "${values[0]}"
}

stack_run="$(env_value RUN_ID "$M4_LEZ_STACK_MANIFEST")"
source_commit="$(env_value LEZ_V02_SOURCE_COMMIT "$M4_LEZ_STACK_MANIFEST")"
channel="$(env_value LEZ_V02_CHANNEL_PUBLIC_KEY "$M4_LEZ_STACK_MANIFEST")"
sequencer="$(env_value LEZ_SEQUENCER_RPC_URL "$M4_LEZ_STACK_MANIFEST")"
indexer="$(env_value LEZ_INDEXER_RPC_URL "$M4_LEZ_STACK_MANIFEST")"
readonly stack_run source_commit channel sequencer indexer
[[ "$stack_run" == "$M4_LEZ_RUN_ID" && "$source_commit" == "$pinned_source_commit" ]] || fail "stack identity differs"
[[ "$channel" =~ ^[0-9a-f]{64}$ && ! "$channel" =~ ^0+$ ]] || fail "invalid channel"
for endpoint in "$sequencer" "$indexer"; do
  [[ "$endpoint" =~ ^http://127\.0\.0\.1:[1-9][0-9]{0,4}/?$ ]] || fail "endpoint is not literal loopback"
  port="${endpoint##*:}"; port="${port%/}"; (( 10#$port <= 65535 )) || fail "port exceeds 65535"
done
[[ "$sequencer" != "$indexer" ]] || fail "sequencer and indexer endpoints collide"

for file in "$m4_manifest" "$deployer_manifest" "$deployer_source"; do [[ -f "$file" && ! -L "$file" ]] || fail "unsafe source $file"; done
[[ -x "$artifact_runner" && ! -L "$artifact_runner" ]] || fail "artifact source verifier is missing or unsafe"
"$artifact_runner" verify-source >/dev/null || fail "artifact source boundary differs"
one_line() {
  local count
  count="$(rg -Fxc "$1" "$2" || true)"
  [[ "$count" == 1 ]] || fail "$3 must occur exactly once"
}
one_line "lez_commit = \"${pinned_source_commit}\"" "$m4_manifest" "M4 source identity"
one_line "elf_sha256 = \"${pinned_guest_sha256}\"" "$m4_manifest" "M4 ELF identity"
one_line "image_id = \"${pinned_program_id}\"" "$m4_manifest" "M4 ImageID"
one_line 'fresh_docker_embedding_required = true' "$m4_manifest" "fresh embedding policy"
one_line 'run_owned_target_required = true' "$m4_manifest" "run-owned target policy"
one_line 'format_version = 1' "$M4_LEZ_ARTIFACT_EVIDENCE" "artifact format"
one_line 'milestone = "M4"' "$M4_LEZ_ARTIFACT_EVIDENCE" "artifact milestone"
one_line 'result = "passed"' "$M4_LEZ_ARTIFACT_EVIDENCE" "artifact result"
one_line 'runtime_external_resources = []' "$M4_LEZ_ARTIFACT_EVIDENCE" "artifact runtime resources"
artifact_run="$(toml_value run_id "$M4_LEZ_ARTIFACT_EVIDENCE")"
guest_elf="$(toml_value elf_path "$M4_LEZ_ARTIFACT_EVIDENCE")"
artifact_sha="$(toml_value elf_sha256 "$M4_LEZ_ARTIFACT_EVIDENCE")"
artifact_image="$(toml_value image_id "$M4_LEZ_ARTIFACT_EVIDENCE")"
readonly artifact_run guest_elf artifact_sha artifact_image
[[ "$artifact_run" =~ ^[a-z0-9][a-z0-9_-]*$ && "$guest_elf" == /* ]] || fail "invalid artifact proof identity"
owner_file "$guest_elf" "checked guest ELF"
runtime_guest="$pinned_guest_sha256"; test_override=false
if [[ -n "${M4_LEZ_CONTRACT_TEST_EXPECTED_GUEST_SHA256:-}" ]]; then
  [[ "${M4_LEZ_CONTRACT_TEST_ONLY:-}" == 1 && "$M4_LEZ_CONTRACT_TEST_EXPECTED_GUEST_SHA256" =~ ^[0-9a-f]{64}$ ]] || fail "unsafe test override"
  runtime_guest="$M4_LEZ_CONTRACT_TEST_EXPECTED_GUEST_SHA256"; test_override=true
fi
readonly runtime_guest test_override
finality_poll_limit=1200
if [[ -n "${M4_LEZ_CONTRACT_TEST_FINALITY_POLLS:-}" ]]; then
  [[ "$test_override" == true && "$M4_LEZ_CONTRACT_TEST_FINALITY_POLLS" =~ ^[1-9][0-9]?$ ]] || fail "unsafe finality poll override"
  finality_poll_limit="$M4_LEZ_CONTRACT_TEST_FINALITY_POLLS"
fi
readonly finality_poll_limit
[[ "$artifact_sha" == "$runtime_guest" && "$artifact_image" == "$pinned_program_id" ]] || fail "artifact identity differs"
[[ "$(sha256sum "$guest_elf" | sed 's/ .*//')" == "$runtime_guest" ]] || fail "artifact ELF differs"

stack_sha="$(sha256sum "$M4_LEZ_STACK_MANIFEST" | sed 's/ .*//')"
proof_sha="$(sha256sum "$M4_LEZ_ARTIFACT_EVIDENCE" | sed 's/ .*//')"
manifest_sha="$(sha256sum "$m4_manifest" | sed 's/ .*//')"
deployer_manifest_sha="$(sha256sum "$deployer_manifest" | sed 's/ .*//')"
deployer_source_sha="$(sha256sum "$deployer_source" | sed 's/ .*//')"
runner_sha="$(sha256sum scripts/run-m4-lez-local-deployment.sh | sed 's/ .*//')"
artifact_runner_sha="$(sha256sum "$artifact_runner" | sed 's/ .*//')"
deployer_sha="$(sha256sum "$M4_LEZ_DEPLOYER" | sed 's/ .*//')"
readonly stack_sha proof_sha manifest_sha deployer_manifest_sha deployer_source_sha artifact_runner_sha runner_sha deployer_sha
[[ "$deployer_sha" == "$M4_LEZ_EXPECTED_DEPLOYER_SHA256" ]] || fail "deployer hash differs"
mkdir -m 0700 "$M4_LEZ_EVIDENCE_ROOT"
printf '%s\n' "$deployer_sha" >"$M4_LEZ_EVIDENCE_ROOT/deployer-sha256-at-submission"
chmod 0600 "$M4_LEZ_EVIDENCE_ROOT/deployer-sha256-at-submission"

rpc() { curl --fail --silent --show-error --noproxy '*' --connect-timeout 2 --max-time 30 -H 'content-type: application/json' --data "$2" "$1"; }
tip() {
  local response value
  for _ in {1..120}; do
    if response="$(rpc "$indexer" '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' 2>/dev/null)" &&
      value="$(jq -er '.result|select(type=="number" and floor==. and .>=1)' <<<"$response" 2>/dev/null)"; then printf '%s\n' "$value"; return; fi
    sleep .25
  done
  fail "finalized tip unavailable"
}
rpc_file() {
  local partial="${3}.partial"
  for _ in {1..120}; do
    if rpc "$1" "$2" >"$partial" 2>/dev/null && jq -e '.error==null and .result!=null' "$partial" >/dev/null 2>&1; then
      chmod 0600 "$partial"; mv "$partial" "$3"; return
    fi
    sleep .25
  done
  fail "read-only RPC unavailable for $3"
}
validate_block() {
  jq -e --argjson id "$2" '.result.header.block_id==$id
    and (.result.header.hash|strings|test("^[0-9a-f]{64}$"))
    and .result.bedrock_status=="Finalized" and (.result.body.transactions|arrays)' "$1" >/dev/null || fail "invalid finalized $3"
}
block_by_id() {
  rpc_file "$indexer" "$(jq -cn --argjson id "$1" '{jsonrpc:"2.0",id:1,method:"getBlockById",params:[$id]}')" "$2"
  validate_block "$2" "$1" "block $1"
}
inspect_deployments() {
  local block="$1" phase="$2" id="$3" count=0 bytecode hash deployments
  deployments="$(jq -ce '[.result.body.transactions[]
    | select((keys|length)==1 and has("ProgramDeployment")) | .ProgramDeployment
    | if ((keys|sort)==["hash","message"] and (.hash|strings|test("^[0-9a-f]{64}$"))
        and (.message|keys)==["bytecode"] and (.message.bytecode|type)=="string" and (.message.bytecode|length)>0)
      then .message.bytecode else error("malformed ProgramDeployment") end]' "$block")" ||
    fail "malformed deployment transaction at $id"
  while IFS= read -r bytecode; do
    printf '%s' "$bytecode" | base64 --decode >"$scratch_bytecode" || fail "malformed deployment bytecode at $id"
    hash="$(sha256sum "$scratch_bytecode" | sed 's/ .*//')"
    [[ "$hash" != "$runtime_guest" ]] || count=$((count+1))
  done < <(jq -r '.[]' <<<"$deployments")
  jq -cn --arg phase "$phase" --argjson id "$id" --arg hash "$(jq -er '.result.header.hash' "$block")" --argjson exact "$count" \
    '{phase:$phase,block_id:$id,block_hash:$hash,bedrock_status:"Finalized",exact_elf_occurrences:$exact}' >>"$chain_manifest"
  chmod 0600 "$chain_manifest"
  printf '%s\n' "$count"
}

for _ in {1..3600}; do
  pre_tip="$(tip)"
  (( pre_tip >= 80 )) && break
  sleep .25
done
(( pre_tip >= 80 )) || fail "finalized prehistory did not reach 80 blocks"
readonly pre_tip
readonly scan_file="$M4_LEZ_EVIDENCE_ROOT/.scan-block.json"
((pre_tip<=max_finality_blocks)) || fail "pre-deployment finalized history exceeded scan bound"
readonly scratch_bytecode="$M4_LEZ_EVIDENCE_ROOT/.scan-bytecode.bin"
readonly chain_manifest="$M4_LEZ_EVIDENCE_ROOT/finalized-chain-scan.jsonl"
: >"$chain_manifest"; chmod 0600 "$chain_manifest"
pre_exact=0; genesis_indexer_hash=""
for ((height=1; height<=pre_tip; height++)); do
  block_by_id "$height" "$scan_file"
  [[ "$height" != 1 ]] || genesis_indexer_hash="$(jq -er '.result.header.hash' "$scan_file")"
  found="$(inspect_deployments "$scan_file" pre "$height")"; pre_exact=$((pre_exact+found))
done
[[ "$pre_exact" == 0 ]] || fail "exact M4 ELF already existed before send"
readonly anchor="$M4_LEZ_EVIDENCE_ROOT/pre-finalized-anchor.json"
block_by_id "$pre_tip" "$anchor"
[[ "$(jq -S -c '.result' "$scan_file")" == "$(jq -S -c '.result' "$anchor")" ]] || fail "pre-finalized anchor changed on reread"
printf '%s\n' "$pre_tip" >"$M4_LEZ_EVIDENCE_ROOT/pre-deployment-finalized-tip"; chmod 0600 "$M4_LEZ_EVIDENCE_ROOT/pre-deployment-finalized-tip"

[[ "$(sha256sum "$M4_LEZ_DEPLOYER"|sed 's/ .*//')" == "$deployer_sha" ]] || fail "deployer changed before point of use"
deployment="$M4_LEZ_EVIDENCE_ROOT/deployment.json"; partial="${deployment}.partial"
# Sole mutating code path: an unknown outcome is retained and is never automatically retried.
"$M4_LEZ_DEPLOYER" deploy-m4-local --rpc-url "$sequencer" --channel-id "$channel" --timeout-seconds "$timeout_seconds" >"$partial"
chmod 0600 "$partial"
[[ "$(sha256sum "$M4_LEZ_DEPLOYER"|sed 's/ .*//')" == "$deployer_sha" ]] || fail "deployer changed at point of use"
jq -e --arg rpc "$sequencer" --arg channel "$channel" --arg guest "$runtime_guest" --arg image "$pinned_program_id" \
  --arg source "$pinned_ata_source" --argjson words "$pinned_program_words" --argjson auth "$pinned_auth_program" \
  --argjson token "$pinned_token_program" --argjson ata "$pinned_ata_program" '
  (keys|sort)==["inclusion_block_hash","inclusion_block_id","preflight","schema_version","transaction_hash"]
  and .schema_version==1 and .preflight.rpc_url==$rpc and .preflight.channel_id==$channel
  and .preflight.elf_sha256==$guest and .preflight.image_id==$image and .preflight.program_id_words==$words
  and .preflight.authenticated_transfer_program_id==$auth and .preflight.token_program_id==$token
  and .preflight.associated_token_account_program_id==$ata and .preflight.associated_token_account_identity_source==$source
  and (.preflight.genesis_block_hash|strings|test("^[0-9a-f]{64}$")) and .preflight.genesis_block_id==1
  and (.preflight.last_block_id|type=="number") and .preflight.last_block_id==(.preflight.last_block_id|floor) and .preflight.last_block_id>=80
  and (.transaction_hash|strings|test("^[0-9a-f]{64}$")) and (.inclusion_block_id|type=="number") and .inclusion_block_id==(.inclusion_block_id|floor)
  and .inclusion_block_id>.preflight.last_block_id and (.inclusion_block_hash|strings|test("^[0-9a-f]{64}$"))
  and (.preflight.rpc_program_names|sort)==["amm","authenticated_transfer","pinata","privacy_preserving_circuit","token"]
  and (.preflight|keys|sort)==["associated_token_account_identity_source","associated_token_account_program_id","authenticated_transfer_program_id","channel_id","elf_sha256","genesis_block_hash","genesis_block_id","image_id","last_block_id","program_id_words","rpc_program_names","rpc_url","token_program_id"]
' "$partial" >/dev/null || fail "invalid exact deployment evidence"
mv "$partial" "$deployment"
tx="$(jq -er '.transaction_hash' "$deployment")"; inclusion="$(jq -er '.inclusion_block_id' "$deployment")"; inclusion_hash="$(jq -er '.inclusion_block_hash' "$deployment")"
preflight_last="$(jq -er '.preflight.last_block_id' "$deployment")"; genesis="$(jq -er '.preflight.genesis_block_hash' "$deployment")"
[[ "$genesis" == "$genesis_indexer_hash" ]] || fail "finalized genesis differs from sequencer preflight"
((pre_tip<=preflight_last && preflight_last<inclusion)) || fail "pre-finalized tip, sequencer preflight, and inclusion order differ"

cursor=$((pre_tip+1)); post_exact=0; tx_count=0; containing_id=0; containing_file=""; final_tip="$pre_tip"
deadline="$(date +%s)"
deadline=$((deadline+timeout_seconds))
for ((poll=0; poll<finality_poll_limit; poll++)); do
  (( $(date +%s) < deadline )) || fail "finality polling timeout after ${timeout_seconds}s"
  final_tip="$(tip)"; ((final_tip>=pre_tip)) || fail "finalized tip regressed"; ((final_tip-pre_tip<=max_finality_blocks)) || fail "scan exceeded bound"
  while ((cursor<=final_tip)); do
    block_by_id "$cursor" "$scan_file"
    found="$(inspect_deployments "$scan_file" post "$cursor")"; post_exact=$((post_exact+found))
    occurrences="$(transaction_occurrences "$scan_file" "$tx")"; tx_count=$((tx_count+occurrences))
    if ((occurrences>0)); then containing_id="$cursor"; cp -- "$scan_file" "$M4_LEZ_EVIDENCE_ROOT/containing-block.json"; chmod 0600 "$M4_LEZ_EVIDENCE_ROOT/containing-block.json"; containing_file="$M4_LEZ_EVIDENCE_ROOT/containing-block.json"; fi
    cursor=$((cursor+1))
  done
  ((tx_count==0)) || break
  (( $(date +%s) < deadline )) || fail "finality polling timeout after ${timeout_seconds}s"
  sleep .25
done
[[ "$tx_count" == 1 && "$post_exact" == 1 && "$containing_id" != 0 ]] || fail "canonical window lacks exactly one transaction and exact ELF"
containing_hash="$(jq -er '.result.header.hash' "$containing_file")"
hash_file="$M4_LEZ_EVIDENCE_ROOT/containing-block-by-hash.json"; reread="$M4_LEZ_EVIDENCE_ROOT/containing-block-id-reread.json"
rpc_file "$indexer" "$(jq -cn --arg hash "$containing_hash" '{jsonrpc:"2.0",id:1,method:"getBlockByHash",params:[$hash]}')" "$hash_file"; validate_block "$hash_file" "$containing_id" "hash lookup"
block_by_id "$containing_id" "$reread"
canonical="$(jq -S -c '.result' "$containing_file")"
[[ "$canonical" == "$(jq -S -c '.result' "$hash_file")" && "$canonical" == "$(jq -S -c '.result' "$reread")" ]] || fail "ID/hash/ID lookups differ"
[[ "$containing_id" == "$inclusion" && "$containing_hash" == "$inclusion_hash" ]] || fail "sequencer/indexer inclusion differs"
rm -f -- "$scan_file" "$scratch_bytecode"

[[ "$(sha256sum "$M4_LEZ_DEPLOYER"|sed 's/ .*//')" == "$deployer_sha" && "$(sha256sum "$guest_elf"|sed 's/ .*//')" == "$runtime_guest" &&
  "$(sha256sum "$M4_LEZ_ARTIFACT_EVIDENCE"|sed 's/ .*//')" == "$proof_sha" && "$(sha256sum "$M4_LEZ_STACK_MANIFEST"|sed 's/ .*//')" == "$stack_sha" &&
  "$(sha256sum "$m4_manifest"|sed 's/ .*//')" == "$manifest_sha" && "$(sha256sum "$deployer_manifest"|sed 's/ .*//')" == "$deployer_manifest_sha" &&
  "$(sha256sum "$deployer_source"|sed 's/ .*//')" == "$deployer_source_sha" && "$(sha256sum "$artifact_runner"|sed 's/ .*//')" == "$artifact_runner_sha" && "$(sha256sum scripts/run-m4-lez-local-deployment.sh|sed 's/ .*//')" == "$runner_sha" ]] || fail "bound inputs changed"

jq -n --arg run "$M4_LEZ_RUN_ID" --arg artifact_run "$artifact_run" --arg source "$source_commit" --arg channel "$channel" --arg sequencer "$sequencer" --arg indexer "$indexer" \
  --arg guest "$runtime_guest" --arg image "$pinned_program_id" --arg proof "$M4_LEZ_ARTIFACT_EVIDENCE" --arg proof_sha "$proof_sha" --arg deployer "$M4_LEZ_DEPLOYER" \
  --arg deployer_sha "$deployer_sha" --arg manifest_sha "$manifest_sha" --arg artifact_runner_sha "$artifact_runner_sha" --arg deployer_manifest_sha "$deployer_manifest_sha" --arg deployer_source_sha "$deployer_source_sha" \
  --arg runner_sha "$runner_sha" --arg tx "$tx" --arg hash "$containing_hash" --arg genesis "$genesis" --argjson pre "$pre_tip" --argjson last "$preflight_last" \
  --argjson tip "$final_tip" --argjson block "$containing_id" --argjson test_override "$test_override" '
  {schema_version:1,kind:"m4_lez_local_deployment",result:"passed",run_id:$run,
   stack:{lez_source_commit:$source,channel_id:$channel,sequencer_rpc_url:$sequencer,indexer_rpc_url:$indexer,finalized_genesis_hash:$genesis,isolated_loopback_only:true},
   artifact:{proof_run_id:$artifact_run,proof_path:$proof,proof_sha256:$proof_sha,elf_sha256:$guest,image_id:$image,m4_manifest_sha256:$manifest_sha,
     exact_elf_pre_window_occurrences:0,exact_elf_post_window_occurrences:1,finalized_wire_bytecode_equal:true,source_boundary_verifier_sha256:$artifact_runner_sha,contract_test_override_used:$test_override},
   deployer:{binary_path:$deployer,binary_sha256:$deployer_sha,manifest_sha256:$deployer_manifest_sha,source_sha256:$deployer_source_sha,hash_stable_at_point_of_use:true},
   harness:{path:"scripts/run-m4-lez-local-deployment.sh",sha256:$runner_sha},transaction_id:$tx,send_attempts_this_process:1,durable_cross_process_submission_counter:false,
   window:{pre_finalized_tip:$pre,start_height:($pre+1),preflight_last_sequencer_block:$last,finalized_tip:$tip},canonical_window_occurrences:1,
   containing_block_id:$block,containing_block_hash:$hash,bedrock_status:"Finalized",id_hash_id_lookups_equal:true,sequencer_indexer_inclusion_equal:true,
   runtime_external_resources:[],public_rpc_used:false,faucet_used:false,evidence_allowlisted_stack_fields:["RUN_ID","LEZ_V02_SOURCE_COMMIT","LEZ_V02_CHANNEL_PUBLIC_KEY","LEZ_SEQUENCER_RPC_URL","LEZ_INDEXER_RPC_URL"],
   stack_manifest_copied:false,private_material_disclosed:false}' >"$M4_LEZ_EVIDENCE_ROOT/finality.json.partial"
chmod 0600 "$M4_LEZ_EVIDENCE_ROOT/finality.json.partial"; mv "$M4_LEZ_EVIDENCE_ROOT/finality.json.partial" "$M4_LEZ_EVIDENCE_ROOT/finality.json"

raw="$(find "$M4_LEZ_EVIDENCE_ROOT" -maxdepth 1 -type f ! -name 'bundle.json*' -print | sort | while IFS= read -r file; do
  jq -cn --arg path "$(basename "$file")" --arg sha "$(sha256sum "$file"|sed 's/ .*//')" '{path:$path,sha256:$sha}'; done | jq -s '.')"
jq -n --arg run "$M4_LEZ_RUN_ID" --argjson raw "$raw" '{schema_version:1,kind:"m4_lez_local_deployment_bundle",result:"passed",run_id:$run,raw_evidence:$raw,
  secret_safety:{allowlist_enforced:true,stack_manifest_copied:false,private_material_disclosed:false},runtime_external_resources:[],public_rpc_used:false,faucet_used:false}' >"$M4_LEZ_EVIDENCE_ROOT/bundle.json.partial"
chmod 0600 "$M4_LEZ_EVIDENCE_ROOT/bundle.json.partial"; mv "$M4_LEZ_EVIDENCE_ROOT/bundle.json.partial" "$M4_LEZ_EVIDENCE_ROOT/bundle.json"
echo "M4 checked deployment passed finalized local proof: $M4_LEZ_EVIDENCE_ROOT/bundle.json"
