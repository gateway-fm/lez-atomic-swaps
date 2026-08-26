#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly compose_file="tests/e2e/bitcoin-core/compose.yml"
readonly dockerfile="tests/e2e/bitcoin-core/Dockerfile"
readonly provenance_file="tests/e2e/bitcoin-core/provenance.env"
readonly aarch64_provenance_file="tests/e2e/bitcoin-core/provenance-aarch64.env"
readonly verifier_file="scripts/verify-bitcoin-core-release.sh"
readonly runner_file="scripts/run-bitcoin-core-e2e.sh"
readonly service_mode_policy="scripts/test-bitcoin-core-service-mode-policy.sh"

for required_file in \
  "$compose_file" "$dockerfile" "$provenance_file" "$aarch64_provenance_file" \
  "$verifier_file" "$runner_file" \
  "$service_mode_policy"; do
  if [[ ! -f "$required_file" ]]; then
    echo "missing isolated Bitcoin Core fixture: ${required_file}" >&2
    exit 1
  fi
done

if [[ ! -x "$service_mode_policy" ]]; then
  echo "Bitcoin Core service-mode policy must be executable: ${service_mode_policy}" >&2
  exit 1
fi

RUN_ID=policy-check \
BITCOIN_CORE_IMAGE=lez-atomic-swaps-bitcoin-core:policy-check \
BITCOIN_CORE_CONFIG=/tmp/lez-bitcoin-core-policy-check.conf \
BITCOIN_CORE_NETWORK=lez-atomic-swaps-bitcoin-core-policy-check-network \
  docker compose \
    --project-name lez-atomic-swaps-bitcoin-core-policy-check \
    --file "$compose_file" config --quiet

required_compose_terms=(
  'bitcoin_core:'
  '${BITCOIN_CORE_IMAGE:?BITCOIN_CORE_IMAGE is required}'
  '${BITCOIN_CORE_CONFIG:?BITCOIN_CORE_CONFIG is required}:/run-config/bitcoin.conf:ro'
  'target: 18443'
  'published: "0"'
  'host_ip: 127.0.0.1'
  'protocol: tcp'
  'mode: host'
  'org.logos-co.atomic-swaps.run: "${RUN_ID:?RUN_ID is required}"'
  'core_data:/var/lib/bitcoin'
  'org.logos-co.atomic-swaps.scope: bitcoin-core-regtest-e2e'
  'org.logos-co.atomic-swaps.component: bitcoin-core'
  '/tmp:rw,noexec,nosuid,size=128m,mode=1777'
  'mem_limit: 2g'
  'cpus: 2.0'
  'pids_limit: 256'
  'user: "65532:65532"'
  'stop_grace_period: 30s'
  'read_only: true'
  'cap_drop: ["ALL"]'
  'security_opt: ["no-new-privileges:true"]'
  'core_data:'
  'type: tmpfs'
  'o: "uid=65532,gid=65532,mode=0700,noexec,nosuid,nodev,size=1073741824"'
  'name: "${BITCOIN_CORE_NETWORK:?BITCOIN_CORE_NETWORK is required}"'
  'external: true'
)

for term in "${required_compose_terms[@]}"; do
  if ! rg -Fq "$term" "$compose_file"; then
    echo "Bitcoin Core Compose fixture is missing isolation control: ${term}" >&2
    exit 1
  fi
done

if rg -q '^\s*container_name:' "$compose_file"; then
  echo "fixed Docker container names are forbidden" >&2
  exit 1
fi

if rg -q 'published: "?[1-9][0-9]*"?' "$compose_file"; then
  echo "fixed Bitcoin Core host RPC ports are forbidden" >&2
  exit 1
fi

if rg -q '18444' "$compose_file"; then
  echo "publishing the Bitcoin Core Regtest P2P port is forbidden" >&2
  exit 1
fi

if rg -q '^\s*cap_add:' "$compose_file"; then
  echo "Bitcoin Core must not regain Linux capabilities" >&2
  exit 1
fi

