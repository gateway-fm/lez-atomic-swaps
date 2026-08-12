#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export LC_ALL=C
umask 077

readonly pinned_source_commit="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
readonly pinned_program_id="b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"
readonly max_finality_blocks=4096

fail() { echo "M4 LEZ actor onboarding failed: $*" >&2; exit 2; }

transaction_occurrences() {
  jq -er --arg tx "$2" '[.result.body.transactions[]
    | select((keys|length)==1 and has("Public"))
    | .Public | select(.hash==$tx)] | length' "$1"
}

self_test_finality_selector() {
  local fixture tx
  tx="$(printf 'a%.0s' {1..64})"
  fixture="$(jq -cn --arg tx "$tx" '{result:{body:{transactions:[
    {Public:{hash:$tx}},{ProgramDeployment:{hash:$tx}},
    {Public:{hash:$tx},ProgramDeployment:{hash:$tx}}]}}}')"
  [[ "$(transaction_occurrences - "$tx" <<<"$fixture")" == 1 ]] ||
    fail "finality selector is not exact to one-key Public transactions"
}

emit_contract() {
  jq -n '
    {schema_version:1,kind:"m4_lez_actor_onboarding_contract",
     flow:"flow_0_fresh_vault_claims",roles:["maker","taker"],
     submission_count_per_role:1,automatic_submission_retry:false,
     finality_membership_variant:"Public",canonical_window_occurrences_required:1,
     finalized_scan:"bounded_sequential_indexer_blocks_read_only",
     indexer_account_read:"getAccountAtBlock_exact_containing_finalized_block",
     expected_before:{owner:{balance:0,nonce:0},vault:{balance:"genesis_allocation",nonce:0}},
     expected_effect:{owner:{balance:"genesis_allocation",nonce:1},vault:{balance:0,nonce:0}},
     requires_fresh_actor_identities:true,requires_finalized_deployment:true,
     durable_role_state:true,no_clobber:true,monero_or_swap_effects_started:false,
     runtime_external_resources:[],public_rpc_used:false,faucet_used:false,
     private_material_disclosed:false}'
}

case "${1:-}" in
  contract)
    [[ "$#" == 1 ]] || fail "contract accepts no arguments"
    command -v jq >/dev/null || fail "jq is required"
    emit_contract
    exit 0
    ;;
  self-test-finality-selector)
    [[ "$#" == 1 ]] || fail "self-test-finality-selector accepts no arguments"
    command -v jq >/dev/null || fail "jq is required"
    self_test_finality_selector
    exit 0
    ;;
  execute) [[ "$#" == 1 ]] || fail "execute accepts no arguments" ;;
  *) fail "expected contract, self-test-finality-selector, or execute" ;;
esac

for name in basename chmod curl dirname find id jq mkdir mv readlink sed sha256sum sleep sort stat; do
  command -v "$name" >/dev/null || fail "missing required tool: ${name}"
done
required=(M4_ONBOARD_RUN_ID M4_ONBOARD_STACK_MANIFEST M4_ONBOARD_DEPLOYMENT_FINALITY
  M4_ONBOARD_EVIDENCE_ROOT M4_ONBOARD_PRIVATE_ROOT M4_ONBOARD_MAKER_IDENTITY
  M4_ONBOARD_TAKER_IDENTITY M4_ONBOARD_MAKER_PRIVATE_KEY M4_ONBOARD_TAKER_PRIVATE_KEY
  M4_ONBOARD_VAULT_CLAIM_BIN M4_ONBOARD_EXPECTED_VAULT_CLAIM_SHA256)
for name in "${required[@]}"; do
  [[ -n "${!name:-}" ]] || fail "missing environment: ${name}"
done
if [[ "${M4_ONBOARD_TAKER_B_IDENTITY+x}" != \
      "${M4_ONBOARD_TAKER_B_PRIVATE_KEY+x}" ]]; then
  fail "Taker B identity and private key must be supplied together"
fi
taker_b_enabled=0
if [[ -n "${M4_ONBOARD_TAKER_B_IDENTITY:-}" ]]; then
  taker_b_enabled=1
