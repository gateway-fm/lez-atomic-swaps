#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly certificate="docs/evidence/m7-private-demo-video-bundle-1b47a15-20260812.json"
readonly source_map="docs/evidence/m7-private-demo-sources.json"

fail() {
  echo "M7 private demo-video certificate test failed: $*" >&2
  exit 1
}

for dependency in jq rg sha256sum; do
  command -v "$dependency" >/dev/null || fail "missing test dependency: ${dependency}"
done
[[ -f "$certificate" && ! -L "$certificate" ]] || fail "checked certificate is missing or unsafe"
[[ -f "$source_map" && ! -L "$source_map" ]] || fail "source map is missing or unsafe"

jq -e '
  .schema_version == 1 and .kind == "m7_private_demo_video_certificate" and
  .result == "passed" and .privacy == "private_local_stealth" and
  .source_repository_commit == "1b47a158140c3336f8d5e0ac2e2b3e7a8ce12876" and
  .renderer_repository_commit == .source_repository_commit and
  .verifier_repository_commit == .source_repository_commit and
  .source_map_sha256 == "2a2d1892f62681778ebb954a81629262c9326be21c623664cc708b7c45943d15" and
  .private_bundle_sha256 == "a23b7d32b11ce91a44875750c82bccc470659cac380de90d61c9e7b2e743bf5b" and
  .renderer_image == "ghcr.io/charmbracelet/vhs@sha256:9d5fc3dc0c160b0fb1d2212baff07e6bdf3fa9438c504a3237484567302fcf93" and
  .recorded_at == "2026-08-12T20:03:17Z" and
  .pairs == ["xmr","zec"] and .scenarios == ["happy","refund","concurrent"] and
  (.videos | length) == 6 and
  ([.videos[] | [.pair,.scenario] | join(":")] | sort ==
    ["xmr:concurrent","xmr:happy","xmr:refund","zec:concurrent","zec:happy","zec:refund"]) and
  ([.videos[].run_id] | unique | length) == 6 and
  ([.videos[].manifest_sha256] | unique | length) == 6 and
  ([.videos[].video_sha256] | unique | length) == 6 and
  all(.videos[];
    (.manifest_sha256 | test("^[0-9a-f]{64}$")) and
    (.video_sha256 | test("^[0-9a-f]{64}$")) and
    (.duration_seconds | tonumber) > 20 and
    (.duration_seconds | tonumber) < 30 and
    .size_bytes > 100000 and .size_bytes < 1000000) and
  ([.videos[] | select(.pair == "zec" and .scenario == "concurrent")] | length) == 1 and
  (.videos[] | select(.pair == "zec" and .scenario == "concurrent") |
    .evidence_model == "layered_process_concurrency_plus_actual_node_pair_effects" and
    .joined_actual_node_concurrency == false) and
  all(.videos[] | select(.pair != "zec" or .scenario != "concurrent");
    .evidence_model == "joined_actual_nodes" and .joined_actual_node_concurrency == true) and
  .zec_concurrent_evidence_model == "layered_process_concurrency_plus_actual_node_pair_effects" and
  .zec_concurrent_joined_actual_node_run == false and
  .public_rpc_used == false and .public_peer_used == false and
  .faucet_used == false and .public_funds_used == false and
  .public_deployment_used == false and
  .certification_success_depends_on_external_network == false and
  .private_video_bytes_checked_into_git == false and
  .security_assessment_claimed == false
' "$certificate" >/dev/null || fail "certificate invariants are incomplete or inconsistent"

[[ "$(sha256sum "$source_map" | cut -d ' ' -f 1)" == "$(jq -er '.source_map_sha256' "$certificate")" ]] ||
  fail "source map hash no longer matches the rendered bundle"

for pair in xmr zec; do
  for scenario in happy refund concurrent; do
    proof="$(./scripts/verify-m7-private-demo-source.sh "$pair" "$scenario")" ||
      fail "source proof no longer verifies: ${pair}/${scenario}"
    run_id="$(jq -er '.entry.run_id' <<<"$proof")"
    model="$(jq -er '.entry.evidence_model' <<<"$proof")"
    joined="$(jq -cr '.joined_actual_node_concurrency' <<<"$proof")"
    jq -e --arg pair "$pair" --arg scenario "$scenario" --arg run_id "$run_id" --arg model "$model" --argjson joined "$joined" '
        [.videos[] | select(.pair == $pair and .scenario == $scenario and
          .run_id == $run_id and .evidence_model == $model and
          .joined_actual_node_concurrency == $joined)] | length == 1
      ' "$certificate" >/dev/null || fail "certificate/source mismatch: ${pair}/${scenario}"
  done
done

rg -Fq './scripts/test-m7-private-demo-video-actual-certificate.sh' scripts/run-ci-quality-gates.sh ||
  fail "certificate contract is absent from the quality runner"
rg -Fq './scripts/test-m7-private-demo-video-actual-certificate.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI hardening does not pin the certificate contract"

echo "M7 private demo-video certificate test passed"
