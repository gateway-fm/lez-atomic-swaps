#!/bin/bash
# start-basecamp-release.sh — a release Basecamp (APP: the .app bundle;
# 0.2.3 in /Applications, or 0.3.2 from its release dmg), Developer ID
# signed and notarised, with env-both.sh's environment, on its default
# profile or on USER_DIR when set: opening a desk starts its signer in
# lez_core and its Node through lez_swap_node from the installed role package.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
env_lines=()
while IFS= read -r line; do env_lines+=("$line"); done < <("$here/env-both.sh")
args=()
[[ -n "${USER_DIR:-}" ]] && args=(--user-dir "$USER_DIR")
exec env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin:/usr/sbin:/sbin "${env_lines[@]}" \
  "${APP:?the Basecamp .app}/Contents/MacOS/LogosBasecamp" ${args[@]+"${args[@]}"}
