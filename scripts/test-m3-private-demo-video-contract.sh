#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly source_recorder="scripts/record-m3-private-demo.sh"
readonly source_verifier="scripts/verify-m3-private-demo-source.sh"
readonly video_renderer="scripts/render-m3-private-demo-video.sh"
readonly bundle_verifier="scripts/verify-m3-private-demo-video-bundle.sh"
source_fixture="$(pwd)/scripts/fixtures/m3-recording-test-driver.sh"
renderer_fixture="$(pwd)/scripts/fixtures/m3-private-video-test-renderer.sh"
readonly source_fixture renderer_fixture

fail() {
  echo "M3 private demo-video contract failed: $*" >&2
  exit 1
}

for executable in \
  "$source_recorder" \
  "$source_verifier" \
  "$video_renderer" \
  "$bundle_verifier" \
  "$source_fixture" \
  "$renderer_fixture"; do
  [[ -x "$executable" ]] || fail "required executable is missing: ${executable}"
  bash -n "$executable"
done

readonly pinned_image='ghcr.io/charmbracelet/vhs@sha256:9d5fc3dc0c160b0fb1d2212baff07e6bdf3fa9438c504a3237484567302fcf93'
rg -Fq "$pinned_image" "$video_renderer" || fail "VHS renderer image is not digest-pinned"
for isolation_term in '--network none' '--cap-drop ALL' '--security-opt no-new-privileges' '--read-only'; do
  rg -Fq -- "$isolation_term" "$video_renderer" ||
    fail "renderer is missing Docker isolation: ${isolation_term}"
done

test_root="$(mktemp -d /tmp/m3-private-demo-video-contract.XXXXXX)"
cleanup() {
  rm -rf -- "$test_root"
}
trap cleanup EXIT

expected_commit="$(git rev-parse HEAD)"
readonly expected_commit
video_manifests=()
source_manifests=()
source_evidence_files=()
source_support_dirs=()

