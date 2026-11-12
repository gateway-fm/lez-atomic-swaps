#!/bin/bash
# start-basecamp-both.sh — the native Basecamp (0.2.0-RC3 with the QML
# inspector, built from the basecamp flake) with both desks in one process and
# the environment of env-both.sh: opening a desk starts its role's signer in
# lez_core and its Node through lez_swap_node, as in a role's container.
set -euo pipefail
B=$BASECAMP
env_lines=()
while IFS= read -r line; do env_lines+=("$line"); done < <($LOGS/env-both.sh)
exec env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin:/usr/sbin:/sbin "${env_lines[@]}" \
  "$B/bundle/bin/LogosBasecamp" --user-dir "$B/user"
