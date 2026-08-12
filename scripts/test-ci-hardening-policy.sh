#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workflow="${repo_root}/.github/workflows/ci.yml"
quality_runner="${repo_root}/scripts/run-ci-quality-gates.sh"
pin_checker="${repo_root}/scripts/check-github-action-pins.sh"
cost_policy_checker="${repo_root}/scripts/check-lez-cost-evidence.sh"
cost_policy_test="${repo_root}/scripts/test-lez-cost-evidence.sh"
standalone_runner="${repo_root}/scripts/run-lez-standalone-e2e.sh"
spin_checker="${repo_root}/scripts/check-spin-lock-remediation.sh"
release_process_runner="${repo_root}/scripts/test-m4-xmr-release-worker-process.sh"
spin_test="${repo_root}/scripts/test-spin-lock-remediation.sh"
provisional_verifier="${repo_root}/scripts/verify-lez-v02-provisional.sh"
provisional_deployment_contract="${repo_root}/scripts/test-m4-lez-local-deployment-contract.sh"
provisional_methods_build="${repo_root}/compat/lez-v0.2-provisional/escrow/methods/build.rs"
provisional_runner="${repo_root}/scripts/run-m2-taker-sells-lez-poc.sh"
provisional_artifact_manifest="${repo_root}/compat/lez-v0.2-provisional/escrow/methods/guest/deployment-manifest.toml"
canonical_evidence="${repo_root}/docs/evidence/m2-canonical-local-certification-20260714.json"
core_runner="${repo_root}/scripts/run-bitcoin-core-e2e.sh"
core_isolation="${repo_root}/scripts/check-bitcoin-core-isolation.sh"
btc_sdk_manifest="${repo_root}/crates/btc-swap-sdk/Cargo.toml"
core_adapter_manifest="${repo_root}/crates/btc-core-adapter/Cargo.toml"

core_evidence="${repo_root}/docs/evidence/m3-bitcoin-core-smoke-a7393df-20260714.json"
core_musig_evidence="${repo_root}/docs/evidence/m3-bitcoin-core-musig2-f5a9caa-20260715.json"
fail() {
  echo "CI hardening contract failed: $*" >&2
  exit 1
}

require_fixed() {
  local needle="$1"
  local path="$2"
  rg -Fq -- "$needle" "$path" || fail "${path#"${repo_root}/"} is missing: ${needle}"
}

workflow_step() {
  local name="$1"
  awk -v marker="      - name: ${name}" '
    $0 == marker {
      found = 1
    }
    found && $0 != marker && $0 ~ /^      - / {
      exit
    }
    found {
      print
    }
    END {
      if (!found) {
        exit 1
      }
    }
  ' "$workflow"
}

assert_trivy_step() {
  local name="$1"
  local image="$2"
  local exit_code="$3"
  local step
  local required
  local required_fields=(
    'uses: aquasecurity/trivy-action@ed142fd0673e97e23eac54620cfb913e5ce36c25'
    "image-ref: ${image}"
    'format: table'
    "exit-code: \"${exit_code}\""
    'ignore-unfixed: false'
    'vuln-type: os,library'
    'severity: HIGH,CRITICAL'
  )
  step="$(workflow_step "$name")" || fail "missing Trivy step: ${name}"
  for required in "${required_fields[@]}"; do
    rg -Fq -- "$required" <<<"$step" \
      || fail "Trivy step '${name}' is missing: ${required}"
  done
  if rg -Fq -- 'continue-on-error' <<<"$step"; then
    fail "Trivy step '${name}' must not continue on error"
  fi
}

