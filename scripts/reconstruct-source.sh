#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
base_commit="5c384a5151f59bef1a2f19421ef6ab2b004db3d4"
expected_tree="c747dafbdf39ed4615d92b005e63552a32bb60bf"
expected_patch_count=38

fail() {
  echo "source reconstruction failed: $*" >&2
  exit 1
}

[[ "$#" == 1 ]] || fail "usage: $0 <new-destination-directory>"
destination="$1"
[[ -n "$destination" && "$destination" != "/" ]] \
  || fail "refusing an empty or root destination"
[[ ! -e "$destination" ]] || fail "destination already exists: $destination"

patches=("$repo_root"/deploy/full-swap/patches/*.patch)
[[ "${#patches[@]}" == "$expected_patch_count" ]] \
  || fail "expected ${expected_patch_count} patches, found ${#patches[@]}"

for ((index = 1; index <= expected_patch_count; index++)); do
  printf -v expected_prefix '%04d-' "$index"
  patch_name="$(basename "${patches[$((index - 1))]}")"
  [[ "$patch_name" == "$expected_prefix"* ]] \
    || fail "patch ${index} is out of order: ${patch_name}"
done

git -C "$repo_root" cat-file -e "${base_commit}^{commit}" \
  || fail "base commit is not reachable: ${base_commit}"

mkdir -p "$destination"
git -C "$destination" init --quiet
git -C "$destination" config user.name "Patch Verifier"
git -C "$destination" config user.email "patch-verifier@localhost"
git -C "$destination" config commit.gpgSign false
git -C "$destination" fetch --quiet --no-tags "$repo_root" "$base_commit"
git -C "$destination" checkout --quiet --detach FETCH_HEAD
git -C "$destination" -c commit.gpgSign=false am "${patches[@]}"

actual_tree="$(git -C "$destination" rev-parse 'HEAD^{tree}')"
[[ "$actual_tree" == "$expected_tree" ]] \
  || fail "expected tree ${expected_tree}, reconstructed ${actual_tree}"
[[ -z "$(git -C "$destination" status --porcelain)" ]] \
  || fail "reconstructed worktree is not clean"

echo "reconstructed source tree ${actual_tree} at ${destination}"
