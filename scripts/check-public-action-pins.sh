#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
failed=0

while IFS= read -r reference; do
  [[ "$reference" == ./* ]] && continue
  if [[ "$reference" == docker://* ]]; then
    [[ "$reference" =~ @sha256:[0-9a-f]{64}$ ]] || {
      echo "container action is not digest-pinned: ${reference}" >&2
      failed=1
    }
    continue
  fi
  [[ "$reference" =~ ^[^/@[:space:]]+/[^@[:space:]]+@[0-9a-f]{40}$ ]] || {
    echo "action is not pinned to a 40-character commit: ${reference}" >&2
    failed=1
  }
done < <(
  sed -nE 's/^[[:space:]]*(-[[:space:]]*)?uses:[[:space:]]*([^#[:space:]]+).*$/\2/p' \
    "$repo_root"/.github/workflows/*.yml \
    "$repo_root"/.github/workflows/*.yaml 2>/dev/null || true
)

[[ "$failed" == 0 ]] || exit 1
echo "GitHub Action pin policy passed"
