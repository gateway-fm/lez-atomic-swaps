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

required_atomic_sequences=(
  "lez-btc/taker-sells-foreign"
  "lez-btc/taker-sells-lez"
  "lez-zec-transparent/taker-sells-foreign"
  "lez-zec-transparent/taker-sells-lez"
  "lez-xmr/taker-sells-lez"
)

for flow in "${required_atomic_sequences[@]}"; do
  marker="<!-- atomic-sequence: ${flow} -->"
  argument_marker="<!-- atomicity-argument: ${flow} -->"
  if [[ "$(rg -Fc "$marker" "$system_architecture")" -ne 1 ]]; then
    echo "system architecture must contain exactly one atomic sequence for ${flow}" >&2
    exit 1
  fi
  if [[ "$(rg -Fc "$argument_marker" "$system_architecture")" -ne 1 ]]; then
    echo "system architecture must contain exactly one atomicity argument for ${flow}" >&2
    exit 1
  fi
  sequence_start="${marker}"$'\n\n```mermaid\nsequenceDiagram'
  if ! rg -UFq "$sequence_start" "$system_architecture"; then
    echo "atomic sequence marker must immediately precede its Mermaid flow: ${flow}" >&2
    exit 1
  fi
  sequence_block="$(awk -v marker="$marker" '
    $0 == marker { found = 1; next }
    found && $0 == "```mermaid" { mermaid = 1; next }
    mermaid && $0 == "```" { exit }
    mermaid { print }
  ' "$system_architecture")"
  for property in \
    "Taker first lock starts the protocol" \
    "Both locks are proven" \
    "Required invariant cutoff plus margin no later than earliest recovery" \
    "Required invariant late maker lock admission closes before refund authority" \
    "Follower retains claim authority and ClaimEvidenceAvailable stays nonterminal" \
    "No canonical reveal" \
    "Implementation status"; do
    if ! rg -Fq "$property" <<<"$sequence_block"; then
      echo "atomic sequence ${flow} is missing scoped property: ${property}" >&2
      exit 1
    fi
  done

  if [[ "$flow" == lez-btc/* ]]; then
    for property in \
      "Taker process exits after reveal submission" \
      "Fresh process observes reveal and commits revision 3" \
      "ClaimEvidenceAvailable is nonterminal" \
      "Observer exits then another maker process reloads revision 3" \
      "Fresh process projects revision 4 Completed" \
      "Later catch up revisions 3 and 4 observation only"; do
      if ! rg -Fq "$property" <<<"$sequence_block"; then
        echo "BTC atomic sequence ${flow} is missing proved survivor step: ${property}" >&2
        exit 1
      fi
    done
  elif ! rg -Fq \
      "Revealer may disappear and follower uses canonical chain disclosure" \
      <<<"$sequence_block"; then
    echo "planned atomic sequence ${flow} is missing its survivor target" >&2
    exit 1
  fi

  argument_block="$(awk -v marker="$argument_marker" '
    $0 == marker { found = 1; next }
    found && ($0 ~ /^<!-- atomicity-argument:/ || $0 ~ /^### /) { exit }
    found { print }
  ' "$system_architecture")"
  for label in "Economic safety" "Replay/idempotency" \
    "Conditional liveness" "Implementation status"; do
    if ! rg -Fq "**${label}:**" <<<"$argument_block"; then
      echo "atomicity argument ${flow} is missing scoped label: ${label}" >&2
      exit 1
    fi
  done
done

required_flow_properties=(
  "Required invariant cutoff plus margin no later than earliest recovery"
  "Required invariant late maker lock admission closes before refund authority"
  "Follower retains claim authority and ClaimEvidenceAvailable stays nonterminal"
)

for property in "${required_flow_properties[@]}"; do
  if [[ "$(rg -Fc "$property" "$system_architecture")" -ne 5 ]]; then
    echo "all five atomic flows must state: ${property}" >&2
    exit 1
  fi
done


if [[ "$(rg -Fc \
    "Revealer may disappear and follower uses canonical chain disclosure" \
    "$system_architecture")" -ne 3 ]]; then
  echo "the two ZEC flows and one XMR flow must retain the planned survivor target" >&2
  exit 1
fi

for property in \
  "Taker process exits after reveal submission" \
  "Fresh process observes reveal and commits revision 3" \
  "ClaimEvidenceAvailable is nonterminal" \
  "Observer exits then another maker process reloads revision 3" \
  "Fresh process projects revision 4 Completed" \
  "Later catch up revisions 3 and 4 observation only"; do
  if [[ "$(rg -Fc "$property" "$system_architecture")" -ne 2 ]]; then
    echo "both BTC atomic flows must state the proved survivor step: ${property}" >&2
    exit 1
  fi
done

if rg -Fq '<!-- atomic-sequence: lez-xmr/taker-sells-foreign -->' \
    "$system_architecture"; then
  echo "unsupported XMR-first direction must not have an atomic sequence" >&2
  exit 1
fi
if ! rg -Fq 'XMR-first is rejected' "$system_architecture"; then
  echo "architecture must retain the unsupported XMR-first rationale" >&2
  exit 1
fi
if rg -Fq 'lifecycle stays Recovering' "$system_architecture"; then
  echo "architecture must use implemented nonterminal phases, not a Recovering enum" >&2
  exit 1
fi

for term in "Aumayr" "Fournier one-time-VES" "aEUF-CMA" \
  "witness-extractability" "pre-signature-adaptability" \
  "BTC pair-specific F7"; do
  if ! rg -Fq "$term" "$system_architecture"; then
    echo "BTC atomicity write-up is missing formal/F7 boundary: ${term}" >&2
    exit 1
  fi
done

if ! rg -Fq "One finalized claim or refund while the opposite funded leg remains" "$system_architecture"; then
  echo "system architecture must define nonterminal half-state handling" >&2
  exit 1
fi

required_atomicity_arguments=(
  "The BTC construction is atomic under these explicit conditions:"
  "The ZEC construction is atomic under these explicit conditions:"
  "The XMR construction"
)

for argument in "${required_atomicity_arguments[@]}"; do
  if ! rg -Fq "$argument" "$system_architecture"; then
    echo "system architecture is missing pair-specific atomicity argument: ${argument}" >&2
    exit 1
  fi
done

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

./scripts/test-mermaid-github-compatibility.sh
./scripts/render-mermaid-diagrams.sh
