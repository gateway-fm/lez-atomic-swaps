#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
checker="scripts/check-lez-cost-evidence.sh"
canonical="docs/evidence/lez-v0.1.2-escrow-costs.json"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/lez-cost-policy.XXXXXX")"
trap 'rm -rf "$fixture"' EXIT

cp "$canonical" "$fixture/expected.json"
cp "$canonical" "$fixture/actual.json"

"$checker" "$fixture/expected.json" "$fixture/actual.json"

jq '
  .measured_on = "2099-01-01" |
  .operations[0].sessions[0].user_cycles += 1 |
  .operations[0].sessions[0].reserved_cycles -= 1 |
  .operations[0].recursive_user_cycles += 1
' "$fixture/actual.json" >"$fixture/volatile.json"
"$checker" "$fixture/expected.json" "$fixture/volatile.json"

jq '.operations[0].sessions[0].total_cycles += 1' \
  "$fixture/actual.json" >"$fixture/stable-drift.json"
if "$checker" "$fixture/expected.json" "$fixture/stable-drift.json" >/dev/null 2>&1; then
  echo "stable total-cycle drift was accepted" >&2
  exit 1
fi

jq '.execution.image_id = "wrong"' \
  "$fixture/actual.json" >"$fixture/identity-drift.json"
if "$checker" "$fixture/expected.json" "$fixture/identity-drift.json" >/dev/null 2>&1; then
  echo "artifact identity drift was accepted" >&2
  exit 1
fi

jq '.operations[0].recursive_user_cycles = (.operations[0].recursive_user_cycle_budget + 1)' \
  "$fixture/actual.json" >"$fixture/budget-drift.json"
if "$checker" "$fixture/expected.json" "$fixture/budget-drift.json" >/dev/null 2>&1; then
  echo "recursive user-cycle budget violation was accepted" >&2
  exit 1
fi

printf 'LEZ cost evidence stability policy passed\n'
