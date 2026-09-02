#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly ci_workflow=".github/workflows/ci.yml"
readonly public_workflow=".github/workflows/public-readiness.yml"
readonly quality_runner="scripts/run-ci-quality-gates.sh"
readonly ripgrep_installer="scripts/install-ci-ripgrep.sh"
readonly provisional_verifier="scripts/verify-lez-v02-provisional.sh"
readonly provisional_methods_build="compat/lez-v0.2-provisional/escrow/methods/build.rs"
readonly provisional_artifact_manifest="compat/lez-v0.2-provisional/escrow/methods/guest/deployment-manifest.toml"
readonly core_runner="scripts/run-bitcoin-core-e2e.sh"
readonly core_evidence="docs/evidence/m3-bitcoin-core-smoke-a7393df-20260714.json"
readonly core_musig_evidence="docs/evidence/m3-bitcoin-core-musig2-f5a9caa-20260715.json"
readonly btc_sdk_manifest="crates/btc-swap-sdk/Cargo.toml"
readonly core_adapter_manifest="crates/btc-core-adapter/Cargo.toml"

fail() {
  echo "CI hardening policy: $*" >&2
  exit 1
}

require_fixed() {
  local needle="$1"
  local path="$2"
  rg -Fq -- "$needle" "$path" || fail "$path is missing: $needle"
}

workflow_step() {
  local name="$1"
  awk -v marker="      - name: ${name}" '
    $0 == marker { found = 1 }
    found && $0 != marker && $0 ~ /^      - / { exit }
    found { print }
    END { if (!found) exit 1 }
  ' "$ci_workflow"
}

assert_trivy_step() {
  local name="$1"
  local image="$2"
  local exit_code="$3"
  local step
  step="$(workflow_step "$name")" || fail "missing Trivy step: $name"
  for required in \
    'uses: aquasecurity/trivy-action@ed142fd0673e97e23eac54620cfb913e5ce36c25' \
    "image-ref: ${image}" \
    'format: table' \
    "exit-code: \"${exit_code}\"" \
    'ignore-unfixed: false' \
    'vuln-type: os,library' \
    'severity: HIGH,CRITICAL'; do
    rg -Fq -- "$required" <<<"$step" \
      || fail "Trivy step '$name' is missing: $required"
  done
  ! rg -Fq -- 'continue-on-error' <<<"$step" \
    || fail "Trivy step '$name' must not continue on error"
}

for required_path in \
  "$quality_runner" \
  "$ripgrep_installer" \
  "$provisional_verifier" \
  "$provisional_methods_build" \
  "$provisional_artifact_manifest" \
  "$core_runner" \
  "$core_evidence" \
  "$core_musig_evidence" \
  "$btc_sdk_manifest" \
  "$core_adapter_manifest"; do
  [[ -f "$required_path" ]] || fail "missing $required_path"
done
[[ -x "$quality_runner" && -x "$ripgrep_installer" && -x "$core_runner" ]] \
  || fail "CI runners and installers must remain executable"

for workflow in "$ci_workflow" "$public_workflow"; do
  [[ -f "$workflow" ]] || fail "missing $workflow"
  rg -F 'permissions:' "$workflow" >/dev/null || fail "$workflow lacks explicit permissions"
  rg -F 'contents: read' "$workflow" >/dev/null || fail "$workflow must default to read-only contents"
  ! rg -F 'pull_request_target:' "$workflow" >/dev/null \
    || fail "$workflow must not use pull_request_target"

  while IFS= read -r checkout_line; do
    sha="${checkout_line#*@}"
    sha="${sha%% *}"
    [[ "$sha" =~ ^[0-9a-f]{40}$ ]] \
      || fail "$workflow has a non-SHA checkout pin: $checkout_line"
  done < <(rg -o 'actions/checkout@[0-9a-f]+[^[:space:]]*([[:space:]]+#.*)?' "$workflow")

  checkout_count="$(rg -c 'uses: actions/checkout@' "$workflow")"
  credential_count="$(rg -c 'persist-credentials: false' "$workflow")"
  [[ "$checkout_count" == "$credential_count" ]] \
    || fail "$workflow must disable persisted credentials for every checkout"
