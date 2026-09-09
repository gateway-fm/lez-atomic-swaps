#!/usr/bin/env bash
# package-dist.sh — assemble the prebuilt-stack bundle a release attaches:
# the compose file, the runtime scripts (start.sh, up.sh, down.sh, the swap
# and verification helpers), the pinned LEZ config templates, the UI tests
# and release.env naming the published images. Nothing in it needs the
# repository or a build; ./scripts/start.sh pulls the images and runs.
#
#   scripts/package-dist.sh <image-prefix> <image-tag> [out-dir]
#   → <out-dir>/lez-swap-stack-<tag>-arm64.tar.gz (+ .sha256)
#
# Contents come from `git archive HEAD:deploy`, so commit first: untracked
# payloads and runtime state never enter the bundle.
set -euo pipefail

DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd "$DEPLOY_ROOT/.." && pwd)"
prefix="${1:?image prefix, e.g. ghcr.io/gateway-fm/lez-atomic-swaps/lez}"
tag="${2:?image tag, e.g. v0.2.1}"
out="${3:-$REPO_ROOT/dist}"
[[ "$tag" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]*$ ]] || { echo "invalid tag: $tag" >&2; exit 64; }

escrow_program_id="$(sed -n 's/^ *expected_image_id="\([0-9a-f]\{64\}\)".*/\1/p' "$REPO_ROOT/scripts/verify-lez-v02-provisional.sh" | head -1)"
[[ "$escrow_program_id" =~ ^[0-9a-f]{64}$ ]] || { echo "cannot read the escrow program pin" >&2; exit 1; }
commit="$(git -C "$REPO_ROOT" rev-parse HEAD)"

name="lez-swap-stack-${tag}-arm64"
stage="$(mktemp -d "${TMPDIR:-/tmp}/lez-dist.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name" "$out"
git -C "$REPO_ROOT" archive --format=tar HEAD:deploy | tar -x -C "$stage/$name"
# build-only material: the ephemeral builder, the image contexts (payloads are
# inside the published images) and the scripts that need the repository
rm -rf "$stage/$name/builder" "$stage/$name/images" "$stage/$name/tests" \
  "$stage/$name/scripts/from-scratch.sh" "$stage/$name/scripts/stage-assets.sh" \
  "$stage/$name/scripts/stage-basecamp-package.sh" "$stage/$name/scripts/package-dist.sh"
cat >"$stage/$name/release.env" <<ENV
# written by scripts/package-dist.sh; read by scripts/start.sh
LEZ_IMAGE_PREFIX=${prefix}
LEZ_IMAGE_TAG=${tag}
LEZ_ESCROW_PROGRAM_ID=${escrow_program_id}
LEZ_RELEASE_COMMIT=${commit}
ENV
chmod 0644 "$stage/$name/release.env"

archive="$out/$name.tar.gz"
tar -C "$stage" -czf "$archive" "$name"
(cd "$out" && shasum -a 256 "$name.tar.gz" >"$name.tar.gz.sha256")
echo "bundle: $archive"
echo "  $(cat "$out/$name.tar.gz.sha256")"
echo "  images: ${prefix}-{bitcoin-core,btc-miner,btc-explorer,services,explorer,maker-node,taker-node,basecamp-ui,tools}:${tag}"
