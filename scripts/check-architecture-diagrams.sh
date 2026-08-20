#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

mapfile -t documents < <(
  {
    printf '%s\n' docs/diagrams.md
    find docs/architecture docs/milestone-1 -maxdepth 1 -type f -name '*.md' -print
  } | sort
)

if [[ "${#documents[@]}" -eq 0 ]]; then
  echo "no architecture documents found" >&2
  exit 1
fi

for document in "${documents[@]}"; do
  if ! rg -q '^```mermaid$' "$document"; then
    echo "missing Mermaid diagram: ${document}" >&2
    exit 1
  fi
  if ! rg -q '^(flowchart|graph|sequenceDiagram|stateDiagram|classDiagram|erDiagram|C4Context)' "$document"; then
    echo "missing Mermaid component/flow declaration: ${document}" >&2
    exit 1
  fi
  fence_count="$(rg -c '^```' "$document")"
  if (( fence_count % 2 != 0 )); then
    echo "unbalanced Markdown fences: ${document}" >&2
    exit 1
  fi
done