done

./scripts/check-github-action-pins.sh

require_fixed 'tags: ["m*-complete*"]' "$ci_workflow"
require_fixed './scripts/run-ci-quality-gates.sh' "$ci_workflow"
require_fixed 'deploy/images/btc-demo-controller/test_controller.py' "$ci_workflow"
require_fixed 'deploy/images/btc-demo-launcher/test_launcher.py' "$ci_workflow"
require_fixed './scripts/test-spin-lock-remediation.sh' "$ci_workflow"
require_fixed './scripts/check-spin-lock-remediation.sh' "$ci_workflow"
require_fixed './scripts/check-github-action-pins.sh' "$ci_workflow"
require_fixed './scripts/install-ci-ripgrep.sh' "$ci_workflow"
require_fixed 'ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz' "$ripgrep_installer"
require_fixed '4cf9f2741e6c465ffdb7c26f38056a59e2a2544b51f7cc128ef28337eeae4d8e' "$ripgrep_installer"
require_fixed 'npm audit --audit-level=moderate' "$ci_workflow"
require_fixed 'npm run audit:licenses' "$ci_workflow"
require_fixed 'actions/setup-node@820762786026740c76f36085b0efc47a31fe5020' "$ci_workflow"
require_fixed 'node-version: 24.18.0' "$ci_workflow"
require_fixed 'gitleaks_8.30.1_linux_x64.tar.gz' "$public_workflow"
require_fixed '551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb' "$public_workflow"
require_fixed 'cargo-deny-0.20.2-x86_64-unknown-linux-musl.tar.gz' "$public_workflow"
require_fixed '9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f' "$public_workflow"
[[ "$(rg -Fc 'sha256sum --check --strict -' "$public_workflow")" == 2 ]] \
  || fail "$public_workflow must verify both downloaded archives before extraction"
require_fixed 'PUPPETEER_EXECUTABLE_PATH: /usr/bin/google-chrome' "$ci_workflow"
require_fixed 'PUPPETEER_SKIP_DOWNLOAD: "true"' "$ci_workflow"
require_fixed 'M6_UI_TEST_ROOT: ${{ runner.temp }}/lez-m6-ui-${{ github.run_id }}-${{ github.run_attempt }}' "$ci_workflow"
require_fixed 'npm run test:m6:prototype' "$ci_workflow"
require_fixed 'npm run test:m6:basecamp:contract' "$ci_workflow"
require_fixed 'Maker and Taker Basecamp integration' "$ci_workflow"
require_fixed 'bash /repo/scripts/test-m6-basecamp-integration.sh' "$ci_workflow"
require_fixed 'nixos/nix:2.30.2@sha256:7894650fb65234b35c80010e6ca44149b70a4a216118a6b7e5c6f6ae377c8d21' "$ci_workflow"
require_fixed 'cargo fmt --all --check' "$ci_workflow"
require_fixed 'cargo clippy --locked --workspace --all-targets' "$ci_workflow"
require_fixed 'cargo test --locked --workspace --all-targets' "$ci_workflow"
require_fixed 'cargo doc --locked --workspace --no-deps' "$ci_workflow"

readonly cargo_deny_action='uses: EmbarkStudios/cargo-deny-action@bb137d7af7e4fb67e5f82a49c4fce4fad40782fe'
readonly cargo_deny_policy='advisories bans licenses sources'
readonly expected_cargo_deny_steps=6
cargo_deny_steps="$(rg -Fc -- "$cargo_deny_action" "$ci_workflow")"
cargo_deny_policy_steps="$(rg -Fc -- "$cargo_deny_policy" "$ci_workflow")"
[[ "$cargo_deny_steps" == "$expected_cargo_deny_steps" ]] \
  || fail "expected $expected_cargo_deny_steps pinned cargo-deny steps; found $cargo_deny_steps"
[[ "$cargo_deny_policy_steps" == "$expected_cargo_deny_steps" ]] \
  || fail "every cargo-deny step must enforce the complete policy"

