#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

failed=0
while IFS= read -r reference; do
  if [[ "$reference" == ./* ]]; then
    continue
  fi
  if [[ "$reference" == docker://* ]]; then
    if [[ ! "$reference" =~ @sha256:[0-9a-f]{64}$ ]]; then
      echo "container action is not pinned by SHA-256 digest: ${reference}" >&2
      failed=1
    fi
    continue
  fi
  if [[ ! "$reference" =~ ^[^/@[:space:]]+/[^@[:space:]]+@[0-9a-f]{40}$ ]]; then
    echo "GitHub Action is not pinned by a 40-character commit: ${reference}" >&2
    failed=1
  fi
done < <(
  sed -nE 's/^[[:space:]]*-[[:space:]]*uses:[[:space:]]*([^#[:space:]]+).*$/\1/p' \
    .github/workflows/*.yml .github/workflows/*.yaml 2>/dev/null || true
)

if [[ "$failed" != "0" ]]; then
  exit 1
fi

echo "GitHub Action pin policy passed"
