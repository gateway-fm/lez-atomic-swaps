#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export LC_ALL=C
umask 077

fail() {
  echo "M7 BTC accepted-concurrency PoC failed: $*" >&2
  exit 2
}

[[ -z "${M5_BTC_APPLICATION_MODE:-}" || "$M5_BTC_APPLICATION_MODE" == 1 ]] ||
  fail 'M5_BTC_APPLICATION_MODE is fixed to 1'
[[ -z "${M7_BTC_ACCEPTED_CONCURRENCY:-}" || "$M7_BTC_ACCEPTED_CONCURRENCY" == 1 ]] ||
  fail 'M7_BTC_ACCEPTED_CONCURRENCY is fixed to 1'
[[ -z "${M3_ACTOR_POC_ASSET_MODE:-}" || "$M3_ACTOR_POC_ASSET_MODE" == native ]] ||
  fail 'M3_ACTOR_POC_ASSET_MODE is fixed to native'
[[ -z "${M3_ACTOR_POC_SCHEDULE:-}" || "$M3_ACTOR_POC_SCHEDULE" == overlap ]] ||
  fail 'M3_ACTOR_POC_SCHEDULE is fixed to overlap'
[[ -z "${M3_ACTOR_POC_JOURNEY:-}" || "$M3_ACTOR_POC_JOURNEY" == claim ]] ||
  fail 'M3_ACTOR_POC_JOURNEY is fixed to claim'

emit_contract() {
  command -v jq >/dev/null || fail 'jq is required to emit the M7 contract'
  jq -n '
    {
      schema_version: 1,
      kind: "m7_btc_accepted_concurrency_poc_contract",
      execution_performed: false,
      application_mode: 1,
      accepted_application_concurrency: true,
      pair: "bitcoin",
      asset_mode: "native",
      schedule: "overlap",
      journey: "claim",
      accepted_swap_count: 2,
      directions: ["taker_sells_foreign", "taker_sells_lez"],
      shared_application_boundary: {
        maker_daemon_count: 1,
        maker_database_count: 1,
        delivery_directory_count: 1,
        chat_socket_count: 1,
        actor_worker_count: 2
      },
      isolation: {
        distinct_swap_ids: true,
        distinct_agreements: true,
        distinct_actor_stores: true,
        distinct_signing_journals: true,
        distinct_escrows: true,
        distinct_deadlines: true
      },
      ordering: {
        both_applications_accepted_before_actor_activation: true,
        both_swaps_locked_before_settlement: true
      },
      restart: {
        one_daemon_restart_before_activation: true,
        no_acceptance_or_actor_replay: true
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
export M7_BTC_ACCEPTED_CONCURRENCY=1
export M3_ACTOR_POC_ASSET_MODE=native
export M3_ACTOR_POC_SCHEDULE=overlap
export M3_ACTOR_POC_JOURNEY=claim

exec ./scripts/run-m3-actor-local-poc.sh "$@"