required_dockerfile_terms=(
  'cgr.dev/chainguard/glibc-dynamic:latest@sha256:205572d5e48117e14b44b42627890fa8d3e8e65bb37a80abb3317e5151e7f35b'
  'org.opencontainers.image.source="https://github.com/bitcoin/bitcoin"'
  'org.opencontainers.image.version="31.1"'
  'org.opencontainers.image.revision="9be056a8a72b624dae9623b2f7bded92c2a21c91"'
  'ARG BITCOIN_CORE_ARCHIVE_SHA256'
  'org.logos-co.atomic-swaps.release-archive-sha256="${BITCOIN_CORE_ARCHIVE_SHA256}"'
  'org.opencontainers.image.licenses="MIT"'
  'org.logos-co.atomic-swaps.guix-sigs-commit="11fb5156a16f27d71b61d18e23c5ffeb29cc6ee1"'
  'COPY --chmod=0555 bin/bitcoind /usr/local/bin/bitcoind'
  'COPY --chmod=0555 bin/bitcoin-cli /usr/local/bin/bitcoin-cli'
  'USER 65532:65532'
  'ENTRYPOINT ["/usr/local/bin/bitcoind"]'
)

for term in "${required_dockerfile_terms[@]}"; do
  if ! rg -Fq "$term" "$dockerfile"; then
    echo "Bitcoin Core Dockerfile is missing provenance/runtime control: ${term}" >&2
    exit 1
  fi
done

required_provenance_terms=(
  'BITCOIN_CORE_VERSION=31.1'
  'BITCOIN_CORE_TAG=v31.1'
  'BITCOIN_CORE_SOURCE_URL=https://github.com/bitcoin/bitcoin.git'
  'BITCOIN_CORE_SOURCE_COMMIT=9be056a8a72b624dae9623b2f7bded92c2a21c91'
  'BITCOIN_CORE_ARCHIVE_NAME=bitcoin-31.1-x86_64-linux-gnu.tar.gz'
  'BITCOIN_CORE_ARCHIVE_URL=https://bitcoincore.org/bin/bitcoin-core-31.1/bitcoin-31.1-x86_64-linux-gnu.tar.gz'
  'BITCOIN_CORE_ARCHIVE_SHA256=b80d9c3e04da78fb6f0569685673418cf686fadba9042d926d13fb87ff503f9e'
  'BITCOIN_CORE_ARCHIVE_SIZE=90293352'
  'BITCOIN_CORE_GUIX_SIGS_URL=https://github.com/bitcoin-core/guix.sigs.git'
  'BITCOIN_CORE_GUIX_SIGS_COMMIT=11fb5156a16f27d71b61d18e23c5ffeb29cc6ee1'
  'BITCOIN_CORE_GUIX_SIGS_RELEASE=31.1'
  'BITCOIN_CORE_RUNTIME_BASE=cgr.dev/chainguard/glibc-dynamic:latest@sha256:205572d5e48117e14b44b42627890fa8d3e8e65bb37a80abb3317e5151e7f35b'
)

for term in "${required_provenance_terms[@]}"; do
  if ! rg -Fxq "$term" "$provenance_file"; then
    echo "Bitcoin Core provenance contract is missing exact pin: ${term}" >&2
    exit 1
  fi
done

required_aarch64_provenance_terms=(
  'BITCOIN_CORE_VERSION=31.1'
  'BITCOIN_CORE_TAG=v31.1'
  'BITCOIN_CORE_SOURCE_URL=https://github.com/bitcoin/bitcoin.git'
  'BITCOIN_CORE_SOURCE_COMMIT=9be056a8a72b624dae9623b2f7bded92c2a21c91'
  'BITCOIN_CORE_ARCHIVE_NAME=bitcoin-31.1-aarch64-linux-gnu.tar.gz'
  'BITCOIN_CORE_ARCHIVE_URL=https://bitcoincore.org/bin/bitcoin-core-31.1/bitcoin-31.1-aarch64-linux-gnu.tar.gz'
  'BITCOIN_CORE_ARCHIVE_SHA256=dcf1873f2208ba4f962f3398d47e154c39c0084be8f4553e05c940d0ace3d004'
  'BITCOIN_CORE_ARCHIVE_SIZE=85851107'
  'BITCOIN_CORE_GUIX_SIGS_URL=https://github.com/bitcoin-core/guix.sigs.git'
  'BITCOIN_CORE_GUIX_SIGS_COMMIT=11fb5156a16f27d71b61d18e23c5ffeb29cc6ee1'
  'BITCOIN_CORE_GUIX_SIGS_RELEASE=31.1'
  'BITCOIN_CORE_RUNTIME_BASE=cgr.dev/chainguard/glibc-dynamic:latest@sha256:205572d5e48117e14b44b42627890fa8d3e8e65bb37a80abb3317e5151e7f35b'
)

