#!/bin/bash
# session-both.sh <logosctl args...> — ONE logosctl session hosting both roles.
# `session-both.sh daemon-start` starts its daemon with env-both.sh's environment.
set -euo pipefail
L=$LOGOSCTL/bin/logosctl
S=$SESSIONS/both
if [[ "${1:-}" == daemon-start ]]; then
  mkdir -p "$S" $LEZ_NODE_HOME/wallet
  env_lines=()
  while IFS= read -r line; do env_lines+=("$line"); done < <($LOGS/env-both.sh)
  exec env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin:/usr/sbin:/sbin "${env_lines[@]}" "$L" --config-dir "$S" -D -d
fi
exec "$L" --config-dir "$S" "$@"
