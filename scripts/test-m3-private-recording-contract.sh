#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly recorder="scripts/record-m3-private-demo.sh"
fixture="$(pwd)/scripts/fixtures/m3-recording-test-driver.sh"
readonly fixture

fail() {
  echo "M3 private-recording contract failed: $*" >&2
  exit 1
}

[[ -x "$recorder" ]] || fail "recorder is missing or not executable"
[[ -x "$fixture" ]] || fail "test driver is missing or not executable"
bash -n "$recorder"
bash -n "$fixture"

test_root="$(mktemp -d /tmp/m3-private-recording-contract.XXXXXX)"
cleanup() {
  rm -rf -- "$test_root"
}
trap cleanup EXIT

expected_commit="$(git rev-parse HEAD)"
readonly expected_commit

run_fixture() {
  local scenario="$1" run_id="$2"
  local evidence_file="${test_root}/${run_id}-evidence.json"
  env \
    RUN_ID="$run_id" \
    M3_RECORDING_SCENARIO="$scenario" \
    M3_RECORDING_PRIVATE_ROOT="${test_root}/private/${run_id}" \
    M3_RECORDING_TESTING=1 \
    M3_RECORDING_TEST_DRIVER="$fixture" \
    M3_RECORDING_TEST_EVIDENCE_FILE="$evidence_file" \
    M3_RECORDING_TEST_COMMIT="$expected_commit" \
    "$recorder"
}

for scenario in happy refund concurrent; do
  run_id="m3-recording-${scenario}-contract"
  run_fixture "$scenario" "$run_id"
  output_dir="${test_root}/private/${run_id}/${scenario}"
  manifest="${output_dir}/recording.json"
  typescript="${output_dir}/terminal.typescript"
  timing="${output_dir}/terminal.timing"

  [[ -f "$manifest" && ! -L "$manifest" ]] || fail "${scenario} manifest is missing"
  [[ -s "$typescript" && ! -L "$typescript" ]] || fail "${scenario} output recording is missing"
  [[ -s "$timing" && ! -L "$timing" ]] || fail "${scenario} timing recording is missing"
  [[ "$(stat -c '%a' "$output_dir")" == 700 ]] || fail "${scenario} directory is not private"
  [[ "$(stat -c '%a' "$manifest")" == 600 ]] || fail "${scenario} manifest is not private"
  [[ "$(stat -c '%a' "$typescript")" == 600 ]] || fail "${scenario} terminal output is not private"
  [[ "$(stat -c '%a' "$timing")" == 600 ]] || fail "${scenario} timing is not private"

  jq -e \
    --arg scenario "$scenario" \
    --arg run_id "$run_id" \
    --arg commit "$expected_commit" '
      .schema_version == 1 and
      .kind == "m3_private_terminal_recording" and
      .scenario == $scenario and
      .run_id == $run_id and
      .result == "passed" and
      .certification_mode == "test_contract" and
      .privacy == "private_local_stealth" and
      .repository_commit == $commit and
      .networks.bitcoin_core == {version:"31.1",network:"regtest"} and
      .networks.lez == {version:"v0.2.0",network:"private_local"} and
      .external_resources.public_rpc == false and
      .external_resources.faucet == false and
      .external_resources.public_funds == false and
      .external_resources.certification_success_depends_on_external_network == false and
      .recording.format == "util-linux-script-classic-v1" and
      .recording.output_file == "terminal.typescript" and
      .recording.timing_file == "terminal.timing" and
      (.recording.output_sha256 | test("^[0-9a-f]{64}$")) and
      (.recording.timing_sha256 | test("^[0-9a-f]{64}$")) and
      .replay.argv == ["scriptreplay","--log-timing","terminal.timing","--log-out","terminal.typescript"]
    ' "$manifest" >/dev/null || fail "${scenario} manifest contract drifted"

  [[ "$(sha256sum "$typescript" | cut -d ' ' -f 1)" == \
    "$(jq -r '.recording.output_sha256' "$manifest")" ]] ||
    fail "${scenario} output hash drifted"
  [[ "$(sha256sum "$timing" | cut -d ' ' -f 1)" == \
    "$(jq -r '.recording.timing_sha256' "$manifest")" ]] ||
    fail "${scenario} timing hash drifted"
  scriptreplay --summary --log-timing "$timing" --log-out "$typescript" >/dev/null ||
    fail "${scenario} recording is not replayable"

  case "$scenario" in
    happy)
      jq -e '.journey == "claim" and .schedule == "sequential" and
        .evidence_packet_kind == "m3_actor_two_direction_local_poc"' \
        "$manifest" >/dev/null || fail "happy scenario binding drifted"
      ;;
    refund)
      jq -e '.journey == "refund" and .schedule == "sequential" and
        .evidence_packet_kind == "m3_actor_two_direction_refund_local_poc"' \
        "$manifest" >/dev/null || fail "refund scenario binding drifted"
      ;;
    concurrent)
      jq -e '.journey == "claim" and .schedule == "overlap" and
        .evidence_packet_kind == "m3_actor_overlapping_two_swap_local_poc"' \
        "$manifest" >/dev/null || fail "concurrent scenario binding drifted"
      ;;
  esac

  if run_fixture "$scenario" "$run_id" >/dev/null 2>&1; then
    fail "${scenario} recording allowed output overwrite"
  fi
