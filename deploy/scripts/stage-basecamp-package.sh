#!/usr/bin/env bash
# stage-basecamp-package.sh — copy one Nix `*-ui-install` output into the
# basecamp-ui image context and pin its variant tag.
#
#   scripts/stage-basecamp-package.sh <nix-install-output> <maker|taker> [variant]
#
# The module builder tags development builds `linux-arm64-dev`; the bundled
# Basecamp selects the `linux-arm64` variant, so the variant file and the
# manifest's `main` and `hashes` keys are renamed here (values are unchanged:
# the hashes cover the variant's contents, not its name).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

source_root="${1:?nix install output directory}"
role="${2:?maker or taker}"
variant="${3:-linux-arm64}"
[[ "$role" == maker || "$role" == taker ]] || { echo "role must be maker or taker" >&2; exit 64; }
plugin="lez_atomic_swap_${role}"
[[ -d "$source_root/plugins/$plugin" ]] || { echo "$source_root has no plugins/$plugin" >&2; exit 65; }
command -v jq >/dev/null || { echo "jq is required" >&2; exit 66; }

dest="images/basecamp-ui/assets/${role}-user"
rm -rf "$dest"
mkdir -p "$dest"
cp -RL "$source_root"/. "$dest"/
chmod -R u+w "$dest"

manifest="$dest/plugins/$plugin/manifest.json"
variant_file="$dest/plugins/$plugin/variant"
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
echo "staged $plugin ($variant) -> $dest"
