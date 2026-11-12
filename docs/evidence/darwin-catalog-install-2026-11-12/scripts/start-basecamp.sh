#!/bin/bash
# start-basecamp.sh — Basecamp 0.2.0-RC3 with the QML inspector (built from
# the basecamp flake on this Mac) with both desks in one process, on a user
# directory that is a byte-identical copy of the catalog session's modules/
# and plugins/ (same layout: <dir>/{modules,plugins}/<name>/ with the
# variant's files, manifest.json and `variant`), and env-both.sh's
# environment: opening a desk starts its signer in lez_core and its Node
# through lez_swap_node from the installed role package.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
env_lines=()
while IFS= read -r line; do env_lines+=("$line"); done < <("$here/env-both.sh")
exec env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin:/usr/sbin:/sbin "${env_lines[@]}" \
  "$BASECAMP/bin/LogosBasecamp" --user-dir "$USER_DIR"
