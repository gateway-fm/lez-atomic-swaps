#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if (( $# > 0 )); then
  documents=("$@")
else
  mapfile -t documents < <(
    git ls-files --cached --others --exclude-standard -- '*.md'
  )
fi

if (( ${#documents[@]} == 0 )); then
  echo "no Markdown documents found" >&2
  exit 1
fi

status=0
diagram_count=0
for document in "${documents[@]}"; do
  if [[ ! -f "$document" ]]; then
    echo "missing Markdown document: ${document}" >&2
    status=1
    continue
  fi

  if ! count="$(awk '
    function reject(reason) {
      printf "%s:%d: GitHub-incompatible Mermaid: %s\n", FILENAME, FNR, reason > "/dev/stderr"
      failed = 1
    }

    /^```mermaid$/ {
      if (in_mermaid) {
        reject("nested Mermaid fence")
      }
      in_mermaid = 1
      declaration_seen = 0
      diagram_kind = ""
      blocks += 1
      next
    }

    in_mermaid && /^```$/ {
      if (!declaration_seen) {
        reject("missing supported diagram declaration")
      }
      in_mermaid = 0
      next
    }

    in_mermaid {
      if ($0 ~ /%%\{/) {
        reject("configuration directives depend on the host Mermaid policy")
      }
      if ($0 ~ /@\{/) {
        reject("new shape syntax is version-sensitive on GitHub")
      }
      if ($0 ~ /^[[:space:]]*(click|href|callback|call)[[:space:]]/) {
        reject("interactive links or callbacks are disabled by GitHub")
      }
      if (diagram_kind == "sequenceDiagram" &&
          $0 ~ /^[[:space:]]*Note[[:space:]]+(over|right of|left of)[^:]*:.*;/) {
        reject("semicolons terminate Mermaid sequence-note statements")
      }

      if (!declaration_seen && $0 !~ /^[[:space:]]*$/ && $0 !~ /^[[:space:]]*%%/) {
        if ($0 !~ /^(flowchart|graph)[[:space:]]+(TB|TD|BT|RL|LR)[[:space:]]*$/ &&
            $0 !~ /^(sequenceDiagram|stateDiagram-v2|classDiagram|erDiagram)[[:space:]]*$/) {
          reject("unsupported or version-sensitive diagram declaration")
        }
        diagram_kind = $1
        declaration_seen = 1
      }
      next
    }

    END {
      if (in_mermaid) {
        reject("unclosed Mermaid fence")
      }
      print blocks + 0
      exit failed
    }
  ' "$document")"; then
    status=1
    continue
  fi
  diagram_count=$((diagram_count + count))
done

if (( status != 0 )); then
  exit "$status"
fi

if (( diagram_count == 0 )); then
  echo "no Mermaid diagrams found" >&2
  exit 1
fi

echo "checked ${diagram_count} Mermaid diagrams for conservative GitHub compatibility"
