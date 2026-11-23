#!/usr/bin/env bash
set -euo pipefail

# Moves the Basecamp integration job's Nix build dependencies in and out of a
# CI cache directory. It runs inside the job's nixos/nix container, before
# (`import`) and after (`export`) scripts/test-m6-basecamp-integration.sh.
#
# `export` saves the store paths the public binary cache does not serve (the
# Chat and Delivery modules, the Logos SDK builds, the vendored Rust crates),
# closed under their runtime references so that `nix-store --import` accepts
# the archive. It never saves the outputs the test builds: the two
# integration-test checks and the two catalog packages, so a cached archive
# cannot stand in for a test run. Cached paths are input-addressed: a changed
# input changes the derivation and the path, and Nix builds it again.

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly cache_dir="${BASECAMP_NIX_CACHE_DIR:-/cache}"
readonly archive="${cache_dir}/store.nar.gz"
# Uncompressed NAR size above which nothing is saved; the gzip'd archive is
# about a third of it, and GitHub keeps 10 GB of caches per repository.
readonly max_nar_bytes=$((6 * 1024 * 1024 * 1024))
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

import_store() {
  [[ -f "$archive" ]] || {
    echo "no cached Basecamp Nix store; building from source"
    return 0
  }
  echo "importing $(du -h "$archive" | cut -f1) of cached Basecamp Nix store paths"
  if ! gzip -dc "$archive" | nix-store --import >/dev/null; then
    warn "the cached archive did not import completely; missing paths are built from source"
  fi
  rm -f -- "$archive"
}

export_store() {
  if [[ "${BASECAMP_NIX_CACHE_SAVE:-false}" != true ]]; then
    echo "not saving the Basecamp Nix store on this run"
    return 0
  fi
  mkdir -p "$cache_dir"
  rm -f -- "$archive" "${archive}.partial"

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

  local work tested candidates selected closure
  work="$(mktemp -d)"
  tested="${work}/tested"
  candidates="${work}/candidates"
  selected="${work}/selected"
  closure="${work}/closure"

  nix-store --query --outputs "${root_drvs[@]}" | sort -u >"$tested"
  nix-store --query --requisites --include-outputs "${root_drvs[@]}" \
    | grep -v '\.drv$' | sort -u | comm -23 - "$tested" >"$candidates"
  # Keep what cache.nixos.org did not sign: built here or imported from the
  # previous archive. Signed paths are substituted again on the next run.
  local path _
  xargs -r "${nix[@]}" path-info --sigs <"$candidates" \
    | { grep -v -F 'cache.nixos.org-1:' || true; } \
    | while read -r path _; do printf '%s\n' "$path"; done \
    | sort -u >"$selected"
  [[ -s "$selected" ]] || {
    warn "no locally built paths to save"
    return 0
  }
  xargs -r nix-store --query --requisites <"$selected" | sort -u >"$closure"
  if [[ -n "$(comm -12 "$closure" "$tested")" ]]; then
    warn "a saved path refers to a tested output; not saving"
    return 0
  fi

  local nar_bytes=0 size
  while read -r path size; do
    nar_bytes=$((nar_bytes + size))
  done < <(xargs -r "${nix[@]}" path-info --size <"$closure")
  echo "saving $(wc -l <"$selected") locally built paths, $(wc -l <"$closure") with their closure, ${nar_bytes} NAR bytes"
  if (( nar_bytes > max_nar_bytes )); then
    warn "closure of ${nar_bytes} bytes exceeds the ${max_nar_bytes}-byte cap; not saving"
    return 0
  fi
  mapfile -t paths <"$closure"
  nix-store --export "${paths[@]}" | gzip -1 >"${archive}.partial"
  mv -- "${archive}.partial" "$archive"
  chmod 0644 "$archive"
  echo "saved $(du -h "$archive" | cut -f1) Basecamp Nix store archive"
}

case "${1:-}" in
  import) import_store ;;
  export) export_store ;;
  *) fail "usage: $0 import|export" ;;
esac
