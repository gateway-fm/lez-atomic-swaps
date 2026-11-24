#!/usr/bin/env bash
set -euo pipefail

# Moves the Basecamp integration job's Nix build dependencies in and out of a
# CI cache directory. It runs inside the job's nixos/nix container, before
# (`import`) and after (`export`) scripts/test-m6-basecamp-integration.sh.
#
# `export` saves what a later run needs so that it builds only what comes
# from this repository: the inputs of every derivation built from
# apps/basecamp (the four tested derivations, the derived role sources and
# the role modules), closed under their runtime references, plus the crate
# archives the test prefetches. It leaves out:
#   - every output built from the repository, the two integration-test
#     checks and the two catalog packages among them, so that a cached
#     archive can never stand in for a build or a test run;
#   - what only built those inputs (the Rust toolchains, the Nim compiler,
#     vendored crate directories, upstream sources): with the inputs present
#     Nix never needs them;
#   - every path cache.nixos.org serves: their names are saved instead, and
#     `import` fetches them again before it imports the archive.
# The archive is an uncompressed NAR; actions/cache compresses it with zstd.
# Cached paths are input-addressed: a changed input changes the derivation
# and the path, and Nix builds it again.

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly cache_dir="${BASECAMP_NIX_CACHE_DIR:-/cache}"
# What the cache step stores, and the line ci-cache-gate.sh reports when there
# is nothing in it.
readonly store_dir="${cache_dir}/store"
readonly archive="${store_dir}/store.nar"
readonly substituted="${store_dir}/substituted-paths"
readonly status_file="${cache_dir}/export-status"
readonly -a nix=(nix --extra-experimental-features 'nix-command flakes')
# The nixos/nix image has coreutils, findutils and grep, but no awk or sed.
export LC_ALL=C

fail() {
  echo "Basecamp Nix store cache: $*" >&2
  exit 1
}

warn() {
  echo "::warning::Basecamp Nix store cache: $*"
}

# skip REASON: record why nothing is saved, for the job summary.
skip() {
  echo "Basecamp Nix store cache: $1"
  printf '%s\n' "$1" >"$status_file"
}

nar_bytes() { # nar_bytes FILE: the NAR size of the store paths listed in FILE
  local total=0 path size
  while read -r path size; do
    total=$((total + size))
  done < <(xargs -r "${nix[@]}" path-info --size <"$1")
  echo "$total"
}

import_store() {
  [[ -f "$archive" ]] || {
    echo "no cached Basecamp Nix store; building from source"
    return 0
  }
  local imported=true
  if [[ -s "$substituted" ]]; then
    echo "fetching the $(wc -l <"$substituted") paths the cached archive refers to from cache.nixos.org"
    if ! xargs -r nix-store --realise <"$substituted" >/dev/null; then
      warn "cache.nixos.org did not serve every path the archive refers to; building from source"
      imported=false
    fi
  fi
  if [[ "$imported" == true ]]; then
    echo "importing $(du -h "$archive" | cut -f1) of cached Basecamp Nix store paths"
    if ! nix-store --import <"$archive" >/dev/null; then
      warn "the cached archive did not import completely; missing paths are built from source"
    fi
  fi
  rm -f -- "$archive" "$substituted"
}

