#!/usr/bin/env bash
set -euo pipefail

# Tests the CI cache scripts: ci-cache-gate.sh (which caches a run saves, and
# what the job summary says) and ci-cargo-cache-prune.sh (what of a Cargo
# target directory a cache may hold).

cd "$(dirname "${BASH_SOURCE[0]}")/.."
readonly repository="$PWD"

for command in cargo head jq tar zstd; do
  command -v "$command" >/dev/null || {
    echo "${command} is required to test the CI cache scripts" >&2
    exit 1
  }
done

work="$(mktemp -d "${TMPDIR:-/tmp}/lez-ci-cache-test.XXXXXX")"
trap 'rm -rf -- "$work"' EXIT

fail() {
  echo "CI cache script test failed: $*" >&2
  exit 1
}

# gate NAME CAP PATH... with CI_CACHE_* from the caller; leaves the step's
# outputs in $work/output, its summary in $work/summary, its log in $work/log.
gate() {
  : >"${work}/output"
  : >"${work}/summary"
  GITHUB_OUTPUT="${work}/output" GITHUB_STEP_SUMMARY="${work}/summary" GITHUB_WORKSPACE="$work" \
    "${repository}/scripts/ci-cache-gate.sh" "$@" >"${work}/log" 2>&1
}

expect_output() {
  grep -Fxq -- "$1" "${work}/output" || fail "expected output '$1', got: $(cat "${work}/output")"
}

expect_summary() {
  grep -Fq -- "$1" "${work}/summary" || fail "expected summary '$1', got: $(cat "${work}/summary")"
}

random_mib() { # random_mib FILE MIB: data zstd cannot shrink
  head -c "$(($2 * 1024 * 1024))" /dev/urandom >"$1"
}

mkdir -p "${work}/cache/kept" "${work}/cache/skipped" "${work}/home/.cargo/registry"
random_mib "${work}/cache/kept/data" 2
random_mib "${work}/cache/skipped/data" 3
head -c $((64 * 1024 * 1024)) /dev/zero >"${work}/zeros"
random_mib "${work}/home/.cargo/registry/data" 1

# A pull request run restores and saves nothing.
CI_CACHE_SAVE=false CI_CACHE_HIT=true gate pr-cache 8 cache
expect_output "save=false"
expect_summary '- Cache `pr-cache`: restored (exact key); not saved: only a push to main or a dispatch with save-caches saves'

# A saving run does not save a key it restored.
CI_CACHE_SAVE=true CI_CACHE_HIT=true gate stored-cache 8 cache
expect_output "save=false"
expect_summary 'not saved: this key is stored already'

# The cap applies to the compressed archive.
CI_CACHE_SAVE=true CI_CACHE_HIT=false CI_CACHE_MATCHED_KEY=older-key gate fitting-cache 3 "cache/kept
cache/missing"
expect_output "save=true"
expect_summary '- Cache `fitting-cache`: restored an older generation (older-key); saving 3 MiB compressed (cap 3 MiB)'
grep -Eq '^compressed-bytes=2[0-9]{6}$' "${work}/output" || fail "expected about 2 MiB compressed: $(cat "${work}/output")"

# The gate's number is what the actions/cache command produces: tar --posix
# piped through zstdmt, which is zstd -T0 at level 3 without --long, so a
# repeat 4 MiB back (beyond level 3's window) is stored twice.
mkdir -p "${work}/repeats"
random_mib "${work}/repeats/a" 4
cp "${work}/repeats/a" "${work}/repeats/b"
CI_CACHE_SAVE=true gate repeats 16 "${work}/repeats"
expected="$(tar --posix -c -f - -P -- "${work}/repeats" 2>/dev/null | zstd -T0 -q -c | wc -c | tr -d '[:space:]')"
measured="$(sed -n 's/^compressed-bytes=//p' "${work}/output")"
# Within 0.1%: pax headers carry the read times.
(( measured * 1000 > expected * 999 && measured * 1000 < expected * 1001 )) \
  || fail "the gate measured ${measured} bytes, the actions/cache command ${expected}"
(( expected > 8 * 1024 * 1024 )) || fail "the measurement must not find repeats beyond zstd's level-3 window: ${expected}"

CI_CACHE_SAVE=true gate zeros 1 "${work}/zeros"
expect_output "save=true"
expect_summary 'not restored (no cache for this key); saving 1 MiB compressed (cap 1 MiB)'

