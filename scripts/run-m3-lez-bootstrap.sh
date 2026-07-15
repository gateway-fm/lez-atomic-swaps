#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly expected_guest_sha256="a199c5be062adcb27cf63c62d9f5688b37058b4699ce7e1767fd26eeceb5e293"
readonly expected_program_id="39b6a4db85374de9359ea82164ef415019919475f656d597c5ab2231bc104dec"
readonly auth_transfer_program_id="dcbbfebcd59399961ed9973b8307dc475fd4c5ca5779aacfe7588f7dbc3f4a71"

fail() {
  echo "M3 LEZ bootstrap failed: $*" >&2
  exit 2
}

emit_contract() {
  jq -n --arg guest "$expected_guest_sha256" --arg program "$expected_program_id" '
    {
      schema_version: 1,
      kind: "m3_lez_bootstrap_contract",
      verified_artifact_target_required: true,
      canonical_guest_artifact_independently_hashed: true,
      embedded_guest_sha256: $guest,
      escrow_program_id: $program,
      deployment_submission_count: 1,
      deployment_finality: "sequential_indexer_block_id_and_hash",
      fresh_identity_vault_claims: ["maker", "taker"],
      vault_claim_submission_count_per_role: 1,
      vault_claim_finality: "exact_finalized_transaction_and_account_effects",
      finalized_read_retries: "bounded_read_only_never_resubmit",
      evidence_binds_script_binary_manifest_source: true,
      public_rpc_used: false,
      faucet_used: false
    }'
}

if [[ "${1:-}" == "contract" ]]; then
  [[ "$#" == 1 ]] || fail "contract accepts no other arguments"
  command -v jq >/dev/null || fail "jq is required"
  emit_contract
  exit 0
fi
[[ "${1:-}" == "execute" && "$#" == 1 ]] || fail "expected contract or execute"

for command_name in chmod curl date jq mkdir mv readlink sed sha256sum sleep stat; do
  command -v "$command_name" >/dev/null || fail "missing required tool: ${command_name}"
done

required_environment=(
  M3_POC_RUN_ID
  M3_POC_EVIDENCE_DIR
  M3_POC_LEZ_BOOTSTRAP_ROOT
  M3_POC_LEZ_BOOTSTRAP_MANIFEST
  M3_POC_LEZ_MANIFEST
  M3_POC_LEZ_SEQUENCER_RPC_URL
  M3_POC_LEZ_INDEXER_RPC_URL
  M3_POC_LEZ_CHANNEL_ID
  M3_POC_MAKER_LEZ_PRIVATE_KEY
  M3_POC_TAKER_LEZ_PRIVATE_KEY
  M3_POC_VAULT_CLAIM_BIN
  LEZ_V02_ARTIFACT_TARGET_DIR
)
for variable in "${required_environment[@]}"; do
  [[ -n "${!variable:-}" ]] || fail "required environment is missing: ${variable}"
