#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly runner="scripts/run-bitcoin-core-e2e.sh"
readonly provenance_verifier="scripts/verify-bitcoin-core-release.sh"

fail() {
  echo "Bitcoin Core service-mode policy failed: $*" >&2
  exit 1
}

require_fixed() {
  local needle="$1"
  rg -Fq -- "$needle" "$runner" || fail "runner is missing: ${needle}"
}

require_provenance_fixed() {
  local needle="$1"
  rg -Fq -- "$needle" "$provenance_verifier" ||
    fail "provenance verifier is missing: ${needle}"
}

[[ -x "$runner" ]] || fail "runner is missing or not executable"
[[ -x "$provenance_verifier" ]] || fail "provenance verifier is missing or not executable"
bash -n "$runner"
bash -n "$provenance_verifier"

for term in \
  'required_commands=(awk chmod cp curl date diff git gpg gpgconf' \
  'cleanup_gpg_agent()' \
  'gpgconf --homedir "$gnupg_home" --kill gpg-agent' \
  'trap cleanup_gpg_agent EXIT'; do
  require_provenance_fixed "$term"
done

invalid_run_id="service-mode-invalid-$$"
if invalid_output="$(
  RUN_ID="$invalid_run_id" BITCOIN_CORE_E2E_MODE=invalid "$runner" 2>&1
)"; then
  fail "an invalid mode reached the runtime"
fi
if [[ "$invalid_output" != *"BITCOIN_CORE_E2E_MODE must be fixture or service"* ]]; then
  fail "invalid mode did not fail with the explicit validation error"
fi
if [[ -e ".e2e/${invalid_run_id}" || -L ".e2e/${invalid_run_id}" ]]; then
  fail "invalid mode created run state before validation"
fi

required_contract_terms=(
  'mode="${BITCOIN_CORE_E2E_MODE:-fixture}"'
  'if [[ "$mode" != "fixture" && "$mode" != "service" ]]; then'
  'if [[ "$mode" == "fixture" ]]; then'
  'if [[ "$mode" == "service" ]]; then'
  'finish_service_mode'
  'scope: "bitcoin_core_service_provision"'
  'p2tr_fixture_lifecycle_executed: false'
  'p2tr_fixture_proof_claimed: false'
  'adaptor_signature_proof_claimed: false'
  'scalar_extraction_proof_claimed: false'
  'lez_composition_proof_claimed: false'
  'atomicity_proof_claimed: false'
  'BITCOIN_CORE_FUNDING_SECRET_KEY_HEX=%s'
  'BITCOIN_CORE_MAKER_CURL_CONFIG=%s'
  'BITCOIN_CORE_TAKER_CURL_CONFIG=%s'
  'BITCOIN_CORE_MAKER_BASIC_CREDENTIALS=%s'
  'BITCOIN_CORE_TAKER_BASIC_CREDENTIALS=%s'
  'BITCOIN_CORE_FUNDING_CREDENTIALS=%s'
  'BITCOIN_CORE_RPC_URL=%s'
  'rpc_publication: "dynamic_literal_loopback_only"'
  'runtime_external_resources: []'
  'public_rpc_used: false'
  'faucet_used: false'
  'public_funds_used: false'
  'BITCOIN_CORE_E2E_KEEP_RUNNING:-0'
)
for term in "${required_contract_terms[@]}"; do
  require_fixed "$term"
done

mature_line="$(
  rg -n -F 'core_cli gettxout "$coinbase_txid" "$coinbase_vout" >"${evidence_dir}/mature-funding.json"' \
    "$runner" | cut -d: -f1
)"
service_exit_line="$(
  rg -n -F 'if [[ "$mode" == "service" ]]; then' "$runner" | cut -d: -f1
)"
p2tr_line="$(
  rg -n -F 'core_cli getmempoolinfo >"${evidence_dir}/pre-p2tr-mempool.json"' \
    "$runner" | cut -d: -f1
)"
if [[ ! "$mature_line" =~ ^[0-9]+$ || ! "$service_exit_line" =~ ^[0-9]+$ ||
      ! "$p2tr_line" =~ ^[0-9]+$ ]]; then
  fail "could not locate the provisioning/service/P2TR split"
fi
if (( mature_line >= service_exit_line || service_exit_line >= p2tr_line )); then
  fail "service mode must exit after mature funding and before the P2TR fixture lifecycle"
fi

service_body="$(sed -n '/^finish_service_mode() {$/,/^}$/p' "$runner")"
if [[ -z "$service_body" ]]; then
  fail "service evidence function is missing"
fi
if rg -q \
  'contract_evidence|funding_transaction_evidence|cooperative_spend_evidence|helper_sha256' \
  <<<"$service_body"; then
  fail "service evidence must not consume or imply fixture transaction evidence"
fi
if rg -q 'fixture_[a-z_]+:[[:space:]]*true' <<<"$service_body"; then
  fail "service evidence must never assert a fixture proof"
fi

echo "Bitcoin Core service-mode policy is complete"
