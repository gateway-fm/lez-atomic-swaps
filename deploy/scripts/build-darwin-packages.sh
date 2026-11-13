#!/usr/bin/env bash
# build-darwin-packages.sh — the darwin-arm64 variant of every package a
# party's desk installs, built on an Apple-silicon Mac from this checkout:
#
#   scripts/build-darwin-packages.sh --out <dir>
#
# writes <dir>/<name>-darwin-arm64.lgx, each carrying the darwin-arm64
# variant only, for
#   lez_atomic_swap_maker, lez_atomic_swap_taker   the desks (lgx-portable)
#   lez_swap_node                                   modules/lez-swap-node
#   lez_maker_node, lez_taker_node                  modules/lez-<role>-node with
#                                                   the Node added by
#                                                   package-role-node-darwin.sh
#   lez_core                                        logos-execution-zone-module
#                                                   on LEZ v0.2.4 with
#                                                   builder/patches (as
#                                                   from-scratch.sh builds it)
# The Linux variants come from release-packages.yml's Linux jobs; that
# workflow merges them with these. lez_network and lez_indexer_module have no
# darwin-arm64 variant: the LEZ network's operator desk is not installed in a
# party's Basecamp, and its node module has not run on macOS.
#
# Needs Nix (flakes), jq, cargo through rustup (rust-toolchain.toml and
# 1.96.0 for the sidecar) and, for lez_core, the Metal compiler risc0 uses:
# Xcode's, or the Metal Toolchain component on Xcode 26
# (`xcodebuild -downloadComponent MetalToolchain`). The submodules
# third_party/logos-execution-zone and third_party/logos-execution-zone-module
# must be checked out at their pins. NIX_BUILD_ARGS adds arguments to every
# `nix build` (for example `--max-jobs 2 --cores 4`). lez_core takes about
# 40 minutes on an empty store.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
deploy="$(pwd)"
repo="$(cd .. && pwd)"

out=""
while (($#)); do
  case "$1" in
    --out) out="${2:?}"; shift 2 ;;
    -h|--help) sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "usage: $0 --out <dir>" >&2; exit 64 ;;
  esac
done
[[ -n "$out" ]] || { echo "usage: $0 --out <dir>" >&2; exit 64; }
[[ "$(uname -s)/$(uname -m)" == Darwin/arm64 ]] || { echo "build on macOS on Apple silicon (this is $(uname -s)/$(uname -m))" >&2; exit 69; }
for tool in nix jq cargo git; do command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 66; }; done
variant=darwin-arm64
mkdir -p "$out"
out="$(cd "$out" && pwd)"

# the same pins as from-scratch.sh's build_logos_module
pin() { sed -n "s/^readonly $1=//p" "$deploy/scripts/from-scratch.sh"; }
lez_commit="$(pin LEZ_MODULES_LEZ_COMMIT)"
wallet_commit="$(pin LEZ_WALLET_MODULE_COMMIT)"
for sub in "third_party/logos-execution-zone $lez_commit" "third_party/logos-execution-zone-module $wallet_commit"; do
  read -r path commit <<<"$sub"
  got="$(git -C "$repo/$path" rev-parse HEAD 2>/dev/null || true)"
  [[ "$got" == "$commit" ]] || { echo "$path is at ${got:-nothing}, expected $commit (git submodule update --init $path)" >&2; exit 65; }
done

work="$(mktemp -d "${TMPDIR:-/tmp}/lez-darwin-pkgs.XXXXXX")"
trap 'rm -rf "$work"' EXIT
read -r -a extra <<<"${NIX_BUILD_ARGS:-}"
nixb() { # nixb <out-link> <flake-ref> [args...]
  local link="$1" ref="$2"; shift 2
  nix --extra-experimental-features "nix-command flakes" build -L --accept-flake-config --no-write-lock-file \
    ${extra[@]+"${extra[@]}"} "$ref" --out-link "$work/$link" "$@"
}
one() { # one <name> <out-link>: the link's package as <name>-darwin-arm64.lgx, checked
  local name="$1" lgx
  lgx="$(find -L "$work/$2" -maxdepth 1 -name '*.lgx' | head -1)"
  [[ -n "$lgx" ]] || { echo "$2 built no .lgx" >&2; exit 65; }
  install -m 0644 "$lgx" "$out/$name-$variant.lgx"
  tar -xzOf "$out/$name-$variant.lgx" manifest.json | jq -e --arg n "$name" --arg v "$variant" \
    '.name == $n and (.main | keys) == [$v]' >/dev/null || { echo "$lgx is not $name for $variant only" >&2; exit 65; }
  echo "$name $variant -> $out/$name-$variant.lgx"
}

# lez_core: the module and LEZ copied without .git, builder/patches applied
# (0001-lez-* to LEZ, the others to the module), the module's flake built
# with the patched LEZ in place of the one it names.
mkdir -p "$work/src/lez" "$work/src/mod"
tar -C "$repo/third_party/logos-execution-zone" --exclude=.git -cf - . | tar -C "$work/src/lez" -xf -
tar -C "$repo/third_party/logos-execution-zone-module" --exclude=.git -cf - . | tar -C "$work/src/mod" -xf -
for p in "$deploy"/builder/patches/logos-execution-zone-module/*.patch; do
  case "$(basename "$p")" in 0001-lez-*) git -C "$work/src/lez" apply "$p" ;; *) git -C "$work/src/mod" apply "$p" ;; esac
done
nixb core "path:$work/src/mod#lgx-portable" --no-update-lock-file --override-input logos-execution-zone "path:$work/src/lez"
one lez_core core

nixb swap-node "path:$repo/modules/lez-swap-node#packages.aarch64-darwin.lgx-portable"
one lez_swap_node swap-node
for role in maker taker; do
  nixb "$role-plugin" "path:$repo/modules/lez-$role-node#packages.aarch64-darwin.lgx-portable"
  "$deploy/scripts/package-role-node-darwin.sh" "$role" "$(find -L "$work/$role-plugin" -maxdepth 1 -name '*.lgx' | head -1)" \
    "$out/lez_${role}_node-$variant.lgx"
  tar -xzOf "$out/lez_${role}_node-$variant.lgx" manifest.json | jq -e --arg v "$variant" '(.main | keys) == [$v]' >/dev/null
  # The desks' flakes take the repository as their source (git+file: the
  # committed tree).
  nixb "desk-$role" "git+file://$repo?dir=apps/basecamp/$role#packages.aarch64-darwin.lgx-portable"
  one "lez_atomic_swap_$role" "desk-$role"
done
(cd "$out" && shasum -a 256 -- *-"$variant".lgx)
