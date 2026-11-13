#!/bin/bash
# session.sh <logosctl args...> — the one logosctl session (a fresh
# --config-dir) that installs both desks from the catalog and hosts both
# roles. `session.sh daemon-start` starts its daemon with env-both.sh's
# environment and a minimal PATH (the launcher runs under /bin/bash 3.2).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
L=$LOGOSCTL/bin/logosctl
if [[ "${1:-}" == daemon-start ]]; then
  mkdir -p "$SESSION" "$LEZ_NODE_HOME/wallet"
  env_lines=()
  while IFS= read -r line; do env_lines+=("$line"); done < <("$here/env-both.sh")
  exec env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin:/usr/sbin:/sbin "${env_lines[@]}" "$L" --config-dir "$SESSION" -D -d
fi
exec "$L" --config-dir "$SESSION" "$@"