[[ -f "$quality_runner" ]] || fail "missing scripts/run-ci-quality-gates.sh"
[[ -f "$pin_checker" ]] || fail "missing scripts/check-github-action-pins.sh"
[[ -x "$spin_checker" ]] || fail "missing executable spin lock remediation checker"
[[ -x "$release_process_runner" ]] || fail "missing executable M4 release process runner"
[[ -x "$spin_test" ]] || fail "missing executable spin lock remediation regression test"
[[ -f "$provisional_verifier" ]] || fail "missing scripts/verify-lez-v02-provisional.sh"
[[ -x "$provisional_deployment_contract" ]] || fail "missing executable M4 LEZ deployment contract test"
[[ -x "$cost_policy_checker" ]] || fail "missing executable LEZ cost evidence checker"
[[ -x "$cost_policy_test" ]] || fail "missing executable LEZ cost evidence regression test"
[[ -x "$standalone_runner" ]] || fail "missing executable LEZ standalone runner"
[[ -f "$provisional_methods_build" ]] || fail "missing provisional methods build script"
[[ -f "$provisional_runner" ]] || fail "missing M2 PoC runner"
[[ -f "$provisional_artifact_manifest" ]] || fail "missing provisional artifact manifest"
[[ -x "$core_runner" ]] || fail "missing executable Bitcoin Core E2E runner"
[[ -x "$core_isolation" ]] || fail "missing executable Bitcoin Core isolation checker"
[[ -f "$core_evidence" ]] || fail "missing retained Bitcoin Core evidence"
[[ -f "$core_musig_evidence" ]] || fail "missing retained Bitcoin Core MuSig2 evidence"
[[ -f "$canonical_evidence" ]] || fail "missing canonical M2 evidence packet"

require_fixed 'tags: ["m*-complete*"]' "$workflow"
require_fixed './scripts/run-ci-quality-gates.sh' "$workflow"
require_fixed './scripts/test-m7-review-readiness-contract.sh' "$workflow"
require_fixed './scripts/test-m7-hard-requirements-audit.sh' "$quality_runner"
require_fixed './scripts/test-m7-submission-requirements-audit.sh' "$quality_runner"
require_fixed './scripts/test-m7-xmr-sdk-facade-contract.sh' "$quality_runner"
require_fixed './scripts/test-m7-xmr-accepted-concurrency-contract.sh' "$quality_runner"
require_fixed './scripts/test-m7-route-health-contract.sh' "$quality_runner"
require_fixed './scripts/test-m7-maker-all-pair-action-matrix.sh' "$quality_runner"
require_fixed './scripts/test-m7-zec-taker-direction-process.sh' "$quality_runner"
require_fixed './scripts/test-m7-zec-first-lock-refund-contract.sh' "$quality_runner"
require_fixed './scripts/test-m7-zec-first-lock-refund-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-zebra-reorg-evidence-contract.sh' "$quality_runner"
require_fixed './scripts/test-m7-zebra-reorg-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-zebra-application-reorg-contract.sh' "$quality_runner"
require_fixed './scripts/test-m7-zebra-application-reorg-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-tag17-actual-poc-contract.sh' "$quality_runner"
require_fixed './scripts/test-m7-tag17-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-taker-claim-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-taker-claim-process-kill-contract.sh' "$quality_runner"
require_fixed './scripts/test-m7-taker-claim-process-kill-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-taker-claim-sweep-process-kill-contract.sh' "$quality_runner"
require_fixed './scripts/test-m7-taker-claim-sweep-process-kill-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-maker-tag15-process-kill-contract.sh' "$quality_runner"
require_fixed './scripts/test-m7-maker-tag15-process-kill-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-btc-accepted-concurrency-contract.sh' "$quality_runner"
require_fixed './scripts/test-m7-btc-accepted-concurrency-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-zec-accepted-process-kill-contract.sh' "$quality_runner"
require_fixed './scripts/test-m7-zec-accepted-process-kill-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-maker-refund-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-maker-refund-process-kill-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-r4-recovery-baseline-contract.sh' "$quality_runner"
require_fixed './scripts/test-m7-f7-custom-token-refund-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-joined-abandonment-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-losing-tag16-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-losing-tag17-actual-certificate.sh' "$quality_runner"
require_fixed './scripts/test-m7-spel-idl-contract.sh' "$quality_runner"
require_fixed './scripts/test-monero-stagenet-guide-contract.sh' "$quality_runner"
require_fixed './scripts/test-zcash-testnet-guide-contract.sh' "$quality_runner"
require_fixed './scripts/test-spin-lock-remediation.sh' "$workflow"
require_fixed './scripts/check-spin-lock-remediation.sh' "$workflow"
require_fixed './scripts/test-m4-xmr-release-worker-process.sh' "$workflow"
require_fixed './scripts/test-m4-lez-local-deployment-contract.sh' "$workflow"
require_fixed './scripts/check-github-action-pins.sh' "$workflow"
require_fixed 'npm audit --audit-level=moderate' "$workflow"
require_fixed 'npm run audit:licenses' "$workflow"
require_fixed 'actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444' "$workflow"
require_fixed 'node-version: 24.18.0' "$workflow"
require_fixed 'PUPPETEER_EXECUTABLE_PATH: /usr/bin/google-chrome' "$workflow"
require_fixed 'PUPPETEER_SKIP_DOWNLOAD: "true"' "$workflow"
require_fixed 'M6_UI_TEST_ROOT: ${{ runner.temp }}/lez-m6-ui-${{ github.run_id }}-${{ github.run_attempt }}' "$workflow"
require_fixed 'google-chrome --version' "$workflow"
require_fixed 'npm run test:m6:prototype' "$workflow"
require_fixed 'npm run test:m6:basecamp:contract' "$workflow"
require_fixed './scripts/test-lez-cost-evidence.sh' "$workflow"
require_fixed './scripts/check-lez-cost-evidence.sh "$cost_evidence" "$cost_json"' \
  "$standalone_runner"
