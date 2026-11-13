#!/bin/bash
# string-then-object.sh <user dir> — the candidate manifest form, applied to
# an installed profile: before each pinned dependency object its bare name
# (["lez_core", {"name": "lez_core", "version": "0.4.1-sign.4"}, ...]).
# Basecamp 0.2.x reads the strings; logosctl and Basecamp 0.3.x the objects.
set -euo pipefail
for m in "${1:?user dir}"/modules/*/manifest.json "$1"/plugins/*/manifest.json; do
  jq 'if (.dependencies // []) | any(type == "object") then
        .dependencies |= [.[] | if type == "object" then .name, . else . end]
      else . end' "$m" >"$m.tmp"
  mv "$m.tmp" "$m"
done
