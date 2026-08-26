#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export LC_ALL=C
umask 077

fail() {
  echo "M5 BTC application PoC failed: $*" >&2
  exit 2
}

[[ -z "${M5_BTC_APPLICATION_MODE:-}" || "$M5_BTC_APPLICATION_MODE" == 1 ]] ||
  fail 'M5_BTC_APPLICATION_MODE is fixed to 1'
[[ -z "${M3_ACTOR_POC_ASSET_MODE:-}" || "$M3_ACTOR_POC_ASSET_MODE" == native ]] ||
  fail 'M3_ACTOR_POC_ASSET_MODE is fixed to native'
[[ -z "${M3_ACTOR_POC_SCHEDULE:-}" || "$M3_ACTOR_POC_SCHEDULE" == sequential ]] ||
  fail 'M3_ACTOR_POC_SCHEDULE is fixed to sequential'
[[ -z "${M3_ACTOR_POC_JOURNEY:-}" || "$M3_ACTOR_POC_JOURNEY" == claim ]] ||
  fail 'M3_ACTOR_POC_JOURNEY is fixed to claim'

emit_contract() {
  command -v jq >/dev/null || fail 'jq is required to emit the M5 BTC contract'
  jq -n '
    {
      schema_version: 1,
      kind: "m5_btc_application_poc_contract",
      execution_performed: false,
      application_mode: 1,
      pair: "bitcoin",
      direction: "taker_sells_foreign",
      asset_mode: "native",
      schedule: "sequential",
      journey: "claim",
      application_order: [
        "delivery_only_daemon",
        "maker_publish",
        "taker_plan",
        "derived_swap_id",
        "stage_two",
        "canonical_draft_export",
        "authorized_daemon_and_taker",
        "role_fixed_provisioning",
        "actual_role_actors"
      ],
      role_model: {
        maker: "real_cli_daemon_and_role_fixed_actor",
        taker: "real_cli_and_role_fixed_actor",
        shared_private_authority: false
      },
      runtime_external_resources: {
        public_rpc: false,
        faucet: false,
        public_funds: false,
        bitcoin_core: "isolated_regtest",
        lez: "isolated_v0_2",
        test_funds: "deterministic_local_genesis_and_regtest_outputs"
      }
    }'
}

if [[ "$#" == 1 && "$1" == contract ]]; then
  emit_contract
  exit 0
fi

[[ -n "${M5_LEZ_DEPLOYER_SHA256:-}" ]] ||
  fail 'M5_LEZ_DEPLOYER_SHA256 is required'
[[ "$M5_LEZ_DEPLOYER_SHA256" =~ ^[0-9a-f]{64}$ ]] ||
  fail 'M5_LEZ_DEPLOYER_SHA256 must be a lowercase SHA-256 digest'

export M5_BTC_APPLICATION_MODE=1
export M3_ACTOR_POC_ASSET_MODE=native
export M3_ACTOR_POC_SCHEDULE=sequential
export M3_ACTOR_POC_JOURNEY=claim

# The delegated runner owns unique run IDs, endpoint tuple locks, process
# registration, exact-node evidence, and scoped cleanup.
exec ./scripts/run-m3-actor-local-poc.sh "$@"
