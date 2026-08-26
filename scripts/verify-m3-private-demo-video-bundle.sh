#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
repository_root="$(pwd -P)"
readonly repository_root
readonly source_verifier="${repository_root}/scripts/verify-m3-private-demo-source.sh"
readonly vhs_image='ghcr.io/charmbracelet/vhs@sha256:9d5fc3dc0c160b0fb1d2212baff07e6bdf3fa9438c504a3237484567302fcf93'

fail() {
  echo "M3 private demo-video bundle failed: $*" >&2
  exit 1
}

[[ -x "$source_verifier" && ! -L "$source_verifier" ]] || fail "source verifier is missing"

for dependency in dd git jq realpath sha256sum stat; do
  command -v "$dependency" >/dev/null 2>&1 || fail "missing dependency: ${dependency}"
done

(( $# == 3 )) || fail "provide exactly the happy, refund, and concurrent video manifests"
readonly testing="${M3_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING:-0}"
case "$testing" in
  0) readonly expected_mode="live_actual_nodes" ;;
  1) readonly expected_mode="test_contract" ;;
  *) fail "M3_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING must be exactly 0 or 1" ;;
esac
readonly output_file="${M3_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT:?M3_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT is required}"
[[ "$output_file" == /* && "$output_file" =~ ^[a-zA-Z0-9_./-]+$ ]] ||
  fail "bundle output must use an absolute safe path"
[[ ! -e "$output_file" && ! -L "$output_file" ]] || fail "bundle output already exists"

declare -A seen_scenarios=()
declare -A seen_run_ids=()
entries=()
source_commit=""
renderer_commit=""
networks_json=""
verifier_repository_commit="$(git rev-parse --verify HEAD)"
readonly verifier_repository_commit

for manifest in "$@"; do
  [[ -f "$manifest" && ! -L "$manifest" ]] || fail "video manifest must be a regular file"
  manifest_abs="$(realpath -e -- "$manifest")"
  [[ "$(stat -c '%a' "$manifest_abs")" == 600 ]] || fail "video manifest must have mode 0600"
  jq -e --arg mode "$expected_mode" '
    .schema_version == 1 and .kind == "m3_private_demo_video" and .result == "passed" and
    .certification_mode == $mode and .privacy == "private_local_stealth" and
    (.scenario == "happy" or .scenario == "refund" or .scenario == "concurrent") and
    (.run_id | test("^[a-zA-Z0-9][a-zA-Z0-9._-]{0,95}$")) and
    (.source_repository_commit | test("^[0-9a-f]{40}$")) and
    (.renderer_repository_commit | test("^[0-9a-f]{40}$")) and
    .networks.bitcoin_core.version == "31.1" and .networks.bitcoin_core.network == "regtest" and
    .networks.lez.version == "v0.2.0" and .networks.lez.network == "private_local" and
    .external_resources.public_rpc == false and .external_resources.faucet == false and
    .external_resources.public_funds == false and
    .external_resources.certification_success_depends_on_external_network == false and
    .video.format == "video/mp4" and .video.file == "demo.mp4" and
    (.video.sha256 | test("^[0-9a-f]{64}$")) and .video.size_bytes > 0 and
    (.video.duration_seconds | test("^[0-9]+([.][0-9]+)?$")) and
    .video.renderer.network == "none" and
    .walkthrough.file == "walkthrough.txt" and
    (.walkthrough.sha256 | test("^[0-9a-f]{64}$")) and
    .walkthrough.tape_file == "demo.tape" and
    (.walkthrough.tape_sha256 | test("^[0-9a-f]{64}$")) and
    .walkthrough.demo_file == "demo.sh" and
    (.walkthrough.demo_sha256 | test("^[0-9a-f]{64}$")) and
    .proof.file == "proof.json" and (.proof.sha256 | test("^[0-9a-f]{64}$")) and
    .proof.source_input_count >= 14 and
    (.source_recording.manifest_sha256 | test("^[0-9a-f]{64}$")) and
    (.source_recording.output_sha256 | test("^[0-9a-f]{64}$")) and
    (.source_recording.timing_sha256 | test("^[0-9a-f]{64}$")) and
    (.source_recording.evidence_sha256 | test("^[0-9a-f]{64}$")) and
    (.demonstrates | index("both_trade_directions")) != null and
    (.demonstrates | index("actual_node_effects")) != null and
    (if $mode == "live_actual_nodes" then
      .video.renderer.name == "VHS" and .video.renderer.version == "0.11.0" and
      .video.renderer.image == "ghcr.io/charmbracelet/vhs@sha256:9d5fc3dc0c160b0fb1d2212baff07e6bdf3fa9438c504a3237484567302fcf93"
    else
      .video.renderer.name == "contract_fixture" and .video.renderer.version == "test-only" and
      .video.renderer.image == "none"
    end)
  ' "$manifest_abs" >/dev/null || fail "video manifest contract is invalid"

  scenario="$(jq -er '.scenario' "$manifest_abs")"
  run_id="$(jq -er '.run_id' "$manifest_abs")"
  current_source_commit="$(jq -er '.source_repository_commit' "$manifest_abs")"
  current_renderer_commit="$(jq -er '.renderer_repository_commit' "$manifest_abs")"
  [[ -z "${seen_scenarios[$scenario]:-}" ]] || fail "duplicate video scenario: ${scenario}"
  [[ -z "${seen_run_ids[$run_id]:-}" ]] || fail "duplicate video run ID: ${run_id}"
  seen_scenarios["$scenario"]=1
  seen_run_ids["$run_id"]=1
  if [[ -z "$source_commit" ]]; then
    source_commit="$current_source_commit"
    renderer_commit="$current_renderer_commit"
    networks_json="$(jq -cS '.networks | {bitcoin_core:{version:.bitcoin_core.version,network:.bitcoin_core.network},lez:{version:.lez.version,network:.lez.network}}' "$manifest_abs")"
  else
    [[ "$current_source_commit" == "$source_commit" ]] || fail "video source commits differ"
    [[ "$current_renderer_commit" == "$renderer_commit" ]] || fail "video renderer commits differ"
    [[ "$(jq -cS '.networks | {bitcoin_core:{version:.bitcoin_core.version,network:.bitcoin_core.network},lez:{version:.lez.version,network:.lez.network}}' "$manifest_abs")" == "$networks_json" ]] ||
      fail "video chain versions or networks differ"
  fi

  manifest_dir="$(dirname "$manifest_abs")"
  video_file="${manifest_dir}/$(jq -er '.video.file' "$manifest_abs")"
  walkthrough_file="${manifest_dir}/$(jq -er '.walkthrough.file' "$manifest_abs")"
  tape_file="${manifest_dir}/$(jq -er '.walkthrough.tape_file' "$manifest_abs")"
  demo_file="${manifest_dir}/$(jq -er '.walkthrough.demo_file' "$manifest_abs")"
  proof_file="${manifest_dir}/$(jq -er '.proof.file' "$manifest_abs")"
  for private_file in "$video_file" "$walkthrough_file" "$tape_file" "$demo_file" "$proof_file"; do
    [[ -s "$private_file" && ! -L "$private_file" ]] || fail "video artifact is missing"
    [[ "$(stat -c '%a' "$private_file")" == 600 ]] || fail "video artifact must have mode 0600"
  done
  [[ "$(dd if="$video_file" bs=1 skip=4 count=4 status=none)" == ftyp ]] || fail "video is not MP4"
  video_sha256="$(sha256sum "$video_file" | cut -d ' ' -f 1)"
  [[ "$video_sha256" == "$(jq -er '.video.sha256' "$manifest_abs")" ]] || fail "video hash mismatch"
  [[ "$(stat -c '%s' "$video_file")" == "$(jq -er '.video.size_bytes' "$manifest_abs")" ]] ||
    fail "video size mismatch"
  [[ "$(sha256sum "$walkthrough_file" | cut -d ' ' -f 1)" == "$(jq -er '.walkthrough.sha256' "$manifest_abs")" ]] ||
    fail "walkthrough hash mismatch"
  [[ "$(sha256sum "$tape_file" | cut -d ' ' -f 1)" == "$(jq -er '.walkthrough.tape_sha256' "$manifest_abs")" ]] ||
    fail "tape hash mismatch"
  [[ "$(sha256sum "$demo_file" | cut -d ' ' -f 1)" == "$(jq -er '.walkthrough.demo_sha256' "$manifest_abs")" ]] ||
    fail "demo script hash mismatch"
  [[ "$(sha256sum "$proof_file" | cut -d ' ' -f 1)" == "$(jq -er '.proof.sha256' "$manifest_abs")" ]] ||
    fail "proof hash mismatch"
  [[ "$(jq -er '.source_inputs | length' "$proof_file")" == "$(jq -er '.proof.source_input_count' "$manifest_abs")" ]] ||
    fail "proof source-input count mismatch"

  source_manifest_path="$(jq -er '.source_recording.manifest' "$manifest_abs")"
  if [[ "$source_manifest_path" == /* ]]; then
    source_manifest_file="$source_manifest_path"
  else
    source_manifest_file="${repository_root}/${source_manifest_path}"
  fi
  [[ -f "$source_manifest_file" && ! -L "$source_manifest_file" ]] || fail "bound source recording is missing"
  [[ "$(sha256sum "$source_manifest_file" | cut -d ' ' -f 1)" == "$(jq -er '.source_recording.manifest_sha256' "$manifest_abs")" ]] ||
    fail "bound source recording hash mismatch"
  regenerated_proof="$("$source_verifier" "$source_manifest_file")" ||
    fail "bound source evidence no longer verifies"
  [[ "$regenerated_proof" == "$(jq -cS '.' "$proof_file")" ]] ||
    fail "stored proof differs from freshly verified source evidence"
  [[ "$(jq -er '.repository_commit' "$proof_file")" == "$current_source_commit" ]] ||
    fail "proof source commit mismatch"
  [[ "$(jq -er '.certification_mode' "$proof_file")" == "$expected_mode" ]] ||
    fail "proof certification mode mismatch"

  case "$scenario" in
    happy) scenario_term="terminal_claim_completion" ;;
    refund) scenario_term="ordered_timelock_refunds" ;;
    concurrent) scenario_term="simultaneous_revision_two_overlap" ;;
  esac
  jq -e --arg term "$scenario_term" '.demonstrates | index($term) != null' "$manifest_abs" >/dev/null ||
    fail "video does not demonstrate its selected scenario"
  [[ "$(jq -er '.scenario_assertion' "$proof_file")" == "$scenario_term" ]] ||
    fail "fresh proof does not establish the selected scenario"

  if [[ "$testing" == 0 ]]; then
    command -v docker >/dev/null 2>&1 || fail "missing dependency: docker"
    docker image inspect "$vhs_image" >/dev/null 2>&1 || fail "pinned VHS image is absent"
    probe_suffix="$(sha256sum "$video_file" | cut -c 1-12)"
    probe_json="$(docker run --rm \
      --name "lez-atomic-swaps-vhs-bundle-probe-${probe_suffix}" \
      --network none --read-only --cap-drop ALL --security-opt no-new-privileges \
      --pids-limit 32 --memory 128m --cpus 1 --user "$(id -u):$(id -g)" \
      --mount "type=bind,src=${manifest_dir},dst=/vhs,readonly" \
      --entrypoint ffprobe "$vhs_image" -v error -count_frames -select_streams v:0 \
      -show_entries stream=codec_name,width,height,nb_read_frames \
      -show_entries format=duration -of json /vhs/demo.mp4)" || fail "video decode probe failed"
    jq -e --arg duration "$(jq -er '.video.duration_seconds' "$manifest_abs")" '
      (.streams | length) == 1 and .streams[0].codec_name == "h264" and
      .streams[0].width == 1280 and .streams[0].height == 720 and
      (.streams[0].nb_read_frames | tonumber) > 0 and
      ((.format.duration | tonumber) - ($duration | tonumber) | fabs) < 0.001
    ' <<<"$probe_json" >/dev/null || fail "video stream/duration contract failed"
  fi

  manifest_ref="$manifest_abs"
  [[ "$manifest_abs" == "${repository_root}/"* ]] && manifest_ref="${manifest_abs#"${repository_root}/"}"
  entries+=("$(jq -cn \
    --arg scenario "$scenario" \
    --arg run_id "$run_id" \
    --arg manifest "$manifest_ref" \
    --arg manifest_sha256 "$(sha256sum "$manifest_abs" | cut -d ' ' -f 1)" \
    --arg video_sha256 "$video_sha256" \
    --arg duration_seconds "$(jq -er '.video.duration_seconds' "$manifest_abs")" \
    '{scenario:$scenario,run_id:$run_id,manifest:$manifest,manifest_sha256:$manifest_sha256,video_sha256:$video_sha256,duration_seconds:$duration_seconds}')")
done

for required_scenario in happy refund concurrent; do
  [[ -n "${seen_scenarios[$required_scenario]:-}" ]] || fail "missing video scenario: ${required_scenario}"
done
if [[ "$testing" == 0 ]]; then
  [[ -z "$(git status --porcelain=v1 --untracked-files=normal)" ]] || fail "live video bundle requires a clean worktree"
  for commit in "$source_commit" "$renderer_commit"; do
    git cat-file -e "${commit}^{commit}" >/dev/null 2>&1 || fail "video-bound commit is absent"
    git merge-base --is-ancestor "$commit" "$verifier_repository_commit" || fail "video-bound commit is not an ancestor"
  done
  if [[ "$output_file" == "${repository_root}/"* ]]; then
    git check-ignore -q -- "$output_file" || fail "private bundle inside the repository must be ignored"
  fi
fi

videos_json="$(printf '%s\n' "${entries[@]}" | jq -cs 'sort_by(.scenario)')"
recorded_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
readonly videos_json recorded_at
output_parent="$(dirname "$output_file")"
umask 077
mkdir -p -- "$output_parent"
[[ -d "$output_parent" && ! -L "$output_parent" ]] || fail "bundle parent is not a regular directory"
output_tmp="$(mktemp "${output_parent}/.video-bundle.json.XXXXXX")"
trap 'rm -f -- "${output_tmp:-}"' EXIT
jq -n \
  --arg certification_mode "$expected_mode" \
  --arg source_repository_commit "$source_commit" \
  --arg renderer_repository_commit "$renderer_commit" \
  --arg verifier_repository_commit "$verifier_repository_commit" \
  --arg recorded_at "$recorded_at" \
  --argjson networks "$networks_json" \
  --argjson videos "$videos_json" '
    {
      schema_version: 1,
      kind: "m3_private_demo_video_bundle",
      result: "passed",
      certification_mode: $certification_mode,
      privacy: "private_local_stealth",
      source_repository_commit: $source_repository_commit,
      renderer_repository_commit: $renderer_repository_commit,
      verifier_repository_commit: $verifier_repository_commit,
      recorded_at: $recorded_at,
      networks: $networks,
      scenarios: ["happy","refund","concurrent"],
      videos: $videos,
      public_rpc_used: false,
      faucet_used: false,
      public_funds_used: false,
      certification_success_depends_on_external_network: false
    }
  ' >"$output_tmp"
chmod 600 -- "$output_tmp"
mv -- "$output_tmp" "$output_file"
trap - EXIT
echo "M3 private demo-video bundle passed: ${output_file}"