require_fixed 'cargo fmt --all --check' "$workflow"
require_fixed 'cargo clippy --locked --workspace --all-targets --all-features -- -D warnings' \
  "$workflow"
require_fixed 'cargo test --locked --workspace --all-targets' "$workflow"
require_fixed 'cargo test --locked -p lez-maker-node --features test-crash-hooks --test daemon_actor_supervisor_cli' \
  "$workflow"
require_fixed 'cargo doc --locked --workspace --all-features --no-deps' "$workflow"

readonly cargo_deny_action='uses: EmbarkStudios/cargo-deny-action@bb137d7af7e4fb67e5f82a49c4fce4fad40782fe'
readonly cargo_deny_policy='advisories bans licenses sources'
readonly expected_cargo_deny_steps=13
cargo_deny_steps="$(rg -Fc -- "$cargo_deny_action" "$workflow")"
cargo_deny_policy_steps="$(rg -Fc -- "$cargo_deny_policy" "$workflow")"
[[ "$cargo_deny_steps" == "$expected_cargo_deny_steps" ]] \
  || fail "expected ${expected_cargo_deny_steps} pinned cargo-deny steps; found ${cargo_deny_steps}"
[[ "$cargo_deny_policy_steps" == "$expected_cargo_deny_steps" ]] \
  || fail "every cargo-deny step must check advisories, bans, licenses, and sources"

cargo_deny_manifests=(
  fuzz/Cargo.toml
  compat/spel-zec-escrow/Cargo.toml
  compat/spel-zec-escrow/methods/guest/Cargo.toml
  compat/spel-zec-escrow/methods/Cargo.toml
  compat/lez-standalone-e2e/Cargo.toml
  compat/lez-v0_1_2-sidecar/Cargo.toml
  compat/lez-v0_2-sidecar/Cargo.toml
  compat/lez-v0_2-xmr-release-service/Cargo.toml
  compat/lez-v0.2-provisional/Cargo.toml
  compat/lez-v0.2-provisional/escrow/methods/Cargo.toml
  compat/lez-v0.2-provisional/escrow/methods/guest/Cargo.toml
  compat/lez-v0.2-provisional/escrow/deployer/Cargo.toml
)
for manifest in "${cargo_deny_manifests[@]}"; do
  require_fixed "manifest-path: ${manifest}" "$workflow"
