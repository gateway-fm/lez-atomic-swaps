#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export LC_ALL=C
umask 077

fail() {
  echo "M7 XMR accepted-concurrency PoC failed: $*" >&2
  exit 2
}

[[ -z "${M5_XMR_APPLICATION_MODE:-}" || "$M5_XMR_APPLICATION_MODE" == 1 ]] ||
  fail 'M5_XMR_APPLICATION_MODE is fixed to 1'
[[ -z "${M7_XMR_ACCEPTED_CONCURRENCY:-}" || "$M7_XMR_ACCEPTED_CONCURRENCY" == 1 ]] ||
  fail 'M7_XMR_ACCEPTED_CONCURRENCY is fixed to 1'
[[ -z "${M5_XMR_JOURNEY:-}" || "$M5_XMR_JOURNEY" == claim ]] ||
  fail 'M5_XMR_JOURNEY is fixed to claim'
[[ -z "${M7_XMR_SEMANTIC_CLAIM:-}" || "$M7_XMR_SEMANTIC_CLAIM" == 1 ]] ||
  fail 'M7_XMR_SEMANTIC_CLAIM is fixed to 1'

emit_contract() {
  command -v jq >/dev/null || fail 'jq is required to emit the M7 contract'
  jq -n '
    {
      schema_version: 1,
      kind: "m7_xmr_accepted_concurrency_poc_contract",
      execution_performed: false,
      pair: "monero",
      direction: "taker_sells_lez",
      journey: "claim",
      accepted_swap_count: 2,
      shared_application_boundary: {
        maker_daemon_count: 1,
        maker_database_count: 1,
        delivery_directory_count: 1,
        chat_socket_count: 1,
        actor_worker_count: 2
      },
      shared_chain_boundary: {
        lez_v0_2_stack_count: 1,
        monerod_regtest_count: 1,
        program_deployment_count: 1
      },
      isolation: {
        distinct_swap_ids: true,
        distinct_agreements: true,
        distinct_actor_stores: true,
        distinct_role_journals: true,
        distinct_monero_outputs: true,
        distinct_lez_escrows: true
      },
      ordering: {
        both_applications_accepted_before_actor_activation: true,
        both_swaps_in_flight_before_settlement: true
      },
      replay: {
        one_daemon_restart_before_activation: true,
        terminal_resubmission_count: 0
      },
      runtime_external_resources: {
        public_rpc: false,
        public_peer: false,
        faucet: false,
        public_funds: false,
        public_deployment: false,
        monero: "one_isolated_official_0_18_5_1_regtest",
        lez: "one_isolated_v0_2_local_devnet"
      }
    }'
}

if [[ "$#" == 1 && "$1" == contract ]]; then
  emit_contract
  exit 0
fi

export M5_XMR_APPLICATION_MODE=1
export M7_XMR_ACCEPTED_CONCURRENCY=1
export M5_XMR_JOURNEY=claim
export M7_XMR_SEMANTIC_CLAIM=1

exec ./scripts/run-m5-xmr-application-poc.sh "$@"