for term in "${required_aarch64_provenance_terms[@]}"; do
  if ! rg -Fxq "$term" "$aarch64_provenance_file"; then
    echo "Bitcoin Core ARM64 provenance contract is missing exact pin: ${term}" >&2
    exit 1
  fi
done

required_runner_architecture_terms=(
  'x86_64) provenance_file="tests/e2e/bitcoin-core/provenance.env"'
  'arm64 | aarch64) provenance_file="tests/e2e/bitcoin-core/provenance-aarch64.env"'
  'export BITCOIN_CORE_PROVENANCE_FILE="$provenance_file"'
  '--build-arg "BITCOIN_CORE_ARCHIVE_SHA256=${BITCOIN_CORE_ARCHIVE_SHA256}"'
)
for term in "${required_runner_architecture_terms[@]}"; do
  if ! rg -Fq -- "$term" "$runner_file"; then
    echo "Bitcoin Core runner is missing architecture/provenance binding: ${term}" >&2
    exit 1
  fi
done

for lifecycle_script in "$verifier_file" "$runner_file"; do
  if [[ -f "$lifecycle_script" ]] && rg -n -q \
    'docker[[:space:]]+(system|container|image|volume|network)[[:space:]]+prune|(^|[;&|[:space:]])(pkill|killall)([;&|[:space:]]|$)' \
    "$lifecycle_script"; then
    echo "broad Docker or process cleanup is forbidden: ${lifecycle_script}" >&2
    exit 1
  fi
done

run_label_count="$(rg -F -c 'org.logos-co.atomic-swaps.run:' "$compose_file")"
scope_label_count="$(rg -F -c 'org.logos-co.atomic-swaps.scope:' "$compose_file")"
if (( run_label_count != 2 || scope_label_count != 2 )); then
  echo "service and data volume must carry run/scope labels" >&2
  exit 1
fi

required_runner_network_terms=(
  'docker network create'
  '--opt com.docker.network.bridge.enable_ip_masquerade=false'
  '--label "org.logos-co.atomic-swaps.run=${run_id}"'
  "--label 'org.logos-co.atomic-swaps.scope=bitcoin-core-regtest-e2e'"
  "--label 'org.logos-co.atomic-swaps.component=bitcoin-core-network'"
  'docker network rm "$network"'
)
for term in "${required_runner_network_terms[@]}"; do
  if ! rg -Fq -- "$term" "$runner_file"; then
    echo "Bitcoin Core runner is missing run-network isolation control: ${term}" >&2
    exit 1
  fi
done

required_runner_runtime_terms=(
  "--label 'org.logos-co.atomic-swaps.component=bitcoin-core-image'"
  'docker volume create'
  'readonly -a compose=(docker compose --project-name "$project" --file "$compose_file")'
  '"${compose[@]}" config --quiet'
  '"${compose[@]}" create'
  'container_id="$(docker ps -aq --filter "name=${project}-bitcoin_core" | head -1)"'
  'docker start "$container_id"'
  'docker container rm --force "$container_id"'
  'docker volume rm "$volume"'
  'docker image rm "$image"'
  'lifecycle: "exact_id_compose_create"'
  'compose_contract_validated: true'
)
for term in "${required_runner_runtime_terms[@]}"; do
  if ! rg -Fq -- "$term" "$runner_file"; then
    echo "Bitcoin Core runner is missing direct-runtime isolation control: ${term}" >&2
    exit 1
  fi
done

"$service_mode_policy"
