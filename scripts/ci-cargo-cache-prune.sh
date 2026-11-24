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
# incremental state, rustdoc output, and every Risc0 guest build (the
# risc0-build `riscv-guest` tree and the verifier's run-local Docker guest
# source), so a guest ELF and its ImageID never come from a cache.

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

rm -rf -- "${target_dir}/riscv-guest" "${target_dir}/docker-guest-source" "${target_dir}/doc"
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
done < <(find "$target_dir" -mindepth 2 -maxdepth 3 -type d -name .fingerprint)

size_kib="$(du -sk "$target_dir" | cut -f1)"
echo "pruned workspace and guest build output from ${target_dir}: $((size_kib / 1024)) MiB left"

# CI_CACHE_MAX_GIB bounds what a save may add to the repository's 10 GB of
# caches. Above it the step output `fits` is set to false and the job's save
# step does not run; the run itself is unaffected.
if [[ -n "${CI_CACHE_MAX_GIB:-}" ]] && (( size_kib > CI_CACHE_MAX_GIB * 1024 * 1024 )); then
  echo "::warning::${target_dir} holds $((size_kib / 1024)) MiB after pruning, above the ${CI_CACHE_MAX_GIB} GiB cache cap; not saving it"
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    echo "fits=false" >>"$GITHUB_OUTPUT"
  fi
fi
