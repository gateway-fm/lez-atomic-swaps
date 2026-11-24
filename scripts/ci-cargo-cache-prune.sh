#!/usr/bin/env bash
set -euo pipefail

# Prunes a Cargo target directory that a CI cache restores or saves, so that
# the cache holds only third-party build output.
#
#   ci-cargo-cache-prune.sh TARGET_DIR MANIFEST...
#
# Every package of the workspaces named by MANIFEST loses its compiled
# artifacts, fingerprints and build-script output, so the job builds and tests
# the repository's own code from source on every run. Run it right after the
# restore, before any Cargo command, and again before the save. Removed always:
# incremental state, rustdoc output, the tests' scratch directory (`tmp`), and
# every Risc0 guest build (the risc0-build `riscv-guest` tree and the
# verifier's run-local Docker guest source), so a guest ELF and its ImageID
# never come from a cache. scripts/ci-cache-gate.sh then decides whether what
# is left is saved.

cd "$(dirname "${BASH_SOURCE[0]}")/.."
shopt -s nullglob

if (( $# < 2 )); then
  echo "usage: $0 TARGET_DIR MANIFEST..." >&2
  exit 2
fi
target_dir="$1"
shift

for command in cargo du find jq; do
  command -v "$command" >/dev/null || {
    echo "${command} is required to prune the Cargo cache" >&2
    exit 1
  }
done

[[ -d "$target_dir" ]] || exit 0

rm -rf -- "${target_dir}/riscv-guest" "${target_dir}/docker-guest-source" "${target_dir}/doc" \
  "${target_dir}/tmp"
find "$target_dir" -mindepth 1 -maxdepth 3 -type d \( -name incremental -o -name riscv-guest \) \
  -prune -exec rm -rf -- {} +

names=()
for manifest in "$@"; do
  [[ -f "$manifest" ]] || {
    echo "missing manifest: ${manifest}" >&2
    exit 1
  }
  while IFS= read -r name; do
    names+=("$name" "${name//-/_}")
  done < <(
    cargo metadata --no-deps --format-version 1 --manifest-path "$manifest" \
      | jq -r '.packages[] | .name, .targets[].name'
  )
done
(( ${#names[@]} > 0 )) || {
  echo "no workspace packages found in: $*" >&2
  exit 1
}
pattern="$(printf '%s\n' "${names[@]}" | sort -u | sed 's/[.[\*^$+?(){}|]/\\&/g' | paste -sd '|' -)"

# Compiled artifacts carry a 16-hex-digit hash; the copies Cargo places in the
# profile directory itself (binaries, rlibs, their .d files) carry none, and
# Cargo places them there for the workspace's own targets only.
while IFS= read -r fingerprint; do
  profile="${fingerprint%/.fingerprint}"
  for kind in .fingerprint build deps examples; do
    [[ -d "${profile}/${kind}" ]] || continue
    for entry in "${profile}/${kind}"/*; do
      if [[ "${entry##*/}" =~ ^(lib)?(${pattern})-[0-9a-f]{16}([.].*)?$ ]]; then
        rm -rf -- "$entry"
      fi
    done
  done
  for entry in "${profile}"/*; do
    [[ -f "$entry" ]] || continue
    if [[ "${entry##*/}" =~ ^(lib)?(${pattern})([.].*)?$ ]]; then
      rm -f -- "$entry"
    fi
  done
done < <(find "$target_dir" -mindepth 2 -maxdepth 3 -type d -name .fingerprint)

size_kib="$(du -sk "$target_dir" | cut -f1)"
echo "pruned workspace and guest build output from ${target_dir}: $((size_kib / 1024)) MiB left"
# Where the rest is, for whoever sizes the cache.
find "$target_dir" -mindepth 1 -maxdepth 2 -type d -exec du -sk {} + \
  | sort -rn | head -n 8 \
  | while read -r kib path; do echo "  $((kib / 1024)) MiB ${path}"; done || true