done

require_fixed './scripts/check-bitcoin-core-isolation.sh' "$workflow"
require_fixed './scripts/check-lez-v02-docker-isolation.sh' "$workflow"
require_fixed './scripts/run-bitcoin-core-e2e.sh' "$workflow"
require_fixed "git ls-files --cached --others --exclude-standard -z -- '*.sh'" "$quality_runner"
require_fixed 'M4_XMR_RELEASE_WORKER_BIN="${worker_binary}" cargo test' "$release_process_runner"
require_fixed 'subprocess_worker_admits_once_and_restart_observes_only' "$release_process_runner"
require_fixed '--ignored' "$release_process_runner"
release_runner_cargo_arg_uses="$(rg -Fc -- '"${cargo_args[@]}"' "$release_process_runner")"
[[ "$release_runner_cargo_arg_uses" == 2 ]] || fail "M4 release runner must apply locked/offline arguments to build and test"

require_fixed 'actionlint_1.7.12_linux_amd64.tar.gz' "$quality_runner"
require_fixed '8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8' "$quality_runner"
require_fixed 'hadolint-linux-x86_64' "$quality_runner"
require_fixed '6bf226944684f56c84dd014e8b979d27425c0148f61b3bd99bcc6f39e9dc5a47' "$quality_runner"
require_fixed 'docker-compose-linux-x86_64' "$quality_runner"
require_fixed 'f9ebc6ebdb19d769b793c245a736caaeb198c62587f13b25c660c13b4987f959' "$quality_runner"
require_fixed 'shellcheck-v0.11.0.linux.x86_64.tar.gz' "$quality_runner"
require_fixed 'b7af85e41cc99489dcc21d66c6d5f3685138f06d34651e6d34b42ec6d54fe6f6' "$quality_runner"
require_fixed '"$shellcheck" --severity=warning' "$quality_runner"
require_fixed 'M3_ACTOR_CONTRACT_REQUIRE_BINARIES=0 ./scripts/test-m3-actor-local-poc-contract.sh' \
  "$quality_runner"
require_fixed './scripts/test-m3-node-startup-coordinator.sh' "$quality_runner"
require_fixed './scripts/test-m3-phase-timings-contract.sh' "$quality_runner"
require_fixed './scripts/test-m3-direction-phase-timings-contract.sh' "$quality_runner"
require_fixed './scripts/test-m3-f7-token-fixture-contract.sh' "$quality_runner"
require_fixed './scripts/test-m3-private-recording-contract.sh' "$quality_runner"
require_fixed './scripts/check-m3-cryptographic-vectors.sh' "$quality_runner"
require_fixed './scripts/test-bitcoin-testnet4-route-contract.sh' "$quality_runner"
require_fixed 'Verify M3 official cryptographic vectors' "$workflow"
require_fixed 'cargo test --locked -p lez-btc-swap-sdk --test bip340_vectors --test bip327_vectors --test adaptor_vectors' "$workflow"
require_fixed 'Verify Bitcoin Testnet4 route contract' "$workflow"
require_fixed './scripts/test-bitcoin-testnet4-route-contract.sh' "$workflow"
require_fixed 'k256 = { version = "=0.13.4", default-features = false, features = ["schnorr"] }' \
  "$btc_sdk_manifest"
require_fixed 'jsonrpsee-http-client = { version = "=0.26.0", default-features = false, features = ["tls"] }' \
  "$core_adapter_manifest"
require_fixed 'git ls-files --cached --others --exclude-standard -z' "$quality_runner"
require_fixed 'config --quiet' "$quality_runner"

