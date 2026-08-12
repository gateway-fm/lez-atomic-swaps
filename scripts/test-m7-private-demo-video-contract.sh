#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly source_map="docs/evidence/m7-private-demo-sources.json"
readonly source_verifier="scripts/verify-m7-private-demo-source.sh"
readonly renderer="scripts/render-m7-private-demo-video.sh"
readonly bundle_verifier="scripts/verify-m7-private-demo-video-bundle.sh"
renderer_fixture="$(pwd)/scripts/fixtures/m3-private-video-test-renderer.sh"
readonly renderer_fixture
readonly pinned_image='ghcr.io/charmbracelet/vhs@sha256:9d5fc3dc0c160b0fb1d2212baff07e6bdf3fa9438c504a3237484567302fcf93'

fail() {
  echo "M7 private demo-video contract failed: $*" >&2
  exit 1
}

for executable in "$source_verifier" "$renderer" "$bundle_verifier" "$renderer_fixture"; do
  [[ -x "$executable" && ! -L "$executable" ]] || fail "missing executable: ${executable}"
  bash -n "$executable"
done
jq -e '
  .schema_version == 1 and .kind == "m7_private_demo_source_map" and
  .privacy == "private_local_stealth" and (.entries | length) == 6 and
  ([.entries[] | [.pair,.scenario] | join(":")] | sort ==
    ["xmr:concurrent","xmr:happy","xmr:refund","zec:concurrent","zec:happy","zec:refund"]) and
  (.entries[] | select(.pair == "zec" and .scenario == "concurrent") |
    .evidence_model == "layered_process_concurrency_plus_actual_node_pair_effects") and
  .runtime_external_resources == {public_rpc:false,public_peer:false,faucet:false,
    public_funds:false,public_deployment:false}
' "$source_map" >/dev/null || fail "source map contract drifted"
rg -Fq "$pinned_image" "$renderer" || fail "renderer image is not digest-pinned"
rg -Fq "$pinned_image" "$bundle_verifier" || fail "bundle probe image is not digest-pinned"
for isolation_term in '--network none' '--read-only' '--cap-drop ALL' '--security-opt no-new-privileges'; do
  rg -Fq -- "$isolation_term" "$renderer" ||
    fail "renderer is missing Docker isolation: ${isolation_term}"
  rg -Fq -- "$isolation_term" "$bundle_verifier" ||
    fail "bundle verifier is missing Docker isolation: ${isolation_term}"
done

test_root="$(mktemp -d /tmp/m7-private-demo-video-contract.XXXXXX)"
cleanup() {
  rm -rf -- "$test_root"
}
trap cleanup EXIT
readonly expected_commit="$(git rev-parse --verify HEAD)"
manifests=()

