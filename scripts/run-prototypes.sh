#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if ! command -v node >/dev/null 2>&1; then
    echo 'node is required to serve the prototypes' >&2
    exit 1
fi

exec node apps/m6-prototypes/server.mjs