done
for variable in M3_POC_EVIDENCE_DIR M3_POC_LEZ_BOOTSTRAP_ROOT \
  M3_POC_LEZ_BOOTSTRAP_MANIFEST M3_POC_LEZ_MANIFEST \
  M3_POC_MAKER_LEZ_PRIVATE_KEY M3_POC_TAKER_LEZ_PRIVATE_KEY \
  M3_POC_VAULT_CLAIM_BIN LEZ_V02_ARTIFACT_TARGET_DIR; do
  [[ "${!variable}" == /* ]] || fail "path environment must be absolute: ${variable}"
done
for endpoint in "$M3_POC_LEZ_SEQUENCER_RPC_URL" "$M3_POC_LEZ_INDEXER_RPC_URL"; do
  [[ "$endpoint" =~ ^http://127\.0\.0\.1:[1-9][0-9]{0,4}/?$ ]] ||
    fail "LEZ endpoint must be literal-loopback HTTP"
  endpoint_port="${endpoint##*:}"
  endpoint_port="${endpoint_port%/}"
  (( 10#$endpoint_port <= 65535 )) || fail "LEZ endpoint port exceeds 65535"
done
[[ "$M3_POC_LEZ_CHANNEL_ID" =~ ^[0-9a-f]{64}$ &&
   ! "$M3_POC_LEZ_CHANNEL_ID" =~ ^0+$ ]] || fail "LEZ channel identity is invalid"

readonly deployer="${LEZ_V02_ARTIFACT_TARGET_DIR}/debug/lez-zec-escrow-v02-deployer"
readonly guest_elf="${LEZ_V02_ARTIFACT_TARGET_DIR}/riscv-guest/lez-zec-escrow-v02-methods/lez-zec-escrow-v02-guest/riscv32im-risc0-zkvm-elf/docker/zec_escrow_v02.bin"
readonly deployer_manifest="compat/lez-v0.2-provisional/escrow/deployer/Cargo.toml"
readonly deployer_source="compat/lez-v0.2-provisional/escrow/deployer/src/main.rs"
readonly guest_manifest="compat/lez-v0.2-provisional/escrow/methods/guest/Cargo.toml"
readonly guest_source="compat/lez-v0.2-provisional/escrow/methods/guest/src/main.rs"
readonly vault_manifest="compat/lez-v0_2-sidecar/Cargo.toml"
readonly vault_source="compat/lez-v0_2-sidecar/src/bin/lez-v02-vault-claim-poc.rs"
[[ -d "$LEZ_V02_ARTIFACT_TARGET_DIR" && ! -L "$LEZ_V02_ARTIFACT_TARGET_DIR" ]] ||
  fail "verified artifact target is missing or unsafe"
for binary in "$deployer" "$M3_POC_VAULT_CLAIM_BIN"; do
  [[ -x "$binary" && -f "$binary" && ! -L "$binary" ]] ||
    fail "required LEZ bootstrap binary is missing or unsafe: ${binary}"
  [[ "$(readlink -f "$binary")" == "$binary" ]] || fail "bootstrap binary path is not canonical"
done
[[ -f "$guest_elf" && ! -L "$guest_elf" && "$(readlink -f "$guest_elf")" == "$guest_elf" ]] ||
  fail "canonical checked guest ELF is missing or unsafe"
guest_elf_sha256="$(sha256sum "$guest_elf" | sed 's/ .*//')"
readonly guest_elf_sha256
[[ "$guest_elf_sha256" == "$expected_guest_sha256" ]] ||
  fail "independent checked guest ELF SHA-256 does not match the pinned artifact"
for source_file in "$deployer_manifest" "$deployer_source" "$guest_manifest" "$guest_source" \
  "$vault_manifest" "$vault_source"; do
  [[ -f "$source_file" && ! -L "$source_file" ]] ||
    fail "bootstrap source identity is missing or unsafe: ${source_file}"
done
[[ ! -e "$M3_POC_LEZ_BOOTSTRAP_ROOT" && ! -L "$M3_POC_LEZ_BOOTSTRAP_ROOT" ]] ||
  fail "refusing to reuse LEZ bootstrap state"
[[ ! -e "$M3_POC_LEZ_BOOTSTRAP_MANIFEST" && ! -L "$M3_POC_LEZ_BOOTSTRAP_MANIFEST" ]] ||
  fail "refusing to overwrite LEZ bootstrap manifest"

mkdir -m 0700 "$M3_POC_LEZ_BOOTSTRAP_ROOT"
readonly deployment_evidence="${M3_POC_EVIDENCE_DIR}/lez-deployment.json"
readonly bootstrap_evidence="${M3_POC_EVIDENCE_DIR}/lez-bootstrap.json"

manifest_value() {
  local key="$1"
  local -a values=()
  mapfile -t values < <(sed -n "s/^${key}=//p" "$M3_POC_LEZ_MANIFEST")
  [[ "${#values[@]}" == 1 && -n "${values[0]}" ]] ||
    fail "LEZ manifest does not contain exactly one ${key}"
  printf '%s\n' "${values[0]}"
}

rpc() {
  local endpoint="$1"
  local request="$2"
  curl --fail --silent --show-error --noproxy '*' \
    --connect-timeout 2 --max-time 30 -H 'content-type: application/json' \
    --data "$request" "$endpoint"
}

finalized_tip() {
  local response tip
  for _ in {1..120}; do
    if response="$(rpc "$M3_POC_LEZ_INDEXER_RPC_URL" \
      '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' 2>/dev/null)" &&
      tip="$(jq -er '.result | numbers' <<<"$response" 2>/dev/null)"; then
      printf '%s\n' "$tip"
      return 0
    fi
    sleep 0.25
  done
  fail "finalized tip remained unavailable after bounded read-only retries"
}

rpc_read_file() {
  local endpoint="$1" request="$2" output="$3"
  local partial="${output}.partial"
  for _ in {1..120}; do
    if rpc "$endpoint" "$request" >"$partial" 2>/dev/null &&
      jq -e '.error == null and .result != null' "$partial" >/dev/null 2>&1; then
      chmod 0600 "$partial"
      mv "$partial" "$output"
      return 0
    fi
    sleep 0.25
  done
  fail "bounded read-only RPC remained unavailable: ${output}"
}

prove_finalized_transaction() {
  local label="$1"
  local transaction_id="$2"
  local start_height="$3"
  local cursor=$((start_height + 1))
  local tip height block_file block_hash hash_file occurrences containing_file=""
  local count=0 containing_height=0
  [[ "$transaction_id" =~ ^[0-9a-f]{64}$ ]] || fail "${label} transaction ID is invalid"
  for _ in {1..1200}; do
    tip="$(finalized_tip)"
    (( tip - start_height <= 4096 )) || fail "${label} finality scan exceeded 4096 blocks"
    while (( cursor <= tip )); do
      height="$cursor"
      block_file="${M3_POC_EVIDENCE_DIR}/${label}-finalized-block-${height}.json"
      rpc_read_file "$M3_POC_LEZ_INDEXER_RPC_URL" \
        "$(jq -cn --argjson height "$height" \
          '{jsonrpc:"2.0",id:1,method:"getBlockById",params:[$height]}')" "$block_file"
      occurrences="$(jq -er --arg tx "$transaction_id" \
        '[.result.body.transactions[] | select(.Public.hash == $tx)] | length' "$block_file")"
      if (( occurrences > 0 )); then
        count=$((count + occurrences))
        containing_height="$height"
        containing_file="$block_file"
      fi
      cursor=$((cursor + 1))
    done
    if (( count > 0 )); then
      break
    fi
    sleep 0.25
  done
  [[ "$count" == 1 && "$containing_height" != 0 ]] ||
    fail "${label} was not found exactly once in the bounded finalized window"
  block_hash="$(jq -er '.result.header.hash | strings' "$containing_file")"
  jq -e --argjson height "$containing_height" --arg hash "$block_hash" '
    .result.header.block_id == $height
    and .result.header.hash == $hash
    and .result.bedrock_status == "Finalized"
  ' "$containing_file" >/dev/null || fail "${label} containing block is not finalized"
  hash_file="${M3_POC_EVIDENCE_DIR}/${label}-finalized-block-by-hash.json"
  rpc_read_file "$M3_POC_LEZ_INDEXER_RPC_URL" \
    "$(jq -cn --arg hash "$block_hash" \
      '{jsonrpc:"2.0",id:1,method:"getBlockByHash",params:[$hash]}')" "$hash_file"
  [[ "$(jq -S -c '.result' "$containing_file")" == "$(jq -S -c '.result' "$hash_file")" ]] ||
    fail "${label} finalized block ID/hash lookups disagree"
  jq -n --arg label "$label" --arg tx "$transaction_id" \
    --argjson start "$start_height" --argjson tip "$tip" \
    --argjson block "$containing_height" --arg hash "$block_hash" '
    {schema_version:1,label:$label,transaction_id:$tx,window:{start_height:($start + 1),finalized_tip:$tip},occurrences:1,containing_block_id:$block,containing_block_hash:$hash,bedrock_status:"Finalized",id_hash_lookups_equal:true}' \
    >"${M3_POC_EVIDENCE_DIR}/${label}-finality.json"
  chmod 0600 "${M3_POC_EVIDENCE_DIR}/${label}-finality.json"
  printf '%s\n' "$containing_height"
}

deployment_start="$(finalized_tip)"
"$deployer" deploy-local --rpc-url "$M3_POC_LEZ_SEQUENCER_RPC_URL" \
  --channel-id "$M3_POC_LEZ_CHANNEL_ID" --timeout-seconds 300 >"$deployment_evidence"
chmod 0600 "$deployment_evidence"
jq -e --arg rpc "$M3_POC_LEZ_SEQUENCER_RPC_URL" \
  --arg channel "$M3_POC_LEZ_CHANNEL_ID" --arg guest "$expected_guest_sha256" \
  --arg program "$expected_program_id" '
  .schema_version == 1
  and .preflight.rpc_url == $rpc
  and .preflight.channel_id == $channel
  and .preflight.elf_sha256 == $guest
  and .preflight.image_id == $program
  and (.transaction_hash | test("^[0-9a-f]{64}$"))
  and .inclusion_block_id > .preflight.last_block_id
  and (.inclusion_block_hash | test("^[0-9a-f]{64}$"))
' "$deployment_evidence" >/dev/null || fail "checked guest deployment evidence is invalid"
deployment_tx="$(jq -er '.transaction_hash' "$deployment_evidence")"
deployment_block="$(prove_finalized_transaction lez-deployment "$deployment_tx" "$deployment_start")"
[[ "$deployment_block" == "$(jq -er '.inclusion_block_id' "$deployment_evidence")" ]] ||
  fail "deployment sequencer and finalized indexer block IDs disagree"
[[ "$(jq -er '.containing_block_hash' "${M3_POC_EVIDENCE_DIR}/lez-deployment-finality.json")" == \
   "$(jq -er '.inclusion_block_hash' "$deployment_evidence")" ]] ||
  fail "deployment sequencer and finalized indexer block hashes disagree"

claim_vault_for_role() {
  local role="$1"
  local private_key allocation owner_id vault_id role_root claim_evidence claim_start claim_tx
  local claim_block owner_output vault_output
  case "$role" in
    maker)
      private_key="$M3_POC_MAKER_LEZ_PRIVATE_KEY"
      allocation="$(manifest_value LEZ_V02_MAKER_GENESIS_ALLOCATION)"
      owner_id="$(manifest_value LEZ_V02_MAKER_ACCOUNT_ID)"
      vault_id="$(manifest_value LEZ_V02_MAKER_VAULT_ACCOUNT_ID)"
      ;;
    taker)
      private_key="$M3_POC_TAKER_LEZ_PRIVATE_KEY"
      allocation="$(manifest_value LEZ_V02_TAKER_GENESIS_ALLOCATION)"
      owner_id="$(manifest_value LEZ_V02_TAKER_ACCOUNT_ID)"
      vault_id="$(manifest_value LEZ_V02_TAKER_VAULT_ACCOUNT_ID)"
      ;;
  esac
  role_root="${M3_POC_LEZ_BOOTSTRAP_ROOT}/${role}"
  mkdir -m 0700 "$role_root"
  claim_evidence="${M3_POC_EVIDENCE_DIR}/${role}-vault-claim.json"
  claim_start="$(finalized_tip)"
  "$M3_POC_VAULT_CLAIM_BIN" --role "$role" --run-id "$M3_POC_RUN_ID" \
    --request-id "${role}-vault-claim-0001" --state-directory "$role_root" \
    --private-key-file "$private_key" --sequencer-url "$M3_POC_LEZ_SEQUENCER_RPC_URL" \
    --chain-id "$M3_POC_LEZ_CHANNEL_ID" --escrow-program-id "$expected_program_id" \
    --allocation "$allocation" >"$claim_evidence"
  chmod 0600 "$claim_evidence"
  jq -e --arg role "$role" --argjson allocation "$allocation" '
    .schema == "lez_v02_vault_claim_poc_v1"
    and .role == $role
    and .allocation == $allocation
    and .submission.decision == "admitted"
    and .durable_state == "admitted"
    and .durable_attempt_count == 1
    and .before.owner.balance == 0
    and .before.vault.balance == $allocation
    and (.transaction_id | test("^[0-9a-f]{64}$"))
  ' "$claim_evidence" >/dev/null || fail "${role} Vault Claim evidence is invalid"
  claim_tx="$(jq -er '.transaction_id' "$claim_evidence")"
  claim_block="$(prove_finalized_transaction "${role}-vault-claim" "$claim_tx" "$claim_start")"
  owner_output="${M3_POC_EVIDENCE_DIR}/${role}-owner-after-vault-claim.json"
  vault_output="${M3_POC_EVIDENCE_DIR}/${role}-vault-after-vault-claim.json"
  rpc_read_file "$M3_POC_LEZ_INDEXER_RPC_URL" \
    "$(jq -cn --arg account "$owner_id" --argjson block "$claim_block" \
      '{jsonrpc:"2.0",id:1,method:"getAccountAtBlock",params:[$account,$block]}')" "$owner_output"
  rpc_read_file "$M3_POC_LEZ_INDEXER_RPC_URL" \
    "$(jq -cn --arg account "$vault_id" --argjson block "$claim_block" \
      '{jsonrpc:"2.0",id:1,method:"getAccountAtBlock",params:[$account,$block]}')" "$vault_output"
  jq -e --argjson allocation "$allocation" '.result.balance == $allocation and .result.nonce == 1' \
    "$owner_output" >/dev/null || fail "${role} finalized owner Claim effect is invalid"
  jq -e '.result.balance == 0 and .result.nonce == 0' "$vault_output" >/dev/null ||
    fail "${role} finalized Vault Claim effect is invalid"
}

