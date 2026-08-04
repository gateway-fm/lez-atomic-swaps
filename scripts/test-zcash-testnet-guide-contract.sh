#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly guide="docs/zcash-testnet-setup.md"

fail() {
  echo "Zcash Testnet guide contract failed: $*" >&2
  exit 1
}

[[ -s "$guide" ]] || fail "missing non-empty $guide"

required_literals=(
  "Zebra 6.0.0"
  "v6.0.0"
  "Self-hosted"
  "Public Zebrad provider route"
  "Tatum"
  "transparent"
  "project-owned signer"
  "build_funding_transaction"
  "build_claim_transaction"
  "build_refund_transaction"
  "getblockchaininfo"
  "gettxout"
  "sendrawtransaction"
  "faucet"
  "External-resource and flakiness summary"
  "No public"
  "manual-user-flows.md"
)

for literal in "${required_literals[@]}"; do
  rg -Fq -- "$literal" "$guide" || fail "guide is missing: $literal"
done

for stale in \
  "project-owned transparent key custody and signing. Zallet is optional funding infrastructure, not the swap signer. Never import production seeds to bridge this gap." \
  "The intended swap address is produced by project-owned testnet key custody, which is not implemented yet." \
  "This section remains blocked until project-owned transparent signing"; do
  if rg -Fq -- "$stale" "$guide"; then
    fail "guide retains stale signer claim: $stale"
  fi
done

if rg -n -i '(^|[^[:alpha:]])(todo|tbd|placeholder|fill me)([^[:alpha:]]|$)' "$guide"; then
  fail "guide contains an unresolved placeholder"
fi

[[ "$(rg -c '^\`\`\`mermaid$' "$guide")" -ge 1 ]] \
  || fail "guide requires a component/authority diagram"
rg -Fq "./scripts/test-zcash-testnet-guide-contract.sh" \
  scripts/run-ci-quality-gates.sh \
  || fail "CI quality gates do not run the Zcash guide contract"

echo "Zcash Testnet guide contract passed without public network calls"