# Above its cap a cache is not saved, and the log carries an error annotation.
CI_CACHE_SAVE=true gate large-cache 4 cache
expect_output "save=false"
expect_summary '- Cache `large-cache`: not restored (no cache for this key); not saved: 6 MiB compressed, above its 4 MiB cap'
grep -Fq '::error title=Cache not saved::large-cache: 6 MiB compressed, above its 4 MiB cap' "${work}/log" \
  || fail "an oversized cache must raise an error annotation: $(cat "${work}/log")"

# `~/` is the home directory, as for actions/cache.
# shellcheck disable=SC2088 # the literal `~/` a workflow passes
HOME="${work}/home" CI_CACHE_SAVE=true gate home-cache 2 '~/.cargo/registry'
expect_output "save=true"

# An exclusion is refused: actions/cache would store the directory anyway.
if CI_CACHE_SAVE=true gate excluding-cache 8 "cache
!${work}/cache/skipped"; then
  fail "a ! exclusion must be refused"
fi
grep -Fq 'actions/cache does not exclude' "${work}/log" || fail "the refusal must say why: $(cat "${work}/log")"

# Nothing to save: the reason comes from the note file when there is one.
printf 'every path the test needs is on cache.nixos.org\nsecond line\n' >"${work}/note"
CI_CACHE_SAVE=true CI_CACHE_NOTE_FILE="${work}/note" gate empty-cache 8 "${work}/missing"
expect_output "save=false"
expect_summary '- Cache `empty-cache`: not restored (no cache for this key); not saved: every path the test needs is on cache.nixos.org'
CI_CACHE_SAVE=true gate empty-cache 8 "${work}/missing"
expect_summary 'not saved: nothing to save'

# A cap that is not a whole number of MiB is a usage error.
if CI_CACHE_SAVE=true gate bad-cap 1.5 cache; then
  fail "a fractional cap must be refused"
fi

# The prune keeps third-party build output only.
mkdir -p "${work}/crate/src"
cat >"${work}/crate/Cargo.toml" <<'TOML'
[package]
name = "demo-pkg"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "demo-tool"
path = "src/main.rs"

[workspace]
TOML
touch "${work}/crate/src/lib.rs" "${work}/crate/src/main.rs"
readonly target="${work}/target"
readonly hash=0123456789abcdef
removed=(
  "debug/.fingerprint/demo-pkg-${hash}/"
  "debug/build/demo-pkg-${hash}/"
  "debug/deps/libdemo_pkg-${hash}.rlib"
  "debug/deps/demo_tool-${hash}"
  "debug/deps/demo_tool-${hash}.d"
  "debug/demo-tool"
  "debug/demo-tool.d"
  "debug/libdemo_pkg.rlib"
  "debug/libdemo_pkg.d"
  "debug/incremental/demo_pkg-${hash}/"
  "tmp/scratch/"
  "doc/demo_pkg/"
  "riscv-guest/guest/"
  "docker-guest-source/guest/"
)
kept=(
  "debug/.cargo-lock"
  "debug/.fingerprint/serde-${hash}/"
  "debug/build/serde-${hash}/"
  "debug/deps/libserde-${hash}.rlib"
  "debug/deps/libdemo_pkg_extra-${hash}.rlib"
  "debug/demo-pkg-extra"
  "release/.fingerprint/serde-${hash}/"
  "release/deps/libserde-${hash}.rlib"
)
for entry in "${removed[@]}" "${kept[@]}"; do
  if [[ "$entry" == */ ]]; then
    mkdir -p "${target}/${entry}"
    touch "${target}/${entry}file"
  else
    mkdir -p "$(dirname "${target}/${entry}")"
    touch "${target}/${entry}"
  fi
done
"${repository}/scripts/ci-cargo-cache-prune.sh" "$target" "${work}/crate/Cargo.toml" >"${work}/log"
for entry in "${removed[@]}"; do
  [[ ! -e "${target}/${entry}" ]] || fail "the prune left ${entry}"
done
for entry in "${kept[@]}"; do
  [[ -e "${target}/${entry}" ]] || fail "the prune removed ${entry}"
done
grep -q "^pruned workspace and guest build output from ${target}: " "${work}/log" \
  || fail "the prune must report what is left: $(cat "${work}/log")"
"${repository}/scripts/ci-cargo-cache-prune.sh" "${work}/no-target" "${work}/crate/Cargo.toml" \
  || fail "a missing target directory is nothing to prune"

echo "CI cache script tests passed"
