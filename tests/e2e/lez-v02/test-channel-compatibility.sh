#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../../.."

source_dir="${LEZ_V02_SOURCE_DIR:-/tmp/lez-v020-native-investigation}"
runner="scripts/run-lez-v02-stack.sh"
readonly upstream_channel="0101010101010101010101010101010101010101010101010101010101010101"
readonly genesis_channel="0000000000000000000000000000000000000000000000000000000000000000"
readonly runtime_channel="b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"
readonly deterministic_seed="0ab865b8054be13810889714c1f1d82c3d8bb2e4510c26d0edc35cc653f306c2"
readonly upstream_genesis_time_hex="2c04626900000000"

sequencer_channel="$(jq -r '.bedrock_config.channel_id' \
  "${source_dir}/lez/sequencer/service/configs/docker/sequencer_config.json")"
indexer_channel="$(jq -r '.channel_id' \
  "${source_dir}/lez/indexer/service/configs/docker/indexer_config.json")"
if [[ "$sequencer_channel" != "$upstream_channel" || "$indexer_channel" != "$upstream_channel" ]]; then
  echo "expected the locked upstream LEZ v0.2 Docker configs to select channel 01" >&2
  exit 1
fi

if ! rg -Fq "channel_id: '${genesis_channel}'" \
    "${source_dir}/bedrock/deployment-settings.yaml"; then
  echo "expected the locked Bedrock genesis fixture to inscribe channel 00" >&2
  exit 1
fi
if ! rg -Fq "$upstream_genesis_time_hex" \
    "${source_dir}/bedrock/deployment-settings.yaml"; then
  echo "expected the locked Bedrock fixture to embed its stale January 2026 genesis time" >&2
  exit 1
fi

required_runner_terms=(
  "readonly channel_id=\"${runtime_channel}\""
  "readonly genesis_channel_id=\"${genesis_channel}\""
  "readonly bedrock_signing_key_hex=\"${deterministic_seed}\""
  "readonly upstream_lez_channel_id=\"${upstream_channel}\""
  'jq --arg channel "$channel_id"'
  '.channel_id = $channel'
  '.bedrock_config.channel_id = $channel'
  'LEZ_V02_UPSTREAM_CHANNEL_MISMATCH=%s'
  '"$upstream_lez_channel_id" >>"$manifest"'
  'readonly upstream_genesis_time_hex="2c04626900000000"'
  'genesis_time_hex='
  's/${upstream_genesis_time_hex}/${genesis_hex}/'
)

for term in "${required_runner_terms[@]}"; do
  if ! rg -Fq -- "$term" "$runner"; then
    echo "runner does not preserve genesis-00 while binding the seeded runtime channel: ${term}" >&2
    exit 1
  fi
done

echo "LEZ v0.2 channel compatibility contract passed"
