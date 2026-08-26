#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
repository_root="$(pwd -P)"
readonly repository_root

fail() {
  echo "M3 private recording failed: $*" >&2
  exit 1
}

for dependency in git jq script scriptreplay sha256sum stat; do
  command -v "$dependency" >/dev/null 2>&1 || fail "missing dependency: ${dependency}"
done

readonly run_id="${RUN_ID:?RUN_ID is required}"
readonly scenario="${M3_RECORDING_SCENARIO:?M3_RECORDING_SCENARIO is required}"
readonly testing="${M3_RECORDING_TESTING:-0}"

[[ "$run_id" =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]{0,95}$ ]] ||
  fail "RUN_ID must be 1-96 safe path characters"

case "$scenario" in
  happy)
    readonly journey="claim"
    readonly schedule="sequential"
    readonly packet_kind="m3_actor_two_direction_local_poc"
    ;;
  refund)
    readonly journey="refund"
    readonly schedule="sequential"
    readonly packet_kind="m3_actor_two_direction_refund_local_poc"
    ;;
  concurrent)
    readonly journey="claim"
    readonly schedule="overlap"
    readonly packet_kind="m3_actor_overlapping_two_swap_local_poc"
    ;;
  *) fail "M3_RECORDING_SCENARIO must be happy, refund, or concurrent" ;;
esac

case "$testing" in
  0 | 1) ;;
  *) fail "M3_RECORDING_TESTING must be exactly 0 or 1" ;;
esac

if [[ "$testing" == 1 ]]; then
  [[ -n "${M3_RECORDING_TEST_DRIVER:-}" ]] || fail "test mode requires a test driver"
  [[ -n "${M3_RECORDING_TEST_EVIDENCE_FILE:-}" ]] ||
    fail "test mode requires a test evidence file"
  readonly driver="${M3_RECORDING_TEST_DRIVER}"
  readonly evidence_file="${M3_RECORDING_TEST_EVIDENCE_FILE}"
else
  [[ -z "${M3_RECORDING_TEST_DRIVER:-}" ]] ||
    fail "the test-driver override is forbidden outside test mode"
  [[ -z "${M3_RECORDING_TEST_EVIDENCE_FILE:-}" ]] ||
    fail "the test-evidence override is forbidden outside test mode"
  [[ -z "$(git status --porcelain=v1 --untracked-files=normal)" ]] ||
    fail "certifying recordings require a clean worktree"
  readonly driver="${repository_root}/scripts/run-m3-actor-local-poc.sh"
  readonly evidence_file="${repository_root}/.e2e/${run_id}/m3-actor-poc/evidence/m3-actor-local-poc.json"
fi

