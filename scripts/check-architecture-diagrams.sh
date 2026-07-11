#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

system_architecture="docs/architecture/system-architecture.md"

if [[ ! -f "$system_architecture" ]]; then
  echo "missing canonical system architecture: ${system_architecture}" >&2
  exit 1
fi

required_system_terms=(
  'Maker operator'
  'Taker'
  'Logos Core'
  'Delivery / Chat'
  'LEZ sequencer'
  'Bitcoin Core'
  'monerod'
  'Zebra'
)

for term in "${required_system_terms[@]}"; do
  if ! rg -Fq "$term" "$system_architecture"; then
    echo "system architecture is missing actor/component: ${term}" >&2
    exit 1
  fi
done

if [[ "$(rg -c '^sequenceDiagram$' "$system_architecture")" -lt 3 ]]; then
  echo "system architecture must diagram happy, recovery, and restart flows" >&2
  exit 1
fi

if ! rg -q '^flowchart TB$' "$system_architecture"; then
  echo "system architecture must contain a top-to-bottom component diagram" >&2
  exit 1
fi

mapfile -t documents < <(
  find docs/architecture docs/milestone-1 -maxdepth 1 -type f -name '*.md' -print | sort
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
