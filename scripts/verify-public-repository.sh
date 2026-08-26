#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"

fail() {
  echo "public repository contract failed: $*" >&2
  exit 1
}

required=(
  LICENSE LICENSE-MIT LICENSE-APACHE NOTICE THIRD_PARTY_NOTICES
  README.md CONTRIBUTING.md CODE_OF_CONDUCT.md SECURITY.md
  .github/CODEOWNERS .github/pull_request_template.md
)
for path in "${required[@]}"; do
  [[ -s "$repo_root/$path" ]] || fail "missing or empty ${path}"
done

personal_remote='github.com/'"mandrigin/lez-atomic-swaps"
if git -C "$repo_root" grep -n -F "$personal_remote" -- . >/dev/null; then
  fail "current documentation still references the private personal remote"
fi

git -C "$repo_root" show --check --format= HEAD
"$repo_root/scripts/check-public-action-pins.sh"

if command -v sha256sum >/dev/null 2>&1; then
  media_hash="$(sha256sum "$repo_root/media/music/bit-quest.mp3" | awk '{print $1}')"
else
  media_hash="$(shasum -a 256 "$repo_root/media/music/bit-quest.mp3" | awk '{print $1}')"
fi
[[ "$media_hash" == "6467ae09a7ed2e95e021031d230cfc71175b9081845d2778c2c0240feb8f3c94" ]] \
  || fail "Bit Quest source hash does not match its attribution record"

task_tmp="$(mktemp -d "${TMPDIR:-/tmp}/lez-public-verify.XXXXXX")"
cleanup() {
  case "$task_tmp" in
    */lez-public-verify.*) rm -rf -- "$task_tmp" ;;
    *) echo "refusing unexpected cleanup path: ${task_tmp}" >&2 ;;
  esac
}
trap cleanup EXIT

"$repo_root/scripts/reconstruct-source.sh" "$task_tmp/source"
echo "public repository contract passed"