require_fixed 'Scan repository-owned runtime base for high and critical vulnerabilities' "$workflow"
require_fixed 'gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7' "$workflow"
require_fixed 'Report high and critical vulnerabilities in exact Logos Bedrock dependency' "$workflow"
require_fixed 'ghcr.io/logos-blockchain/logos-blockchain@sha256:91d6c5bf07e07fcfba5e7cf07d21ee686a6bc4b9f6210f2d28bffbcad9a3729f' "$workflow"
require_fixed 'Runtime findings fail-hard; classified upstream findings remain visible' "$workflow"
require_fixed 'rapidsnark_root="${RAPIDSNARK_LIB_DIR%/rapidsnark-linux-x86_64-pic-v0.0.8/lib}"' "$workflow"
require_fixed 'unzip -q "${rapidsnark_archive}" -d "${rapidsnark_root}"' "$workflow"

assert_trivy_step 'Scan repository-owned runtime base for high and critical vulnerabilities' 'gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7' 1
assert_trivy_step 'Report pinned Risc0 guest builder high and critical vulnerabilities' 'risczero/risc0-guest-builder:r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be' 0
assert_trivy_step 'Report high and critical vulnerabilities in exact Logos Bedrock dependency' 'ghcr.io/logos-blockchain/logos-blockchain@sha256:91d6c5bf07e07fcfba5e7cf07d21ee686a6bc4b9f6210f2d28bffbcad9a3729f' 0
assert_trivy_step 'Scan minimal Zebra image for high and critical vulnerabilities' '${{ env.ZEBRA_IMAGE }}' 1
assert_trivy_step 'Scan exact Bitcoin Core image for high and critical vulnerabilities' 'lez-atomic-swaps-bitcoin-core:github-btc-${{ github.run_id }}-${{ github.run_attempt }}' 1
report_only_scans="$(rg -Fc -- 'exit-code: "0"' "$workflow")"
[[ "$report_only_scans" == "2" ]] \
  || fail "exactly two classified report-only vulnerability scans are allowed"

require_fixed 'isolated Bitcoin Core 31.1 Regtest MuSig2 adaptor P2TR spend' "$workflow"
require_fixed 'Verify release and run role-aware MuSig2 adaptor P2TR funding and claim' "$workflow"
require_fixed 'BITCOIN_CORE_E2E_KEEP_RUNNING: "1"' "$workflow"
require_fixed 'BITCOIN_CORE_E2E_REQUIRE_CLEAN: "1"' "$workflow"
require_fixed 'readonly actor_allowlist="getblockchaininfo,getnetworkinfo,getblockhash,getblock,getblockheader,getrawtransaction,gettxout,gettxspendingprevout,getindexinfo,getmempoolinfo,getrawmempool,getmempoolentry,testmempoolaccept,sendrawtransaction"' "$core_runner"
require_fixed 'create_basic_credentials' "$core_runner"
require_fixed 'BITCOIN_CORE_MAKER_BASIC_CREDENTIALS=' "$core_runner"
require_fixed 'BITCOIN_CORE_TAKER_BASIC_CREDENTIALS=' "$core_runner"
require_fixed 'Scan exact Bitcoin Core image for high and critical vulnerabilities' "$workflow"
require_fixed 'image-ref: lez-atomic-swaps-bitcoin-core:github-btc-${{ github.run_id }}-${{ github.run_attempt }}' "$workflow"
require_fixed 'docker container rm --force "${container}"' "$workflow"
require_fixed 'org.logos-co.atomic-swaps.run=${RUN_ID}' "$workflow"
require_fixed 'cargo check --locked --manifest-path "$guest_manifest" --bins' "$provisional_verifier"
require_fixed 'cargo clippy --locked --manifest-path "$guest_manifest" --bins -- -D warnings' "$provisional_verifier"
require_fixed 'risc0_rust_version="1.94.1"' "$provisional_verifier"

