#!/usr/bin/env bash
# package-role-node.sh — build a role's Node payload package: the role's
# plugin package (modules/lez-<role>-node, built by Nix) with the Node and
# everything it runs added to its variant under bin/.
#
#   scripts/package-role-node.sh <maker|taker> <plugin.lgx> <out.lgx>
#
# The executables are the ones staged into the role's image context
# (images/<role>-node/: from-scratch.sh --only rust, and the LEZ v0.2 sidecar
# from --only stage), stripped of their debug sections as the images used to
# do; the launcher is modules/lez-role-node/node-entrypoint.sh. bin/payload.json
# names every file with its sha256 and the sources it was built from. The
# result is written with lgx (the module builder's, from the Nix store in the
# lez-build-nix volume, or LEZ_NIX_VOLUME), in a throwaway container; nothing
# from the store runs on the host.
#
# The package carries one platform variant, this host's (linux-amd64 or
# linux-arm64): the Node does not build off Linux (crates/swap-store).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

role="${1:?maker|taker}"
plugin="${2:?the plugin package of modules/lez-<role>-node}"
out="${3:?the package to write}"
case "$role" in
  maker) bins=(lez-maker-node lez-maker-cli lez-btc-maker-actor lez-zec-maker-actor) ;;
  taker) bins=(lez-taker-node lez-taker-cli lez-taker-registry-init lez-btc-taker-actor lez-zec-taker-actor) ;;
  *) echo "role must be maker or taker" >&2; exit 64 ;;
esac
bins+=(lez-v02-bridge-poc lez-runtime-healthcheck)
context="images/$role-node"
launcher="../modules/lez-role-node/node-entrypoint.sh"
volume="${LEZ_NIX_VOLUME:-lez-build-nix}"
for b in "${bins[@]}"; do
  [[ -x "$context/$b" ]] || { echo "$context/$b is not staged (from-scratch.sh --only rust, --only stage)" >&2; exit 66; }
done
[[ -f "$plugin" ]] || { echo "no plugin package $plugin (from-scratch.sh --only build:role-node-modules)" >&2; exit 66; }
# The sidecar must come from the same compat/lez-v0_2-sidecar tree the Node
# binaries were built beside (from-scratch.sh writes both stamps); a binary
# left over from an older checkout is refused here.
[[ "$(cat "$context/sidecar-tree.txt" 2>/dev/null)" == "$(cat "$context/sidecar-expected.txt" 2>/dev/null)" ]] || {
  echo "the staged sidecar (tree $(cat "$context/sidecar-tree.txt" 2>/dev/null || echo unstamped)) does not match the sidecar sources these Node binaries expect ($(cat "$context/sidecar-expected.txt" 2>/dev/null || echo unstamped)); run from-scratch.sh --only build:sidecar and --only stage" >&2
  exit 1
}

work="$(mktemp -d)"
trap 'docker run --rm -v "$work:/w" debian:bookworm-slim rm -rf /w/x /w/pkg.lgx >/dev/null 2>&1; rm -rf "$work"' EXIT
mkdir -p "$work/in"
for b in "${bins[@]}"; do cp "$context/$b" "$work/in/"; done
install -m 0755 "$launcher" "$work/in/node-entrypoint.sh"
cp "$plugin" "$work/plugin.lgx"
jq -n --arg role "$role" --arg source "$(cat "$context/build-source.txt" 2>/dev/null || echo unknown)" \
  --arg sidecar "$(cat "$context/sidecar-source.txt" 2>/dev/null || echo unknown)" \
  --arg sidecar_tree "$(cat "$context/sidecar-tree.txt")" \
  '{schema_version:1, role:$role, build_source:$source, sidecar_source:$sidecar, sidecar_tree:$sidecar_tree}' >"$work/in/payload.base.json"

# Run in the container (bin/ laid out, stripped, summed, added with lgx).
cat >"$work/pack.sh" <<'PACK'
set -euo pipefail
apt-get -qq update >/dev/null && DEBIAN_FRONTEND=noninteractive apt-get -qq install -y --no-install-recommends binutils jq >/dev/null
lgx="$(find /nix/store -maxdepth 3 -path '/nix/store/*-lgx-[0-9]*/bin/lgx' 2>/dev/null | head -1)"
[[ -n "$lgx" ]] || { echo "no lgx in the Nix store: build a module first (from-scratch.sh --only build:role-node-modules)" >&2; exit 66; }
variant="$(tar -xzOf /w/plugin.lgx manifest.json | jq -r '.main | keys[0]')"
main="$(tar -xzOf /w/plugin.lgx manifest.json | jq -r --arg v "$variant" '.main[$v]')"
"$lgx" extract /w/plugin.lgx -v "$variant" -o /w/x >/dev/null
mkdir -p "/w/x/$variant/bin"
for f in /w/in/*; do
  [[ "$f" == */payload.base.json ]] && continue
  install -m 0755 "$f" "/w/x/$variant/bin/"
  [[ "$f" == */node-entrypoint.sh ]] || strip "/w/x/$variant/bin/${f##*/}"
done
(cd "/w/x/$variant/bin" && sha256sum -- *) | jq -R 'split("  ") | {(.[1]): .[0]}' | jq -s add >/w/sums.json
jq --slurpfile files /w/sums.json --arg variant "$variant" '. + {variant: $variant, files: $files[0]}' \
  /w/in/payload.base.json >"/w/x/$variant/bin/payload.json"
chmod 0644 "/w/x/$variant/bin/payload.json"
cp /w/plugin.lgx /w/pkg.lgx
"$lgx" add /w/pkg.lgx -v "$variant" -f "/w/x/$variant" --main "$main" -y >/dev/null
"$lgx" verify /w/pkg.lgx >/dev/null
# The Node runs from the installed files: they must keep their execute bit.
tar -tvzf /w/pkg.lgx | awk -v v="variants/$variant/bin/" \
  'index($6, v) == 1 && $1 ~ /^-/ && $6 !~ /payload\.json$/ && $1 !~ /^-..x/ {bad = 1; print "not executable in the package: " $6} END {exit bad}'
chown "$(stat -c %u:%g /w)" /w/pkg.lgx
PACK
docker run --rm -v "$volume:/nix:ro" -v "$work:/w" debian:bookworm-slim bash /w/pack.sh
mkdir -p "$(dirname "$out")"
install -m 0644 "$work/pkg.lgx" "$out"
echo "packaged $(tar -xzOf "$out" manifest.json | jq -r '"\(.name) \(.version) (\(.main | keys | join(",")))"') -> $out ($(du -h "$out" | cut -f1))"