fi
readonly taker_b_enabled
for name in M4_ONBOARD_STACK_MANIFEST M4_ONBOARD_DEPLOYMENT_FINALITY \
  M4_ONBOARD_EVIDENCE_ROOT M4_ONBOARD_PRIVATE_ROOT M4_ONBOARD_MAKER_IDENTITY \
  M4_ONBOARD_TAKER_IDENTITY M4_ONBOARD_MAKER_PRIVATE_KEY M4_ONBOARD_TAKER_PRIVATE_KEY \
  M4_ONBOARD_VAULT_CLAIM_BIN; do
  [[ "${!name}" == /* ]] || fail "path must be absolute: ${name}"
done
if [[ "$taker_b_enabled" == 1 ]]; then
  for name in M4_ONBOARD_TAKER_B_IDENTITY M4_ONBOARD_TAKER_B_PRIVATE_KEY; do
    [[ "${!name}" == /* ]] || fail "path must be absolute: ${name}"
  done
fi
[[ "$M4_ONBOARD_RUN_ID" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]] || fail "invalid run ID"
[[ "$M4_ONBOARD_EXPECTED_VAULT_CLAIM_SHA256" =~ ^[0-9a-f]{64}$ ]] ||
  fail "invalid expected Vault Claim binary hash"

owner_file() {
  local path="$1" label="$2" mode
  [[ -f "$path" && ! -L "$path" && "$(readlink -f "$path")" == "$path" ]] ||
    fail "${label} is missing or unsafe"
  [[ "$(stat -c '%u' "$path")" == "$(id -u)" && "$(stat -c '%h' "$path")" == 1 ]] ||
    fail "${label} ownership is unsafe"
  mode="$(stat -c '%a' "$path")"
  (( (8#$mode & 077) == 0 )) || fail "${label} must be owner-only"
}

owner_directory() {
  local path="$1" label="$2" mode
  [[ -d "$path" && ! -L "$path" && "$(readlink -f "$path")" == "$path" ]] ||
    fail "${label} is missing or unsafe"
  [[ "$(stat -c '%u' "$path")" == "$(id -u)" ]] || fail "${label} owner differs"
  mode="$(stat -c '%a' "$path")"
  (( (8#$mode & 077) == 0 )) || fail "${label} must be owner-only"
}

checked_executable() {
  local path="$1" label="$2" mode
  [[ -f "$path" && ! -L "$path" && "$(readlink -f "$path")" == "$path" ]] ||
    fail "${label} is missing or unsafe"
  [[ "$(stat -c '%u' "$path")" == "$(id -u)" && "$(stat -c '%h' "$path")" == 1 ]] ||
    fail "${label} ownership is unsafe"
  mode="$(stat -c '%a' "$path")"
  (( (8#$mode & 022) == 0 )) || fail "${label} must not be group/other writable"
  [[ -x "$path" ]] || fail "${label} is not executable"
}

for path_label in \
  "$M4_ONBOARD_STACK_MANIFEST|stack manifest" \
  "$M4_ONBOARD_DEPLOYMENT_FINALITY|deployment finality" \
  "$M4_ONBOARD_MAKER_IDENTITY|Maker identity" \
  "$M4_ONBOARD_TAKER_IDENTITY|Taker identity" \
  "$M4_ONBOARD_MAKER_PRIVATE_KEY|Maker private key" \
  "$M4_ONBOARD_TAKER_PRIVATE_KEY|Taker private key"; do
  IFS='|' read -r checked_path checked_label <<<"$path_label"
  owner_file "$checked_path" "$checked_label"
done
if [[ "$taker_b_enabled" == 1 ]]; then
  for path_label in \
    "$M4_ONBOARD_TAKER_B_IDENTITY|Taker B identity" \
    "$M4_ONBOARD_TAKER_B_PRIVATE_KEY|Taker B private key"; do
    IFS='|' read -r checked_path checked_label <<<"$path_label"
    owner_file "$checked_path" "$checked_label"
  done
fi
owner_directory "$M4_ONBOARD_PRIVATE_ROOT" "private actor root"
checked_executable "$M4_ONBOARD_VAULT_CLAIM_BIN" "Vault Claim binary"
[[ "$(sha256sum "$M4_ONBOARD_VAULT_CLAIM_BIN" | sed 's/ .*//')" == \
   "$M4_ONBOARD_EXPECTED_VAULT_CLAIM_SHA256" ]] || fail "Vault Claim binary hash differs"
[[ ! -e "$M4_ONBOARD_EVIDENCE_ROOT" && ! -L "$M4_ONBOARD_EVIDENCE_ROOT" ]] ||
  fail "refusing to reuse actor-onboarding evidence"
owner_directory "$(dirname "$M4_ONBOARD_EVIDENCE_ROOT")" "actor-onboarding evidence parent"
readonly state_root="${M4_ONBOARD_PRIVATE_ROOT}/flow0-vault-claims"
[[ ! -e "$state_root" && ! -L "$state_root" ]] || fail "refusing to reuse actor-onboarding state"

env_value() {
  local key="$1" file="$2"; local -a values=()
  mapfile -t values < <(sed -n "s/^${key}=//p" "$file")
  [[ "${#values[@]}" == 1 && -n "${values[0]}" ]] || fail "expected one ${key} in stack manifest"
  printf '%s\n' "${values[0]}"
}

identity_value() {
  jq -er --arg key "$1" '.[$key] | strings | select(length>0)' "$2" ||
    fail "identity field is missing: $1"
}

stack_run="$(env_value RUN_ID "$M4_ONBOARD_STACK_MANIFEST")"
source_commit="$(env_value LEZ_V02_SOURCE_COMMIT "$M4_ONBOARD_STACK_MANIFEST")"
channel="$(env_value LEZ_V02_CHANNEL_PUBLIC_KEY "$M4_ONBOARD_STACK_MANIFEST")"
sequencer="$(env_value LEZ_SEQUENCER_RPC_URL "$M4_ONBOARD_STACK_MANIFEST")"
indexer="$(env_value LEZ_INDEXER_RPC_URL "$M4_ONBOARD_STACK_MANIFEST")"
readonly stack_run source_commit channel sequencer indexer
[[ "$stack_run" == "$M4_ONBOARD_RUN_ID" && "$source_commit" == "$pinned_source_commit" ]] ||
  fail "stack identity differs"
[[ "$channel" =~ ^[0-9a-f]{64}$ && ! "$channel" =~ ^0+$ ]] || fail "invalid channel identity"
for endpoint in "$sequencer" "$indexer"; do
  [[ "$endpoint" =~ ^http://127\.0\.0\.1:[1-9][0-9]{0,4}/?$ ]] || fail "endpoint is not literal loopback"
  port="${endpoint##*:}"; port="${port%/}"; (( 10#$port <= 65535 )) || fail "endpoint port exceeds 65535"
done
[[ "$sequencer" != "$indexer" ]] || fail "sequencer and indexer endpoints collide"

actor_labels=(maker taker)
if [[ "$taker_b_enabled" == 1 ]]; then actor_labels+=(taker-b); fi
for role in "${actor_labels[@]}"; do
  case "$role" in
    maker) identity="$M4_ONBOARD_MAKER_IDENTITY"; upper=MAKER ;;
    taker) identity="$M4_ONBOARD_TAKER_IDENTITY"; upper=TAKER ;;
    taker-b) identity="$M4_ONBOARD_TAKER_B_IDENTITY"; upper=TAKER_B ;;
  esac
  jq -e '.schema=="lez-v0.2-local-actor-identity" and .version==2
    and (.account_id|strings|test("^[1-9A-HJ-NP-Za-km-z]{43,44}$"))
    and (.vault_account_id|strings|test("^[1-9A-HJ-NP-Za-km-z]{43,44}$"))
    and (.account_id_hex|strings|test("^[0-9a-f]{64}$"))
    and (.vault_account_id_hex|strings|test("^[0-9a-f]{64}$"))' "$identity" >/dev/null ||
    fail "${role} identity evidence is invalid"
  account="$(env_value "LEZ_V02_${upper}_ACCOUNT_ID" "$M4_ONBOARD_STACK_MANIFEST")"
  vault="$(env_value "LEZ_V02_${upper}_VAULT_ACCOUNT_ID" "$M4_ONBOARD_STACK_MANIFEST")"
  [[ "$account" == "$(identity_value account_id "$identity")" &&
     "$vault" == "$(identity_value vault_account_id "$identity")" ]] ||
    fail "${role} fresh identity differs from stack genesis"
done
maker_allocation="$(env_value LEZ_V02_MAKER_GENESIS_ALLOCATION "$M4_ONBOARD_STACK_MANIFEST")"
taker_allocation="$(env_value LEZ_V02_TAKER_GENESIS_ALLOCATION "$M4_ONBOARD_STACK_MANIFEST")"
taker_b_allocation=0
if [[ "$taker_b_enabled" == 1 ]]; then
  taker_b_allocation="$(env_value LEZ_V02_TAKER_B_GENESIS_ALLOCATION "$M4_ONBOARD_STACK_MANIFEST")"
fi
readonly maker_allocation taker_allocation taker_b_allocation
[[ "$maker_allocation" == 100000 && "$taker_allocation" == 200000 ]] || fail "genesis allocations differ"
[[ "$taker_b_enabled" == 0 || "$taker_b_allocation" == 300000 ]] || fail "Taker B genesis allocation differs"

jq -e --arg run "$M4_ONBOARD_RUN_ID" --arg channel "$channel" --arg sequencer "$sequencer" \
  --arg indexer "$indexer" --arg program "$pinned_program_id" '
  .schema_version==1 and .kind=="m4_lez_local_deployment" and .result=="passed"
  and .run_id==$run and .stack.channel_id==$channel
  and .stack.sequencer_rpc_url==$sequencer and .stack.indexer_rpc_url==$indexer
  and .stack.isolated_loopback_only==true and .artifact.image_id==$program
  and .canonical_window_occurrences==1 and .bedrock_status=="Finalized"
  and .id_hash_id_lookups_equal==true and .sequencer_indexer_inclusion_equal==true
  and .runtime_external_resources==[] and .public_rpc_used==false and .faucet_used==false
' "$M4_ONBOARD_DEPLOYMENT_FINALITY" >/dev/null || fail "finalized deployment binding is invalid"

poll_limit=1200
if [[ -n "${M4_ONBOARD_CONTRACT_TEST_FINALITY_POLLS:-}" ]]; then
  [[ "${M4_ONBOARD_CONTRACT_TEST_ONLY:-}" == 1 &&
     "$M4_ONBOARD_CONTRACT_TEST_FINALITY_POLLS" =~ ^[1-9][0-9]?$ ]] || fail "unsafe poll test override"
  poll_limit="$M4_ONBOARD_CONTRACT_TEST_FINALITY_POLLS"
fi
readonly poll_limit

mkdir -m 0700 "$M4_ONBOARD_EVIDENCE_ROOT" "$state_root"
# Normalize exact mode because the Vault Claim durable store rejects non-0700 directories.
chmod 0700 "$M4_ONBOARD_EVIDENCE_ROOT" "$state_root"
runner_sha="$(sha256sum scripts/run-m4-lez-actor-onboarding.sh | sed 's/ .*//')"
stack_sha="$(sha256sum "$M4_ONBOARD_STACK_MANIFEST" | sed 's/ .*//')"
deployment_sha="$(sha256sum "$M4_ONBOARD_DEPLOYMENT_FINALITY" | sed 's/ .*//')"
maker_identity_sha="$(sha256sum "$M4_ONBOARD_MAKER_IDENTITY" | sed 's/ .*//')"
taker_identity_sha="$(sha256sum "$M4_ONBOARD_TAKER_IDENTITY" | sed 's/ .*//')"
taker_b_identity_sha=""
if [[ "$taker_b_enabled" == 1 ]]; then
  taker_b_identity_sha="$(sha256sum "$M4_ONBOARD_TAKER_B_IDENTITY" | sed 's/ .*//')"
fi
vault_claim_sha="$(sha256sum "$M4_ONBOARD_VAULT_CLAIM_BIN" | sed 's/ .*//')"
readonly runner_sha stack_sha deployment_sha maker_identity_sha taker_identity_sha taker_b_identity_sha vault_claim_sha

rpc() {
  curl --fail --silent --show-error --noproxy '*' --connect-timeout 2 --max-time 30 \
    -H 'content-type: application/json' --data "$2" "$1"
}

tip() {
  local response value
  for ((attempt=0; attempt<poll_limit; attempt++)); do
    if response="$(rpc "$indexer" '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' 2>/dev/null)" &&
      value="$(jq -er '.result|select(type=="number" and floor==. and .>=1)' <<<"$response" 2>/dev/null)"; then
      printf '%s\n' "$value"; return 0
    fi
    sleep .25
  done
  fail "finalized tip remained unavailable"
}

rpc_file() {
  local endpoint="$1" request="$2" output="$3" partial="${3}.partial"
  [[ ! -e "$output" && ! -e "$partial" ]] || fail "refusing to overwrite RPC evidence"
  for ((attempt=0; attempt<poll_limit; attempt++)); do
    if rpc "$endpoint" "$request" >"$partial" 2>/dev/null &&
      jq -e '.error==null and .result!=null' "$partial" >/dev/null 2>&1; then
      chmod 0600 "$partial"; mv "$partial" "$output"; return 0
    fi
    rm -f -- "$partial"
    sleep .25
  done
  fail "read-only RPC remained unavailable for $(basename "$output")"
}

validate_block() {
  jq -e --argjson id "$2" '.result.header.block_id==$id
    and (.result.header.hash|strings|test("^[0-9a-f]{64}$"))
    and .result.bedrock_status=="Finalized" and (.result.body.transactions|arrays)' "$1" >/dev/null ||
    fail "invalid finalized block: $3"
}

prove_finalized_claim() {
  local role="$1" tx="$2" start="$3"
  local cursor=$((start+1))
  local current_tip height block occurrences count=0 containing="" containing_id=0 hash hash_file reread canonical
  for ((poll=0; poll<poll_limit; poll++)); do
    current_tip="$(tip)"
    (( current_tip >= start && current_tip - start <= max_finality_blocks )) || fail "${role} finality window invalid"
    while (( cursor <= current_tip )); do
      height="$cursor"; block="${M4_ONBOARD_EVIDENCE_ROOT}/${role}-finalized-block-${height}.json"
      rpc_file "$indexer" "$(jq -cn --argjson id "$height" '{jsonrpc:"2.0",id:1,method:"getBlockById",params:[$id]}')" "$block"
      validate_block "$block" "$height" "${role} height ${height}"
      occurrences="$(transaction_occurrences "$block" "$tx")"; count=$((count+occurrences))
      if (( occurrences > 0 )); then containing="$block"; containing_id="$height"; fi
      cursor=$((cursor+1))
    done
    (( count == 0 )) || break
    sleep .25
  done
  [[ "$count" == 1 && "$containing_id" != 0 ]] || fail "${role} Claim lacks exactly one finalized occurrence"
  hash="$(jq -er '.result.header.hash' "$containing")"
  hash_file="${M4_ONBOARD_EVIDENCE_ROOT}/${role}-containing-block-by-hash.json"
  reread="${M4_ONBOARD_EVIDENCE_ROOT}/${role}-containing-block-id-reread.json"
  rpc_file "$indexer" "$(jq -cn --arg hash "$hash" '{jsonrpc:"2.0",id:1,method:"getBlockByHash",params:[$hash]}')" "$hash_file"
  validate_block "$hash_file" "$containing_id" "${role} hash lookup"
  rpc_file "$indexer" "$(jq -cn --argjson id "$containing_id" '{jsonrpc:"2.0",id:1,method:"getBlockById",params:[$id]}')" "$reread"
  validate_block "$reread" "$containing_id" "${role} ID reread"
  canonical="$(jq -S -c '.result' "$containing")"
  [[ "$canonical" == "$(jq -S -c '.result' "$hash_file")" &&
     "$canonical" == "$(jq -S -c '.result' "$reread")" ]] || fail "${role} finalized ID/hash/ID reads differ"
  jq -n --arg role "$role" --arg tx "$tx" --arg hash "$hash" --argjson start "$start" \
    --argjson block "$containing_id" --argjson tip "$current_tip" '
    {schema_version:1,role:$role,transaction_id:$tx,window:{start_height:($start+1),finalized_tip:$tip},
     occurrences:1,containing_block_id:$block,containing_block_hash:$hash,
     bedrock_status:"Finalized",id_hash_id_lookups_equal:true}' \
    >"${M4_ONBOARD_EVIDENCE_ROOT}/${role}-finality.json"
  chmod 0600 "${M4_ONBOARD_EVIDENCE_ROOT}/${role}-finality.json"
  printf '%s\n' "$containing_id"
}

claim_role() {
  local role="$1" protocol_role="$2" key identity allocation account vault owner_hex vault_hex role_state request evidence partial
  local start tx block owner_output vault_output summary
  case "$role" in
    maker) key="$M4_ONBOARD_MAKER_PRIVATE_KEY"; identity="$M4_ONBOARD_MAKER_IDENTITY"; allocation="$maker_allocation" ;;
    taker) key="$M4_ONBOARD_TAKER_PRIVATE_KEY"; identity="$M4_ONBOARD_TAKER_IDENTITY"; allocation="$taker_allocation" ;;
    taker-b) key="$M4_ONBOARD_TAKER_B_PRIVATE_KEY"; identity="$M4_ONBOARD_TAKER_B_IDENTITY"; allocation="$taker_b_allocation" ;;
    *) fail "unsupported actor label: ${role}" ;;
  esac
  account="$(identity_value account_id "$identity")"; vault="$(identity_value vault_account_id "$identity")"
  owner_hex="$(identity_value account_id_hex "$identity")"; vault_hex="$(identity_value vault_account_id_hex "$identity")"
  role_state="${state_root}/${role}"; mkdir -m 0700 "$role_state"; chmod 0700 "$role_state"
  request="${role}-flow0-vault-claim-0001"
  evidence="${M4_ONBOARD_EVIDENCE_ROOT}/${role}-vault-claim.json"; partial="${evidence}.partial"
  start="$(tip)"
  # Sole mutating call for this role. Failure/unknown is retained and never automatically resubmitted.
  "$M4_ONBOARD_VAULT_CLAIM_BIN" --role "$protocol_role" --run-id "$M4_ONBOARD_RUN_ID" \
    --request-id "$request" --state-directory "$role_state" --private-key-file "$key" \
    --sequencer-url "$sequencer" --chain-id "$channel" --escrow-program-id "$pinned_program_id" \
    --allocation "$allocation" --max-scan-blocks "$max_finality_blocks" >"$partial"
  chmod 0600 "$partial"
  jq -e --arg role "$protocol_role" --arg run "$M4_ONBOARD_RUN_ID" --arg request "$request" \
    --arg channel "$channel" --arg program "$pinned_program_id" --arg owner "$owner_hex" \
    --arg vault "$vault_hex" --argjson allocation "$allocation" '
    .schema=="lez_v02_vault_claim_poc_v1" and .role==$role and .run_id==$run and .request_id==$request
    and .runtime.chain_id==$channel and .runtime.channel_id==$channel and .runtime.escrow_program_id==$program
    and .allocation==$allocation and (.transaction_id|strings|test("^[0-9a-f]{64}$"))
    and .submission.decision=="admitted" and .durable_state=="admitted" and .durable_attempt_count==1
    and .before.owner.account_id==$owner and .before.owner.balance==0 and .before.owner.nonce==0
    and .before.vault.account_id==$vault and .before.vault.balance==$allocation and .before.vault.nonce==0
    and .finality=="not_observed_in_this_poc_slice"
  ' "$partial" >/dev/null || fail "${role} Vault Claim evidence is invalid"
  mv "$partial" "$evidence"
  tx="$(jq -er '.transaction_id' "$evidence")"
  block="$(prove_finalized_claim "$role" "$tx" "$start")"
  owner_output="${M4_ONBOARD_EVIDENCE_ROOT}/${role}-owner-at-finalized-claim.json"
  vault_output="${M4_ONBOARD_EVIDENCE_ROOT}/${role}-vault-at-finalized-claim.json"
  rpc_file "$indexer" "$(jq -cn --arg account "$account" --argjson block "$block" '{jsonrpc:"2.0",id:1,method:"getAccountAtBlock",params:[$account,$block]}')" "$owner_output"
  rpc_file "$indexer" "$(jq -cn --arg account "$vault" --argjson block "$block" '{jsonrpc:"2.0",id:1,method:"getAccountAtBlock",params:[$account,$block]}')" "$vault_output"
  jq -e --argjson allocation "$allocation" '.result.balance==$allocation and .result.nonce==1' "$owner_output" >/dev/null ||
    fail "${role} finalized owner effect is invalid"
  jq -e '.result.balance==0 and .result.nonce==0' "$vault_output" >/dev/null ||
    fail "${role} finalized Vault effect is invalid"
  summary="${M4_ONBOARD_EVIDENCE_ROOT}/${role}-summary.json"
  jq -n --arg role "$role" --arg protocol_role "$protocol_role" --arg account "$account" --arg vault "$vault" --arg tx "$tx" \
    --argjson block "$block" --argjson allocation "$allocation" \
    '{role:$role,protocol_role:$protocol_role,account_id:$account,vault_account_id:$vault,transaction_id:$tx,submission_count:1,finalized_block_id:$block,
      canonical_window_occurrences:1,owner_after:{balance:$allocation,nonce:1},vault_after:{balance:0,nonce:0}}' >"$summary"
  chmod 0600 "$summary"
}

