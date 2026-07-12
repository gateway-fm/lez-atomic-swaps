#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

mmdc="${MERMAID_CLI:-node_modules/.bin/mmdc}"
if [[ ! -x "$mmdc" ]]; then
  echo "missing Mermaid CLI; run npm ci" >&2
  exit 1
fi

puppeteer_arguments=()
if [[ "${MERMAID_ALLOW_NO_SANDBOX:-0}" == "1" ]]; then
  puppeteer_arguments=(
    --puppeteerConfigFile
    scripts/mermaid-puppeteer-no-sandbox.json
  )
fi

render_dir="$(mktemp -d "${TMPDIR:-/tmp}/lez-mermaid-render.XXXXXX")"
trap 'rm -rf -- "$render_dir"' EXIT

mapfile -t documents < <(
  find docs/architecture docs/milestone-1 -maxdepth 1 -type f -name '*.md' -print | sort
)

block_count=0
for document in "${documents[@]}"; do
  document_key="${document//\//_}"
  awk -v output_dir="$render_dir" -v document_key="$document_key" '
    /^```mermaid$/ {
      in_mermaid = 1
      block += 1
      output = sprintf("%s/%s-%03d.mmd", output_dir, document_key, block)
      next
    }
    in_mermaid && /^```$/ {
      close(output)
      in_mermaid = 0
      next
    }
    in_mermaid { print > output }
  ' "$document"
done

shopt -s nullglob
diagrams=("$render_dir"/*.mmd)
if [[ "${#diagrams[@]}" -eq 0 ]]; then
  echo "no Mermaid blocks found to render" >&2
  exit 1
fi

for diagram in "${diagrams[@]}"; do
  echo "rendering ${diagram##*/}"
  "$mmdc" \
    "${puppeteer_arguments[@]}" \
    --input "$diagram" \
    --output "${diagram%.mmd}.svg" \
    --backgroundColor transparent \
    --quiet
  block_count=$((block_count + 1))
done

echo "rendered ${block_count} Mermaid diagrams"