export_store() {
  rm -rf -- "$store_dir"
  rm -f -- "$status_file"
  mkdir -p "$cache_dir"
  if [[ "${BASECAMP_NIX_CACHE_SAVE:-false}" != true ]]; then
    skip "this run does not save the Basecamp Nix store"
    return 0
  fi

  local system chat_ref
  system="$("${nix[@]}" eval --impure --raw --expr builtins.currentSystem)"
  chat_ref="$(./scripts/basecamp-chat-module-ref.sh)"
  local -a root_drvs
  mapfile -t root_drvs < <(
    "${nix[@]}" path-info --derivation --no-update-lock-file \
      "path:apps/basecamp#checks.${system}.lez-maker-ui" \
      "path:apps/basecamp#checks.${system}.lez-taker-ui"
    for role in maker taker; do
      "${nix[@]}" path-info --derivation --no-write-lock-file \
        --override-input chat_module "$chat_ref" \
        "path:apps/basecamp?dir=${role}#lgx-portable"
    done
  )
  (( ${#root_drvs[@]} == 4 )) || fail "expected four tested derivations, found ${#root_drvs[@]}"

  local work
  work="$(mktemp -d)"
  local tested="${work}/tested" drvs="${work}/drvs" sources="${work}/sources"
  local own="${work}/own" own_outputs="${work}/own-outputs" inputs="${work}/inputs"
  local wanted="${work}/wanted" invalid="${work}/invalid" closure="${work}/closure"
  local signed="${work}/signed" selected="${work}/selected" everything="${work}/everything"

  nix-store --query --outputs "${root_drvs[@]}" | sort -u >"$tested"
  nix-store --query --requisites "${root_drvs[@]}" | grep '\.drv$' | sort -u >"$drvs"

  # The repository's sources as the flakes copy them into the store: the
  # flake tree itself and each directory a flake names by path.
  {
    "${nix[@]}" eval --impure --raw --expr \
      "(builtins.getFlake \"path:${PWD}/apps/basecamp\").outPath" || true
    echo
    local directory
    for directory in apps/basecamp/*/; do
      nix-store --add "${directory%/}" || true
    done
  } | { grep . || true; } | sort -u >"$sources"
  # Every derivation built from them, and the four tested ones.
  {
    xargs -r nix-store --query --referrers-closure <"$sources" | { grep '\.drv$' || true; } \
      | sort -u | comm -12 - "$drvs"
    printf '%s\n' "${root_drvs[@]}"
  } | sort -u >"$own"
  xargs -r nix-store --query --outputs <"$own" | sort -u >"$own_outputs"

  # What those derivations are built from: input sources as they are, input
  # derivations by their outputs. The crate archives are added because the
  # test prefetches every one of the closure that is missing.
  # shellcheck disable=SC2094 # both read $own; the pipeline writes $inputs
  xargs -r nix-store --query --references <"$own" | sort -u \
    | comm -23 - "$own" | comm -23 - "$sources" >"$inputs"
  {
    grep -v '\.drv$' "$inputs" || true
    { grep '\.drv$' "$inputs" || true; } | xargs -r nix-store --query --outputs
    { grep -e '-crate-.*\.tar\.gz\.drv$' "$drvs" || true; } | xargs -r nix-store --query --outputs
  } | sort -u >"$wanted"
  xargs -r nix-store --check-validity --print-invalid <"$wanted" | sort -u >"$invalid"
  comm -23 "$wanted" "$invalid" | xargs -r nix-store --query --requisites | sort -u >"$closure"
  if [[ -n "$(comm -12 "$closure" "$own_outputs")" ]]; then
    warn "a dependency refers to an output built from the repository; not saving"
    skip "a dependency refers to an output built from the repository"
    return 0
  fi
  if [[ -n "$(comm -12 "$closure" "$tested")" ]]; then
    skip "a saved path would be a tested output"
    return 0
  fi

  # Keep what cache.nixos.org did not sign (built here or imported from an
  # earlier archive); name the rest, which `import` fetches again.
  local path _
  xargs -r "${nix[@]}" path-info --sigs <"$closure" \
    | { grep -F 'cache.nixos.org-1:' || true; } \
    | while read -r path _; do printf '%s\n' "$path"; done \
    | sort -u >"$signed"
  comm -23 "$closure" "$signed" >"$selected"
  [[ -s "$selected" ]] || {
    skip "every path the test needs is on cache.nixos.org"
    return 0
  }

  # For comparison: everything the cold build produced that cache.nixos.org
  # does not serve, which is what a whole-closure archive would hold.
  nix-store --query --requisites --include-outputs "${root_drvs[@]}" | { grep -v '\.drv$' || true; } \
    | sort -u | comm -23 - "$own_outputs" | comm -23 - "$closure" >"$everything"
  local selected_bytes left_out_bytes free_bytes
  selected_bytes="$(nar_bytes "$selected")"
  left_out_bytes="$(nar_bytes "$everything")"
  echo "$(wc -l <"$own") derivations are built from the repository and never saved:"
  while read -r path; do
    echo "  ${path#/nix/store/????????????????????????????????-}"
  done <"$own"
  echo "saving $(wc -l <"$selected") paths (${selected_bytes} NAR bytes) and the names of $(wc -l <"$signed") paths cache.nixos.org serves"
  echo "left out: $(wc -l <"$everything") build-only paths (${left_out_bytes} NAR bytes, cache.nixos.org's included)"

  # The runner's disk holds the Nix store already; leave 2 GiB free.
  free_bytes="$(df --output=avail -B1 "$cache_dir" | tail -n 1 | tr -d ' ')"
  if (( selected_bytes + 2 * 1024 * 1024 * 1024 > free_bytes )); then
    skip "${selected_bytes} NAR bytes do not fit the ${free_bytes} bytes free on the runner"
    return 0
  fi
  mkdir -p "$store_dir"
  local -a paths
  mapfile -t paths <"$selected"
  nix-store --export "${paths[@]}" >"${archive}.partial"
  mv -- "${archive}.partial" "$archive"
  cp -- "$signed" "$substituted"
  chmod 0755 "$store_dir"
  chmod 0644 "$archive" "$substituted"
  echo "wrote $(du -h "$archive" | cut -f1) Basecamp Nix store archive"
}

case "${1:-}" in
  import) import_store ;;
  # A failed export must not fail the job (the release workflows require
  # main's CI to be green): it runs in a child process, and a failure leaves
  # nothing to save and says so.
  export)
    if ! bash "${BASH_SOURCE[0]}" export-now; then
      rm -rf -- "$store_dir"
      warn "the export failed; not saving"
      skip "the export failed (see the job log)"
    fi
    ;;
  export-now) export_store ;;
  *) fail "usage: $0 import|export" ;;
esac
