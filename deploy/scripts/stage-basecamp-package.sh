#!/usr/bin/env bash
# stage-basecamp-package.sh — install one Logos package into the basecamp-ui
# image context, laid out as Basecamp's package manager installs it.
#
#   scripts/stage-basecamp-package.sh <package.lgx> <maker|taker|network> [variant]
#   scripts/stage-basecamp-package.sh <package.lgx> module [variant]
#
# A desk (maker, taker: the role desks; network: the LEZ Network desk) lands in
# its user directory's plugins/<name>/, a runtime module (lez_core,
# lez_swap_node, chat_module, ...) in the bundle's modules/<name>/: the
# variant's files, the package's manifest.json and a `variant` file naming the
# variant, which is what lgpm and logosctl write. The package is staged as it
# is: these are the release packages (pin-package-dependencies.sh), the same
# files a user installs with logosctl.
#
# The variant must match the host the bundle will run on. Basecamp refuses a
# foreign one ("installed for variant 'linux-arm64' which is not supported on
# this platform") and then starts without the role modules, so the desks never
# appear and every UI suite fails on a launcher it cannot find. A package that
# carries only the module builder's development variant (`<platform>-dev`) is
# refused: build its lgx-portable output.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

source="${1:?package .lgx}"
kind="${2:?maker, taker, network or module}"
case "$(uname -m)" in
  x86_64 | amd64) host_variant=linux-amd64 ;;
  *) host_variant=linux-arm64 ;;
esac
variant="${3:-$host_variant}"
command -v jq >/dev/null || { echo "jq is required" >&2; exit 66; }
[[ -f "$source" ]] || { echo "$source is not a package file (.lgx)" >&2; exit 65; }

unpacked="$(mktemp -d)"
trap 'rm -rf "$unpacked"' EXIT
tar -xzf "$source" -C "$unpacked"
manifest="$unpacked/manifest.json"
name="$(jq -r .name "$manifest")"
if ! jq -e --arg v "$variant" '.main[$v] != null and .hashes["variants/" + $v] != null' "$manifest" >/dev/null; then
  echo "$source ($name) has no $variant variant (it provides: $(jq -r '.main | keys | join(", ")' "$manifest"))" >&2
  if jq -e --arg v "$variant" '.main | has($v + "-dev")' "$manifest" >/dev/null; then
    echo "  that is the module builder's development package; build its lgx-portable output" >&2
  fi
  exit 65
fi

case "$kind" in
  maker|taker|network)
    plugin="lez_atomic_swap_${kind}"
    [[ "$kind" == network ]] && plugin=lez_network
    [[ "$name" == "$plugin" ]] || { echo "$source is $name, not the $kind desk ($plugin)" >&2; exit 65; }
    [[ "$(jq -r .type "$manifest")" == ui_qml ]] || { echo "$source ($name) is not a ui_qml package" >&2; exit 65; }
    user="images/basecamp-ui/assets/${kind}-user"
    rm -rf "$user"
    mkdir -p "$user/modules" "$user/plugins"
    dest="$user/plugins/$name"
    ;;
  module)
    dest="images/basecamp-ui/assets/bundle/modules/$name"
    ;;
  *) echo "kind must be maker, taker, network or module" >&2; exit 64 ;;
esac
rm -rf "$dest"
mkdir -p "$dest"
cp -R "$unpacked/variants/$variant"/. "$dest"/
cp "$manifest" "$dest/"
printf '%s\n' "$variant" > "$dest/variant"
chmod -R u+w "$dest"
echo "staged $name $(jq -r .version "$manifest") ($variant) -> $dest"
