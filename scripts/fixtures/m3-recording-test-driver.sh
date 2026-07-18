#!/usr/bin/env bash
set -euo pipefail

[[ "${M3_RECORDING_TESTING:-0}" == 1 ]] || {
  echo "the M3 recording fixture is test-only" >&2
  exit 64
}

evidence_file="${M3_RECORDING_TEST_EVIDENCE_FILE:?missing test evidence file}"
mkdir -p -- "$(dirname "$evidence_file")"

echo "M3 private demo: ${M3_RECORDING_SCENARIO}"
echo "actors: independent maker and taker"
echo "networks: Bitcoin Core Regtest and LEZ private-local"

if [[ "${M3_RECORDING_TEST_FAIL:-0}" == 1 ]]; then
  echo "intentional recording-driver failure" >&2
  exit 73
fi

case "${M3_RECORDING_SCENARIO}" in
  happy)
    packet_kind="m3_actor_two_direction_local_poc"
    journey="claim"
    schedule="sequential"
    slot_duration_seconds="1.0"
    ;;
  refund)
    packet_kind="m3_actor_two_direction_refund_local_poc"
    journey="refund"
    schedule="sequential"
    slot_duration_seconds="3.0"
    ;;
  concurrent)
    packet_kind="m3_actor_overlapping_two_swap_local_poc"
    journey="claim"
    schedule="overlap"
    slot_duration_seconds="1.0"
    ;;
  *)
    echo "unsupported fixture scenario" >&2
    exit 64
    ;;
esac

jq -n \
  --arg kind "$packet_kind" \
  --arg journey "$journey" \
  --arg schedule "$schedule" \
  --arg run_id "${RUN_ID}" \
  --arg bitcoin_run_id "${RUN_ID}-btc" \
  --arg lez_run_id "${RUN_ID}-lez" \
  --arg slot_duration_seconds "$slot_duration_seconds" \
  --arg repository_commit "${M3_RECORDING_TEST_COMMIT}" '
  {
    schema_version: 1,
    kind: $kind,
    journey: $journey,
    schedule: $schedule,
    result: "passed",
    run_id: $run_id,
    repository_commit: $repository_commit,
    services: {
      bitcoin_core: {
        run_id: $bitcoin_run_id,
        version: "31.1",
        network: "regtest"
      },
      lez: {
        run_id: $lez_run_id,
        version: "v0.2.0",
        network: "private_local",
        slot_duration_seconds: $slot_duration_seconds
      }
    },
    external_resources: {
      public_rpc: false,
      faucet: false,
      public_funds: false,
      certification_success_depends_on_external_network: false
    }
  }
' >"$evidence_file"

echo "M3 private demo passed"
