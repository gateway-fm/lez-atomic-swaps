#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  echo "M7 R7 Bitcoin refund justification contract failed: $*" >&2
  exit 1
}

for command_name in jq rg; do
  command -v "$command_name" >/dev/null || fail "missing test dependency: ${command_name}"
done

readonly refund_decision="docs/architecture/0009-bitcoin-refund-path.md"
readonly reliability_decision="docs/architecture/0207-certify-literal-reliability-acceptance.md"
readonly actual_refunds="docs/evidence/m3-local-two-direction-refund-poc-20260716.json"

for required in \
  '# ADR 0009: Bitcoin uses a Taproot script-path CSV refund' \
  'The cooperative path is a key-path spend.' \
  'The refund is a script-path spend' \
  'consensus protects the refund condition' \
  'does not depend on preserving one pre-signed transaction' \
  'loss/corruption' \
  'malformed timelock' \
  'setup-time fee' \
  'uneconomic or' \
  'safety enforced by' \
  'protocol ordering rather than the output' \
  'itself.' \
  'visible refund branch and timing leakage'; do
  rg -Fq "$required" "$refund_decision" || fail "refund justification is missing: ${required}"
done

jq -e '
  .result == "passed"
  and (.directions | length) == 2
  and all(.directions[];
    .journey == "two_lock_timeout_refund"
    and .terminal.maker.phase == "refunded"
    and .terminal.taker.phase == "refunded"
    and .effects.cooperative_claim_effects_present == false
    and .refunds.bitcoin.spent_exact_funding_output == true
    and .refunds.lez.deadline_satisfied == true
    and .refunds.lez.metadata_status == "refunded"
    and .refunds.lez.custody_balance == "0")
  and .terminal_replay.resubmission_count == 0
' "$actual_refunds" >/dev/null || fail "two-direction actual refund evidence is incomplete"

rg -Fq 'R7 is GREEN for the selected Taproot script-path construction' \
  "$reliability_decision" || fail "ADR 0207 does not record the R7 decision"

echo "M7 R7 Bitcoin refund justification contract passed"