jq -e '
  .schema_version == 1
  and .milestone == "M3"
  and .scope == "bitcoin_core_infrastructure_only"
  and .result == "passed"
  and .tested_repository_commit == "a7393dfb74dc4113a0cb58a54528b3fd6268d0ef"
  and .tested_origin_main_commit == .tested_repository_commit
  and .worktree_clean_before_run == true
  and .core.version == "31.1"
  and .chain.network == "regtest"
  and .chain.final_height == 101
  and .isolation.rpc_publication == "dynamic_literal_loopback_only"
  and .isolation.p2p_port_published == false
  and .isolation.core_network_active == false
  and .isolation.peers_before == 0
  and .isolation.peers_after == 0
  and .actor_rpc.credentials_distinct == true
  and .actor_rpc.plaintext_credentials_disclosed == false
  and .actor_rpc.forbidden_http_403_count > 0
  and .actor_rpc.crossed_credentials_http_401_count == 2
  and .external_dependencies.runtime_external_resources == []
  and .external_dependencies.public_rpc_used == false
  and .external_dependencies.faucet_used == false
  and .external_dependencies.public_funds_used == false
  and .cleanup.status == "passed"
  and .cleanup.exact_run_resources_absent == true
  and .cleanup.foreign_sentinel_survived_exact_cleanup == true
  and (.security.ci_exact_image_trivy_fail_high_critical
    | contains("remote_result_not_locally_observed"))
' "$core_evidence" >/dev/null || fail "retained Bitcoin Core evidence invariants failed"

jq -e '
  .schema_version == 1
  and .milestone == "M3"
  and .scope == "bitcoin_core_musig2_adaptor_p2tr_fixture"
  and .result == "passed"
  and .tested_repository_commit == "f5a9caa66b04b0bec1a86cb732f5a64f63852e6e"
  and .tested_origin_main_commit == .tested_repository_commit
  and .worktree_clean_before_run == true
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
  and .security_claims.nonce_commitment_exchange_proven == false
  and .security_claims.crash_safe_nonce_journal_proven == false
  and .security_claims.lez_composition_proven == false
  and .security_claims.atomicity_proven == false
  and .external_dependencies.runtime_external_resources == []
  and .cleanup.exact_run_resources_absent == true
  and .cleanup.foreign_sentinel_survived_exact_cleanup == true
' "$core_musig_evidence" >/dev/null || fail "retained Bitcoin Core MuSig2 evidence invariants failed"

require_fixed 'risc0_guest_builder_tag="r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be"' "$provisional_verifier"
require_fixed 'risc0_guest_builder="risczero/risc0-guest-builder:${risc0_guest_builder_tag}"' "$provisional_verifier"
require_fixed 'export RISC0_DOCKER_CONTAINER_TAG="$risc0_guest_builder_tag"' "$provisional_verifier"
require_fixed 'guest_build_manifest_dir="${guest_build_root}/escrow/methods/guest"' "$provisional_verifier"
require_fixed 'guest_build_contract_dir="${guest_build_root}/escrow/src"' "$provisional_verifier"
require_fixed 'cp "compat/lez-v0.2-provisional/escrow/src/lib.rs" "$guest_build_contract_dir/lib.rs"' "$provisional_verifier"
require_fixed 'export CARGO_TARGET_DIR="${guest_build_root}/target"' "$provisional_verifier"
require_fixed 'risc0_build::embed_methods_with_options' "$provisional_methods_build"
require_fixed 'r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be' "$provisional_methods_build"
require_fixed '.root_dir("../..")' "$provisional_methods_build"
require_fixed 'expected_elf_sha256="ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee"' "$provisional_verifier"
require_fixed 'expected_image_id="b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"' "$provisional_verifier"
require_fixed 'expected_idl_sha256="04895050affb173d3e87329994ecbbed54781a38d5454ce5b36e155916e4134f"' "$provisional_verifier"
require_fixed 'ESCROW_PROGRAM_ID="${ESCROW_PROGRAM_ID:-b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433}"' "$provisional_runner"
require_fixed 'elf_sha256 = "ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee"' "$provisional_artifact_manifest"
require_fixed 'image_id = "b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433"' "$provisional_artifact_manifest"
require_fixed 'idl_sha256 = "04895050affb173d3e87329994ecbbed54781a38d5454ce5b36e155916e4134f"' "$provisional_artifact_manifest"
require_fixed 'instruction_count = 18' "$provisional_artifact_manifest"
require_fixed 'initialize_token_witnessed_variant = 11' "$provisional_artifact_manifest"
require_fixed 'claim_token_witnessed_variant = 12' "$provisional_artifact_manifest"

