#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly compose_file="tests/e2e/monero/compose.yml"
readonly dockerfile="tests/e2e/monero/Dockerfile"
readonly provenance_file="tests/e2e/monero/provenance.env"
readonly hashes_snapshot="tests/e2e/monero/release/hashes-v0.18.5.1.txt"
readonly signing_key_snapshot="tests/e2e/monero/release/binaryfate-v0.18.5.1.asc"
readonly verifier_file="scripts/verify-monero-release.sh"
readonly runner_file="scripts/run-monero-e2e.sh"
readonly workflow_file=".github/workflows/m4-monero.yml"

fail() {
  echo "Monero isolation contract failed: $*" >&2
  exit 1
}

require_fixed() {
  local needle="$1"
  local path="$2"
  rg -Fq -- "$needle" "$path" || fail "${path} is missing: ${needle}"
}

for required_file in \
  "$compose_file" "$dockerfile" "$provenance_file" "$hashes_snapshot" \
  "$signing_key_snapshot" "$verifier_file" "$runner_file" "$workflow_file"; do
  [[ -f "$required_file" && ! -L "$required_file" ]] ||
    fail "missing regular Monero fixture: ${required_file}"
done
for executable in "$verifier_file" "$runner_file"; do
  [[ -x "$executable" ]] || fail "Monero lifecycle script is not executable: ${executable}"
done

RUN_ID=monero-policy-check \
MONERO_IMAGE=lez-atomic-swaps-monero:policy-check \
MONERO_NETWORK=lez-atomic-swaps-monero-policy-check-private \
MONERO_DAEMON_CONFIG=/tmp/lez-monero-policy-daemon.conf \
MONERO_FUNDING_WALLET_CONFIG=/tmp/lez-monero-policy-funding.conf \
MONERO_MAKER_WALLET_CONFIG=/tmp/lez-monero-policy-maker.conf \
MONERO_TAKER_WALLET_CONFIG=/tmp/lez-monero-policy-taker.conf \
MONERO_DAEMON_HOST_PORT=39001 \
MONERO_FUNDING_WALLET_HOST_PORT=39002 \
MONERO_MAKER_WALLET_HOST_PORT=39003 \
MONERO_TAKER_WALLET_HOST_PORT=39004 \
  docker compose \
    --project-name lez-atomic-swaps-monero-policy-check \
    --file "$compose_file" config --quiet

required_services=(monerod funding_wallet maker_wallet taker_wallet)
for service in "${required_services[@]}"; do
  require_fixed "  ${service}:" "$compose_file"
done

required_compose_terms=(
  'org.logos-co.atomic-swaps.run: "${RUN_ID:?RUN_ID is required}"'
  'org.logos-co.atomic-swaps.scope: monero-regtest-e2e'
  'name: "${MONERO_NETWORK:?MONERO_NETWORK is required}"'
  'external: true'
  'user: "65532:65532"'
  'read_only: true'
  'cap_drop: ["ALL"]'
  'security_opt: ["no-new-privileges:true"]'
  '/tmp:rw,noexec,nosuid,size=128m,mode=1777'
  'type: tmpfs'
  'noexec,nosuid,nodev'
  '127.0.0.1:${MONERO_DAEMON_HOST_PORT:?MONERO_DAEMON_HOST_PORT is required}:18081'
  '127.0.0.1:${MONERO_FUNDING_WALLET_HOST_PORT:?MONERO_FUNDING_WALLET_HOST_PORT is required}:18083'
  '127.0.0.1:${MONERO_MAKER_WALLET_HOST_PORT:?MONERO_MAKER_WALLET_HOST_PORT is required}:18083'
  '127.0.0.1:${MONERO_TAKER_WALLET_HOST_PORT:?MONERO_TAKER_WALLET_HOST_PORT is required}:18083'
)
for term in "${required_compose_terms[@]}"; do
  require_fixed "$term" "$compose_file"
done

[[ "$(rg -F -c 'org.logos-co.atomic-swaps.run:' "$compose_file")" == 8 ]] ||
  fail "all four services and four role stores must carry the run label"
[[ "$(rg -F -c 'org.logos-co.atomic-swaps.scope:' "$compose_file")" == 8 ]] ||
  fail "all four services and four role stores must carry the scope label"
if rg -q '^\s*(container_name|cap_add):' "$compose_file"; then
  fail "fixed container names and regained Linux capabilities are forbidden"
fi
if rg -q '127\.0\.0\.1:[0-9]+:(18081|18083)' "$compose_file"; then
  fail "fixed Monero host RPC ports are forbidden"
fi
if rg -q '(^|[^0-9])(18080|18082)([^0-9]|$)' "$compose_file"; then
  fail "publishing Monero P2P or ZMQ ports is forbidden"
fi

