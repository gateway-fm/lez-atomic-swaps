#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  echo "M7 SPEL IDL/client contract failed: $*" >&2
  exit 1
}

readonly provisional_manifest="compat/lez-v0.2-provisional/Cargo.toml"
readonly contract="compat/lez-v0.2-provisional/escrow/src/lib.rs"
readonly provisional_build="compat/lez-v0.2-provisional/build.rs"
readonly deployment_test="compat/lez-v0.2-provisional/tests/escrow_deployment.rs"
readonly sidecar_build="compat/lez-v0_2-sidecar/build.rs"
readonly artifact_manifest="compat/lez-v0.2-provisional/escrow/methods/guest/deployment-manifest.toml"
readonly checked_artifact_manifest="compat/lez-v0.2-provisional/escrow/methods/guest/m4-deployment-manifest.toml"
readonly artifact_runner="scripts/run-m4-lez-artifact-tests.sh"
readonly artifact_verifier="scripts/verify-lez-v02-provisional.sh"
readonly sidecar_verifier="scripts/verify-lez-v02-sidecar.sh"
readonly adr="docs/architecture/0151-freeze-generated-spel-custody-abi.md"
readonly idl_sha256="04895050affb173d3e87329994ecbbed54781a38d5454ce5b36e155916e4134f"
readonly client_sha256="bcc0d3898343317bdd3bcc0987ec9559db7f4060c4e9fb45f096d1bcd34b48ac"

for path in \
  "$provisional_manifest" \
  "$contract" \
  "$provisional_build" \
  "$deployment_test" \
  "$sidecar_build" \
  "$artifact_manifest" \
  "$checked_artifact_manifest" \
  "$artifact_runner" \
  "$artifact_verifier" \
  "$sidecar_verifier" \
  "$adr"; do
  [[ -s "$path" ]] || fail "missing ${path}"
done

deployment_manifest_sha256="$(sha256sum "$artifact_manifest" | awk '{print $1}')"
artifact_runner_sha256="$(sha256sum "$artifact_runner" | awk '{print $1}')"
rg -Fqx "deployment_manifest_sha256 = \"${deployment_manifest_sha256}\"" \
  "$checked_artifact_manifest" || fail "checked artifact manifest does not bind the deployment manifest"
rg -Fq "require_sha256 \"${deployment_manifest_sha256}\"" "$artifact_runner" ||
  fail "artifact runner source boundary does not bind the deployment manifest"
rg -Fqx "artifact_runner_sha256 = \"${artifact_runner_sha256}\"" \
  "$checked_artifact_manifest" || fail "checked artifact manifest does not bind its runner"
"$artifact_runner" verify-source >/dev/null || fail "artifact source boundary is not executable"

rg -Fq 'spel_commit = "df17acd98436be4f09c55877dae1fe2e73cbcdca"' \
  "$provisional_manifest" || fail "provisional package does not pin the reviewed SPEL commit"
rg -Fq 'lez_tag = "v0.2.0"' "$provisional_manifest" \
  || fail "provisional package does not pin LEZ v0.2.0"

for build in "$provisional_build" "$sidecar_build"; do
  rg -Fq 'spel_client_gen::generate_from_idl_json(lez_zec_escrow_v02::PROGRAM_IDL_JSON)' \
    "$build" || fail "${build} does not generate from the exact escrow IDL"
done
for surface in \
  assert_native_prepare_surface \
  assert_xmr_prepare_surface \
  assert_token_prepare_surface; do
  rg -Fq "$surface" "$sidecar_build" \
    || fail "runtime sidecar does not assert ${surface}"
done

rg -Fq 'Sha256::digest(PROGRAM_IDL_JSON.as_bytes())' "$deployment_test" \
  || fail "deployment test does not derive the raw generated IDL digest"
rg -Fq 'Sha256::digest(generated.as_bytes())' "$deployment_test" \
  || fail "deployment test does not derive the generated client digest"
rg -Fq "$idl_sha256" "$deployment_test" \
  || fail "deployment test does not pin the current IDL digest"
rg -Fq "$client_sha256" "$deployment_test" \
  || fail "deployment test does not pin the current generated client digest"
for evidence in \
  'accounts.depositor_owner' \
  'accounts.aggregate_authority' \
  'create_token_custody' \
  'claim_token_witnessed'; do
  rg -Fq "$evidence" "$deployment_test" "$sidecar_build" \
    || fail "missing generated-client custody evidence: ${evidence}"
done

rg -Fqx "idl_sha256 = \"${idl_sha256}\"" "$artifact_manifest" \
  || fail "deployment manifest IDL digest drift"
rg -Fqx "generated_client_sha256 = \"${client_sha256}\"" "$artifact_manifest" \
  || fail "deployment manifest generated-client digest drift"
rg -Fq "expected_idl_sha256=\"${idl_sha256}\"" "$artifact_verifier" \
  || fail "artifact verifier IDL digest drift"
rg -Fq "expected_generated_client_sha256=\"${client_sha256}\"" "$artifact_verifier" \
  || fail "artifact verifier generated-client digest drift"
rg -Fq 'generated_client_sha256 = \"${expected_generated_client_sha256}\"' \
  "$artifact_verifier" || fail "artifact verifier does not bind the client digest to the manifest"

rg -Fq './scripts/test-m7-spel-idl-contract.sh' scripts/run-ci-quality-gates.sh \
  || fail "CI quality gates do not run this contract"
rg -Fq './scripts/test-m7-spel-idl-contract.sh' scripts/test-ci-hardening-policy.sh \
  || fail "CI hardening policy does not pin this contract"
rg -Fq './scripts/verify-lez-v02-provisional.sh' .github/workflows/ci.yml \
  || fail "CI does not run the artifact-level v0.2 verifier"
rg -Fq './scripts/verify-lez-v02-sidecar.sh' .github/workflows/ci.yml \
  || fail "CI does not run the runtime-sidecar verifier"
for mode in '--locked' '--offline'; do
  rg -Fq -- "$mode" "$sidecar_verifier" \
    || fail "runtime-sidecar verifier does not enforce ${mode}"
done

echo "M7 SPEL IDL/client custody-ABI contract passed"
