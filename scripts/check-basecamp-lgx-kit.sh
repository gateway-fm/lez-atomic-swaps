#!/usr/bin/env bash
# check-basecamp-lgx-kit.sh <package.lgx>...
#
# A packaged Basecamp desk must carry the shared UI kit next to its Main.qml
# in every variant it ships; a package without it compiles nowhere (#50). It
# must be a release package: a development variant (`<platform>-dev`, the
# module builder's `lgx` output) is refused by logosctl.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

[[ $# -gt 0 ]] || { echo "usage: $0 <package.lgx>..." >&2; exit 64; }
kit=()
for file in apps/basecamp/common/qml/*.qml; do kit+=("$(basename "$file")"); done

status=0
for archive in "$@"; do
  entries="$(tar -tzf "$archive")"
  # Plain bash: the pinned Nix CI image has no sed or awk.
  variants=""
  while IFS= read -r entry; do
    [[ "$entry" =~ ^variants/([^/]+)/qml/Main\.qml$ ]] && variants+="${BASH_REMATCH[1]}"$'\n'
  done <<< "$entries"
  variants="${variants%$'\n'}"
  if [[ -z "$variants" ]]; then
    echo "$archive: no variant carries qml/Main.qml" >&2
    status=1
    continue
  fi
  while IFS= read -r variant; do
    if [[ "$variant" == *-dev ]]; then
      echo "$archive: variant $variant is a development build (build lgx-portable)" >&2
      status=1
      continue
    fi
    missing=()
    for name in "${kit[@]}"; do
      printf '%s\n' "$entries" | grep -qx "variants/$variant/qml/$name" || missing+=("$name")
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
      echo "$archive: variant $variant lacks the shared UI kit: ${missing[*]}" >&2
      status=1
    else
      echo "$archive: variant $variant carries the shared UI kit (${#kit[@]} files)"
    fi
  done <<< "$variants"
done
exit "$status"
