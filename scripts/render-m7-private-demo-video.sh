#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
repository_root="$(pwd -P)"
readonly repository_root
readonly source_verifier="${repository_root}/scripts/verify-m7-private-demo-source.sh"
readonly source_map="${repository_root}/docs/evidence/m7-private-demo-sources.json"
readonly vhs_image='ghcr.io/charmbracelet/vhs@sha256:9d5fc3dc0c160b0fb1d2212baff07e6bdf3fa9438c504a3237484567302fcf93'

fail() {
  echo "M7 private demo-video render failed: $*" >&2
  exit 1
}

for dependency in awk dd git jq realpath sha256sum stat; do
  command -v "$dependency" >/dev/null 2>&1 || fail "missing dependency: ${dependency}"
done
(( $# == 3 )) || fail "provide pair, scenario, and an absolute output root"
readonly pair="$1" scenario="$2" output_root="$3"
[[ "$pair" == xmr || "$pair" == zec ]] || fail "pair must be xmr or zec"
[[ "$scenario" == happy || "$scenario" == refund || "$scenario" == concurrent ]] ||
  fail "scenario must be happy, refund, or concurrent"
[[ -x "$source_verifier" && ! -L "$source_verifier" ]] || fail "source verifier is missing"
[[ -f "$source_map" && ! -L "$source_map" ]] || fail "source map is missing"
[[ "$output_root" == /* && "$output_root" =~ ^[a-zA-Z0-9_./-]+$ ]] ||
  fail "output root must use an absolute safe path"
[[ ! -L "$output_root" ]] || fail "output root must not be a symlink"

readonly testing="${M7_PRIVATE_DEMO_VIDEO_TESTING:-0}"
case "$testing" in
  0)
    [[ -z "${M7_PRIVATE_DEMO_VIDEO_TEST_RENDERER:-}" ]] ||
      fail "test renderer override is forbidden outside test mode"
    readonly certification_mode="live_actual_nodes"
    ;;
  1)
    readonly test_renderer="${M7_PRIVATE_DEMO_VIDEO_TEST_RENDERER:?test renderer is required}"
    [[ "$test_renderer" == /* && "$test_renderer" =~ ^[a-zA-Z0-9_./-]+$ ]] ||
      fail "test renderer must use an absolute safe path"
    [[ -x "$test_renderer" && -f "$test_renderer" && ! -L "$test_renderer" ]] ||
      fail "test renderer must be an executable regular file"
    readonly certification_mode="test_contract"
    ;;
  *) fail "M7_PRIVATE_DEMO_VIDEO_TESTING must be exactly 0 or 1" ;;
esac

renderer_repository_commit="$(git rev-parse --verify HEAD)"
readonly renderer_repository_commit
if [[ "$testing" == 0 ]]; then
  command -v docker >/dev/null 2>&1 || fail "missing dependency: docker"
  [[ -z "$(git status --porcelain=v1 --untracked-files=normal)" ]] ||
    fail "live rendering requires a clean worktree"
  docker image inspect "$vhs_image" >/dev/null 2>&1 ||
    fail "the digest-pinned VHS image is absent"
fi

output_dir="${output_root}/${pair}/${scenario}"
readonly output_dir
[[ ! -e "$output_dir" && ! -L "$output_dir" ]] ||
  fail "video output already exists: ${output_dir}"
if [[ "$testing" == 0 && "$output_dir" == "${repository_root}/"* ]]; then
  git check-ignore -q -- "$output_dir" || fail "private output inside the repository must be ignored"
fi
umask 077
mkdir -p -- "$(dirname "$output_dir")"
mkdir -- "$output_dir"
chmod 0700 -- "$output_dir"

readonly proof_file="${output_dir}/proof.json"
readonly walkthrough_file="${output_dir}/walkthrough.txt"
readonly demo_file="${output_dir}/demo.sh"
readonly tape_file="${output_dir}/demo.tape"
readonly video_file="${output_dir}/demo.mp4"
readonly manifest_file="${output_dir}/video.json"

"$source_verifier" "$pair" "$scenario" | jq -cS . >"$proof_file"
chmod 0600 -- "$proof_file"
jq -e --arg pair "$pair" --arg scenario "$scenario" '
  .kind == "m7_private_demo_source_proof" and .result == "passed" and
  .entry.pair == $pair and .entry.scenario == $scenario and
  (.entry.network_lines | length >= 2) and (.entry.flow_lines | length >= 5) and
  (.entry.atomicity | length >= 40)
' "$proof_file" >/dev/null || fail "source proof contract is invalid"

run_id="$(jq -er '.entry.run_id' "$proof_file")"
evidence_model="$(jq -er '.entry.evidence_model' "$proof_file")"
joined_actual_node_concurrency="$(jq -cr '.joined_actual_node_concurrency' "$proof_file")"
readonly run_id evidence_model joined_actual_node_concurrency
pair_upper="${pair^^}"
scenario_upper="${scenario^^}"
readonly pair_upper scenario_upper

{
  printf 'M7 %s-LEZ PRIVATE LOCAL DEMO\n%s\n' "$pair_upper" "$scenario_upper"
  printf 'Run: %s\nSource commit: %s\n\n' "$run_id" "$renderer_repository_commit"
  printf '%s\n' 'LOCAL NETWORKS'
  jq -r '.entry.network_lines[]' "$proof_file"
  printf '%s\n\n' 'Public RPC / peer / faucet / funds / deployment: no / no / no / no / no'
  printf '%s\n' 'ROLE-CORRECT FLOW'
  jq -r '.entry.flow_lines[]' "$proof_file"
  printf '\nATOMICITY BOUNDARY\n%s\n' "$(jq -er '.entry.atomicity' "$proof_file")"
  printf 'Evidence model: %s\nJoined actual-node concurrency: %s\n' "$evidence_model" "$joined_actual_node_concurrency"
  printf '%s\n' 'Result: PASSED'
} >"$walkthrough_file"
chmod 0600 -- "$walkthrough_file"

emit_page() {
  printf 'page'
  local line
  for line in "$@"; do
    printf ' %q' "$line"
  done
  printf '\n'
}

{
  printf '%s\n' '#!/bin/sh' 'set -eu' 'page() {' '  clear' "  printf 'M7 ${pair_upper}-LEZ PRIVATE LOCAL DEMO\\n\\n'" '  printf "%s\\n" "$@"' '  sleep 3' '}'
  emit_page "$scenario_upper" "Run ${run_id}" "Source commit ${renderer_repository_commit:0:12}" "No public RPC, peer, faucet, funds, or deployment"
  mapfile -t networks < <(jq -r '.entry.network_lines[]' "$proof_file")
  emit_page 'LOCAL NODE TOPOLOGY' "${networks[@]}"
  mapfile -t flow < <(jq -r '.entry.flow_lines[]' "$proof_file")
  emit_page 'ROLE-CORRECT FLOW' "${flow[@]}"
  emit_page 'ATOMICITY BOUNDARY' "$(jq -er '.entry.atomicity' "$proof_file")" "Evidence model: ${evidence_model}"
  if [[ "$pair" == zec && "$scenario" == concurrent ]]; then
    emit_page 'EVIDENCE LIMIT' 'Scheduler/state overlap and actual-node ZEC effects are separate bound layers' 'This is not represented as one joined concurrent chain run'
  fi
  emit_page 'VERIFIED RESULT' "Source-map $(jq -er '.source_map_sha256' "$proof_file" | cut -c 1-16)..." 'Proof and every source certificate are SHA-256 bound' 'Replay introduces no claimed second effect' 'RESULT: PASSED'
} >"$demo_file"
chmod 0600 -- "$demo_file"

{
  printf 'Output demo.mp4\n'
  printf 'Set Width 1280\nSet Height 720\nSet FontSize 18\nSet Framerate 30\n'
  printf 'Set TypingSpeed 1ms\nSet Theme "Catppuccin Frappe"\nSet WindowBar Rings\n'
  printf 'Type "sh ./demo.sh"\nEnter\nSleep 24s\n'
} >"$tape_file"
chmod 0600 -- "$tape_file"

if [[ "$testing" == 1 ]]; then
  "$test_renderer" "$tape_file" "$video_file"
  duration_seconds="3.000000"
  renderer_name="contract_fixture"
  renderer_version="test-only"
  renderer_image="none"
else
  container_suffix="$(printf '%s' "${pair}:${scenario}:${run_id}" | sha256sum | cut -c 1-12)"
  container_name="lez-atomic-swaps-m7-vhs-${pair}-${scenario}-${container_suffix}"
  readonly container_suffix container_name
  docker run --rm --name "$container_name" --label "org.logos-co.atomic-swaps.run=${run_id}" --network none --read-only --cap-drop ALL --security-opt no-new-privileges --pids-limit 128 --memory 1g --cpus 2 --tmpfs /tmp:rw,nosuid,nodev,size=128m --user "$(id -u):$(id -g)" --env HOME=/tmp --mount "type=bind,src=${output_dir},dst=/vhs" --workdir /vhs "$vhs_image" demo.tape
  duration_seconds="$(
    docker run --rm --name "${container_name}-probe" --network none --read-only --cap-drop ALL --security-opt no-new-privileges --pids-limit 32 --memory 128m --cpus 1 --user "$(id -u):$(id -g)" --mount "type=bind,src=${output_dir},dst=/vhs,readonly" --entrypoint ffprobe "$vhs_image" -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 /vhs/demo.mp4
  )"
  renderer_name="VHS"
  renderer_version="0.11.0"
  renderer_image="$vhs_image"
fi
readonly duration_seconds renderer_name renderer_version renderer_image
[[ -s "$video_file" && ! -L "$video_file" ]] || fail "renderer did not produce an MP4"
chmod 0600 -- "$video_file"
[[ "$(dd if="$video_file" bs=1 skip=4 count=4 status=none)" == ftyp ]] ||
  fail "rendered output is not an ISO-BMFF MP4"
[[ "$duration_seconds" =~ ^[0-9]+([.][0-9]+)?$ ]] || fail "invalid video duration"
awk -v duration="$duration_seconds" 'BEGIN { exit !(duration > 0) }' ||
  fail "rendered duration must be positive"

source_map_sha256="$(sha256sum "$source_map" | cut -d ' ' -f 1)"
proof_sha256="$(sha256sum "$proof_file" | cut -d ' ' -f 1)"
walkthrough_sha256="$(sha256sum "$walkthrough_file" | cut -d ' ' -f 1)"
demo_sha256="$(sha256sum "$demo_file" | cut -d ' ' -f 1)"
tape_sha256="$(sha256sum "$tape_file" | cut -d ' ' -f 1)"
video_sha256="$(sha256sum "$video_file" | cut -d ' ' -f 1)"
video_size_bytes="$(stat -c '%s' "$video_file")"
readonly source_map_sha256 proof_sha256 walkthrough_sha256 demo_sha256 tape_sha256
readonly video_sha256 video_size_bytes

jq -n --arg mode "$certification_mode" --arg pair "$pair" --arg scenario "$scenario" --arg run_id "$run_id" --arg evidence_model "$evidence_model" --argjson joined "$joined_actual_node_concurrency" --arg commit "$renderer_repository_commit" --arg source_map_sha256 "$source_map_sha256" --arg proof_sha256 "$proof_sha256" --arg walkthrough_sha256 "$walkthrough_sha256" --arg demo_sha256 "$demo_sha256" --arg tape_sha256 "$tape_sha256" --arg video_sha256 "$video_sha256" --arg video_size "$video_size_bytes" --arg duration "$duration_seconds" --arg renderer_name "$renderer_name" --arg renderer_version "$renderer_version" --arg renderer_image "$renderer_image" --argjson networks "$(jq -c '.entry.network_lines' "$proof_file")" '
  {
    schema_version:1,kind:"m7_private_demo_video",result:"passed",
    certification_mode:$mode,privacy:"private_local_stealth",
    pair:$pair,scenario:$scenario,run_id:$run_id,evidence_model:$evidence_model,
    joined_actual_node_concurrency:$joined,
    source_repository_commit:$commit,renderer_repository_commit:$commit,
    source_map:{file:"docs/evidence/m7-private-demo-sources.json",sha256:$source_map_sha256},
    networks:$networks,
    external_resources:{public_rpc:false,public_peer:false,faucet:false,
      public_funds:false,public_deployment:false,
      certification_success_depends_on_external_network:false},
    proof:{file:"proof.json",sha256:$proof_sha256},
    walkthrough:{file:"walkthrough.txt",sha256:$walkthrough_sha256,
      demo_file:"demo.sh",demo_sha256:$demo_sha256,
      tape_file:"demo.tape",tape_sha256:$tape_sha256},
    video:{format:"video/mp4",file:"demo.mp4",sha256:$video_sha256,
      size_bytes:($video_size|tonumber),duration_seconds:$duration,
      renderer:{name:$renderer_name,version:$renderer_version,image:$renderer_image,network:"none"}}
  }
' >"$manifest_file"
chmod 0600 -- "$manifest_file"
echo "M7 private demo-video rendered: ${manifest_file}"
