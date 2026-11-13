#!/bin/bash
# bare-dependency-names.sh <user dir> — what the role images and
# stage-basecamp-package.sh do for Basecamp 0.2.0, which reads only string
# dependency entries: every installed manifest's dependencies as bare names.
# Nothing else in the user directory changes.
set -euo pipefail
for m in "${1:?user dir}"/modules/*/manifest.json "$1"/plugins/*/manifest.json; do
  jq 'if .dependencies then .dependencies |= map(if type == "object" then .name else . end) else . end' "$m" >"$m.tmp"
  mv "$m.tmp" "$m"
done