for pair in xmr zec; do
  for scenario in happy refund concurrent; do
    output_root="${test_root}/videos"
    env M7_PRIVATE_DEMO_VIDEO_TESTING=1 M7_PRIVATE_DEMO_VIDEO_TEST_RENDERER="$renderer_fixture" "$renderer" "$pair" "$scenario" "$output_root" >/dev/null
    output_dir="${output_root}/${pair}/${scenario}"
    manifest="${output_dir}/video.json"
    video="${output_dir}/demo.mp4"
    proof="${output_dir}/proof.json"
    for artifact in "$manifest" "$video" "$proof" "${output_dir}/walkthrough.txt" "${output_dir}/demo.sh" "${output_dir}/demo.tape"; do
      [[ -s "$artifact" && ! -L "$artifact" ]] || fail "${pair}/${scenario} artifact is missing"
      [[ "$(stat -c '%a' "$artifact")" == 600 ]] ||
        fail "${pair}/${scenario} artifact is not mode 0600"
    done
    [[ "$(stat -c '%a' "$output_dir")" == 700 ]] ||
      fail "${pair}/${scenario} directory is not mode 0700"
    [[ "$(dd if="$video" bs=1 skip=4 count=4 status=none)" == ftyp ]] ||
      fail "${pair}/${scenario} fixture lacks MP4 signature"
    jq -e --arg pair "$pair" --arg scenario "$scenario" --arg commit "$expected_commit" '
      .schema_version == 1 and .kind == "m7_private_demo_video" and .result == "passed" and
      .certification_mode == "test_contract" and .privacy == "private_local_stealth" and
      .pair == $pair and .scenario == $scenario and
      .source_repository_commit == $commit and .renderer_repository_commit == $commit and
      .source_map.file == "docs/evidence/m7-private-demo-sources.json" and
      (.source_map.sha256 | test("^[0-9a-f]{64}$")) and
      .video.format == "video/mp4" and .video.file == "demo.mp4" and
      .video.duration_seconds == "3.000000" and
      .video.renderer.name == "contract_fixture" and .video.renderer.network == "none" and
      .external_resources.certification_success_depends_on_external_network == false
    ' "$manifest" >/dev/null || fail "${pair}/${scenario} manifest contract drifted"
    [[ "$(sha256sum "$proof" | cut -d ' ' -f 1)" == "$(jq -er '.proof.sha256' "$manifest")" ]] ||
      fail "${pair}/${scenario} proof hash drifted"
    if [[ "$pair" == zec && "$scenario" == concurrent ]]; then
      jq -e '
        .evidence_model == "layered_process_concurrency_plus_actual_node_pair_effects" and
        .joined_actual_node_concurrency == false
      ' "$manifest" >/dev/null || fail "ZEC concurrent evidence limit was overstated"
    else
      jq -e '.evidence_model == "joined_actual_nodes" and .joined_actual_node_concurrency == true' "$manifest" >/dev/null || fail "${pair}/${scenario} joined evidence drifted"
    fi
    manifests+=("$manifest")
  done
done

bundle="${test_root}/videos/video-bundle.json"
env M7_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING=1 M7_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT="$bundle" "$bundle_verifier" "${manifests[@]}" >/dev/null
[[ -s "$bundle" && ! -L "$bundle" && "$(stat -c '%a' "$bundle")" == 600 ]] ||
  fail "private bundle is missing or has unsafe mode"
jq -e --arg commit "$expected_commit" '
  .schema_version == 1 and .kind == "m7_private_demo_video_bundle" and .result == "passed" and
  .certification_mode == "test_contract" and .privacy == "private_local_stealth" and
  .source_repository_commit == $commit and .renderer_repository_commit == $commit and
  .pairs == ["xmr","zec"] and .scenarios == ["happy","refund","concurrent"] and
  (.videos | length) == 6 and
  ([.videos[] | [.pair,.scenario] | join(":")] | unique | length) == 6 and
  .zec_concurrent_joined_actual_node_run == false and
  .certification_success_depends_on_external_network == false
' "$bundle" >/dev/null || fail "bundle contract drifted"

if env M7_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING=1 M7_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT="${test_root}/videos/duplicate.json" "$bundle_verifier" "${manifests[0]}" "${manifests[0]}" "${manifests[2]}" "${manifests[3]}" "${manifests[4]}" "${manifests[5]}" >/dev/null 2>&1; then
  fail "bundle accepted a duplicate pair/scenario"
fi

video_to_tamper="$(dirname "${manifests[0]}")/demo.mp4"
printf 'tamper' >>"$video_to_tamper"
if env M7_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING=1 M7_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT="${test_root}/videos/tampered.json" "$bundle_verifier" "${manifests[@]}" >/dev/null 2>&1; then
  fail "bundle accepted a tampered video"
fi

if env M7_PRIVATE_DEMO_VIDEO_TEST_RENDERER="$renderer_fixture" "$renderer" xmr happy "${test_root}/forbidden" >/dev/null 2>&1; then
  fail "live mode accepted a fixture renderer"
fi

echo "M7 private demo-video contract passed"
