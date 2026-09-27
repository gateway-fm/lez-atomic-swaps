#!/usr/bin/env bash
# The desks' views are generated from one skeleton
# (apps/basecamp/scripts/generate-desks.py). A view edited by hand drifts from
# the other role and from the next regeneration, so this refuses a tree whose
# committed views are not what the generator produces.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
for role in maker taker; do mkdir -p "$work/apps/basecamp/$role/src/qml"; done
python3 "$ROOT/apps/basecamp/scripts/generate-desks.py" "$work" >/dev/null
status=0
for role in maker taker; do
  view="apps/basecamp/$role/src/qml/Main.qml"
  if ! diff -u "$ROOT/$view" "$work/$view" >/dev/null; then
    echo "$view differs from generate-desks.py; regenerate it:" >&2
    diff -u "$ROOT/$view" "$work/$view" | head -40 >&2
    status=1
  fi
done
[[ $status == 0 ]] && echo "desk views match generate-desks.py"
exit $status
