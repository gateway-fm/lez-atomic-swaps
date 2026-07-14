#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly compose_file="tests/e2e/bitcoin-core/compose.yml"
readonly dockerfile="tests/e2e/bitcoin-core/Dockerfile"
readonly provenance_file="tests/e2e/bitcoin-core/provenance.env"
readonly verifier_file="scripts/verify-bitcoin-core-release.sh"

for required_file in \
  "$compose_file" "$dockerfile" "$provenance_file" "$verifier_file"; do
  if [[ ! -f "$required_file" ]]; then
    echo "missing isolated Bitcoin Core fixture: ${required_file}" >&2
    exit 1
  fi
done

RUN_ID=policy-check \
BITCOIN_CORE_IMAGE=lez-atomic-swaps-bitcoin-core:policy-check \
BITCOIN_CORE_CONFIG=/tmp/lez-bitcoin-core-policy-check.conf \
  docker compose \
    --project-name lez-atomic-swaps-bitcoin-core-policy-check \
    --file "$compose_file" config --quiet

required_compose_terms=(
  'bitcoin_core:'
  '${BITCOIN_CORE_IMAGE:?BITCOIN_CORE_IMAGE is required}'
  '${BITCOIN_CORE_CONFIG:?BITCOIN_CORE_CONFIG is required}:/run-config/bitcoin.conf:ro'
  '127.0.0.1::18443'
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
  'com.docker.network.bridge.enable_ip_masquerade: "false"'
  'cap_drop: ["ALL"]'
  'security_opt: ["no-new-privileges:true"]'
  'internal: true'
  'core_data:'
  'type: tmpfs'
  'o: "uid=65532,gid=65532,mode=0700,noexec,nosuid,nodev,size=1073741824"'
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

if rg -q '127\.0\.0\.1:[0-9]+:18443' "$compose_file"; then
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
  'gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7'
  'org.opencontainers.image.source="https://github.com/bitcoin/bitcoin"'
  'org.opencontainers.image.version="31.1"'
  'org.opencontainers.image.revision="9be056a8a72b624dae9623b2f7bded92c2a21c91"'
  'org.logos-co.atomic-swaps.release-archive-sha256="b80d9c3e04da78fb6f0569685673418cf686fadba9042d926d13fb87ff503f9e"'
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
  'BITCOIN_CORE_GUIX_SIGS_URL=https://github.com/bitcoin-core/guix.sigs.git'
  'BITCOIN_CORE_GUIX_SIGS_COMMIT=11fb5156a16f27d71b61d18e23c5ffeb29cc6ee1'
  'BITCOIN_CORE_GUIX_SIGS_RELEASE=31.1'
  'BITCOIN_CORE_RUNTIME_BASE=gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7'
)

for term in "${required_provenance_terms[@]}"; do
  if ! rg -Fxq "$term" "$provenance_file"; then
    echo "Bitcoin Core provenance contract is missing exact pin: ${term}" >&2
    exit 1
  fi
done

for lifecycle_script in "$verifier_file" scripts/run-bitcoin-core-e2e.sh; do
  if [[ -f "$lifecycle_script" ]] && rg -n -q \
    'docker[[:space:]]+(system|container|image|volume|network)[[:space:]]+prune|(^|[;&|[:space:]])(pkill|killall)([;&|[:space:]]|$)' \
    "$lifecycle_script"; then
    echo "broad Docker or process cleanup is forbidden: ${lifecycle_script}" >&2
    exit 1
  fi
done

run_label_count="$(rg -F -c 'org.logos-co.atomic-swaps.run:' "$compose_file")"
scope_label_count="$(rg -F -c 'org.logos-co.atomic-swaps.scope:' "$compose_file")"
if (( run_label_count != 3 || scope_label_count != 3 )); then
  echo "service, private network, and data volume must all carry run/scope labels" >&2
  exit 1
fi
