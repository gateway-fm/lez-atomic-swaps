#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workflow="${repo_root}/.github/workflows/ci.yml"
quality_runner="${repo_root}/scripts/run-ci-quality-gates.sh"
pin_checker="${repo_root}/scripts/check-github-action-pins.sh"
spin_checker="${repo_root}/scripts/check-spin-lock-remediation.sh"
spin_test="${repo_root}/scripts/test-spin-lock-remediation.sh"
provisional_verifier="${repo_root}/scripts/verify-lez-v02-provisional.sh"
provisional_methods_build="${repo_root}/compat/lez-v0.2-provisional/escrow/methods/build.rs"
provisional_runner="${repo_root}/scripts/run-m2-taker-sells-lez-poc.sh"
provisional_artifact_manifest="${repo_root}/compat/lez-v0.2-provisional/escrow/methods/guest/deployment-manifest.toml"
canonical_evidence="${repo_root}/docs/evidence/m2-canonical-local-certification-20260714.json"
core_runner="${repo_root}/scripts/run-bitcoin-core-e2e.sh"
core_isolation="${repo_root}/scripts/check-bitcoin-core-isolation.sh"

fail() {
  echo "CI hardening contract failed: $*" >&2
  exit 1
}

require_fixed() {
  local needle="$1"
  local path="$2"
  rg -Fq -- "$needle" "$path" || fail "${path#"${repo_root}/"} is missing: ${needle}"
}

[[ -f "$quality_runner" ]] || fail "missing scripts/run-ci-quality-gates.sh"
[[ -f "$pin_checker" ]] || fail "missing scripts/check-github-action-pins.sh"
[[ -x "$spin_checker" ]] || fail "missing executable spin lock remediation checker"
[[ -x "$spin_test" ]] || fail "missing executable spin lock remediation regression test"
[[ -f "$provisional_verifier" ]] || fail "missing scripts/verify-lez-v02-provisional.sh"
[[ -f "$provisional_methods_build" ]] || fail "missing provisional methods build script"
[[ -f "$provisional_runner" ]] || fail "missing M2 PoC runner"
[[ -f "$provisional_artifact_manifest" ]] || fail "missing provisional artifact manifest"
[[ -x "$core_runner" ]] || fail "missing executable Bitcoin Core E2E runner"
[[ -x "$core_isolation" ]] || fail "missing executable Bitcoin Core isolation checker"
[[ -f "$canonical_evidence" ]] || fail "missing canonical M2 evidence packet"

require_fixed 'tags: ["m*-complete*"]' "$workflow"
require_fixed './scripts/run-ci-quality-gates.sh' "$workflow"
require_fixed './scripts/test-spin-lock-remediation.sh' "$workflow"
require_fixed './scripts/check-spin-lock-remediation.sh' "$workflow"
require_fixed './scripts/check-github-action-pins.sh' "$workflow"

require_fixed './scripts/check-bitcoin-core-isolation.sh' "$workflow"
require_fixed './scripts/run-bitcoin-core-e2e.sh' "$workflow"
require_fixed "git ls-files --cached --others --exclude-standard -z -- '*.sh'" "$quality_runner"
require_fixed 'actionlint_1.7.12_linux_amd64.tar.gz' "$quality_runner"
require_fixed '8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8' "$quality_runner"
require_fixed 'hadolint-linux-x86_64' "$quality_runner"
require_fixed '6bf226944684f56c84dd014e8b979d27425c0148f61b3bd99bcc6f39e9dc5a47' "$quality_runner"
require_fixed 'docker-compose-linux-x86_64' "$quality_runner"
require_fixed 'f9ebc6ebdb19d769b793c245a736caaeb198c62587f13b25c660c13b4987f959' "$quality_runner"
require_fixed 'shellcheck-v0.11.0.linux.x86_64.tar.gz' "$quality_runner"
require_fixed 'b7af85e41cc99489dcc21d66c6d5f3685138f06d34651e6d34b42ec6d54fe6f6' "$quality_runner"
require_fixed '"$shellcheck" --severity=warning' "$quality_runner"
require_fixed 'git ls-files --cached --others --exclude-standard -z' "$quality_runner"
require_fixed 'config --quiet' "$quality_runner"

require_fixed 'Scan repository-owned runtime base for high and critical vulnerabilities' "$workflow"
require_fixed 'gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7' "$workflow"
require_fixed 'Report high and critical vulnerabilities in exact Logos Bedrock dependency' "$workflow"
require_fixed 'ghcr.io/logos-blockchain/logos-blockchain@sha256:91d6c5bf07e07fcfba5e7cf07d21ee686a6bc4b9f6210f2d28bffbcad9a3729f' "$workflow"
require_fixed 'Repository-owned findings are fail-hard; Logos-owned findings remain visible' "$workflow"
require_fixed 'rapidsnark_root="${RAPIDSNARK_LIB_DIR%/rapidsnark-linux-x86_64-pic-v0.0.8/lib}"' "$workflow"
require_fixed 'unzip -q "${rapidsnark_archive}" -d "${rapidsnark_root}"' "$workflow"

require_fixed 'isolated Bitcoin Core 31.1 Regtest actor smoke' "$workflow"
require_fixed 'BITCOIN_CORE_E2E_KEEP_RUNNING: "1"' "$workflow"
require_fixed 'Scan exact Bitcoin Core image for high and critical vulnerabilities' "$workflow"
require_fixed 'image-ref: lez-atomic-swaps-bitcoin-core:github-btc-${{ github.run_id }}-${{ github.run_attempt }}' "$workflow"
require_fixed 'docker container rm --force "${container}"' "$workflow"
require_fixed 'org.logos-co.atomic-swaps.run=${RUN_ID}' "$workflow"
require_fixed 'cargo check --locked --manifest-path "$guest_manifest" --bins' "$provisional_verifier"
require_fixed 'cargo clippy --locked --manifest-path "$guest_manifest" --bins -- -D warnings' "$provisional_verifier"
require_fixed 'risc0_rust_version="1.94.1"' "$provisional_verifier"
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
require_fixed 'expected_elf_sha256="c85055f6fe85b71535a322ba84ffc612f5d093954a721ba3b529428814dc9d2e"' "$provisional_verifier"
require_fixed 'expected_image_id="5cf8c5a4eedb3c2873956cb7898eb33a495407c9746fb1a065c99638159329c1"' "$provisional_verifier"
require_fixed 'ESCROW_PROGRAM_ID="${ESCROW_PROGRAM_ID:-5cf8c5a4eedb3c2873956cb7898eb33a495407c9746fb1a065c99638159329c1}"' "$provisional_runner"
require_fixed 'elf_sha256 = "c85055f6fe85b71535a322ba84ffc612f5d093954a721ba3b529428814dc9d2e"' "$provisional_artifact_manifest"
require_fixed 'image_id = "5cf8c5a4eedb3c2873956cb7898eb33a495407c9746fb1a065c99638159329c1"' "$provisional_artifact_manifest"

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