for scenario in happy refund concurrent; do
  run_id="m3-video-${scenario}-contract"
  evidence_file="${test_root}/${run_id}/evidence/m3-actor-local-poc.json"
  recording_root="${test_root}/recordings/${run_id}"
  env \
    RUN_ID="$run_id" \
    M3_RECORDING_SCENARIO="$scenario" \
    M3_RECORDING_PRIVATE_ROOT="$recording_root" \
    M3_RECORDING_TESTING=1 \
    M3_RECORDING_TEST_DRIVER="$source_fixture" \
    M3_RECORDING_TEST_EVIDENCE_FILE="$evidence_file" \
    M3_RECORDING_TEST_COMMIT="$expected_commit" \
    "$source_recorder" >/dev/null

  source_manifest="${recording_root}/${scenario}/recording.json"
  output_root="${test_root}/videos/${run_id}"
  env \
    M3_PRIVATE_DEMO_VIDEO_TESTING=1 \
    M3_PRIVATE_DEMO_VIDEO_TEST_RENDERER="$renderer_fixture" \
    "$video_renderer" "$source_manifest" "$output_root" >/dev/null

  output_dir="${output_root}/${scenario}"
  manifest="${output_dir}/video.json"
  video="${output_dir}/demo.mp4"
  walkthrough="${output_dir}/walkthrough.txt"
  tape="${output_dir}/demo.tape"
  proof="${output_dir}/proof.json"
  demo="${output_dir}/demo.sh"
  for artifact in "$manifest" "$video" "$walkthrough" "$tape" "$proof" "$demo"; do
    [[ -s "$artifact" && ! -L "$artifact" ]] || fail "${scenario} video artifact is missing"
    [[ "$(stat -c '%a' "$artifact")" == 600 ]] || fail "${scenario} video artifact is not private"
  done
  [[ "$(stat -c '%a' "$output_dir")" == 700 ]] || fail "${scenario} video directory is not private"
  [[ "$(dd if="$video" bs=1 skip=4 count=4 status=none)" == ftyp ]] ||
    fail "${scenario} fixture does not have an MP4 signature"

  jq -e \
    --arg scenario "$scenario" \
    --arg run_id "$run_id" \
    --arg commit "$expected_commit" '
      .schema_version == 1 and
      .kind == "m3_private_demo_video" and
      .result == "passed" and
      .scenario == $scenario and
      .run_id == $run_id and
      .certification_mode == "test_contract" and
      .privacy == "private_local_stealth" and
      .source_repository_commit == $commit and
      .renderer_repository_commit == $commit and
      .video.format == "video/mp4" and
      .video.file == "demo.mp4" and
      .video.duration_seconds == "3.000000" and
      .video.renderer.name == "contract_fixture" and
      (.video.sha256 | test("^[0-9a-f]{64}$")) and
      .walkthrough.file == "walkthrough.txt" and
      (.walkthrough.sha256 | test("^[0-9a-f]{64}$")) and
      .walkthrough.tape_file == "demo.tape" and
      (.walkthrough.tape_sha256 | test("^[0-9a-f]{64}$")) and
      .walkthrough.demo_file == "demo.sh" and
      (.walkthrough.demo_sha256 | test("^[0-9a-f]{64}$")) and
      .proof.file == "proof.json" and
      (.proof.sha256 | test("^[0-9a-f]{64}$")) and
      .proof.source_input_count >= 11 and
      .source_recording.manifest_sha256 != "" and
      .source_recording.output_sha256 != "" and
      .source_recording.timing_sha256 != "" and
      .source_recording.evidence_sha256 != "" and
      .external_resources.public_rpc == false and
      .external_resources.faucet == false and
      .external_resources.public_funds == false and
      .external_resources.certification_success_depends_on_external_network == false and
      (.demonstrates | index("both_trade_directions")) != null and
      (.demonstrates | index("actual_node_effects")) != null
    ' "$manifest" >/dev/null || fail "${scenario} video manifest contract drifted"

  case "$scenario" in
    happy)
      jq -e '.demonstrates | index("terminal_claim_completion") != null' "$manifest" >/dev/null ||
        fail "happy video does not declare terminal claim completion"
      ;;
    refund)
      jq -e '.demonstrates | index("ordered_timelock_refunds") != null' "$manifest" >/dev/null ||
        fail "refund video does not declare ordered refunds"
      ;;
    concurrent)
      jq -e '.demonstrates | index("simultaneous_revision_two_overlap") != null' "$manifest" >/dev/null ||
        fail "concurrent video does not declare the overlap barrier"
      ;;
  esac

  [[ "$(sha256sum "$video" | cut -d ' ' -f 1)" == "$(jq -r '.video.sha256' "$manifest")" ]] ||
    fail "${scenario} video hash drifted"
  jq -e --arg scenario "$scenario" '
    .schema_version == 1 and .kind == "m3_private_demo_proof" and
    .scenario == $scenario and .actor_process_model == "fresh_one_shot_process_per_command" and
    (.source_inputs | length >= 11) and
    all(.source_inputs[]; (.sha256 | test("^[0-9a-f]{64}$"))) and
    ([.directions[].role_terminals[].role] | sort == ["maker","maker","taker","taker"])
  ' "$proof" >/dev/null || fail "${scenario} proof contract drifted"
  rg -Fq './demo.sh' "$tape" || fail "${scenario} tape does not execute the role-flow demo"
  video_manifests+=("$manifest")
  source_manifests+=("$source_manifest")
  source_evidence_files+=("$evidence_file")
  source_support_dirs+=("$(dirname "$evidence_file")")

  if env M3_PRIVATE_DEMO_VIDEO_TESTING=1 \
      M3_PRIVATE_DEMO_VIDEO_TEST_RENDERER="$renderer_fixture" \
      "$video_renderer" "$source_manifest" "$output_root" >/dev/null 2>&1; then
    fail "${scenario} video renderer overwrote existing output"
  fi
done

bundle="${test_root}/videos/video-bundle.json"
env M3_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING=1 \
  M3_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT="$bundle" \
  "$bundle_verifier" "${video_manifests[@]}" >/dev/null
