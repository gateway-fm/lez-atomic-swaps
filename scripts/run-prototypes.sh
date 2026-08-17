#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

node_bin=""
if command -v node >/dev/null 2>&1; then
    node_bin="$(command -v node)"
fi

node_major() {
    "$1" --version 2>/dev/null | sed 's/^v//' | cut -d. -f1
}

if [[ -z "$node_bin" || "$(node_major "$node_bin")" -lt 14 ]]; then
    for candidate in \
        "${NVM_DIR:-$HOME/.nvm}/versions/node/v22.20.0/bin/node" \
        "${NVM_DIR:-$HOME/.nvm}/versions/node/v20.20.0/bin/node" \
        "${NVM_DIR:-$HOME/.nvm}/versions/node/v18.7.0/bin/node" \
        /opt/homebrew/bin/node /usr/local/bin/node; do
        if [[ -x "$candidate" && "$(node_major "$candidate")" -ge 14 ]]; then
            node_bin="$candidate"
            break
        fi
    done
fi

if [[ -z "$node_bin" || "$(node_major "$node_bin")" -lt 14 ]]; then
    echo 'node >= 14 is required to serve the prototypes' >&2
    exit 1
fi

exec "$node_bin" apps/m6-prototypes/server.mjs