claim_role maker maker
claim_role taker taker
if [[ "$taker_b_enabled" == 1 ]]; then claim_role taker-b taker; fi

[[ "$(sha256sum scripts/run-m4-lez-actor-onboarding.sh | sed 's/ .*//')" == "$runner_sha" &&
   "$(sha256sum "$M4_ONBOARD_STACK_MANIFEST" | sed 's/ .*//')" == "$stack_sha" &&
   "$(sha256sum "$M4_ONBOARD_DEPLOYMENT_FINALITY" | sed 's/ .*//')" == "$deployment_sha" &&
   "$(sha256sum "$M4_ONBOARD_MAKER_IDENTITY" | sed 's/ .*//')" == "$maker_identity_sha" &&
   "$(sha256sum "$M4_ONBOARD_TAKER_IDENTITY" | sed 's/ .*//')" == "$taker_identity_sha" &&
   ( "$taker_b_enabled" == 0 || \
     "$(sha256sum "$M4_ONBOARD_TAKER_B_IDENTITY" | sed 's/ .*//')" == "$taker_b_identity_sha" ) &&
   "$(sha256sum "$M4_ONBOARD_VAULT_CLAIM_BIN" | sed 's/ .*//')" == "$vault_claim_sha" ]] ||
  fail "actor-onboarding bound inputs changed"