required_dockerfile_terms=(
  'gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7'
  'org.opencontainers.image.source="https://github.com/monero-project/monero"'
  'org.opencontainers.image.version="0.18.5.1"'
  'org.opencontainers.image.revision="4f92268d7c16741cfb41e5bbe2aa46cc260a9ea5"'
  'org.opencontainers.image.licenses="BSD-3-Clause"'
  'org.logos-co.atomic-swaps.release-archive-sha256="22a7dda7b0cb699fdd6b7674c3b4a4465b337cc98a54983523b759e1e7cc9958"'
  'org.logos-co.atomic-swaps.release-signer="81AC591FE9C4B65C5806AFC3F0AF4D462A0BDF92"'
  'COPY --chmod=0555 bin/monerod /usr/local/bin/monerod'
  'COPY --chmod=0555 bin/monero-wallet-rpc /usr/local/bin/monero-wallet-rpc'
  'USER 65532:65532'
)
for term in "${required_dockerfile_terms[@]}"; do
  require_fixed "$term" "$dockerfile"
done

required_provenance_terms=(
  'MONERO_VERSION=0.18.5.1'
  'MONERO_TAG=v0.18.5.1'
  'MONERO_SOURCE_COMMIT=4f92268d7c16741cfb41e5bbe2aa46cc260a9ea5'
  'MONERO_ARCHIVE_SHA256=22a7dda7b0cb699fdd6b7674c3b4a4465b337cc98a54983523b759e1e7cc9958'
  'MONERO_HASHES_SHA256=a6a7afab3c26147b31ffd264d8c5939e50c05d43555fc48d9f0112c1093a2afb'
  'MONERO_HASHES_SNAPSHOT=tests/e2e/monero/release/hashes-v0.18.5.1.txt'
  'MONERO_SIGNING_KEY_SHA256=7dcb19c87d41a4399b4877054111f7dbfd30531545678bc2be218bd56903904c'
  'MONERO_SIGNING_KEY_SNAPSHOT=tests/e2e/monero/release/binaryfate-v0.18.5.1.asc'
  'MONERO_SIGNER_FINGERPRINT=81AC591FE9C4B65C5806AFC3F0AF4D462A0BDF92'
)
for term in "${required_provenance_terms[@]}"; do
  rg -Fxq -- "$term" "$provenance_file" ||
    fail "provenance contract is missing exact pin: ${term}"
done
printf '%s  %s\n' \
  a6a7afab3c26147b31ffd264d8c5939e50c05d43555fc48d9f0112c1093a2afb \
  "$hashes_snapshot" | sha256sum --check --strict >/dev/null
printf '%s  %s\n' \
  7dcb19c87d41a4399b4877054111f7dbfd30531545678bc2be218bd56903904c \
  "$signing_key_snapshot" | sha256sum --check --strict >/dev/null

for lifecycle_script in "$verifier_file" "$runner_file"; do
  if rg -n -q \
    'docker[[:space:]]+(system|container|image|volume|network)[[:space:]]+prune|(^|[;&|[:space:]])(pkill|killall)([;&|[:space:]]|$)' \
    "$lifecycle_script"; then
    fail "broad Docker or process cleanup is forbidden: ${lifecycle_script}"
  fi
done

required_runner_terms=(
  'MONERO_E2E_REQUIRE_CLEAN'
  'LocalAddr => "127.0.0.1"'
  'LocalPort => 0'
  '--opt com.docker.network.bridge.enable_ip_masquerade=false'
  '--label "org.logos-co.atomic-swaps.run=${run_id}"'
  "--label 'org.logos-co.atomic-swaps.scope=monero-regtest-e2e'"
  'wrong_actor_http_code'
  'if [[ "$wrong_actor_http_code" != "401" ]]'
  'and .result.nettype == "fakechain"'
  'and .result.offline == true'
  'runtime_external_resources: []'
  'public_rpc_used: false'
  'faucet_used: false'
  'public_funds_used: false'
  'write_secret_value "${credentials_dir}/daemon.username" daemon'
  'write_secret_value "${credentials_dir}/daemon.password" "$daemon_secret"'
  'MONERO_DAEMON_USERNAME_FILE'
  'MONERO_DAEMON_PASSWORD_FILE'
  'scope: "Monero local-functional Regtest topology bootstrap; not an atomic swap"'
  'docker network rm "$network"'
  'docker image rm "$image"'
  'foreign_sentinel_survived_exact_cleanup'
  'broad_cleanup_used: false'
)
for term in "${required_runner_terms[@]}"; do
  require_fixed "$term" "$runner_file"
done

required_ci_terms=(
  'monero-regtest:'
  'isolated Monero 0.18.5.1 Regtest topology bootstrap (not a swap)'
  'RUN_ID: github-xmr-${{ github.run_id }}-${{ github.run_attempt }}'
  'MONERO_E2E_KEEP_RUNNING: "1"'
  'MONERO_E2E_REQUIRE_CLEAN: "1"'
  './scripts/check-monero-isolation.sh'
  './scripts/run-monero-e2e.sh'
  'Scan exact verified Monero image for high and critical vulnerabilities'
  'uses: aquasecurity/trivy-action@ed142fd0673e97e23eac54620cfb913e5ce36c25'
  'image-ref: lez-atomic-swaps-monero:github-xmr-${{ github.run_id }}-${{ github.run_attempt }}'
  'exit-code: "1"'
  'severity: HIGH,CRITICAL'
  'uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02'
  'path: .e2e/${{ env.RUN_ID }}/monero/evidence/'
  'include-hidden-files: true'
  'Remove only the exact Monero CI resources'
)
for term in "${required_ci_terms[@]}"; do
  require_fixed "$term" "$workflow_file"
done

echo "Monero release, topology isolation, and CI security contract passed"
