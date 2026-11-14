#!/bin/bash
# start-basecamp.sh — Basecamp 0.3.2 (APP) on USER_DIR with env-both.sh's
# environment; opening a desk starts its signer in lez_core and its Node
# through lez_swap_node from the installed role package.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
env_lines=()
while IFS= read -r line; do env_lines+=("$line"); done < <("$here/env-both.sh")
exec env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin:/usr/sbin:/sbin "${env_lines[@]}" \
  "${APP:?}/Contents/MacOS/LogosBasecamp" --user-dir "${USER_DIR:?}"
