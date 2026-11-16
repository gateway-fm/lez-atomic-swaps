#!/usr/bin/env bash
# package-role-node-darwin.sh — build a role's Node payload package for
# macOS on Apple silicon (variant darwin-arm64), on that Mac: the role's
# plugin package (modules/lez-<role>-node, built by Nix for darwin-arm64)
# with the Node and everything it runs added to its variant under bin/.
#
#   scripts/package-role-node-darwin.sh <maker|taker> <plugin.lgx> <out.lgx>
#
# The Linux sibling, package-role-node.sh, packages the executables the
# images stage; this one builds them here with cargo from this checkout
# (release, --locked; LEZ_CARGO_JOBS, default 6): the same executables, the
# role Node with pair-zec, its CLI, actors, the health probe and the LEZ v0.2
# sidecar (compat/lez-v0_2-sidecar, toolchain 1.96.0). Each is stripped with
# the macOS strip and signed again ad hoc (stripping invalidates the linker's
# signature, and Apple silicon runs no unsigned code). The launcher is
# modules/lez-role-node/node-entrypoint.sh. bin/payload.json names every
# file with its sha256 and the sources it was built from.
#
# The package is written with the module builder's lgx: LGX names it, or it
# is the lgx that built the plugin (the plugin must then be the builder's
# output in /nix/store, where its derivation records which lgx it used).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
repo="$(cd .. && pwd)"

role="${1:?maker|taker}"
plugin="${2:?the darwin-arm64 plugin package of modules/lez-<role>-node}"
out="${3:?the package to write}"
# The executables of package-role-node.sh (test_role_node_package.py keeps
# the two lists the same).
case "$role" in
  maker) bins=(lez-maker-node lez-maker-cli lez-btc-maker-actor lez-zec-maker-actor) ;;
  taker) bins=(lez-taker-node lez-taker-cli lez-taker-registry-init lez-btc-taker-actor lez-zec-taker-actor) ;;
  *) echo "role must be maker or taker" >&2; exit 64 ;;
esac
bins+=(lez-v02-bridge-poc lez-runtime-healthcheck)
variant=darwin-arm64
launcher="../modules/lez-role-node/node-entrypoint.sh"

[[ "$(uname -s)/$(uname -m)" == Darwin/arm64 ]] || { echo "build on macOS on Apple silicon (this is $(uname -s)/$(uname -m))" >&2; exit 69; }
[[ -f "$plugin" ]] || { echo "no plugin package $plugin" >&2; exit 66; }
manifest="$(tar -xzOf "$plugin" manifest.json)"
main="$(jq -r --arg v "$variant" '.main[$v] // empty' <<<"$manifest")"
[[ -n "$main" ]] || { echo "$plugin has no $variant variant ($(jq -r '.main | keys | join(",")' <<<"$manifest"))" >&2; exit 65; }
[[ "$(jq -r .name <<<"$manifest")" == "lez_${role}_node" ]] || { echo "$plugin is not lez_${role}_node" >&2; exit 65; }

lgx="${LGX:-}"
if [[ -z "$lgx" ]]; then
  store_path="$(realpath "$plugin")"
  store_path="/nix/store/$(cut -d/ -f4 <<<"$store_path")"
  if [[ "$store_path" == /nix/store/?* ]] && command -v nix-store >/dev/null \
      && deriver="$(nix-store -q --deriver "$store_path" 2>/dev/null)" && [[ -e "$deriver" ]]; then
    lgx_drv="$(nix-store -qR "$deriver" | grep -E -- '-lgx-[0-9][^/]*\.drv$' | head -1)"
    [[ -n "$lgx_drv" ]] && lgx="$(nix-store -q --outputs "$lgx_drv" | head -1)/bin/lgx"
  fi
fi
[[ -n "$lgx" && -x "$lgx" ]] || { echo "no lgx: set LGX to the module builder's lgx (the plugin's derivation names none)" >&2; exit 66; }

