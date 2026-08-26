#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  echo "M7 R6 timelock rationale contract failed: $*" >&2
  exit 1
}

for command_name in jq rg cargo; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

readonly profile="docs/milestone-1/parameter-profiles.md"
readonly decision="docs/architecture/0207-certify-literal-reliability-acceptance.md"

for required in \
  'time variance' \
  'network congestion' \
  'clock drift' \
  '## Recovery horizons' \
  '## Margin budgets' \
  'BTC, taker sells BTC' \
  'BTC, taker sells LEZ' \
  'ZEC, taker sells ZEC' \
  'ZEC, taker sells LEZ' \
  'XMR, taker sells LEZ' \
  'No finite block count is a literal worst-case bound on a proof-of-work chain.' \
  'absent by design until calibration and formal review'; do
  rg -Fq "$required" "$profile" || fail "parameter rationale is missing: ${required}"
done

cargo test --quiet -p lez-swap-core --test typed_refund_schedule
cargo test --quiet -p lez-zec-swap-sdk --test zec_profiles

jq -e '.result == "passed"
  and .deadline.protocol_deadline_changed == false
  and .atomicity.signed_deadline_preserved == true' \
  docs/evidence/m7-actual-zec-first-lock-refund-8981e32-20260812.json >/dev/null ||
  fail "actual ZEC deadline evidence is incomplete"
jq -e '.result == "passed" and .ordering.confirmations_mined_only_after_restart == true' \
  docs/evidence/m7-actual-maker-refund-process-kill-f8bee63-20260808.json >/dev/null ||
  fail "actual XMR recovery-window evidence is incomplete"

rg -Fq 'R6 is GREEN at the documented private-local profile boundary' "$decision" ||
  fail "ADR 0207 does not record the R6 decision"

echo "M7 R6 timelock rationale contract passed"
