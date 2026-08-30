#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly static_crate_root="https://static.crates.io/crates"
readonly static_crate_pattern='https://static\.crates\.io/crates/([^/]+)/([^/]+)/(download|[^/]+\.crate)'
readonly legacy_crate_pattern='https://crates.io/api/v1/crates/([^/]+)/([^/]+)/download'
readonly -a nix=(nix --extra-experimental-features 'nix-command flakes')

fail() {
  echo "M6 Basecamp integration test failed: $*" >&2
  exit 1
}

for command_name in nix nix-store; do
  command -v "$command_name" >/dev/null || fail "${command_name} is required"
done

system="$("${nix[@]}" eval --impure --raw --expr builtins.currentSystem)"
maker="path:apps/basecamp#checks.${system}.lez-maker-ui"
taker="path:apps/basecamp#checks.${system}.lez-taker-ui"
maker_drv="$("${nix[@]}" path-info --derivation --no-update-lock-file "$maker")"
taker_drv="$("${nix[@]}" path-info --derivation --no-update-lock-file "$taker")"

# Older pinned nixpkgs revisions still generate the crates.io API download URL,
# which was retired in 2026. Preserve every fixed-output hash and version while
# making those derivations realizable from crates.io's immutable static archive.
for root_drv in "$maker_drv" "$taker_drv"; do
  while IFS= read -r derivation; do
    [[ "$derivation" == *-crate-*.tar.gz.drv ]] || continue
    output="$(nix-store --query --outputs "$derivation")"
    nix-store --check-validity "$output" >/dev/null 2>&1 && continue

    definition="$("${nix[@]}" derivation show "$derivation")"
    # Current nixpkgs already points at the immutable static archive. Leave
    # those derivations to the normal build; only repair retired API URLs.
    [[ "$definition" =~ $static_crate_pattern ]] && continue
    [[ "$definition" =~ $legacy_crate_pattern ]] ||
      fail "unrealized crate derivation has no recognized source: ${derivation}"
    crate_name="${BASH_REMATCH[1]}"
    crate_version="${BASH_REMATCH[2]}"
    archive="${static_crate_root}/${crate_name}/${crate_name}-${crate_version}.crate"
    output_name="${output##*/}"
    output_name="${output_name#*-}"
    echo "prefetching ${crate_name} ${crate_version} from the immutable static archive"
    "${nix[@]}" store prefetch-file --name "$output_name" "$archive" >/dev/null
    # `nix store prefetch-file` already inserts the archive. Verify the exact
    # fixed-output path instead of depending on its human/JSON output format.
    nix-store --check-validity "$output" >/dev/null 2>&1 ||
      fail "static archive did not realize the expected fixed output: ${output}"
  done < <(nix-store --query --requisites "$root_drv")
done

"${nix[@]}" build --no-update-lock-file --no-link "$maker" "$taker"
echo "Maker and Taker Basecamp integration tests passed"