raw="$(find "$M4_ONBOARD_EVIDENCE_ROOT" -maxdepth 1 -type f ! -name 'summary.json*' -print | sort |
  while IFS= read -r file; do
    jq -cn --arg path "$(basename "$file")" --arg sha "$(sha256sum "$file" | sed 's/ .*//')" '{path:$path,sha256:$sha}'
  done | jq -s '.')"
taker_b_summary=null
total_submission_count=2
if [[ "$taker_b_enabled" == 1 ]]; then
  taker_b_summary="$(<"$M4_ONBOARD_EVIDENCE_ROOT/taker-b-summary.json")"
  total_submission_count=3
fi
jq -n --arg run "$M4_ONBOARD_RUN_ID" --arg channel "$channel" --arg program "$pinned_program_id" \
  --arg runner_sha "$runner_sha" --arg binary_sha "$vault_claim_sha" --arg deployment_sha "$deployment_sha" \
  --argjson maker "$(<"$M4_ONBOARD_EVIDENCE_ROOT/maker-summary.json")" \
  --argjson taker "$(<"$M4_ONBOARD_EVIDENCE_ROOT/taker-summary.json")" \
  --argjson taker_b "$taker_b_summary" --argjson total "$total_submission_count" --argjson raw "$raw" '
  {schema_version:1,kind:"m4_lez_actor_onboarding",result:"passed",flow:"flow_0_fresh_vault_claims",
   run_id:$run,channel_id:$channel,escrow_program_id:$program,
   deployment:{finalized_evidence_sha256:$deployment_sha},
   harness:{path:"scripts/run-m4-lez-actor-onboarding.sh",sha256:$runner_sha},
   vault_claim_binary_sha256:$binary_sha,
   actors:({maker:$maker,taker:$taker} + (if $taker_b == null then {} else {"taker-b":$taker_b} end)),
   raw_evidence:$raw,total_submission_count:$total,automatic_submission_retry:false,
   monero_or_swap_effects_started:false,runtime_external_resources:[],public_rpc_used:false,
   faucet_used:false,private_material_disclosed:false}' >"$M4_ONBOARD_EVIDENCE_ROOT/summary.json.partial"
chmod 0600 "$M4_ONBOARD_EVIDENCE_ROOT/summary.json.partial"
mv "$M4_ONBOARD_EVIDENCE_ROOT/summary.json.partial" "$M4_ONBOARD_EVIDENCE_ROOT/summary.json"
echo "M4 fresh actor Vault Claims passed finalized local proof: $M4_ONBOARD_EVIDENCE_ROOT/summary.json"
