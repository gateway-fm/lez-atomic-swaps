#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly guide="docs/monero-stagenet-setup.md"
readonly provenance="tests/e2e/monero/provenance.env"

fail() {
  echo "Monero Stagenet guide contract failed: $*" >&2
  exit 1
}

[[ -s "$guide" ]] || fail "missing non-empty $guide"

# shellcheck source=/dev/null
source "$provenance"

required_literals=(
  "Monero ${MONERO_VERSION}"
  "$MONERO_TAG"
  "$MONERO_ARCHIVE_SHA256"
  "./scripts/verify-monero-release.sh"
  "--stagenet"
  "--rpc-login"
  "--untrusted-daemon"
  "38081"
  "38088"
  "get_info"
  "get_connections"
  "get_height"
  "get_balance"
  "get_address"
  "transfer"
  "Self-hosted"
  "Public remote node"
  "faucet"
  "No public Stagenet"
  "External resources and flakiness"
  "docs/manual-user-flows.md"
)

for literal in "${required_literals[@]}"; do
  rg -Fq -- "$literal" "$guide" || fail "guide is missing: $literal"
done

if rg -n -i '(^|[^[:alpha:]])(todo|tbd|placeholder|fill me)([^[:alpha:]]|$)' "$guide"; then
  fail "guide contains an unresolved placeholder"
fi

[[ "$(rg -c '^\`\`\`mermaid$' "$guide")" -ge 1 ]] \
  || fail "guide requires a component/authority diagram"
[[ "$(rg -c '^## ' "$guide")" -ge 8 ]] \
  || fail "guide is missing the required operational sections"

rg -Fq "./scripts/test-monero-stagenet-guide-contract.sh" \
  scripts/run-ci-quality-gates.sh \
  || fail "CI quality gates do not run the Stagenet guide contract"

echo "Monero Stagenet guide contract passed without public network calls"
