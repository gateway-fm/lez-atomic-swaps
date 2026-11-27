#!/bin/bash
# session.sh <logosctl args...> — the ONE logosctl session (Logos host) of the
# native role. `session.sh daemon-start` starts its daemon with env-native.sh's
# environment and PATH=/usr/bin:/bin:/usr/sbin:/sbin (so the launcher runs
# under macOS's /bin/bash 3.2, as a desktop host would start it).
# Needs: LOGOSCTL (the logosctl binary), SESSION (its config dir), STACK,
# LEZ_NODE_HOME, ROLE.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
if [[ "${1:-}" == daemon-start ]]; then
  mkdir -p "$SESSION" "$LEZ_NODE_HOME/wallet/lez"
  chmod 700 "$SESSION" "$LEZ_NODE_HOME/wallet" "$LEZ_NODE_HOME/wallet/lez"
  env_lines=()
  while IFS= read -r line; do env_lines+=("$line"); done < <("$here/env-native.sh" "$STACK" "$LEZ_NODE_HOME" "$ROLE")
  exec env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin:/usr/sbin:/sbin "${env_lines[@]}" \
    "$LOGOSCTL" --config-dir "$SESSION" -D -d
fi
exec "$LOGOSCTL" --config-dir "$SESSION" "$@"