[[ "$driver" == /* ]] || fail "recording driver must use an absolute path"
[[ "$driver" =~ ^[a-zA-Z0-9_./-]+$ ]] || fail "recording driver path contains unsafe characters"
[[ -f "$driver" && ! -L "$driver" && -x "$driver" ]] ||
  fail "recording driver must be an executable regular file"

repository_commit="$(git rev-parse --verify HEAD)"
readonly repository_commit
[[ "$repository_commit" =~ ^[0-9a-f]{40}$ ]] || fail "could not resolve repository commit"
if [[ "$testing" == 1 ]]; then
  readonly certification_mode="test_contract"
else
  readonly certification_mode="live_actual_nodes"
fi

if [[ -n "${M3_RECORDING_PRIVATE_ROOT:-}" ]]; then
  readonly recording_root="${M3_RECORDING_PRIVATE_ROOT}"
else
  readonly recording_root="${repository_root}/.e2e/${run_id}/m3-recordings"
fi
[[ "$recording_root" == /* ]] || fail "recording root must be absolute"
[[ ! -L "$recording_root" ]] || fail "recording root must not be a symlink"

readonly output_dir="${recording_root}/${scenario}"
[[ ! -e "$output_dir" && ! -L "$output_dir" ]] ||
  fail "recording output already exists: ${output_dir}"

if [[ "$testing" == 0 && "$output_dir" == "${repository_root}/"* ]]; then
  git check-ignore -q -- "$output_dir" ||
    fail "recordings inside the repository must be ignored by Git"
fi

umask 077
mkdir -p -- "$recording_root"
[[ -d "$recording_root" && ! -L "$recording_root" ]] ||
  fail "recording root is not a regular directory"
mkdir -- "$output_dir"
chmod 700 -- "$output_dir"

readonly typescript_file="${output_dir}/terminal.typescript"
readonly timing_file="${output_dir}/terminal.timing"
readonly manifest_file="${output_dir}/recording.json"

export RUN_ID="$run_id"
export M3_RECORDING_SCENARIO="$scenario"
export M3_ACTOR_POC_JOURNEY="$journey"
export M3_ACTOR_POC_SCHEDULE="$schedule"

set +e
script \
  --quiet \
  --return \
  --flush \
  --logging-format=classic \
  --log-out "$typescript_file" \
  --log-timing "$timing_file" \
  --command "$driver"
driver_status=$?
set -e

chmod 600 -- "$typescript_file" "$timing_file" 2>/dev/null || true
if (( driver_status != 0 )); then
  fail "recorded driver exited with status ${driver_status}; private diagnostics were retained"
fi

[[ -s "$typescript_file" && ! -L "$typescript_file" ]] ||
  fail "terminal output recording is missing or empty"
[[ -s "$timing_file" && ! -L "$timing_file" ]] ||
  fail "terminal timing recording is missing or empty"
scriptreplay --summary --log-timing "$timing_file" --log-out "$typescript_file" >/dev/null ||
  fail "terminal recording is not replayable"

[[ -f "$evidence_file" && ! -L "$evidence_file" ]] ||
  fail "live evidence packet is missing"
jq -e \
  --arg kind "$packet_kind" \
  --arg journey "$journey" \
  --arg schedule "$schedule" \
  --arg run_id "$run_id" \
  --arg repository_commit "$repository_commit" '
    .schema_version == 1 and
    .kind == $kind and
    .journey == $journey and
    .schedule == $schedule and
    .result == "passed" and
    .run_id == $run_id and
    .repository_commit == $repository_commit and
    .services.bitcoin_core.version == "31.1" and
    .services.bitcoin_core.network == "regtest" and
    .services.lez.version == "v0.2.0" and
    .services.lez.network == "private_local" and
    .external_resources.public_rpc == false and
    .external_resources.faucet == false and
    .external_resources.public_funds == false and
    .external_resources.certification_success_depends_on_external_network == false
  ' "$evidence_file" >/dev/null || fail "live evidence does not satisfy the selected D1 scenario"

output_sha256="$(sha256sum "$typescript_file" | cut -d ' ' -f 1)"
timing_sha256="$(sha256sum "$timing_file" | cut -d ' ' -f 1)"
evidence_sha256="$(sha256sum "$evidence_file" | cut -d ' ' -f 1)"
script_version="$(script --version | head -n 1)"
scriptreplay_version="$(scriptreplay --version | head -n 1)"
recorded_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
readonly output_sha256 timing_sha256 evidence_sha256
readonly script_version scriptreplay_version recorded_at

evidence_path="$evidence_file"
if [[ "$evidence_file" == "${repository_root}/"* ]]; then
  evidence_path="${evidence_file#"${repository_root}/"}"
fi

manifest_tmp="$(mktemp "${output_dir}/.recording.json.XXXXXX")"
trap 'rm -f -- "${manifest_tmp:-}"' EXIT
jq -n \
  --arg scenario "$scenario" \
  --arg journey "$journey" \
  --arg schedule "$schedule" \
  --arg packet_kind "$packet_kind" \
  --arg run_id "$run_id" \
  --arg certification_mode "$certification_mode" \
  --arg repository_commit "$repository_commit" \
  --arg recorded_at "$recorded_at" \
  --arg output_sha256 "$output_sha256" \
  --arg timing_sha256 "$timing_sha256" \
  --arg evidence_sha256 "$evidence_sha256" \
  --arg evidence_path "$evidence_path" \
  --arg script_version "$script_version" \
  --arg scriptreplay_version "$scriptreplay_version" \
  --argjson networks "$(jq -c '.services' "$evidence_file")" \
  --argjson external_resources "$(jq -c '.external_resources' "$evidence_file")" '
  {
    schema_version: 1,
    kind: "m3_private_terminal_recording",
    scenario: $scenario,
    journey: $journey,
    schedule: $schedule,
    evidence_packet_kind: $packet_kind,
    run_id: $run_id,
    result: "passed",
    certification_mode: $certification_mode,
    privacy: "private_local_stealth",
    repository_commit: $repository_commit,
    recorded_at: $recorded_at,
    networks: $networks,
    external_resources: $external_resources,
    recording: {
      format: "util-linux-script-classic-v1",
      output_file: "terminal.typescript",
      timing_file: "terminal.timing",
      output_sha256: $output_sha256,
      timing_sha256: $timing_sha256,
      tool_versions: {
        script: $script_version,
        scriptreplay: $scriptreplay_version
      }
    },
    replay: {
      cwd: ".",
      argv: [
        "scriptreplay",
        "--log-timing",
        "terminal.timing",
        "--log-out",
        "terminal.typescript"
      ]
    },
    evidence: {
      packet: $evidence_path,
      sha256: $evidence_sha256
    }
  }
' >"$manifest_tmp"
chmod 600 -- "$manifest_tmp"
mv -- "$manifest_tmp" "$manifest_file"
trap - EXIT

echo "M3 private ${scenario} recording passed: ${manifest_file}"
echo "Replay from its directory with: scriptreplay --log-timing terminal.timing --log-out terminal.typescript"
