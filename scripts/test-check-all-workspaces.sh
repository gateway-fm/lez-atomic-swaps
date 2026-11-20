#!/usr/bin/env bash
# Keeps scripts/check-all.sh equal to the tree and to CI (issue #171):
#   - its workspace list is exactly the Cargo workspaces of the tree, found
#     from every tracked Cargo.toml (third_party/ submodules excluded);
#   - its root excludes are the ones the rust job of ci.yml uses;
#   - the clippy and doc commands it copies are still the ones ci.yml and the
#     verifier scripts run. When one of them changes, change check-all.sh too.
# The needles below are literal script text; they must not expand.
# shellcheck disable=SC2016
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly check_all="scripts/check-all.sh"
readonly ci_workflow=".github/workflows/ci.yml"
readonly provisional_verifier="scripts/verify-lez-v02-provisional.sh"
readonly sidecar_verifier="scripts/verify-lez-v02-sidecar.sh"
readonly spel_verifier="scripts/verify-spel-compatibility.sh"
readonly standalone_runner="scripts/run-lez-standalone-e2e.sh"

fail() {
  echo "check-all workspace policy: $*" >&2
  exit 1
}

require_fixed() {
  local needle="$1"
  local path="$2"
  grep -Fq -- "$needle" "$path" || fail "$path no longer contains: $needle (update ${check_all} with it)"
}

command -v cargo >/dev/null || fail "cargo is required"

listed="$("$check_all" --list-workspaces | LC_ALL=C sort)"