for manifest in \
  compat/lez-v0_2-sidecar/Cargo.toml \
  compat/lez-v0.2-provisional/Cargo.toml \
  compat/lez-v0.2-provisional/escrow/methods/Cargo.toml \
  compat/lez-v0.2-provisional/escrow/methods/guest/Cargo.toml \
  compat/lez-v0.2-provisional/escrow/deployer/Cargo.toml; do
  require_fixed "manifest-path: $manifest" "$ci_workflow"
done

require_fixed 'actionlint_1.7.12_linux_amd64.tar.gz' "$quality_runner"
require_fixed '8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8' "$quality_runner"
require_fixed 'hadolint-linux-x86_64' "$quality_runner"
require_fixed '6bf226944684f56c84dd014e8b979d27425c0148f61b3bd99bcc6f39e9dc5a47' "$quality_runner"
require_fixed 'docker-compose-linux-x86_64' "$quality_runner"
require_fixed 'f9ebc6ebdb19d769b793c245a736caaeb198c62587f13b25c660c13b4987f959' "$quality_runner"
require_fixed 'shellcheck-v0.11.0.linux.x86_64.tar.gz' "$quality_runner"
require_fixed 'b7af85e41cc99489dcc21d66c6d5f3685138f06d34651e6d34b42ec6d54fe6f6' "$quality_runner"
require_fixed '"$shellcheck" --severity=warning' "$quality_runner"
require_fixed "git ls-files --cached --others --exclude-standard -z -- '*.sh'" "$quality_runner"
require_fixed 'M3_ACTOR_CONTRACT_REQUIRE_BINARIES=0 ./scripts/test-m3-actor-local-poc-contract.sh' "$quality_runner"
require_fixed './scripts/test-m3-node-startup-coordinator.sh' "$quality_runner"
require_fixed './scripts/test-m3-phase-timings-contract.sh' "$quality_runner"
require_fixed './scripts/test-m3-direction-phase-timings-contract.sh' "$quality_runner"
require_fixed './scripts/test-m3-official-wallet-cache-contract.sh' "$quality_runner"
require_fixed './scripts/test-m3-f7-token-fixture-contract.sh' "$quality_runner"
require_fixed './scripts/test-m3-private-recording-contract.sh' "$quality_runner"
require_fixed './scripts/test-m3-private-demo-video-contract.sh' "$quality_runner"
require_fixed './scripts/test-v0-1-1-release-media-contract.sh' "$quality_runner"
require_fixed './scripts/check-m3-cryptographic-vectors.sh' "$quality_runner"
require_fixed './scripts/test-bitcoin-testnet4-route-contract.sh' "$quality_runner"
require_fixed 'node ./scripts/check-m6-prototype-contract.mjs' "$quality_runner"
require_fixed 'node ./scripts/check-m6-basecamp-package-contract.mjs' "$quality_runner"
require_fixed 'git ls-files --cached --others --exclude-standard -z' "$quality_runner"
require_fixed 'config --quiet' "$quality_runner"
require_fixed 'BTC_RPC_PASSWORD="ci-quality-${RANDOM}-${RANDOM}"' "$quality_runner"
require_fixed 'LEZ_M3_RUNNER_REPO=/tmp/lez-ci-quality-runner-repo' "$quality_runner"
require_fixed 'LEZ_M3_RUNNER_REPO_IN_CONTAINER=/tmp/lez-ci-quality-runner-repo' "$quality_runner"

if rg -F 'COPY --chmod=0555 zec-' deploy/images/maker-node/Dockerfile >/dev/null; then
  fail "the public BTC Basecamp image must not package future ZEC executables"
fi
if rg -F 'lez-taker-' deploy/images/maker-node/Dockerfile >/dev/null; then
  fail "the Maker image must not package Taker executables"
fi
if rg -F 'lez-maker-' deploy/images/taker-node/Dockerfile >/dev/null; then
  fail "the Taker image must not package Maker executables"
fi

require_fixed 'Verify M3 official cryptographic vectors' "$ci_workflow"
require_fixed 'cargo test --locked -p lez-btc-swap-sdk --test bip340_vectors --test bip327_vectors --test adaptor_vectors' "$ci_workflow"
require_fixed 'Verify Bitcoin Testnet4 route contract' "$ci_workflow"
require_fixed 'k256 = { version = "=0.13.4", default-features = false, features = ["schnorr"] }' "$btc_sdk_manifest"
require_fixed 'jsonrpsee-http-client = { version = "=0.26.0", default-features = false, features = ["tls"] }' "$core_adapter_manifest"

