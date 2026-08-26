#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
repository_root="$(pwd -P)"
readonly repository_root

fail() {
  echo "M3 private demo-video render failed: $*" >&2
  exit 1
}

for dependency in awk dd git jq realpath scriptreplay sha256sum stat; do
  command -v "$dependency" >/dev/null 2>&1 || fail "missing dependency: ${dependency}"
done

(( $# == 2 )) || fail "provide exactly a recording manifest and an absolute output root"
readonly source_manifest="$1"
readonly output_root="$2"
readonly source_verifier="${repository_root}/scripts/verify-m3-private-demo-source.sh"
[[ -x "$source_verifier" && ! -L "$source_verifier" ]] || fail "source verifier is missing"
readonly testing="${M3_PRIVATE_DEMO_VIDEO_TESTING:-0}"
readonly vhs_image='ghcr.io/charmbracelet/vhs@sha256:9d5fc3dc0c160b0fb1d2212baff07e6bdf3fa9438c504a3237484567302fcf93'

case "$testing" in
  0)
    [[ -z "${M3_PRIVATE_DEMO_VIDEO_TEST_RENDERER:-}" ]] ||
      fail "the test renderer override is forbidden outside test mode"
    readonly certification_mode="live_actual_nodes"
    ;;
  1)
    [[ -n "${M3_PRIVATE_DEMO_VIDEO_TEST_RENDERER:-}" ]] ||
      fail "test mode requires a renderer fixture"
    readonly test_renderer="${M3_PRIVATE_DEMO_VIDEO_TEST_RENDERER}"
    [[ "$test_renderer" == /* && "$test_renderer" =~ ^[a-zA-Z0-9_./-]+$ ]] ||
      fail "test renderer must use an absolute safe path"
    [[ -f "$test_renderer" && ! -L "$test_renderer" && -x "$test_renderer" ]] ||
      fail "test renderer must be an executable regular file"
    readonly certification_mode="test_contract"
    ;;
  *) fail "M3_PRIVATE_DEMO_VIDEO_TESTING must be exactly 0 or 1" ;;
esac

[[ -f "$source_manifest" && ! -L "$source_manifest" ]] ||
  fail "source recording manifest must be a regular non-symlink file"
source_manifest_abs="$(realpath -e -- "$source_manifest")"
readonly source_manifest_abs
[[ "$(stat -c '%a' "$source_manifest_abs")" == 600 ]] ||
  fail "source recording manifest must have mode 0600"
[[ "$output_root" == /* && "$output_root" =~ ^[a-zA-Z0-9_./-]+$ ]] ||
  fail "output root must use an absolute safe path"
[[ ! -L "$output_root" ]] || fail "output root must not be a symlink"

jq -e --arg mode "$certification_mode" '
  .schema_version == 1 and
  .kind == "m3_private_terminal_recording" and
  .result == "passed" and
  .certification_mode == $mode and
  .privacy == "private_local_stealth" and
  (.scenario == "happy" or .scenario == "refund" or .scenario == "concurrent") and
  (.run_id | test("^[a-zA-Z0-9][a-zA-Z0-9._-]{0,95}$")) and
  (.repository_commit | test("^[0-9a-f]{40}$")) and
  .networks.bitcoin_core.version == "31.1" and
  .networks.bitcoin_core.network == "regtest" and
  .networks.lez.version == "v0.2.0" and
  .networks.lez.network == "private_local" and
  .external_resources.public_rpc == false and
  .external_resources.faucet == false and
  .external_resources.public_funds == false and
  .external_resources.certification_success_depends_on_external_network == false and
  .recording.format == "util-linux-script-classic-v1" and
  .recording.output_file == "terminal.typescript" and
  .recording.timing_file == "terminal.timing" and
  (.recording.output_sha256 | test("^[0-9a-f]{64}$")) and
  (.recording.timing_sha256 | test("^[0-9a-f]{64}$")) and
  (.evidence.sha256 | test("^[0-9a-f]{64}$"))
' "$source_manifest_abs" >/dev/null || fail "source recording manifest contract is invalid"

scenario="$(jq -er '.scenario' "$source_manifest_abs")"
run_id="$(jq -er '.run_id' "$source_manifest_abs")"
source_repository_commit="$(jq -er '.repository_commit' "$source_manifest_abs")"
readonly scenario run_id source_repository_commit
recording_dir="$(dirname "$source_manifest_abs")"
typescript_file="${recording_dir}/$(jq -er '.recording.output_file' "$source_manifest_abs")"
timing_file="${recording_dir}/$(jq -er '.recording.timing_file' "$source_manifest_abs")"
for private_file in "$typescript_file" "$timing_file"; do
  [[ -s "$private_file" && ! -L "$private_file" ]] || fail "source terminal recording is missing"
  [[ "$(stat -c '%a' "$private_file")" == 600 ]] || fail "source terminal recording must have mode 0600"
done
output_sha256="$(sha256sum "$typescript_file" | cut -d ' ' -f 1)"
timing_sha256="$(sha256sum "$timing_file" | cut -d ' ' -f 1)"
readonly output_sha256 timing_sha256
[[ "$output_sha256" == "$(jq -er '.recording.output_sha256' "$source_manifest_abs")" ]] ||
  fail "source terminal output hash mismatch"
[[ "$timing_sha256" == "$(jq -er '.recording.timing_sha256' "$source_manifest_abs")" ]] ||
  fail "source terminal timing hash mismatch"
scriptreplay --summary --log-timing "$timing_file" --log-out "$typescript_file" >/dev/null ||
  fail "source terminal recording is not replayable"

evidence_path="$(jq -er '.evidence.packet' "$source_manifest_abs")"
if [[ "$evidence_path" == /* ]]; then
  evidence_file="$evidence_path"
else
  evidence_file="${repository_root}/${evidence_path}"
fi
[[ -f "$evidence_file" && ! -L "$evidence_file" ]] || fail "source actual-node evidence is missing"
evidence_abs="$(realpath -e -- "$evidence_file")"
evidence_sha256="$(sha256sum "$evidence_abs" | cut -d ' ' -f 1)"
readonly evidence_abs evidence_sha256
[[ "$evidence_sha256" == "$(jq -er '.evidence.sha256' "$source_manifest_abs")" ]] ||
  fail "source actual-node evidence hash mismatch"

case "$scenario" in
  happy)
    expected_kind="m3_actor_two_direction_local_poc"
    expected_journey="claim"
    expected_schedule="sequential"
    expected_terminal="completed"
    scenario_title="HAPPY PATH"
    scenario_flow="Taker first lock -> Maker second lock -> revealing claim -> extracted follow-up claim"
    demonstrates='["both_trade_directions","actual_node_effects","role_separated_maker_taker","private_local_nodes","zero_replay_resubmission","terminal_claim_completion"]'
    ;;
  refund)
    expected_kind="m3_actor_two_direction_refund_local_poc"
    expected_journey="refund"
    expected_schedule="sequential"
    expected_terminal="refunded"
    scenario_title="REFUND / TIMEOUT"
    scenario_flow="Taker first lock -> Maker second lock -> no reveal -> earlier Maker refund -> later Taker refund"
    demonstrates='["both_trade_directions","actual_node_effects","role_separated_maker_taker","private_local_nodes","zero_replay_resubmission","ordered_timelock_refunds"]'
    ;;
  concurrent)
    expected_kind="m3_actor_overlapping_two_swap_local_poc"
    expected_journey="claim"
    expected_schedule="overlap"
    expected_terminal="completed"
    scenario_title="CONCURRENT SWAPS"
    scenario_flow="Opposite directions overlap at both-legs-locked revision 2, then settle independently"
    demonstrates='["both_trade_directions","actual_node_effects","role_separated_maker_taker","private_local_nodes","zero_replay_resubmission","simultaneous_revision_two_overlap"]'
    ;;
esac
readonly expected_kind expected_journey expected_schedule expected_terminal
readonly scenario_title scenario_flow demonstrates

jq -e \
  --arg kind "$expected_kind" \
  --arg journey "$expected_journey" \
  --arg schedule "$expected_schedule" \
  --arg terminal "$expected_terminal" \
  --arg run_id "$run_id" \
  --arg commit "$source_repository_commit" '
    .schema_version == 1 and .kind == $kind and .journey == $journey and
    .schedule == $schedule and .result == "passed" and .run_id == $run_id and
    .repository_commit == $commit and .private_material_disclosed == false and
    .replay_resubmission_count == 0 and
    ([.directions[].direction] | sort == ["taker_sells_foreign","taker_sells_lez"]) and
    all(.directions[];
      .terminal_revision == 4 and .terminal_phase == $terminal and
      .expected_unique_effects == {bitcoin:2,lez:3} and
      .maker_second_lock_effect_count == 1 and
      (.stage_two_evidence_sha256 | test("^[0-9a-f]{64}$"))) and
    (if $schedule == "overlap" then
      .concurrency.simultaneous_in_flight == true and
      .concurrency.overlap_revision == 2 and
      .concurrency.overlap_phase == "both_legs_locked" and
      .concurrency.distinct_funding_outpoints == true and
      .concurrency.distinct_agreements == true and
      .concurrency.distinct_actor_state_dbs == true and
      .concurrency.distinct_signing_journals == true and
      .concurrency.distinct_signer_sessions_per_domain == true and
      .concurrency.distinct_escrows == true and
      .concurrency.distinct_deadlines == true
    else true end)
  ' "$evidence_abs" >/dev/null || fail "actual-node evidence does not prove the selected demo scenario"

renderer_repository_commit="$(git rev-parse --verify HEAD)"
readonly renderer_repository_commit
if [[ "$testing" == 0 ]]; then
  command -v docker >/dev/null 2>&1 || fail "missing dependency: docker"
  [[ -z "$(git status --porcelain=v1 --untracked-files=normal)" ]] ||
    fail "live demo-video rendering requires a clean worktree"
  git cat-file -e "${source_repository_commit}^{commit}" >/dev/null 2>&1 ||
    fail "source recording commit is absent from this repository"
  git merge-base --is-ancestor "$source_repository_commit" "$renderer_repository_commit" ||
    fail "source recording commit is not an ancestor of the renderer checkout"
  docker image inspect "$vhs_image" >/dev/null 2>&1 ||
    fail "pinned VHS image is absent; pull ghcr.io/charmbracelet/vhs:v0.11.0 and verify its digest"
fi

output_dir="${output_root}/${scenario}"
readonly output_dir
[[ ! -e "$output_dir" && ! -L "$output_dir" ]] || fail "video output already exists: ${output_dir}"
if [[ "$testing" == 0 && "$output_dir" == "${repository_root}/"* ]]; then
  git check-ignore -q -- "$output_dir" || fail "private videos inside the repository must be ignored by Git"
fi
umask 077
mkdir -p -- "$output_root"
[[ -d "$output_root" && ! -L "$output_root" ]] || fail "output root is not a regular directory"
mkdir -- "$output_dir"
chmod 700 -- "$output_dir"

walkthrough_file="${output_dir}/walkthrough.txt"
tape_file="${output_dir}/demo.tape"
demo_file="${output_dir}/demo.sh"
proof_file="${output_dir}/proof.json"
video_file="${output_dir}/demo.mp4"
manifest_file="${output_dir}/video.json"
readonly walkthrough_file tape_file demo_file proof_file video_file manifest_file

proof_tmp="$(mktemp "${output_dir}/.proof.json.XXXXXX")"
trap 'rm -f -- "${proof_tmp:-}"' EXIT
"$source_verifier" "$source_manifest_abs" >"$proof_tmp"
jq -e --arg scenario "$scenario" --arg run_id "$run_id" --arg commit "$source_repository_commit" \
  --arg mode "$certification_mode" '
    .schema_version == 1 and .kind == "m3_private_demo_proof" and .result == "passed" and
    .scenario == $scenario and .run_id == $run_id and .repository_commit == $commit and
    .certification_mode == $mode and (.source_inputs | length >= 14) and
    ([.directions[].role_terminals[].role] | sort == ["maker","maker","taker","taker"])
  ' "$proof_tmp" >/dev/null || fail "source proof contract is invalid"
chmod 600 -- "$proof_tmp"
mv -- "$proof_tmp" "$proof_file"
trap - EXIT

runtime_ms="$(jq -r '.performance.phase_timings.total_duration_ms // "not_recorded_in_contract_fixture"' "$evidence_abs")"
readonly runtime_ms
{
  printf '%s\n' 'M3 BTC-LEZ PRIVATE DEMO' "$scenario_title"
  printf 'Run: %s\nEvidence commit: %s\n' "$run_id" "$source_repository_commit"
  printf 'Source recording SHA-256: %s\n\n' "$(sha256sum "$source_manifest_abs" | cut -d ' ' -f 1)"
  printf '%s\n' 'ACTUAL LOCAL NODES'
  jq -r '"Bitcoin Core " + .services.bitcoin_core.version + " / " + .services.bitcoin_core.network,
    "LEZ " + .services.lez.version + " / " + .services.lez.network + " / slots " +
      .services.lez.slot_duration_seconds + "s"' "$evidence_abs"
  printf 'Public RPC / faucet / public funds: no / no / no\nRuntime evidence (ms): %s\n\n' "$runtime_ms"
  printf '%s\n%s\n\n' 'ROLE FLOW' "$scenario_flow"
  jq -r '.directions[] |
    "Direction: " + .direction,
    "  terminal: revision " + (.terminal_revision|tostring) + " / " + .terminal_phase,
    "  effects: Bitcoin=" + (.expected_unique_effects.bitcoin|tostring) +
      " LEZ=" + (.expected_unique_effects.lez|tostring) + " Maker-second-lock=1",
    "  stage-two evidence: " + .stage_two_evidence_sha256' "$evidence_abs"
  if [[ "$scenario" == concurrent ]]; then
    printf '\n%s\n' 'CONCURRENCY BARRIER'
    jq -r '"Both swaps in flight: " + (.concurrency.simultaneous_in_flight|tostring),
      "Overlap: revision " + (.concurrency.overlap_revision|tostring) + " / " + .concurrency.overlap_phase,
      "Distinct agreements/outpoints/stores/journals/sessions/escrows/deadlines: yes"' "$evidence_abs"
  fi
  printf '\n%s\n' 'ATOMICITY AND RECOVERY'
  printf '%s\n' \
    'Taker-first canonical lock is required before the Maker lock.' \
    'No adaptor witness is revealed before both locks are canonical.'
  if [[ "$scenario" == refund ]]; then
    printf '%s\n' 'Without a reveal, the earlier Maker-funded leg refunds before the later Taker-funded leg.'
  else
    printf '%s\n' 'The revealing claim enables point-checked extraction and the follow-up claim.'
  fi
  printf '%s\n' \
    'Replay public-effect resubmissions: 0' \
    'Atomicity claim: protocol ordering plus recoverability; no distributed atomic commit.' \
    'Result: PASSED'
} >"$walkthrough_file"
chmod 600 -- "$walkthrough_file"

emit_page() {
  printf 'page'
  local line
  for line in "$@"; do
    printf ' %q' "$line"
  done
  printf '\n'
}

{
  printf '%s\n' '#!/bin/sh' 'set -eu' \
    'page() {' \
    '  clear' \
    '  printf "M3 BTC-LEZ PRIVATE DEMO\\n\\n"' \
    '  printf "%s\\n" "$@"' \
    '  sleep 4' \
    '}'
  emit_page \
    "$scenario_title" \
    "Run $run_id" \
    "Bitcoin Core 31.1 Regtest and LEZ v0.2.0 private-local" \
    "Fresh one-shot Maker and Taker processes" \
    "Source commit ${source_repository_commit:0:12}" \
    "Source proof $(sha256sum "$proof_file" | cut -c 1-16)..."

  if [[ "$scenario" == concurrent ]]; then
    emit_page \
      'CONCURRENCY BARRIER - OBSERVED' \
      'taker_sells_foreign: revision 2 / both_legs_locked' \
      'taker_sells_lez: revision 2 / both_legs_locked' \
      'Both swaps simultaneously in flight on shared local nodes' \
      'Distinct agreements, outpoints, stores, journals, sessions, escrows, deadlines'
  fi

  for direction in taker_sells_foreign taker_sells_lez; do
    btc_first="$(jq -er --arg direction "$direction" '.directions[] | select(.direction == $direction) | .effects.bitcoin_effect_ids[0]' "$proof_file")"
    btc_terminal="$(jq -er --arg direction "$direction" '.directions[] | select(.direction == $direction) | .effects.bitcoin_effect_ids[1]' "$proof_file")"
    lez_first="$(jq -er --arg direction "$direction" '.directions[] | select(.direction == $direction) | .effects.lez_effect_ids[0]' "$proof_file")"
    lez_second="$(jq -er --arg direction "$direction" '.directions[] | select(.direction == $direction) | .effects.lez_effect_ids[1]' "$proof_file")"
    lez_terminal="$(jq -er --arg direction "$direction" '.directions[] | select(.direction == $direction) | .effects.lez_effect_ids[2]' "$proof_file")"
    if [[ "$direction" == taker_sells_foreign ]]; then
      lock_lines=(
        "TAKER  persist and submit Bitcoin first lock ${btc_first:0:12}..."
        'BITCOIN  canonical first lock observed'
        "MAKER  persist and submit LEZ initialize/fund ${lez_first:0:8}.../${lez_second:0:8}..."
      )
      if [[ "$scenario" == refund ]]; then
        terminal_lines=(
          'NO REVEAL  cooperative claim effects absent'
          "MAKER  earlier finalized LEZ refund ${lez_terminal:0:12}..."
          "TAKER  later confirmed Bitcoin refund ${btc_terminal:0:12}..."
        )
      else
        terminal_lines=(
          "TAKER  finalized LEZ revealing claim ${lez_terminal:0:12}..."
          'MAKER  point-check and extract adaptor witness'
          "MAKER  confirmed Bitcoin follow-up claim ${btc_terminal:0:12}..."
        )
      fi
    else
      lock_lines=(
        "TAKER  persist and submit LEZ initialize/fund ${lez_first:0:8}.../${lez_second:0:8}..."
        'LEZ  finalized first lock observed'
        "MAKER  persist and submit Bitcoin second lock ${btc_first:0:12}..."
      )
      if [[ "$scenario" == refund ]]; then
        terminal_lines=(
          'NO REVEAL  cooperative claim effects absent'
          "MAKER  earlier confirmed Bitcoin refund ${btc_terminal:0:12}..."
          "TAKER  later finalized LEZ refund ${lez_terminal:0:12}..."
        )
      else
        terminal_lines=(
          "TAKER  confirmed Bitcoin revealing claim ${btc_terminal:0:12}..."
          'MAKER  point-check and extract adaptor witness'
          "MAKER  finalized LEZ follow-up claim ${lez_terminal:0:12}..."
        )
      fi
    fi
    emit_page \
      "DIRECTION  $direction" \
      "${lock_lines[@]}" \
      "${terminal_lines[@]}" \
      "MAKER + TAKER  revision 4 / $expected_terminal" \
      'REPLAY  public-effect resubmissions 0'
  done
  emit_page \
    'CONDITIONAL ATOMICITY' \
    'Taker first lock is canonical before Maker second lock' \
    'No adaptor witness is revealed before both locks are canonical' \
    "$scenario_flow" \
    'Safety is ordering plus recoverability; no distributed atomic commit' \
    'RESULT  PASSED - bound to actual-node proof.json'
} >"$demo_file"
chmod 600 -- "$demo_file"

{
  printf 'Output demo.mp4\n'
  printf 'Set Width 1280\nSet Height 720\nSet FontSize 20\nSet Framerate 30\n'
  printf 'Set TypingSpeed 1ms\nSet Theme "Catppuccin Frappe"\nSet WindowBar Rings\n'
  printf 'Type "sh ./demo.sh"\nEnter\nSleep 22s\n'
} >"$tape_file"
chmod 600 -- "$tape_file"

if [[ "$testing" == 1 ]]; then
  "$test_renderer" "$tape_file" "$video_file"
  duration_seconds="3.000000"
  renderer_name="contract_fixture"
  renderer_version="test-only"
  renderer_image="none"
else
  container_suffix="$(sha256sum "$source_manifest_abs" | cut -c 1-12)"
  container_name="lez-atomic-swaps-vhs-${scenario}-${container_suffix}"
  readonly container_suffix container_name
  docker run --rm \
    --name "$container_name" \
    --label "org.logos-co.atomic-swaps.run=${run_id}" \
    --network none \
    --read-only \
    --cap-drop ALL \
    --security-opt no-new-privileges \
    --pids-limit 128 \
    --memory 1g \
    --cpus 2 \
    --tmpfs /tmp:rw,nosuid,nodev,size=128m \
    --user "$(id -u):$(id -g)" \
    --env HOME=/tmp \
    --mount "type=bind,src=${output_dir},dst=/vhs" \
    --workdir /vhs \
    "$vhs_image" demo.tape
  duration_seconds="$(
    docker run --rm \
      --name "${container_name}-probe" \
      --network none \
      --read-only \
      --cap-drop ALL \
      --security-opt no-new-privileges \
      --pids-limit 32 \
      --memory 128m \
      --cpus 1 \
      --user "$(id -u):$(id -g)" \
      --mount "type=bind,src=${output_dir},dst=/vhs,readonly" \
      --entrypoint ffprobe \
      "$vhs_image" \
      -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 /vhs/demo.mp4
  )"
  renderer_name="VHS"
  renderer_version="0.11.0"
  renderer_image="$vhs_image"
fi
readonly duration_seconds renderer_name renderer_version renderer_image

[[ -s "$video_file" && ! -L "$video_file" ]] || fail "renderer did not produce an MP4"
chmod 600 -- "$video_file"
[[ "$(dd if="$video_file" bs=1 skip=4 count=4 status=none)" == ftyp ]] ||
  fail "rendered output is not an ISO-BMFF MP4"
[[ "$duration_seconds" =~ ^[0-9]+([.][0-9]+)?$ ]] || fail "renderer returned an invalid duration"
awk -v duration="$duration_seconds" 'BEGIN { exit !(duration > 0) }' ||
  fail "rendered video duration must be positive"

source_manifest_ref="$source_manifest_abs"
evidence_ref="$evidence_abs"
if [[ "$source_manifest_abs" == "${repository_root}/"* ]]; then
  source_manifest_ref="${source_manifest_abs#"${repository_root}/"}"
fi
if [[ "$evidence_abs" == "${repository_root}/"* ]]; then
  evidence_ref="${evidence_abs#"${repository_root}/"}"
fi
source_manifest_sha256="$(sha256sum "$source_manifest_abs" | cut -d ' ' -f 1)"
walkthrough_sha256="$(sha256sum "$walkthrough_file" | cut -d ' ' -f 1)"
tape_sha256="$(sha256sum "$tape_file" | cut -d ' ' -f 1)"
demo_sha256="$(sha256sum "$demo_file" | cut -d ' ' -f 1)"
proof_sha256="$(sha256sum "$proof_file" | cut -d ' ' -f 1)"
proof_source_input_count="$(jq -er '.source_inputs | length' "$proof_file")"
video_sha256="$(sha256sum "$video_file" | cut -d ' ' -f 1)"
video_size_bytes="$(stat -c '%s' "$video_file")"
rendered_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
readonly source_manifest_ref evidence_ref source_manifest_sha256
readonly walkthrough_sha256 tape_sha256 demo_sha256 proof_sha256 proof_source_input_count
readonly video_sha256 video_size_bytes rendered_at

jq -n \
  --arg scenario "$scenario" \
  --arg run_id "$run_id" \
  --arg certification_mode "$certification_mode" \
  --arg source_repository_commit "$source_repository_commit" \
  --arg renderer_repository_commit "$renderer_repository_commit" \
  --arg rendered_at "$rendered_at" \
  --arg source_manifest "$source_manifest_ref" \
  --arg source_manifest_sha256 "$source_manifest_sha256" \
  --arg output_sha256 "$output_sha256" \
  --arg timing_sha256 "$timing_sha256" \
  --arg evidence "$evidence_ref" \
  --arg evidence_sha256 "$evidence_sha256" \
  --arg walkthrough_sha256 "$walkthrough_sha256" \
  --arg tape_sha256 "$tape_sha256" \
  --arg demo_sha256 "$demo_sha256" \
  --arg proof_sha256 "$proof_sha256" \
  --arg proof_source_input_count "$proof_source_input_count" \
  --arg video_sha256 "$video_sha256" \
  --arg video_size_bytes "$video_size_bytes" \
  --arg duration_seconds "$duration_seconds" \
  --arg renderer_name "$renderer_name" \
  --arg renderer_version "$renderer_version" \
  --arg renderer_image "$renderer_image" \
  --argjson demonstrates "$demonstrates" \
  --argjson networks "$(jq -c '.networks' "$source_manifest_abs")" \
  --argjson external_resources "$(jq -c '.external_resources' "$source_manifest_abs")" '
    {
      schema_version: 1,
      kind: "m3_private_demo_video",
      result: "passed",
      scenario: $scenario,
      run_id: $run_id,
      certification_mode: $certification_mode,
      privacy: "private_local_stealth",
      source_repository_commit: $source_repository_commit,
      renderer_repository_commit: $renderer_repository_commit,
      rendered_at: $rendered_at,
      networks: $networks,
      external_resources: $external_resources,
      demonstrates: $demonstrates,
      source_recording: {
        manifest: $source_manifest,
        manifest_sha256: $source_manifest_sha256,
        output_sha256: $output_sha256,
        timing_sha256: $timing_sha256,
        evidence: $evidence,
        evidence_sha256: $evidence_sha256
      },
      walkthrough: {
        file: "walkthrough.txt",
        sha256: $walkthrough_sha256,
        tape_file: "demo.tape",
        tape_sha256: $tape_sha256,
        demo_file: "demo.sh",
        demo_sha256: $demo_sha256
      },
      proof: {
        file: "proof.json",
        sha256: $proof_sha256,
        source_input_count: ($proof_source_input_count | tonumber)
      },
      video: {
        format: "video/mp4",
        file: "demo.mp4",
        sha256: $video_sha256,
        size_bytes: ($video_size_bytes | tonumber),
        duration_seconds: $duration_seconds,
        renderer: {
          name: $renderer_name,
          version: $renderer_version,
          image: $renderer_image,
          network: "none"
        }
      }
    }
  ' >"$manifest_file"
chmod 600 -- "$manifest_file"

echo "M3 private ${scenario} demo video passed: ${manifest_file}"
