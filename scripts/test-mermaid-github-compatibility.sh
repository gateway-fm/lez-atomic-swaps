#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

checker="scripts/check-mermaid-github-compatibility.sh"
fixture_dir="$(mktemp -d "${TMPDIR:-/tmp}/lez-mermaid-compat-test.XXXXXX")"
trap 'rm -rf -- "$fixture_dir"' EXIT

safe_fixture="$fixture_dir/safe.md"
unsafe_directive_fixture="$fixture_dir/unsafe-directive.md"
unsafe_interaction_fixture="$fixture_dir/unsafe-interaction.md"
unsafe_beta_fixture="$fixture_dir/unsafe-beta.md"
unsafe_sequence_note_semicolon_fixture="$fixture_dir/unsafe-sequence-note-semicolon.md"
unsafe_reserved_actor_fixture="$fixture_dir/unsafe-reserved-actor.md"

printf '%s\n' \
  '# Safe' \
  '' \
  '```mermaid' \
  'flowchart LR' \
  '    User["Maker operator"] -->|"Bearer JSON-RPC"| Daemon["Maker daemon"]' \
  '```' >"$safe_fixture"

printf '%s\n' \
  '# Unsafe directive' \
  '' \
  '```mermaid' \
  '%%{init: {"securityLevel": "loose"}}%%' \
  'flowchart LR' \
  '    A --> B' \
  '```' >"$unsafe_directive_fixture"

printf '%s\n' \
  '# Unsafe interaction' \
  '' \
  '```mermaid' \
  'flowchart LR' \
  '    A --> B' \
  '    click B href "https://example.invalid"' \
  '```' >"$unsafe_interaction_fixture"

printf '%s\n' \
  '# Version-sensitive beta syntax' \
  '' \
  '```mermaid' \
  'architecture-beta' \
  '    service api(server)[API]' \
  '```' >"$unsafe_beta_fixture"

printf '%s\n' \
  '# Unsafe sequence note semicolon' \
  '' \
  '```mermaid' \
  'sequenceDiagram' \
  '    participant A' \
  '    participant B' \
  '    Note over A,B: First clause; second clause' \
  '```' >"$unsafe_sequence_note_semicolon_fixture"

printf '%s\n' \
  '# Unsafe reserved actor identifier' \
  '' \
  '```mermaid' \
  'sequenceDiagram' \
  '    actor Actor as Swap actor' \
  '    participant Node as Chain node' \
  '    Actor->>Node: Observe' \
  '```' >"$unsafe_reserved_actor_fixture"

"$checker" "$safe_fixture"

for unsafe_fixture in \
  "$unsafe_directive_fixture" \
  "$unsafe_interaction_fixture" \
  "$unsafe_beta_fixture" \
  "$unsafe_sequence_note_semicolon_fixture" \
  "$unsafe_reserved_actor_fixture"
do
  if "$checker" "$unsafe_fixture" >/dev/null 2>&1; then
    echo "checker accepted GitHub-unsafe Mermaid: ${unsafe_fixture}" >&2
    exit 1
  fi
done

echo "Mermaid GitHub compatibility contract passed"
