#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fixture_dir="tests/e2e/lez-v02"
compose_file="${fixture_dir}/compose.yml"
dockerfile="${fixture_dir}/Dockerfile"
cryptarchia_policy="${fixture_dir}/cryptarchia-advanced.jq"
readiness_test="${fixture_dir}/test-readiness-policy.sh"
channel_test="${fixture_dir}/test-channel-compatibility.sh"
bootstrap_test="${fixture_dir}/test-channel-bootstrap-contract.sh"
missing_channel_policy="${fixture_dir}/missing-channel-response.sh"
missing_channel_test="${fixture_dir}/test-missing-channel-response.sh"
runner="scripts/run-lez-v02-stack.sh"

for required_file in "$compose_file" "$dockerfile" "$cryptarchia_policy" "$readiness_test" "$channel_test" "$bootstrap_test" "$missing_channel_policy" "$missing_channel_test" "$runner"; do
  if [[ ! -f "$required_file" ]]; then
    echo "missing isolated LEZ v0.2 fixture: ${required_file}" >&2
    exit 1
  fi
done

RUN_ID=policy-check \
LEZ_V02_IMAGE=lez-atomic-swaps-lez-v02:policy-check \
LEZ_V02_SOURCE_DIR=/tmp/lez-v020-native-investigation \
LEZ_V02_RUN_DIR=/tmp/lez-v02-policy-check \
LEZ_V02_UID=65532 \
LEZ_V02_GID=65532 \
docker compose \
  --project-name lez-atomic-swaps-lez-v02-policy-check \
  --file "$compose_file" config --quiet

required_compose_terms=(
  'ghcr.io/logos-blockchain/logos-blockchain@sha256:91d6c5bf07e07fcfba5e7cf07d21ee686a6bc4b9f6210f2d28bffbcad9a3729f'
  '${LEZ_V02_IMAGE:?LEZ_V02_IMAGE is required}'
  'expose: ["18080"]'
  'expose: ["3040"]'
  'expose: ["8779"]'
  'org.logos-co.atomic-swaps.run'
  'user: "${LEZ_V02_UID:?LEZ_V02_UID is required}:${LEZ_V02_GID:?LEZ_V02_GID is required}"'
  'read_only: true'
  'cap_drop: ["ALL"]'
  'security_opt: ["no-new-privileges:true"]'
  'pids_limit:'
  'mem_limit:'
  'cpus:'
  'com.docker.network.bridge.enable_ip_masquerade: "false"'
  '${LEZ_V02_SOURCE_DIR:?LEZ_V02_SOURCE_DIR is required}/bedrock:/opt/lez-v0.2-source/bedrock:ro'
  '${LEZ_V02_RUN_DIR:?LEZ_V02_RUN_DIR is required}'
)

for term in "${required_compose_terms[@]}"; do
  if ! rg -Fq -- "$term" "$compose_file"; then
    echo "LEZ v0.2 Compose fixture is missing isolation control: ${term}" >&2
    exit 1
  fi
done

if rg -q '^\s*container_name:' "$compose_file"; then
  echo "fixed Docker container names are forbidden" >&2
  exit 1
fi

if rg -q '^\s*ports:' "$compose_file"; then
  echo "Compose host publication is forbidden; the runner owns effective ephemeral bindings" >&2
  exit 1
fi

if rg -q '^\s*cap_add:' "$compose_file"; then
  echo "LEZ v0.2 services must not regain Linux capabilities" >&2
  exit 1
fi

required_dockerfile_terms=(
  'gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7'
  'COPY --chmod=0555 sequencer_service /usr/local/bin/sequencer_service'
  'COPY --chmod=0555 indexer_service /usr/local/bin/indexer_service'
  'COPY --chmod=0555 r0vm /usr/local/bin/r0vm'
  'ENV RISC0_SERVER_PATH=/usr/local/bin/r0vm'
  'USER 65532:65532'
)

for term in "${required_dockerfile_terms[@]}"; do
  if ! rg -Fq -- "$term" "$dockerfile"; then
    echo "LEZ v0.2 Dockerfile is missing provenance/runtime control: ${term}" >&2
    exit 1
  fi
done

required_runner_terms=(
  'project="lez-atomic-swaps-lez-v02-${run_id}"'
  '^[a-z0-9][a-z0-9_-]{0,63}$'
  'run_dir="$(pwd)/.e2e/${run_id}/lez-v02"'
  'refusing to reuse LEZ v0.2 run state'
  'docker compose --project-name "$project"'
  'docker port "$container_id"'
  '${service}-effective-ports.json'
  'cryptarchia_policy="tests/e2e/lez-v02/cryptarchia-advanced.jq"'
  '-f "$cryptarchia_policy"'
  'docker network create'
  '--opt com.docker.network.bridge.enable_ip_masquerade=false'
  '--publish "127.0.0.1::18080"'
  '--publish "127.0.0.1::3040"'
  '--publish "127.0.0.1::8779"'
  'docker container rm --force "$container_id"'
  'docker network rm "$network"'
  '/cryptarchia/info'
  'readonly channel_id="b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"'
  'readonly genesis_channel_id="0000000000000000000000000000000000000000000000000000000000000000"'
  'readonly bedrock_signing_key_hex="0ab865b8054be13810889714c1f1d82c3d8bb2e4510c26d0edc35cc653f306c2"'
  '/channel/${channel_id}'
  '"method":"checkHealth"'
  '"method":"getChannelId"'
  '"method":"getProgramIds"'
  '"method":"getBlock"'
  '"method":"getLastBlockId"'
  '"method":"getLastFinalizedBlockId"'
  'getBlockById'
  'getBlockByHash'
  'sequencer-finalized-block.borsh'
  'od -An -tu8 -N8'
  'xxd -p -s 40 -l 32'
  'indexer-finalized-block-by-hash.json'
  'cleanup_failed=0'
  'trap - EXIT'
  'docker container ls --all --quiet'
  'label=org.logos-co.atomic-swaps.run=${run_id}'
  'docker network inspect "$network"'
  'docker image inspect "$LEZ_V02_IMAGE"'
  'return "$cleanup_failed"'
  'count_fixed_occurrences'
  'source_genesis_time_occurrences'
  'generated_genesis_time_occurrences'
)

for term in "${required_runner_terms[@]}"; do
  if ! rg -Fq -- "$term" "$runner"; then
    echo "LEZ v0.2 runner is missing lifecycle/readiness control: ${term}" >&2
    exit 1
  fi
done

if rg -q 'docker (system|container|network|volume) prune|docker rm|docker stop|docker kill' "$runner"; then
  echo "global or unscoped Docker cleanup is forbidden" >&2
  exit 1
fi

bash "$readiness_test"
bash "$channel_test"
bash "$bootstrap_test"
bash "$missing_channel_test"

echo "LEZ v0.2 Docker isolation contract passed"