found="$(
  git ls-files --cached --others --exclude-standard -z -- 'Cargo.toml' '*/Cargo.toml' \
    | while IFS= read -r -d '' manifest; do
        [[ "$manifest" == third_party/* ]] && continue
        root="$(cargo locate-project --workspace --message-format plain \
          --manifest-path "$manifest")" || {
          echo "cargo cannot place ${manifest} in a workspace" >&2
          exit 1
        }
        printf '%s\n' "${root#"$(pwd -P)/"}"
      done | LC_ALL=C sort -u
)"
[[ -n "$found" ]] || fail "no Cargo workspace found"

if [[ "$listed" != "$found" ]]; then
  echo "check-all workspace policy: ${check_all} and the tree disagree" >&2
  diff <(printf '%s\n' "$listed") <(printf '%s\n' "$found") \
    | sed -n 's/^< /  listed, not in the tree: /p; s/^> /  in the tree, not listed: /p' >&2
  exit 1
fi

listed_excludes="$(
  awk '/^readonly root_excludes=\(/ { on = 1; next } on && /^\)/ { exit } on { print $1 }' \
    "$check_all" | LC_ALL=C sort
)"
grep -q '^readonly root_excludes=(' "$check_all" || fail "${check_all} has no root_excludes list"
for step in 'cargo clippy --locked --workspace --all-targets' \
  'cargo test --locked --workspace --all-targets' \
  'cargo doc --locked --workspace --no-deps'; do
  step_excludes="$(
    awk -v step="$step" '
      index($0, step) { on = 1; next }
      on && /^ *--exclude / { print $2; next }
      on { exit }
    ' "$ci_workflow" | LC_ALL=C sort
  )"
  [[ "$step_excludes" == "$listed_excludes" ]] \
    || fail "the '${step}' step of ${ci_workflow} excludes other crates than ${check_all}"
done

# Every separate workspace that check-all.sh lints is in exactly one shard of
# the lint-separate job of ci.yml.
sharded="$(
  awk '
    /^  lint-separate:/ { on = 1; next }
    on && /^  [A-Za-z0-9_-]+:$/ { exit }
    on && /^ +manifests:/ { block = 1; sub(/^ +manifests: *(>-)?/, ""); if ($0 != "") print; next }
    on && block && /^ +compat\/[^ ]+Cargo\.toml$/ { sub(/^ +/, ""); print; next }
    { block = 0 }
  ' "$ci_workflow" | LC_ALL=C sort
)"
lintable="$("$check_all" --list-separate-lint | LC_ALL=C sort)"
if [[ "$sharded" != "$lintable" ]]; then
  echo "check-all workspace policy: the lint-separate shards of ${ci_workflow} differ from what ${check_all} lints" >&2
  diff <(printf '%s\n' "$lintable") <(printf '%s\n' "$sharded") \
    | sed -n 's/^< /  in no shard: /p; s/^> /  in a shard twice or not linted: /p' >&2
  exit 1
fi

# Root workspace: the rust job of ci.yml.
require_fixed 'cargo fmt --all --check' "$ci_workflow"
require_fixed '-- -D warnings' "$ci_workflow"
require_fixed 'cargo clippy --locked -p lez-maker-node -p lez-taker-node --all-features --all-targets -- -D warnings' "$ci_workflow"
require_fixed 'RUSTDOCFLAGS: -D warnings' "$ci_workflow"

# Provisional LEZ v0.2 workspaces.
require_fixed 'cargo clippy --locked --manifest-path "$root_manifest" --all-targets -- -D warnings' "$provisional_verifier"
require_fixed 'RUSTDOCFLAGS="-D warnings" cargo doc --locked --manifest-path "$root_manifest" --no-deps' "$provisional_verifier"
require_fixed 'cargo clippy --locked --manifest-path "$guest_manifest" --bins -- -D warnings' "$provisional_verifier"
require_fixed 'cargo doc --locked --manifest-path "$guest_manifest" --no-deps --bins' "$provisional_verifier"
require_fixed 'cargo clippy --locked --manifest-path "$methods_manifest" --all-targets -- -D warnings' "$provisional_verifier"
require_fixed 'cargo doc --locked --manifest-path "$methods_manifest" --no-deps' "$provisional_verifier"
require_fixed 'cargo clippy --locked --manifest-path "$deployer_manifest" --all-targets -- -D warnings' "$provisional_verifier"
require_fixed 'cargo doc --locked --manifest-path "$deployer_manifest" --no-deps --bins' "$provisional_verifier"
require_fixed 'root_manifest="compat/lez-v0.2-provisional/Cargo.toml"' "$provisional_verifier"
require_fixed 'methods_manifest="compat/lez-v0.2-provisional/escrow/methods/Cargo.toml"' "$provisional_verifier"
require_fixed 'guest_manifest="compat/lez-v0.2-provisional/escrow/methods/guest/Cargo.toml"' "$provisional_verifier"
require_fixed 'deployer_manifest="compat/lez-v0.2-provisional/escrow/deployer/Cargo.toml"' "$provisional_verifier"

# LEZ v0.2 sidecar.
sidecar_flags="$(tr -s ' \\\n' ' ' <"$sidecar_verifier")"
for needle in \
  'cargo +1.96.0 clippy --manifest-path "${manifest}" --locked --offline --all-targets --all-features -- -D warnings' \
  'cargo +1.96.0 rustdoc --manifest-path "${manifest}" --locked --offline --all-features --lib -- -D warnings' \
  'readonly crate_dir="${repository_root}/compat/lez-v0_2-sidecar"'; do
  grep -Fq -- "$needle" <<<"$sidecar_flags" \
    || fail "${sidecar_verifier} no longer runs: ${needle} (update ${check_all} with it)"
done

# SPEL v0.1 escrow and the standalone LEZ end-to-end workspaces.
require_fixed 'fixture="compat/spel-zec-escrow/Cargo.toml"' "$spel_verifier"
require_fixed 'cargo clippy --locked --manifest-path "$fixture" --all-targets -- -D warnings' "$spel_verifier"
require_fixed 'RUSTDOCFLAGS="-D warnings" cargo doc --locked --manifest-path "$fixture" --no-deps' "$spel_verifier"
require_fixed 'guest_manifest="compat/spel-zec-escrow/methods/guest/Cargo.toml"' "$standalone_runner"
require_fixed 'methods_manifest="compat/spel-zec-escrow/methods/Cargo.toml"' "$standalone_runner"
require_fixed 'standalone_manifest="compat/lez-standalone-e2e/Cargo.toml"' "$standalone_runner"
require_fixed 'cargo clippy --locked --manifest-path "$guest_manifest" -- -D warnings' "$standalone_runner"
require_fixed 'cargo clippy --locked --manifest-path "$methods_manifest" --all-targets -- -D warnings' "$standalone_runner"
require_fixed 'cargo clippy --locked --manifest-path "$standalone_manifest" --all-targets -- -D warnings' "$standalone_runner"

echo "check-all covers the $(wc -l <<<"$found" | tr -d ' ') Cargo workspaces of the tree with the flags CI and the verifiers use"
