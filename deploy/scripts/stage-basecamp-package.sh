#!/usr/bin/env bash
# stage-basecamp-package.sh — put one Nix-built Basecamp package into the
# basecamp-ui image context and pin its variant tag.
#
#   scripts/stage-basecamp-package.sh <nix-install-output> <maker|taker> [variant]
#   scripts/stage-basecamp-package.sh <package.lgx> module [variant]
#
# Role packages come from the `*-ui-install` trees and land in the role user
# directory. Runtime modules the role packages depend on (chat_module,
# delivery_module) come from `.lgx` archives and land in the bundle's modules
# directory, next to the modules Basecamp ships with.
#
# The module builder tags development builds `linux-arm64-dev`; the bundled
# Basecamp selects the `linux-arm64` variant, so the variant file and the
# manifest's `main` and `hashes` keys are renamed here (values are unchanged:
# the hashes cover the variant's contents, not its name).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

source="${1:?nix install output directory or .lgx archive}"
kind="${2:?maker, taker or module}"
variant="${3:-linux-arm64}"
command -v jq >/dev/null || { echo "jq is required" >&2; exit 66; }

case "$kind" in
  maker|taker)
    plugin="lez_atomic_swap_${kind}"
    [[ -d "$source/plugins/$plugin" ]] || { echo "$source has no plugins/$plugin" >&2; exit 65; }
    dest="images/basecamp-ui/assets/${kind}-user"
    rm -rf "$dest"
    mkdir -p "$dest"
    cp -RL "$source"/. "$dest"/
    package="$dest/plugins/$plugin"
    ;;
  module)
    unpacked="$(mktemp -d)"
    trap 'rm -rf "$unpacked"' EXIT
    tar -xzf "$source" -C "$unpacked"
    name="$(jq -r .name "$unpacked/manifest.json")"
    built="$(jq -r '.main | keys[0]' "$unpacked/manifest.json")"
    package="images/basecamp-ui/assets/bundle/modules/$name"
    dest="$package"
    rm -rf "$package"
    mkdir -p "$package"
    cp -RL "$unpacked/variants/$built"/. "$package"/
    cp "$unpacked/manifest.json" "$package/"
    printf '%s\n' "$built" > "$package/variant"
    ;;
  *) echo "kind must be maker, taker or module" >&2; exit 64 ;;
esac
chmod -R u+w "$dest"

manifest="$package/manifest.json"
variant_file="$package/variant"
current="$(tr -d '[:space:]' < "$variant_file")"
if [[ "$current" != "$variant" ]]; then
  printf '%s\n' "$variant" > "$variant_file"
  jq --arg from "$current" --arg to "$variant" '
    .main = (.main | with_entries(if .key == $from then .key = $to else . end))
    | .hashes = (.hashes | with_entries(if .key == ("variants/" + $from) then .key = ("variants/" + $to) else . end))
  ' "$manifest" > "$manifest.tmp"
  mv "$manifest.tmp" "$manifest"
fi
jq -e --arg v "$variant" '.main[$v] != null and .hashes["variants/" + $v] != null' "$manifest" >/dev/null
echo "staged $(jq -r .name "$manifest") ($variant) -> $dest"