[[ -s "$bundle" && ! -L "$bundle" ]] || fail "video bundle is missing"
[[ "$(stat -c '%a' "$bundle")" == 600 ]] || fail "video bundle is not private"
jq -e --arg commit "$expected_commit" '
  .schema_version == 1 and
  .kind == "m3_private_demo_video_bundle" and
  .result == "passed" and
  .certification_mode == "test_contract" and
  .privacy == "private_local_stealth" and
  .source_repository_commit == $commit and
  .renderer_repository_commit == $commit and
  .scenarios == ["happy","refund","concurrent"] and
  (.videos | length == 3) and
  ([.videos[].scenario] | sort == ["concurrent","happy","refund"]) and
  ([.videos[].run_id] | unique | length == 3) and
  all(.videos[]; (.video_sha256 | test("^[0-9a-f]{64}$")))
' "$bundle" >/dev/null || fail "video bundle contract drifted"

if env M3_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING=1 \
    M3_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT="${test_root}/videos/duplicate.json" \
    "$bundle_verifier" "${video_manifests[0]}" "${video_manifests[0]}" \
      "${video_manifests[2]}" >/dev/null 2>&1; then
  fail "video bundle accepted duplicate scenarios"
fi
source_terminal="$(dirname "${source_manifests[0]}")/terminal.typescript"
cp -- "$source_terminal" "${source_terminal}.before-tamper"
printf 'tampered-after-render' >>"$source_terminal"
if env M3_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING=1 \
    M3_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT="${test_root}/videos/source-terminal-tamper.json" \
    "$bundle_verifier" "${video_manifests[@]}" >/dev/null 2>&1; then
  fail "video bundle accepted a tampered source terminal recording"
fi
mv -- "${source_terminal}.before-tamper" "$source_terminal"

cp -- "${source_evidence_files[1]}" "${source_evidence_files[1]}.before-tamper"
printf 'tampered-after-render' >>"${source_evidence_files[1]}"
if env M3_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING=1 \
    M3_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT="${test_root}/videos/source-evidence-tamper.json" \
    "$bundle_verifier" "${video_manifests[@]}" >/dev/null 2>&1; then
  fail "video bundle accepted tampered actual-node evidence"
fi
mv -- "${source_evidence_files[1]}.before-tamper" "${source_evidence_files[1]}"

refund_role_action="${source_support_dirs[1]}/taker_sells_foreign-lez-maker-refund-submit-maker.json"
cp -- "$refund_role_action" "${refund_role_action}.before-tamper"
jq '.role = "taker"' "$refund_role_action" >"${refund_role_action}.changed"
chmod 0600 "${refund_role_action}.changed"
mv -- "${refund_role_action}.changed" "$refund_role_action"
if env M3_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING=1 \
    M3_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT="${test_root}/videos/refund-role-tamper.json" \
    "$bundle_verifier" "${video_manifests[@]}" >/dev/null 2>&1; then
  fail "video bundle accepted an invalid Maker refund role"
fi
mv -- "${refund_role_action}.before-tamper" "$refund_role_action"

refund_confirmation="${source_support_dirs[1]}/taker_sells_foreign-bitcoin-taker-refund-confirmed.json"
cp -- "$refund_confirmation" "${refund_confirmation}.before-tamper"
jq '.result.blocktime = 50' "$refund_confirmation" >"${refund_confirmation}.changed"
chmod 0600 "${refund_confirmation}.changed"
mv -- "${refund_confirmation}.changed" "$refund_confirmation"
if env M3_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING=1 \
    M3_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT="${test_root}/videos/refund-order-tamper.json" \
    "$bundle_verifier" "${video_manifests[@]}" >/dev/null 2>&1; then
  fail "video bundle accepted a later refund confirmed before its signed bound"
fi
mv -- "${refund_confirmation}.before-tamper" "$refund_confirmation"

printf 'tamper' >>"$(dirname "${video_manifests[0]}")/demo.mp4"
if env M3_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING=1 \
    M3_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT="${test_root}/videos/tamper.json" \
    "$bundle_verifier" "${video_manifests[@]}" >/dev/null 2>&1; then
  fail "video bundle accepted a tampered MP4"
fi

if env M3_PRIVATE_DEMO_VIDEO_TEST_RENDERER="$renderer_fixture" \
    "$video_renderer" "${video_manifests[1]}" "${test_root}/forbidden" >/dev/null 2>&1; then
  fail "production mode accepted the test renderer override"
fi

echo "M3 private demo-video contract passed"