for role in maker taker; do
  claim_vault_for_role "$role"
done

deployer_sha="$(sha256sum "$deployer" | sed 's/ .*//')"
vault_claim_sha="$(sha256sum "$M3_POC_VAULT_CLAIM_BIN" | sed 's/ .*//')"
bootstrap_script_sha="$(sha256sum scripts/run-m3-lez-bootstrap.sh | sed 's/ .*//')"
deployer_manifest_sha="$(sha256sum "$deployer_manifest" | sed 's/ .*//')"
deployer_source_sha="$(sha256sum "$deployer_source" | sed 's/ .*//')"
guest_manifest_sha="$(sha256sum "$guest_manifest" | sed 's/ .*//')"
guest_source_sha="$(sha256sum "$guest_source" | sed 's/ .*//')"
vault_manifest_sha="$(sha256sum "$vault_manifest" | sed 's/ .*//')"
vault_source_sha="$(sha256sum "$vault_source" | sed 's/ .*//')"
genesis_hash="$(jq -er '.preflight.genesis_block_hash' "$deployment_evidence")"
jq -n --arg run "$M3_POC_RUN_ID" --arg guest "$expected_guest_sha256" \
  --arg program "$expected_program_id" --arg genesis "$genesis_hash" \
  --arg deployment_tx "$deployment_tx" --argjson deployment_block "$deployment_block" \
  --arg bootstrap_script_sha "$bootstrap_script_sha" \
  --arg guest_path "$guest_elf" --arg guest_manifest "$guest_manifest" \
  --arg guest_manifest_sha "$guest_manifest_sha" --arg guest_source "$guest_source" \
  --arg guest_source_sha "$guest_source_sha" --arg deployer_path "$deployer" \
  --arg deployer_sha "$deployer_sha" --arg deployer_manifest "$deployer_manifest" \
  --arg deployer_manifest_sha "$deployer_manifest_sha" --arg deployer_source "$deployer_source" \
  --arg deployer_source_sha "$deployer_source_sha" \
  --arg vault_path "$M3_POC_VAULT_CLAIM_BIN" --arg vault_claim_sha "$vault_claim_sha" \
  --arg vault_manifest "$vault_manifest" --arg vault_manifest_sha "$vault_manifest_sha" \
  --arg vault_source "$vault_source" --arg vault_source_sha "$vault_source_sha" '
  {schema_version:1,kind:"m3_lez_bootstrap",result:"passed",run_id:$run,
   harness:{path:"scripts/run-m3-lez-bootstrap.sh",sha256:$bootstrap_script_sha},
   guest:{artifact_path:$guest_path,elf_sha256:$guest,independently_hashed:true,
     program_id:$program,deployer_sha256:$deployer_sha,
     source_identity:{manifest:$guest_manifest,manifest_sha256:$guest_manifest_sha,
       entrypoint:$guest_source,entrypoint_sha256:$guest_source_sha}},
   deployer:{binary_path:$deployer_path,binary_sha256:$deployer_sha,
     source_identity:{manifest:$deployer_manifest,manifest_sha256:$deployer_manifest_sha,
       entrypoint:$deployer_source,entrypoint_sha256:$deployer_source_sha}},
   runtime:{genesis_block_hash:$genesis},
   deployment:{transaction_id:$deployment_tx,finalized_block_id:$deployment_block,submission_count:1},
   vault_claims:{maker:{submission_count:1,finalized_account_effect:true},
     taker:{submission_count:1,finalized_account_effect:true},
     binary_path:$vault_path,binary_sha256:$vault_claim_sha,
     source_identity:{manifest:$vault_manifest,manifest_sha256:$vault_manifest_sha,
       entrypoint:$vault_source,entrypoint_sha256:$vault_source_sha}},
   public_rpc_used:false,faucet_used:false,private_material_disclosed:false}' >"$bootstrap_evidence"
