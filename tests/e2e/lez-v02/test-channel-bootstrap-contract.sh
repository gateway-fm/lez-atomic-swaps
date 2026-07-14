#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../../.."

readonly runner="scripts/run-lez-v02-stack.sh"
readonly runtime_channel="b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"
readonly deterministic_seed="0ab865b8054be13810889714c1f1d82c3d8bb2e4510c26d0edc35cc653f306c2"

required_runner_terms=(
  "readonly channel_id=\"${runtime_channel}\""
  "readonly bedrock_signing_key_hex=\"${deterministic_seed}\""
  'bedrock_signing_key'
  'xxd -r -p'
  'bedrock-channel-before-bootstrap'
  'missing_channel_policy="tests/e2e/lez-v02/missing-channel-response.sh"'
  'source "$missing_channel_policy"'
  'lez_v02_is_missing_channel_response'
  'docker start "${containers[sequencer]}"'
  'wait_for_bootstrap_channel'
  '.accredited_keys == [\$public_key]'
  'bedrock-channel-after-bootstrap.json'
  'wait_for_channel_advance'
  'bedrock-channel-after-finality.json'
  '.[1].tip_slot >= .[0].tip_slot'
  '.[1].tip_message != .[0].tip_message'
  'docker start "${containers[indexer]}"'
  'LEZ_V02_CHANNEL_PUBLIC_KEY='
  'finalized_id >= 2'
  'LEZ_V02_READINESS_SCOPE=service-onboarding-finality-non-genesis-and-exact-finalized-actor-preclaim-state'
)
for term in "${required_runner_terms[@]}"; do
  if ! rg -Fq -- "$term" "$runner"; then
    echo "runner is missing the seeded LEZ-to-Bedrock channel onboarding path: ${term}" >&2
    exit 1
  fi
done

sequencer_start_line="$(rg -n -F 'docker start "${containers[sequencer]}"' "$runner" | cut -d: -f1)"
channel_wait_line="$(rg -n -F 'wait_for_bootstrap_channel' "$runner" | tail -n1 | cut -d: -f1)"
indexer_start_line="$(rg -n -F 'docker start "${containers[indexer]}"' "$runner" | cut -d: -f1)"
if [[ -z "$sequencer_start_line" || -z "$channel_wait_line" || -z "$indexer_start_line" ]] ||
   (( sequencer_start_line >= channel_wait_line || channel_wait_line >= indexer_start_line )); then
  echo "runner must onboard in Bedrock -> sequencer -> on-chain channel -> indexer order" >&2
  exit 1
fi

echo "LEZ v0.2 deterministic channel bootstrap contract passed"
