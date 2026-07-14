#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../../.."

readonly runner="scripts/run-lez-v02-stack.sh"
readonly contract="compat/lez-v0.2-provisional/local-stack.toml"
readonly maker_account="B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd"
readonly maker_vault="7Mzr43PK9VxpcvwdjgL8PeE4nb2aG9FqBKLfkoH8RBmQ"
readonly maker_allocation="100000"
readonly taker_account="34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib"
readonly taker_vault="AXLjVw4tKTgieQoGRgXMVLVVaB4c5YnL1YTogZdX1cpH"
readonly taker_allocation="200000"

required_runner_terms=(
  "readonly maker_account_id=\"${maker_account}\""
  "readonly maker_vault_account_id=\"${maker_vault}\""
  "readonly maker_genesis_allocation=\"${maker_allocation}\""
  "readonly taker_account_id=\"${taker_account}\""
  "readonly taker_vault_account_id=\"${taker_vault}\""
  "readonly taker_genesis_allocation=\"${taker_allocation}\""
  '.genesis = ['
  '{"supply_account": {"account_id": $maker, "balance": ($maker_amount | tonumber)}},'
  '{"supply_account": {"account_id": $taker, "balance": ($taker_amount | tonumber)}}'
  'generated_actor_genesis_entries'
  'maker and taker genesis identities, Vaults, and allocations must remain distinct'
  'if [[ -e "$run_dir" ]]'
  'refusing to reuse LEZ v0.2 run state:'
  'LEZ_V02_MAKER_ACCOUNT_ID='
  'LEZ_V02_MAKER_VAULT_ACCOUNT_ID='
  'LEZ_V02_MAKER_GENESIS_ALLOCATION='
  'LEZ_V02_TAKER_ACCOUNT_ID='
  'LEZ_V02_TAKER_VAULT_ACCOUNT_ID='
  'LEZ_V02_TAKER_GENESIS_ALLOCATION='
  '"method":"getAccountAtBlock"'
  'LEZ_V02_ACTOR_GENESIS_FINALIZED_BLOCK_ID='
  'LEZ_V02_READINESS_SCOPE=service-onboarding-finality-non-genesis-and-exact-finalized-actor-preclaim-state'
)
for term in "${required_runner_terms[@]}"; do
  if ! rg -Fq -- "$term" "$runner"; then
    echo "runner is missing deterministic independent actor genesis behavior: ${term}" >&2
    exit 1
  fi
done

for value in "$maker_account" "$maker_vault" "$taker_account" "$taker_vault"; do
  if [[ "$(rg -Fo -- "$value" "$runner" | wc -l | tr -d '[:space:]')" != "1" ]]; then
    echo "public actor identifier must occur exactly once in runner constants: ${value}" >&2
    exit 1
  fi
done

if rg -Fq 'maker_private_key' "$runner" ||
   rg -Fq 'taker_private_key' "$runner" ||
   rg -Fq 'actor_private_key' "$runner"; then
  echo "runner must not contain actor private-key material" >&2
  exit 1
fi

required_contract_terms=(
  '[actors.maker]'
  "account_id = \"${maker_account}\""
  "vault_account_id = \"${maker_vault}\""
  "genesis_allocation = ${maker_allocation}"
  '[actors.taker]'
  "account_id = \"${taker_account}\""
  "vault_account_id = \"${taker_vault}\""
  "genesis_allocation = ${taker_allocation}"
  'genesis_policy = "exact_two_public_supply_accounts_fresh_state_only"'
  'actor_genesis_status = "runtime_green_sequencer_and_exact_finalized_indexer_preclaim_state"'
  'indexer_preclaim_binding = "getAccountAtBlock_exact_last_finalized_block_id"'
  'config_sha256 = "3ddeb4d9159cdd584dc9423deaac0897896edfd4cd27d2a509bec08077e1b49d"'
  'builder_sha256 = "7c72530e5ccdb72dda636511dd237b913e5865b18430f5920b50ffb4ade97df3"'
  'faucet_program_sha256 = "4cc6e9fbb404ea03468ccdd886c1d6426de736a5b7ac3564d39d04f58ed33936"'
  'vault_core_sha256 = "36bdae7c0c2dafeea98f97d1964388f0a21203f312b230e603923760c5073846"'
)
for term in "${required_contract_terms[@]}"; do
  if ! rg -Fq -- "$term" "$contract"; then
    echo "local-stack contract is missing an actor genesis invariant: ${term}" >&2
    exit 1
  fi
done

echo "LEZ v0.2 independent actor genesis contract passed"