chmod 0600 "$bootstrap_evidence"

{
  printf 'M3_POC_LEZ_ESCROW_PROGRAM_ID=%s\n' "$expected_program_id"
  printf 'M3_POC_LEZ_AUTH_TRANSFER_PROGRAM_ID=%s\n' "$auth_transfer_program_id"
  printf 'M3_POC_LEZ_GENESIS_BLOCK_HASH=%s\n' "$genesis_hash"
  printf 'M3_POC_LEZ_DEPLOYMENT_TRANSACTION_ID=%s\n' "$deployment_tx"
  printf 'M3_POC_LEZ_BOOTSTRAP_EVIDENCE=%s\n' "$bootstrap_evidence"
  printf 'M3_POC_LEZ_DEPLOYER_SHA256=%s\n' "$deployer_sha"
  printf 'M3_POC_LEZ_VAULT_CLAIM_SHA256=%s\n' "$vault_claim_sha"
} >"${M3_POC_LEZ_BOOTSTRAP_MANIFEST}.partial"
chmod 0600 "${M3_POC_LEZ_BOOTSTRAP_MANIFEST}.partial"
mv "${M3_POC_LEZ_BOOTSTRAP_MANIFEST}.partial" "$M3_POC_LEZ_BOOTSTRAP_MANIFEST"

echo "M3 LEZ checked deployment and fresh-identity Vault bootstrap passed: ${bootstrap_evidence}"
