#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  echo "M7 review-readiness contract failed: $*" >&2
  exit 1
}

readonly template_commit="63ecf397ca5dae4b81de85a578ec839a78fec1c0"
readonly template_sha256="$(sha256sum /tmp/lez-m7-doc-packet-template.yml 2>/dev/null | cut -d ' ' -f 1 || true)"

required_files=(
  docs/milestone-7/README.md
  docs/milestone-7/mainnet-readiness.md
  docs/milestone-7/review-scope.md
  docs/milestone-7/findings-register.md
  docs/milestone-7/doc-packets/btc-sdk.md
  docs/milestone-7/doc-packets/xmr-sdk.md
  docs/milestone-7/doc-packets/zec-sdk.md
  docs/milestone-7/doc-packets/maker-cli.md
  docs/milestone-7/doc-packets/taker-cli.md
)

for path in "${required_files[@]}"; do
  [[ -s "$path" ]] || fail "missing non-empty ${path}"
done

packet_fields=(
  "## What the user achieves"
  "## Why it matters"
  "## Key components"
  "## Repository"
  "## Runtime target"
  "## Prerequisites"
  "## Commands and expected outputs"
  "## Success command"
  "## Expected result"
  "## Configuration details"
  "## Failure modes and limits"
  "## GitHub point of contact"
  "## Discord point of contact"
  "## Existing docs or specs"
  "## Hardware requirements"
  "## Estimated time to complete"
  "## Security notes"
)

for packet in docs/milestone-7/doc-packets/*.md; do
  rg -Fq "logos-docs-template-commit: ${template_commit}" "$packet" \
    || fail "${packet} does not pin the official template"
  for field in "${packet_fields[@]}"; do
    [[ "$(rg -Fc "$field" "$packet")" == 1 ]] \
      || fail "${packet} must contain exactly one ${field}"
  done
  if rg -n -i '(^|[^[:alpha:]])(todo|tbd|placeholder|fill me)([^[:alpha:]]|$)' "$packet"; then
    fail "${packet} contains an unresolved placeholder"
  fi
done

readonly readiness="docs/milestone-7/mainnet-readiness.md"
for section in \
  "## Per-chain protocol designs" \
  "## LEZ escrow design" \
  "## Cross-chain atomicity arguments" \
  "## Timelock handling" \
  "## Security assumptions" \
  "## Known limitations" \
  "## Operations runbook"; do
  [[ "$(rg -Fc "$section" "$readiness")" == 1 ]] \
    || fail "mainnet-readiness write-up is missing ${section}"
done

readonly review_scope="docs/milestone-7/review-scope.md"
for scope in \
  "LEZ escrow Rust and Risc0" \
  "Bitcoin Taproot and pre-signed transactions" \
  "Zcash transparent HTLC" \
  "Monero adaptor and cross-curve DLEQ" \
  "Coordinator state machine" \
  "Daemon authentication and IPC"; do
  rg -Fq "$scope" "$review_scope" || fail "review scope is missing ${scope}"
done

readonly findings="docs/milestone-7/findings-register.md"
for severity in Critical High Medium Low Informational; do
  rg -Fq "| ${severity} |" "$findings" \
    || fail "findings register is missing ${severity} handling"
done

rg -Fq "${template_commit}" docs/milestone-7/README.md \
  || fail "M7 README does not pin the template authority"
rg -Fq "doc-packet.yml" docs/milestone-7/README.md \
  || fail "M7 README does not name the template"

# The temporary authority capture is useful during development but is not a CI
# dependency. When it exists, prove the recorded source was not edited locally.
if [[ -n "$template_sha256" ]]; then
  rg -Fq "$template_sha256" docs/milestone-7/README.md \
    || fail "M7 README does not record the captured template SHA-256"
fi

echo "M7 review-readiness documentation contract passed"