for forbidden in \
  'm4-monero' \
  'monero-regtest' \
  'zebra-regtest' \
  'lez-v02-xmr-release-worker' \
  'coordinator-fuzz' \
  'test-m7-' \
  'test-m5-' \
  'test-m4-'; do
  ! rg -F "$forbidden" .github/workflows scripts/run-ci-quality-gates.sh >/dev/null \
    || fail "out-of-scope release gate remains: $forbidden"
done

trivy_fail_hard_steps="$(rg -c 'exit-code: "1"' "$ci_workflow")"
(( trivy_fail_hard_steps >= 2 )) \
  || fail "runtime and Bitcoin image scans must fail hard"

rg -F './scripts/check-bitcoin-core-isolation.sh' "$ci_workflow" >/dev/null \
  || fail "Bitcoin isolation contract is not enforced"
rg -F './scripts/check-lez-v02-docker-isolation.sh' "$ci_workflow" >/dev/null \
  || fail "LEZ isolation contract is not enforced"

assert_trivy_step \
  'Scan repository-owned runtime base for high and critical vulnerabilities' \
  'cgr.dev/chainguard/glibc-dynamic:latest@sha256:205572d5e48117e14b44b42627890fa8d3e8e65bb37a80abb3317e5151e7f35b' 1
assert_trivy_step \
  'Report pinned Risc0 guest builder high and critical vulnerabilities' \
  'risczero/risc0-guest-builder:r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be' 0
assert_trivy_step \
  'Report high and critical vulnerabilities in exact Logos Bedrock dependency' \
  'ghcr.io/logos-blockchain/logos-blockchain@sha256:91d6c5bf07e07fcfba5e7cf07d21ee686a6bc4b9f6210f2d28bffbcad9a3729f' 0
assert_trivy_step \
  'Scan exact Bitcoin Core image for high and critical vulnerabilities' \
  'lez-atomic-swaps-bitcoin-core:github-btc-${{ github.run_id }}-${{ github.run_attempt }}' 1
report_only_scans="$(rg -Fc -- 'exit-code: "0"' "$ci_workflow")"
[[ "$report_only_scans" == 2 ]] \
  || fail "exactly two classified upstream scans may report without failing"

require_fixed 'isolated Bitcoin Core 31.1 Regtest MuSig2 adaptor P2TR spend' "$ci_workflow"
require_fixed 'BITCOIN_CORE_E2E_KEEP_RUNNING: "1"' "$ci_workflow"
require_fixed 'BITCOIN_CORE_E2E_REQUIRE_CLEAN: "1"' "$ci_workflow"
require_fixed 'readonly actor_allowlist="getblockchaininfo,getnetworkinfo,getblockhash,getblock,getblockheader,getrawtransaction,gettxout,gettxspendingprevout,getindexinfo,getmempoolinfo,getrawmempool,getmempoolentry,testmempoolaccept,sendrawtransaction"' "$core_runner"
require_fixed 'create_basic_credentials' "$core_runner"
require_fixed 'BITCOIN_CORE_MAKER_BASIC_CREDENTIALS=' "$core_runner"
require_fixed 'BITCOIN_CORE_TAKER_BASIC_CREDENTIALS=' "$core_runner"
require_fixed 'mapfile -t containers < <(docker container ls --all --quiet' "$ci_workflow"
require_fixed 'if (( ${#containers[@]} > 1 )); then' "$ci_workflow"
require_fixed 'docker container rm --force "${containers[0]}"' "$ci_workflow"
require_fixed 'org.logos-co.atomic-swaps.run=${RUN_ID}' "$ci_workflow"

