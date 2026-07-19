#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly actionlint_version="1.7.12"
readonly actionlint_sha256="8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8"
readonly hadolint_version="2.14.0"
readonly hadolint_sha256="6bf226944684f56c84dd014e8b979d27425c0148f61b3bd99bcc6f39e9dc5a47"
readonly compose_version="5.3.1"
readonly compose_sha256="f9ebc6ebdb19d769b793c245a736caaeb198c62587f13b25c660c13b4987f959"
readonly shellcheck_version="0.11.0"
readonly shellcheck_sha256="b7af85e41cc99489dcc21d66c6d5f3685138f06d34651e6d34b42ec6d54fe6f6"

for command in bash curl flock git id jq ldd readelf rustup script scriptreplay \
  sed sha256sum stat tar; do
  command -v "$command" >/dev/null || {
    echo "${command} is required by the CI quality gate" >&2
    exit 1
  }
done

tools_dir="$(mktemp -d "${TMPDIR:-/tmp}/lez-ci-quality.XXXXXX")"
trap 'rm -rf "$tools_dir"' EXIT

download_checked() {
  local url="$1"
  local sha256="$2"
  local destination="$3"
  curl --fail --silent --show-error --location --retry 3 --retry-all-errors \
    --output "$destination" "$url"
  printf '%s  %s\n' "$sha256" "$destination" | sha256sum --check --strict >/dev/null
}

actionlint_archive="${tools_dir}/actionlint_1.7.12_linux_amd64.tar.gz"
download_checked \
  "https://github.com/rhysd/actionlint/releases/download/v${actionlint_version}/actionlint_${actionlint_version}_linux_amd64.tar.gz" \
  "$actionlint_sha256" "$actionlint_archive"
tar -xzf "$actionlint_archive" -C "$tools_dir" actionlint

hadolint="${tools_dir}/hadolint-linux-x86_64"
download_checked \
  "https://github.com/hadolint/hadolint/releases/download/v${hadolint_version}/hadolint-linux-x86_64" \
  "$hadolint_sha256" "$hadolint"

compose="${tools_dir}/docker-compose-linux-x86_64"
download_checked \
  "https://github.com/docker/compose/releases/download/v${compose_version}/docker-compose-linux-x86_64" \
  "$compose_sha256" "$compose"

shellcheck_archive="${tools_dir}/shellcheck-v0.11.0.linux.x86_64.tar.gz"
download_checked \
  "https://github.com/koalaman/shellcheck/releases/download/v${shellcheck_version}/shellcheck-v${shellcheck_version}.linux.x86_64.tar.gz" \
  "$shellcheck_sha256" "$shellcheck_archive"
tar -xzf "$shellcheck_archive" -C "$tools_dir"
shellcheck="${tools_dir}/shellcheck-v${shellcheck_version}/shellcheck"
chmod 0555 "${tools_dir}/actionlint" "$hadolint" "$compose" "$shellcheck"

mapfile -d '' shell_files < <(
  git ls-files --cached --others --exclude-standard -z -- '*.sh'
)
if (( ${#shell_files[@]} == 0 )); then
  echo "no shell files discovered" >&2
  exit 1
fi
bash -n "${shell_files[@]}"
"$shellcheck" --severity=warning "${shell_files[@]}"
M3_ACTOR_CONTRACT_REQUIRE_BINARIES=0 ./scripts/test-m3-actor-local-poc-contract.sh
./scripts/test-m3-node-startup-coordinator.sh
./scripts/test-m3-phase-timings-contract.sh
./scripts/test-m3-direction-phase-timings-contract.sh
./scripts/test-m3-official-wallet-cache-contract.sh
./scripts/test-m3-f7-token-fixture-contract.sh
./scripts/test-m3-private-recording-contract.sh
./scripts/test-m3-private-demo-video-contract.sh
./scripts/check-m3-cryptographic-vectors.sh
./scripts/test-bitcoin-testnet4-route-contract.sh

mapfile -d '' workflow_files < <(
  git ls-files --cached --others --exclude-standard -z -- \
    '.github/workflows/*.yml' '.github/workflows/*.yaml'
)
if (( ${#workflow_files[@]} == 0 )); then
  echo "no GitHub Actions workflows discovered" >&2
  exit 1
fi
"${tools_dir}/actionlint" -shellcheck "$shellcheck" "${workflow_files[@]}"

mapfile -d '' repository_files < <(
  git ls-files --cached --others --exclude-standard -z
)
dockerfiles=()
compose_files=()
for path in "${repository_files[@]}"; do
  basename="${path##*/}"
  case "$basename" in
    Dockerfile|Dockerfile.*)
      dockerfiles+=("$path")
      ;;
    compose.yml|compose.yaml|compose.*.yml|compose.*.yaml|docker-compose.yml|docker-compose.yaml|docker-compose.*.yml|docker-compose.*.yaml)
      compose_files+=("$path")
      ;;
  esac
done
if (( ${#dockerfiles[@]} == 0 || ${#compose_files[@]} == 0 )); then
  echo "Dockerfile and Compose discovery must both be non-empty" >&2
  exit 1
fi
"$hadolint" --failure-threshold warning "${dockerfiles[@]}"

for compose_file in "${compose_files[@]}"; do
  RUN_ID=ci-quality \
  LEZ_V02_IMAGE=lez-atomic-swaps-lez-v02:ci-quality \
  LEZ_V02_SOURCE_DIR=/tmp/lez-v02-ci-quality-source \
  LEZ_V02_RUN_DIR=/tmp/lez-v02-ci-quality-run \
  LEZ_V02_UID=65532 \
  LEZ_V02_GID=65532 \
  ZEBRA_IMAGE=lez-atomic-swaps-zebra:ci-quality \
  BITCOIN_CORE_IMAGE=lez-atomic-swaps-bitcoin-core:ci-quality \
  BITCOIN_CORE_CONFIG=/tmp/lez-bitcoin-core-ci-quality.conf \
  BITCOIN_CORE_NETWORK=lez-atomic-swaps-bitcoin-core-ci-quality-network \
  MONERO_IMAGE=lez-atomic-swaps-monero:ci-quality \
  MONERO_NETWORK=lez-atomic-swaps-monero-ci-quality-network \
  MONERO_DAEMON_CONFIG=/tmp/lez-monero-ci-quality-daemon.conf \
  MONERO_FUNDING_WALLET_CONFIG=/tmp/lez-monero-ci-quality-funding.conf \
  MONERO_MAKER_WALLET_CONFIG=/tmp/lez-monero-ci-quality-maker.conf \
  MONERO_TAKER_WALLET_CONFIG=/tmp/lez-monero-ci-quality-taker.conf \
  MONERO_DAEMON_HOST_PORT=39001 \
  MONERO_FUNDING_WALLET_HOST_PORT=39002 \
  MONERO_MAKER_WALLET_HOST_PORT=39003 \
  MONERO_TAKER_WALLET_HOST_PORT=39004 \
    "$compose" --project-name "lez-ci-quality-${RANDOM}" \
      --file "$compose_file" config --quiet
done

echo "CI shell, workflow, Dockerfile, and Compose quality gates passed"