jq -e '
  .schema_version == 1
  and .runtime_implementation_base_commit == "bb53daf40d3b30def7dee173a2577dc691de01f8"
  and .canonical_build.artifact.elf_sha256 == "c85055f6fe85b71535a322ba84ffc612f5d093954a721ba3b529428814dc9d2e"
  and .canonical_build.artifact.image_id == "5cf8c5a4eedb3c2873956cb7898eb33a495407c9746fb1a065c99638159329c1"
  and .canonical_build.artifact.direct_docker_equals_methods_embedded == true
  and .canonical_local_deployment.bedrock_status == "Finalized"
  and .canonical_local_deployment.inclusion_block_id == 2582
  and ([.corridors[].direction] | sort) == ["taker_sells_foreign", "taker_sells_lez"]
  and all(.corridors[];
    .result == "completed"
    and .actors.maker.terminal_revision == 4
    and .actors.taker.terminal_revision == 4
    and .zcash_htlc.claim_spent_exact_htlc_outpoint == true)
  and ([.corridors[].lez_native_escrow | .. | objects |
    select(has("metadata_status")) | .custody_balance] == [0, 0])
  and .atomicity_evidence.both_terminal_outcomes == "both_claimed"
  and .assertions.all_six_lez_transactions_in_finalized_blocks == true
  and .assertions.superseded_f838_program_absent_from_both_actor_pairs == true
  and .assertions.public_rpc_or_faucet_used == false
  and .cleanup_and_secret_boundary.complete_resource_cleanup_claimed == false
' "$canonical_evidence" >/dev/null || fail "canonical M2 evidence invariants failed"

historical_evidence=(
  "37203dc770b6649f0486dfafc72810ff7637dea2b89b90f4e963062cf2943ab5 docs/evidence/m2-local-onboarding-20260714.json"
  "3184ed8a1bb9bcd32c3474940db24340e8dad1aafa48561163d5b75abb2bf942 docs/evidence/m2-taker-sells-lez-corridor-20260714.json"
  "ba933bc4b129bce2f13a7b35e496de1fbed2a1401384147c43688192718afb73 docs/evidence/m2-taker-sells-foreign-corridor-20260714.json"
  "b862f88133a4f1248164b8b940c96ec213c470541c4ab2767398cc48591710e8 docs/evidence/m2-schema-v3-local-corridors-20260714.json"
)
for historical in "${historical_evidence[@]}"; do
  expected="${historical%% *}"
  relative_path="${historical#* }"
  actual="$(sha256sum "${repo_root}/${relative_path}" | cut -d " " -f 1)"
  [[ "$actual" == "$expected" ]] || fail "immutable evidence changed: ${relative_path}"
done
if rg -Fq 'risc0_build::embed_methods();' "$provisional_methods_build"; then
  fail "v0.2 methods must embed the canonical Docker-built guest"
fi
require_fixed 'cargo risczero build --manifest-path escrow/methods/guest/Cargo.toml' "$provisional_verifier"
if rg -Fq 'cargo test --locked --manifest-path "$guest_manifest"' "$provisional_verifier"; then
  fail "zkVM guest verifier must not build a host unit-test harness"
fi

if rg -Uq $'readonly REPOSITORY_ROOT\nREPOSITORY_ROOT=|readonly repository_root\nrepository_root=' \
    "${repo_root}/scripts"; then
  fail "a shell wrapper assigns a variable only after marking it readonly"
fi

"$pin_checker"

echo "CI hardening contract is complete"
