#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
repository_root="$(pwd -P)"
readonly repository_root

fail() {
  echo "M3 private recording bundle failed: $*" >&2
  exit 1
}

network_contract() {
  jq -cS '
    {
      bitcoin_core: {
        version: .networks.bitcoin_core.version,
        network: .networks.bitcoin_core.network
      },
      lez: {
        version: .networks.lez.version,
        network: .networks.lez.network
      }
    }
  ' "$1"
}

for dependency in git jq realpath scriptreplay sha256sum stat; do
  command -v "$dependency" >/dev/null 2>&1 || fail "missing dependency: ${dependency}"
done

(( $# == 3 )) || fail "provide exactly the happy, refund, and concurrent recording manifests"
readonly testing="${M3_RECORDING_BUNDLE_TESTING:-0}"
case "$testing" in
  0)
    readonly expected_certification_mode="live_actual_nodes"
    ;;
  1)
    readonly expected_certification_mode="test_contract"
    ;;
  *)
    fail "M3_RECORDING_BUNDLE_TESTING must be exactly 0 or 1"
    ;;
esac

readonly output_file="${M3_RECORDING_BUNDLE_OUTPUT:?M3_RECORDING_BUNDLE_OUTPUT is required}"
[[ "$output_file" == /* ]] || fail "bundle output path must be absolute"
[[ ! -e "$output_file" && ! -L "$output_file" ]] || fail "bundle output already exists"

declare -A seen_scenarios=()
declare -A seen_run_ids=()
entries=()
bundle_commit=""
networks_json=""
readonly verifier_repository_commit="$(git rev-parse --verify HEAD)"

for manifest in "$@"; do
  [[ -f "$manifest" && ! -L "$manifest" ]] || fail "manifest must be a regular non-symlink file"
  manifest_abs="$(realpath -e -- "$manifest")"
  [[ "$(stat -c '%a' "$manifest_abs")" == 600 ]] || fail "recording manifest must have mode 0600"

  jq -e --arg mode "$expected_certification_mode" '
    .schema_version == 1 and
    .kind == "m3_private_terminal_recording" and
    .result == "passed" and
    .certification_mode == $mode and
    .privacy == "private_local_stealth" and
    (.scenario == "happy" or .scenario == "refund" or .scenario == "concurrent") and
    (.run_id | test("^[a-zA-Z0-9][a-zA-Z0-9._-]{0,95}$")) and
    (.repository_commit | test("^[0-9a-f]{40}$")) and
    (.recorded_at | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$")) and
    .networks.bitcoin_core.run_id == (.run_id + "-btc") and
    .networks.bitcoin_core.version == "31.1" and
    .networks.bitcoin_core.network == "regtest" and
    .networks.lez.run_id == (.run_id + "-lez") and
    .networks.lez.version == "v0.2.0" and
    .networks.lez.network == "private_local" and
    .networks.lez.slot_duration_seconds ==
      (if .scenario == "refund" then "3.0" else "1.0" end) and
    .external_resources.public_rpc == false and
    .external_resources.faucet == false and
    .external_resources.public_funds == false and
    .external_resources.certification_success_depends_on_external_network == false and
    .recording.format == "util-linux-script-classic-v1" and
    .recording.output_file == "terminal.typescript" and
    .recording.timing_file == "terminal.timing" and
    (.recording.output_sha256 | test("^[0-9a-f]{64}$")) and
    (.recording.timing_sha256 | test("^[0-9a-f]{64}$")) and
    .replay.argv == [
      "scriptreplay",
      "--log-timing",
      "terminal.timing",
      "--log-out",
      "terminal.typescript"
    ] and
    (.evidence.sha256 | test("^[0-9a-f]{64}$")) and
    (.evidence.packet | type == "string" and length > 0)
  ' "$manifest_abs" >/dev/null || fail "recording manifest contract is invalid"

  scenario="$(jq -er '.scenario' "$manifest_abs")"
  run_id="$(jq -er '.run_id' "$manifest_abs")"
  repository_commit="$(jq -er '.repository_commit' "$manifest_abs")"
  [[ -z "${seen_scenarios[$scenario]:-}" ]] || fail "duplicate recording scenario: ${scenario}"
  [[ -z "${seen_run_ids[$run_id]:-}" ]] || fail "duplicate recording run ID: ${run_id}"
  seen_scenarios["$scenario"]=1
  seen_run_ids["$run_id"]=1

  if [[ -z "$bundle_commit" ]]; then
    bundle_commit="$repository_commit"
    networks_json="$(network_contract "$manifest_abs")"
  else
    [[ "$repository_commit" == "$bundle_commit" ]] ||
      fail "all recordings must bind the same repository commit"
    [[ "$(network_contract "$manifest_abs")" == "$networks_json" ]] ||
      fail "all recordings must bind the same chain versions and networks"
  fi

  recording_dir="$(dirname "$manifest_abs")"
  typescript_file="${recording_dir}/terminal.typescript"
  timing_file="${recording_dir}/terminal.timing"
  [[ -s "$typescript_file" && ! -L "$typescript_file" ]] ||
    fail "terminal output recording is missing"
  [[ -s "$timing_file" && ! -L "$timing_file" ]] ||
    fail "terminal timing recording is missing"
  [[ "$(stat -c '%a' "$typescript_file")" == 600 ]] ||
    fail "terminal output recording must have mode 0600"
  [[ "$(stat -c '%a' "$timing_file")" == 600 ]] ||
    fail "terminal timing recording must have mode 0600"

  output_sha256="$(sha256sum "$typescript_file" | cut -d ' ' -f 1)"
  timing_sha256="$(sha256sum "$timing_file" | cut -d ' ' -f 1)"
  [[ "$output_sha256" == "$(jq -er '.recording.output_sha256' "$manifest_abs")" ]] ||
    fail "terminal output hash mismatch"
  [[ "$timing_sha256" == "$(jq -er '.recording.timing_sha256' "$manifest_abs")" ]] ||
    fail "terminal timing hash mismatch"
  scriptreplay --summary --log-timing "$timing_file" --log-out "$typescript_file" >/dev/null ||
    fail "terminal recording is not replayable"

  evidence_path="$(jq -er '.evidence.packet' "$manifest_abs")"
  if [[ "$evidence_path" == /* ]]; then
    evidence_file="$evidence_path"
  else
    evidence_file="${repository_root}/${evidence_path}"
  fi
  [[ -f "$evidence_file" && ! -L "$evidence_file" ]] ||
    fail "bound actual-node evidence packet is missing"
  evidence_abs="$(realpath -e -- "$evidence_file")"
  evidence_sha256="$(sha256sum "$evidence_abs" | cut -d ' ' -f 1)"
  [[ "$evidence_sha256" == "$(jq -er '.evidence.sha256' "$manifest_abs")" ]] ||
    fail "actual-node evidence packet hash mismatch"

  case "$scenario" in
    happy)
      expected_kind="m3_actor_two_direction_local_poc"
      expected_journey="claim"
      expected_schedule="sequential"
      ;;
    refund)
      expected_kind="m3_actor_two_direction_refund_local_poc"
      expected_journey="refund"
      expected_schedule="sequential"
      ;;
    concurrent)
      expected_kind="m3_actor_overlapping_two_swap_local_poc"
      expected_journey="claim"
      expected_schedule="overlap"
      ;;
  esac

  jq -e \
    --arg kind "$expected_kind" \
    --arg journey "$expected_journey" \
    --arg schedule "$expected_schedule" \
    --arg run_id "$run_id" \
    --arg repository_commit "$repository_commit" \
    --slurpfile evidence "$evidence_abs" '
      .evidence_packet_kind == $kind and
      .journey == $journey and
      .schedule == $schedule and
      .run_id == $run_id and
      .repository_commit == $repository_commit and
      .networks == $evidence[0].services and
      .external_resources == $evidence[0].external_resources and
      $evidence[0].schema_version == 1 and
      $evidence[0].kind == $kind and
      $evidence[0].journey == $journey and
      $evidence[0].schedule == $schedule and
      $evidence[0].result == "passed" and
      $evidence[0].run_id == $run_id and
      $evidence[0].repository_commit == $repository_commit
    ' "$manifest_abs" >/dev/null || fail "recording does not bind the selected live scenario"

  manifest_ref="$manifest_abs"
  evidence_ref="$evidence_abs"
  if [[ "$manifest_abs" == "${repository_root}/"* ]]; then
    manifest_ref="${manifest_abs#"${repository_root}/"}"
  fi
  if [[ "$evidence_abs" == "${repository_root}/"* ]]; then
    evidence_ref="${evidence_abs#"${repository_root}/"}"
  fi
  manifest_sha256="$(sha256sum "$manifest_abs" | cut -d ' ' -f 1)"
  entries+=("$(
    jq -cn \
      --arg scenario "$scenario" \
      --arg run_id "$run_id" \
      --arg manifest "$manifest_ref" \
      --arg manifest_sha256 "$manifest_sha256" \
      --arg output_sha256 "$output_sha256" \
      --arg timing_sha256 "$timing_sha256" \
      --arg evidence "$evidence_ref" \
      --arg evidence_sha256 "$evidence_sha256" '
        {
          scenario: $scenario,
          run_id: $run_id,
          manifest: $manifest,
          manifest_sha256: $manifest_sha256,
          output_sha256: $output_sha256,
          timing_sha256: $timing_sha256,
          evidence: $evidence,
          evidence_sha256: $evidence_sha256
        }
      '
  )")
done

for required_scenario in happy refund concurrent; do
  [[ -n "${seen_scenarios[$required_scenario]:-}" ]] ||
    fail "missing recording scenario: ${required_scenario}"
done

if [[ "$testing" == 0 ]]; then
  git cat-file -e "${bundle_commit}^{commit}" >/dev/null 2>&1 ||
    fail "live recording bundle commit is not present in this repository"
  git merge-base --is-ancestor "$bundle_commit" "$verifier_repository_commit" ||
    fail "live recording bundle commit is not an ancestor of the verifier checkout"
  [[ -z "$(git status --porcelain=v1 --untracked-files=normal)" ]] ||
    fail "live recording bundle requires a clean worktree"
  if [[ "$output_file" == "${repository_root}/"* ]]; then
    git check-ignore -q -- "$output_file" ||
      fail "private bundle inside the repository must be ignored by Git"
  fi
fi

recordings_json="$(printf '%s\n' "${entries[@]}" | jq -cs 'sort_by(.scenario)')"
recorded_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
readonly recordings_json recorded_at

output_parent="$(dirname "$output_file")"
umask 077
mkdir -p -- "$output_parent"
[[ -d "$output_parent" && ! -L "$output_parent" ]] ||
  fail "bundle output parent must be a regular directory"
output_tmp="$(mktemp "${output_parent}/.recording-bundle.json.XXXXXX")"
trap 'rm -f -- "${output_tmp:-}"' EXIT
jq -n \
  --arg certification_mode "$expected_certification_mode" \
  --arg repository_commit "$bundle_commit" \
  --arg verifier_repository_commit "$verifier_repository_commit" \
  --arg recorded_at "$recorded_at" \
  --argjson networks "$networks_json" \
  --argjson recordings "$recordings_json" '
    {
      schema_version: 1,
      kind: "m3_private_terminal_recording_bundle",
      result: "passed",
      certification_mode: $certification_mode,
      privacy: "private_local_stealth",
      repository_commit: $repository_commit,
      verifier_repository_commit: $verifier_repository_commit,
      recorded_at: $recorded_at,
      networks: $networks,
      isolated_run_network_metadata: "retained_in_each_hashed_recording_manifest",
      recordings: $recordings,
      scenarios: ["happy", "refund", "concurrent"],
      public_rpc_used: false,
      faucet_used: false,
      public_funds_used: false,
      certification_success_depends_on_external_network: false
    }
  ' >"$output_tmp"
chmod 600 -- "$output_tmp"
mv -- "$output_tmp" "$output_file"
trap - EXIT

echo "M3 private terminal-recording bundle passed: ${output_file}"