# Build from this checkout; cargo only relinks what changed.
target="${CARGO_TARGET_DIR:-$repo/target}"
jobs="${LEZ_CARGO_JOBS:-6}"
(cd "$repo" && CARGO_TARGET_DIR="$target" nice -n 10 cargo build --release --locked -j "$jobs" --bins \
  --features lez-maker-node/pair-zec,lez-taker-node/pair-zec \
  -p lez-maker-node -p lez-taker-node -p lez-runtime-healthcheck -p btc-reference-actor -p zec-reference-actor)
(cd "$repo" && CARGO_TARGET_DIR="$target/sidecar" nice -n 10 cargo +1.96.0 build --release --locked -j "$jobs" \
  --manifest-path compat/lez-v0_2-sidecar/Cargo.toml --bin lez-v02-bridge-poc)

source_commit="$(git -C "$repo" rev-parse HEAD)"
git -C "$repo" diff --quiet HEAD || source_commit="$source_commit-dirty"
sidecar_tree="$(git -C "$repo" rev-parse HEAD:compat/lez-v0_2-sidecar)"
git -C "$repo" diff --quiet HEAD -- compat/lez-v0_2-sidecar || \
  sidecar_tree="$sidecar_tree-dirty-$(git -C "$repo" diff HEAD -- compat/lez-v0_2-sidecar | shasum -a 256 | cut -c1-12)"

work="$(mktemp -d "${TMPDIR:-/tmp}/lez-role-pkg.XXXXXX")"
trap 'rm -rf "$work"' EXIT
"$lgx" extract "$plugin" -v "$variant" -o "$work/x" >/dev/null
dir="$work/x/$variant"
mkdir -p "$dir/bin"
for b in "${bins[@]}"; do
  if [[ "$b" == lez-v02-bridge-poc ]]; then src="$target/sidecar/release/$b"; else src="$target/release/$b"; fi
  [[ "$(lipo -archs "$src" 2>/dev/null)" == arm64 ]] || { echo "$src is not an arm64 Mach-O executable" >&2; exit 65; }
  install -m 0755 "$src" "$dir/bin/$b"
  strip "$dir/bin/$b"
  codesign --force --sign - "$dir/bin/$b" 2>/dev/null
  codesign --verify "$dir/bin/$b"
done
install -m 0755 "$launcher" "$dir/bin/node-entrypoint.sh"
(cd "$dir/bin" && shasum -a 256 -- *) | jq -R 'split("  ") | {(.[1]): .[0]}' | jq -s add >"$work/sums.json"
jq -n --arg role "$role" --arg source "$source_commit" --arg sidecar "$source_commit" --arg sidecar_tree "$sidecar_tree" \
  --arg variant "$variant" --slurpfile files "$work/sums.json" \
  '{schema_version:1, role:$role, build_source:$source, sidecar_source:$sidecar, sidecar_tree:$sidecar_tree,
    variant:$variant, files:$files[0]}' >"$dir/bin/payload.json"
chmod 0644 "$dir/bin/payload.json"
cp "$plugin" "$work/pkg.lgx"
chmod 0644 "$work/pkg.lgx"
"$lgx" add "$work/pkg.lgx" -v "$variant" -f "$dir" --main "$main" -y >/dev/null
"$lgx" verify "$work/pkg.lgx" >/dev/null
# The Node runs from the installed files: they must keep their execute bit.
tar -tvzf "$work/pkg.lgx" | awk -v v="variants/$variant/bin/" \
  'index($NF, v) == 1 && $1 ~ /^-/ && $NF !~ /payload\.json$/ && $1 !~ /^-..x/ {bad = 1; print "not executable in the package: " $NF} END {exit bad}'
mkdir -p "$(dirname "$out")"
install -m 0644 "$work/pkg.lgx" "$out"
echo "packaged $(tar -xzOf "$out" manifest.json | jq -r '"\(.name) \(.version) (\(.main | keys | join(",")))"') -> $out ($(du -h "$out" | cut -f1))"