done

if env \
    RUN_ID=m3-recording-invalid-contract \
    M3_RECORDING_SCENARIO=survivor \
    M3_RECORDING_PRIVATE_ROOT="${test_root}/private/m3-recording-invalid-contract" \
    M3_RECORDING_TESTING=1 \
    M3_RECORDING_TEST_DRIVER="$fixture" \
    M3_RECORDING_TEST_EVIDENCE_FILE="${test_root}/invalid-evidence.json" \
    M3_RECORDING_TEST_COMMIT="$expected_commit" \
    "$recorder" >/dev/null 2>&1; then
  fail "recorder accepted a non-D1 scenario"
fi
[[ ! -e "${test_root}/private/m3-recording-invalid-contract" ]] ||
  fail "invalid scenario created output"

failure_dir="${test_root}/private/m3-recording-failure-contract/happy"
if env \
    RUN_ID=m3-recording-failure-contract \
    M3_RECORDING_SCENARIO=happy \
    M3_RECORDING_PRIVATE_ROOT="${test_root}/private/m3-recording-failure-contract" \
    M3_RECORDING_TESTING=1 \
    M3_RECORDING_TEST_DRIVER="$fixture" \
    M3_RECORDING_TEST_EVIDENCE_FILE="${test_root}/failure-evidence.json" \
    M3_RECORDING_TEST_COMMIT="$expected_commit" \
    M3_RECORDING_TEST_FAIL=1 \
    "$recorder" >/dev/null 2>&1; then
  fail "recorder hid a driver failure"
fi
[[ ! -e "${failure_dir}/recording.json" ]] ||
  fail "failed driver produced a passing manifest"
[[ -s "${failure_dir}/terminal.typescript" && -s "${failure_dir}/terminal.timing" ]] ||
  fail "failed driver did not preserve private diagnostic recording"

if env \
    RUN_ID=m3-recording-override-contract \
    M3_RECORDING_SCENARIO=happy \
    M3_RECORDING_PRIVATE_ROOT="${test_root}/private/m3-recording-override-contract" \
    M3_RECORDING_TEST_DRIVER="$fixture" \
    M3_RECORDING_TEST_EVIDENCE_FILE="${test_root}/override-evidence.json" \
    M3_RECORDING_TEST_COMMIT="$expected_commit" \
    "$recorder" >/dev/null 2>&1; then
  fail "production mode accepted the test-driver override"
fi

echo "M3 private terminal-recording contract passed"
