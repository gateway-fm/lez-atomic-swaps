#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
repository_root="$(pwd -P)"
readonly repository_root
readonly source_verifier="${repository_root}/scripts/verify-m7-private-demo-source.sh"
readonly source_map="${repository_root}/docs/evidence/m7-private-demo-sources.json"
readonly vhs_image='ghcr.io/charmbracelet/vhs@sha256:9d5fc3dc0c160b0fb1d2212baff07e6bdf3fa9438c504a3237484567302fcf93'

fail() {
  echo "M7 private demo-video bundle failed: $*" >&2
  exit 1
}

for dependency in dd git jq realpath sha256sum stat; do
  command -v "$dependency" >/dev/null 2>&1 || fail "missing dependency: ${dependency}"
done
[[ -x "$source_verifier" && ! -L "$source_verifier" ]] || fail "source verifier is missing"
(( $# == 6 )) || fail "provide exactly six video manifests"
readonly testing="${M7_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING:-0}"
case "$testing" in
  0) readonly expected_mode="live_actual_nodes" ;;
  1) readonly expected_mode="test_contract" ;;
  *) fail "M7_PRIVATE_DEMO_VIDEO_BUNDLE_TESTING must be exactly 0 or 1" ;;
esac
readonly output_file="${M7_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT:?bundle output is required}"
[[ "$output_file" == /* && "$output_file" =~ ^[a-zA-Z0-9_./-]+$ ]] ||
  fail "bundle output must use an absolute safe path"
[[ ! -e "$output_file" && ! -L "$output_file" ]] || fail "bundle output already exists"

declare -A seen=()
entries=()
source_commit=""
renderer_commit=""
source_map_sha256=""
verifier_repository_commit="$(git rev-parse --verify HEAD)"
readonly verifier_repository_commit

for manifest in "$@"; do
  [[ -f "$manifest" && ! -L "$manifest" ]] || fail "video manifest is missing or unsafe"
  manifest_abs="$(realpath -e -- "$manifest")"
  manifest_dir="$(dirname "$manifest_abs")"
  [[ "$(stat -c '%a' "$manifest_abs")" == 600 ]] || fail "video manifest must be mode 0600"
  jq -e --arg mode "$expected_mode" '
    .schema_version == 1 and .kind == "m7_private_demo_video" and .result == "passed" and
    .certification_mode == $mode and .privacy == "private_local_stealth" and
    (.pair == "xmr" or .pair == "zec") and
    (.scenario == "happy" or .scenario == "refund" or .scenario == "concurrent") and
    (.run_id | test("^[a-zA-Z0-9][a-zA-Z0-9._-]{0,95}$")) and
    (.source_repository_commit | test("^[0-9a-f]{40}$")) and
    .renderer_repository_commit == .source_repository_commit and
    .source_map.file == "docs/evidence/m7-private-demo-sources.json" and
    (.source_map.sha256 | test("^[0-9a-f]{64}$")) and
    .external_resources == {public_rpc:false,public_peer:false,faucet:false,
      public_funds:false,public_deployment:false,
      certification_success_depends_on_external_network:false} and
    .video.format == "video/mp4" and .video.file == "demo.mp4" and
    (.video.sha256 | test("^[0-9a-f]{64}$")) and .video.size_bytes > 0 and
    (.video.duration_seconds | test("^[0-9]+([.][0-9]+)?$"))
  ' "$manifest_abs" >/dev/null || fail "video manifest contract is invalid"

  pair="$(jq -er '.pair' "$manifest_abs")"
  scenario="$(jq -er '.scenario' "$manifest_abs")"
  key="${pair}:${scenario}"
  [[ -z "${seen[$key]:-}" ]] || fail "duplicate video: ${key}"
  seen[$key]=1

  current_source_commit="$(jq -er '.source_repository_commit' "$manifest_abs")"
  current_renderer_commit="$(jq -er '.renderer_repository_commit' "$manifest_abs")"
  current_source_map_sha256="$(jq -er '.source_map.sha256' "$manifest_abs")"
  if [[ -z "$source_commit" ]]; then
    source_commit="$current_source_commit"
    renderer_commit="$current_renderer_commit"
    source_map_sha256="$current_source_map_sha256"
  else
    [[ "$source_commit" == "$current_source_commit" ]] || fail "source commits differ"
    [[ "$renderer_commit" == "$current_renderer_commit" ]] || fail "renderer commits differ"
    [[ "$source_map_sha256" == "$current_source_map_sha256" ]] || fail "source maps differ"
  fi

  [[ "$(sha256sum "$source_map" | cut -d ' ' -f 1)" == "$current_source_map_sha256" ]] ||
    fail "source map hash mismatch"
  for file_key in proof.file walkthrough.file walkthrough.demo_file walkthrough.tape_file video.file; do
    relative="$(jq -er ".${file_key}" "$manifest_abs")"
    [[ "$relative" =~ ^[a-zA-Z0-9._-]+$ ]] || fail "unsafe artifact name"
    artifact="${manifest_dir}/${relative}"
    [[ -s "$artifact" && ! -L "$artifact" ]] || fail "artifact is missing: ${relative}"
    [[ "$(stat -c '%a' "$artifact")" == 600 ]] || fail "artifact must be mode 0600"
  done

  proof_file="${manifest_dir}/$(jq -er '.proof.file' "$manifest_abs")"
  walkthrough_file="${manifest_dir}/$(jq -er '.walkthrough.file' "$manifest_abs")"
  demo_file="${manifest_dir}/$(jq -er '.walkthrough.demo_file' "$manifest_abs")"
  tape_file="${manifest_dir}/$(jq -er '.walkthrough.tape_file' "$manifest_abs")"
  video_file="${manifest_dir}/$(jq -er '.video.file' "$manifest_abs")"
  [[ "$(sha256sum "$proof_file" | cut -d ' ' -f 1)" == "$(jq -er '.proof.sha256' "$manifest_abs")" ]] ||
    fail "proof hash mismatch"
  [[ "$(sha256sum "$walkthrough_file" | cut -d ' ' -f 1)" == "$(jq -er '.walkthrough.sha256' "$manifest_abs")" ]] ||
    fail "walkthrough hash mismatch"
  [[ "$(sha256sum "$demo_file" | cut -d ' ' -f 1)" == "$(jq -er '.walkthrough.demo_sha256' "$manifest_abs")" ]] ||
    fail "demo hash mismatch"
  [[ "$(sha256sum "$tape_file" | cut -d ' ' -f 1)" == "$(jq -er '.walkthrough.tape_sha256' "$manifest_abs")" ]] ||
    fail "tape hash mismatch"
  [[ "$(sha256sum "$video_file" | cut -d ' ' -f 1)" == "$(jq -er '.video.sha256' "$manifest_abs")" ]] ||
    fail "video hash mismatch"
  [[ "$(stat -c '%s' "$video_file")" == "$(jq -er '.video.size_bytes' "$manifest_abs")" ]] ||
    fail "video size mismatch"
  [[ "$(dd if="$video_file" bs=1 skip=4 count=4 status=none)" == ftyp ]] ||
    fail "video is not an MP4"

  regenerated="$("$source_verifier" "$pair" "$scenario" | jq -cS .)" ||
    fail "source evidence no longer verifies"
  [[ "$regenerated" == "$(jq -cS . "$proof_file")" ]] ||
    fail "stored proof differs from fresh source verification"
  jq -e --arg model "$(jq -er '.evidence_model' "$manifest_abs")" --argjson joined "$(jq -cr '.joined_actual_node_concurrency' "$manifest_abs")" '
      .entry.evidence_model == $model and .joined_actual_node_concurrency == $joined
    ' "$proof_file" >/dev/null || fail "evidence-model binding drifted"

  if [[ "$testing" == 0 ]]; then
    command -v docker >/dev/null 2>&1 || fail "missing dependency: docker"
    docker image inspect "$vhs_image" >/dev/null 2>&1 || fail "pinned VHS image is absent"
    probe_suffix="$(sha256sum "$video_file" | cut -c 1-12)"
    probe_json="$(docker run --rm --name "lez-atomic-swaps-m7-vhs-probe-${probe_suffix}" --network none --read-only --cap-drop ALL --security-opt no-new-privileges --pids-limit 32 --memory 128m --cpus 1 --user "$(id -u):$(id -g)" --mount "type=bind,src=${manifest_dir},dst=/vhs,readonly" --entrypoint ffprobe "$vhs_image" -v error -count_frames -select_streams v:0 -show_entries stream=codec_name,width,height,nb_read_frames -show_entries format=duration -of json /vhs/demo.mp4)" ||
      fail "video decode probe failed"
    jq -e --arg duration "$(jq -er '.video.duration_seconds' "$manifest_abs")" '
      (.streams | length) == 1 and .streams[0].codec_name == "h264" and
      .streams[0].width == 1280 and .streams[0].height == 720 and
      (.streams[0].nb_read_frames | tonumber) > 0 and
      ((.format.duration | tonumber) - ($duration | tonumber) | fabs) < 0.001
    ' <<<"$probe_json" >/dev/null || fail "video stream/duration contract failed"
  fi

  entries+=("$(jq -cn --arg pair "$pair" --arg scenario "$scenario" --arg run_id "$(jq -er '.run_id' "$manifest_abs")" --arg model "$(jq -er '.evidence_model' "$manifest_abs")" --argjson joined "$(jq -cr '.joined_actual_node_concurrency' "$manifest_abs")" --arg manifest_sha256 "$(sha256sum "$manifest_abs" | cut -d ' ' -f 1)" --arg video_sha256 "$(sha256sum "$video_file" | cut -d ' ' -f 1)" --arg duration "$(jq -er '.video.duration_seconds' "$manifest_abs")" --arg size "$(stat -c '%s' "$video_file")" '
      {pair:$pair,scenario:$scenario,run_id:$run_id,evidence_model:$model,
       joined_actual_node_concurrency:$joined,manifest_sha256:$manifest_sha256,
       video_sha256:$video_sha256,duration_seconds:$duration,size_bytes:($size|tonumber)}
    ')")
done

for required in xmr:happy xmr:refund xmr:concurrent zec:happy zec:refund zec:concurrent; do
  [[ -n "${seen[$required]:-}" ]] || fail "missing video: ${required}"
done
if [[ "$testing" == 0 ]]; then
  [[ -z "$(git status --porcelain=v1 --untracked-files=normal)" ]] ||
    fail "live bundle verification requires a clean worktree"
  git cat-file -e "${source_commit}^{commit}" >/dev/null 2>&1 || fail "source commit is absent"
  [[ "$source_commit" == "$verifier_repository_commit" ]] ||
    fail "live videos must be rendered and verified from the exact checkout"
  if [[ "$output_file" == "${repository_root}/"* ]]; then
    git check-ignore -q -- "$output_file" || fail "private bundle inside repository must be ignored"
  fi
fi

videos_json="$(printf '%s\n' "${entries[@]}" | jq -cs 'sort_by(.pair,.scenario)')"
recorded_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
readonly videos_json recorded_at
umask 077
mkdir -p -- "$(dirname "$output_file")"
jq -n --arg mode "$expected_mode" --arg source_commit "$source_commit" --arg renderer_commit "$renderer_commit" --arg verifier_commit "$verifier_repository_commit" --arg source_map_sha256 "$source_map_sha256" --arg recorded_at "$recorded_at" --argjson videos "$videos_json" '
  {
    schema_version:1,kind:"m7_private_demo_video_bundle",result:"passed",
    certification_mode:$mode,privacy:"private_local_stealth",
    source_repository_commit:$source_commit,renderer_repository_commit:$renderer_commit,
    verifier_repository_commit:$verifier_commit,source_map_sha256:$source_map_sha256,
    recorded_at:$recorded_at,pairs:["xmr","zec"],
    scenarios:["happy","refund","concurrent"],videos:$videos,
    zec_concurrent_evidence_model:"layered_process_concurrency_plus_actual_node_pair_effects",
    zec_concurrent_joined_actual_node_run:false,
    public_rpc_used:false,public_peer_used:false,faucet_used:false,
    public_funds_used:false,public_deployment_used:false,
    certification_success_depends_on_external_network:false
  }
' >"$output_file"
chmod 0600 -- "$output_file"
echo "M7 private demo-video bundle passed: ${output_file}"