jq -e '
  .schema_version == 1
  and .milestone == "M3"
  and .scope == "bitcoin_core_infrastructure_only"
  and .result == "passed"
  and .worktree_clean_before_run == true
  and .core.version == "31.1"
  and .chain.network == "regtest"
  and .isolation.rpc_publication == "dynamic_literal_loopback_only"
  and .isolation.p2p_port_published == false
  and .actor_rpc.credentials_distinct == true
  and .actor_rpc.plaintext_credentials_disclosed == false
  and .actor_rpc.crossed_credentials_http_401_count == 2
  and .external_dependencies.runtime_external_resources == []
  and .external_dependencies.public_rpc_used == false
  and .cleanup.exact_run_resources_absent == true
  and .cleanup.foreign_sentinel_survived_exact_cleanup == true
' "$core_evidence" >/dev/null || fail "retained Bitcoin Core evidence invariants failed"

jq -e '
  .schema_version == 1
  and .milestone == "M3"
  and .scope == "bitcoin_core_musig2_adaptor_p2tr_fixture"
  and .result == "passed"
  and .clean_worktree_required == true
  and .contract.signing_protocol == "BIP327_MUSIG2_SCHNORR_ADAPTOR"
  and .contract.signer_order == ["maker", "taker"]
  and .cooperative_key_path_claim.adaptor_presignature_bytes == 65
  and .cooperative_key_path_claim.adaptor_presignature_verified == true
  and .cooperative_key_path_claim.final_signature_verified_under_q == true
  and .cooperative_key_path_claim.extracted_point_matches == true
  and .security_claims.musig2_taproot_fixture_proven == true
  and .security_claims.adaptor_signature_fixture_proven == true
  and .security_claims.scalar_extraction_fixture_proven == true
  and .external_dependencies.runtime_external_resources == []
  and .cleanup.exact_run_resources_absent == true
  and .cleanup.foreign_sentinel_survived_exact_cleanup == true
' "$core_musig_evidence" >/dev/null || fail "retained Bitcoin Core MuSig2 evidence invariants failed"

require_fixed 'cargo check --locked --manifest-path "$guest_manifest" --bins' "$provisional_verifier"
require_fixed 'cargo clippy --locked --manifest-path "$guest_manifest" --bins -- -D warnings' "$provisional_verifier"
require_fixed 'risc0_rust_version="1.94.1"' "$provisional_verifier"
require_fixed 'risc0_guest_builder_tag="r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be"' "$provisional_verifier"
require_fixed 'export RISC0_DOCKER_CONTAINER_TAG="$risc0_guest_builder_tag"' "$provisional_verifier"
require_fixed 'risc0_build::embed_methods_with_options' "$provisional_methods_build"
require_fixed 'r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be' "$provisional_methods_build"
require_fixed '.root_dir("../..")' "$provisional_methods_build"
require_fixed 'expected_elf_sha256="237037e1a54187697e7e67a9bf589dfb3eb88c475c7f9b62eb2396144e87c6d0"' "$provisional_verifier"
require_fixed 'expected_image_id="431ab9aec4b21d66e88ecbf8bb83301d5ef4cc0cec0ba0fb76baaa0ac7f9a10b"' "$provisional_verifier"
require_fixed 'elf_sha256 = "237037e1a54187697e7e67a9bf589dfb3eb88c475c7f9b62eb2396144e87c6d0"' "$provisional_artifact_manifest"
require_fixed 'image_id = "431ab9aec4b21d66e88ecbf8bb83301d5ef4cc0cec0ba0fb76baaa0ac7f9a10b"' "$provisional_artifact_manifest"
if rg -Fq 'risc0_build::embed_methods();' "$provisional_methods_build"; then
  fail "v0.2 methods must embed the canonical Docker-built guest"
fi
require_fixed 'cargo risczero build --manifest-path escrow/methods/guest/Cargo.toml' "$provisional_verifier"
if rg -Fq 'cargo test --locked --manifest-path "$guest_manifest"' "$provisional_verifier"; then
  fail "zkVM guest verifier must not build a host unit-test harness"
fi

if rg -Uq $'readonly REPOSITORY_ROOT\nREPOSITORY_ROOT=|readonly repository_root\nrepository_root=' scripts; then
  fail "a shell wrapper assigns a variable only after marking it readonly"
fi

echo "CI hardening policy passed for the M1/M3/M6 public release scope"
